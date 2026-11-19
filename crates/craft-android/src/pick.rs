//! The picker mailbox: Kotlin posts results from its own threads, the app collects them per key.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::task::{Context, Poll, Waker};

use crate::bridge;

static REPAINT: OnceLock<eframe::egui::Context> = OnceLock::new();

type Mailbox = Mutex<HashMap<u32, Vec<Vec<PathBuf>>>>;

fn mailbox() -> &'static Mailbox {
  static MAILBOX: OnceLock<Mailbox> = OnceLock::new();
  MAILBOX.get_or_init(Default::default)
}

pub(crate) fn install_repaint(ctx: eframe::egui::Context) {
  let _ = REPAINT.set(ctx);
}

/// Wakes the UI so it picks up something posted from another thread.
pub(crate) fn wake() {
  if let Some(ctx) = REPAINT.get() {
    ctx.request_repaint();
  }
}

/// A pick that something is awaiting (see [`pick_async`]).
enum Slot {
  Waiting(Option<Waker>),
  Done(Vec<PathBuf>),
}

fn slots() -> &'static Mutex<HashMap<u32, Slot>> {
  static SLOTS: OnceLock<Mutex<HashMap<u32, Slot>>> = OnceLock::new();
  SLOTS.get_or_init(Default::default)
}

/// Keys for [`pick_async`] start here, apart from the small keys apps choose for [`request_pick`].
const ASYNC_KEY_BASE: u32 = 0x4000_0000;
static NEXT_ASYNC_KEY: AtomicU32 = AtomicU32::new(0);

/// Posts picked files for `key` (an empty list means the user cancelled) and wakes the waiter or the UI.
pub(crate) fn post_picked(key: u32, paths: Vec<PathBuf>) {
  let waker = {
    let mut slots = slots().lock().unwrap_or_else(|e| e.into_inner());
    match slots.get_mut(&key) {
      Some(slot) => {
        let waker = match slot {
          Slot::Waiting(waker) => waker.take(),
          Slot::Done(_) => None,
        };
        *slot = Slot::Done(paths);
        Some(waker)
      },
      None => {
        drop(slots);
        if !paths.is_empty() {
          mailbox().lock().unwrap_or_else(|e| e.into_inner()).entry(key).or_default().push(paths);
        }
        None
      },
    }
  };
  if let Some(Some(waker)) = waker {
    waker.wake();
  }
  if let Some(ctx) = REPAINT.get() {
    ctx.request_repaint();
  }
}

/// The result of [`pick_async`]: the chosen files, empty if the user cancelled.
pub struct PickFuture {
  key: u32,
}

impl Future for PickFuture {
  type Output = Vec<PathBuf>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let mut slots = slots().lock().unwrap_or_else(|e| e.into_inner());
    match slots.get_mut(&self.key) {
      Some(Slot::Done(_)) => match slots.remove(&self.key) {
        Some(Slot::Done(paths)) => Poll::Ready(paths),
        _ => Poll::Ready(Vec::new()),
      },
      Some(Slot::Waiting(waker)) => {
        *waker = Some(cx.waker().clone());
        Poll::Pending
      },
      None => Poll::Ready(Vec::new()),
    }
  }
}

impl Drop for PickFuture {
  fn drop(&mut self) {
    slots().lock().unwrap_or_else(|e| e.into_inner()).remove(&self.key);
  }
}

/// Opens Android's document picker and returns a future for the choice, for code written around
/// async file dialogs (such as `rfd::AsyncFileDialog`). The future can be driven from any thread,
/// for example with `pollster::block_on` on a worker.
pub fn pick_async(mime_types: &[&str], multiple: bool) -> PickFuture {
  let key = ASYNC_KEY_BASE + NEXT_ASYNC_KEY.fetch_add(1, Ordering::Relaxed) % 0x3FFF_0000;
  slots().lock().unwrap_or_else(|e| e.into_inner()).insert(key, Slot::Waiting(None));
  if let Err(err) = bridge::start_pick(key, mime_types, multiple) {
    log::error!("Couldn't open the file picker: {err}");
    post_picked(key, Vec::new());
  }
  PickFuture { key }
}

/// Opens Android's document picker for `mime_types` (empty means any file). The files, copied
/// into the app's cache under their original names, arrive later through [`take_picked`] with
/// the same `key`. Pick any number you like for `key`; it only tells requests apart.
pub fn request_pick(key: u32, mime_types: &[&str], multiple: bool) {
  if let Err(err) = bridge::start_pick(key, mime_types, multiple) {
    log::error!("Couldn't open the file picker: {err}");
    post_picked(key, Vec::new());
  }
}

/// What the user chose in finished [`request_pick`] calls for `key`: one list per request.
pub fn take_picked(key: u32) -> Vec<Vec<PathBuf>> {
  mailbox().lock().unwrap_or_else(|e| e.into_inner()).remove(&key).unwrap_or_default()
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;
  use std::task::Wake;
  use std::thread;
  use std::time::Duration;

  use super::*;

  struct ThreadWaker(thread::Thread);

  impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
      self.0.unpark();
    }
  }

  /// Runs `future` to completion on this thread, like `pollster::block_on`.
  fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
      if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
        return value;
      }
      thread::park_timeout(Duration::from_secs(5));
    }
  }

  fn waiting(key: u32) -> PickFuture {
    slots().lock().unwrap().insert(key, Slot::Waiting(None));
    PickFuture { key }
  }

  #[test]
  fn an_awaited_pick_resolves_when_kotlin_posts_the_files() {
    let future = waiting(0x4000_0001);
    let poster = thread::spawn(|| {
      thread::sleep(Duration::from_millis(100));
      post_picked(0x4000_0001, vec![PathBuf::from("/cache/a.pdf"), PathBuf::from("/cache/b.pdf")]);
    });
    assert_eq!(block_on(future), vec![PathBuf::from("/cache/a.pdf"), PathBuf::from("/cache/b.pdf")]);
    poster.join().unwrap();
  }

  #[test]
  fn a_cancelled_pick_resolves_empty() {
    let future = waiting(0x4000_0002);
    post_picked(0x4000_0002, Vec::new());
    assert!(block_on(future).is_empty());
  }

  #[test]
  fn a_result_posted_before_the_first_poll_is_kept() {
    let future = waiting(0x4000_0003);
    post_picked(0x4000_0003, vec![PathBuf::from("/cache/c.pdf")]);
    assert_eq!(block_on(future), vec![PathBuf::from("/cache/c.pdf")]);
  }

  #[test]
  fn dropping_the_future_frees_its_slot() {
    drop(waiting(0x4000_0004));
    assert!(!slots().lock().unwrap().contains_key(&0x4000_0004));
  }

  #[test]
  fn keyed_picks_still_use_the_mailbox() {
    post_picked(7, vec![PathBuf::from("/cache/d.pdf")]);
    post_picked(7, Vec::new());
    assert_eq!(take_picked(7), vec![vec![PathBuf::from("/cache/d.pdf")]]);
    assert!(take_picked(7).is_empty());
  }
}
