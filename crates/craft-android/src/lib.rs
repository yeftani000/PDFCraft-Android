//! Android runtime shared by the Craft apps.
//!
//! The app's `apps/pdfcraft-android` crate is a `cdylib` whose `android_main`
//! calls [`run`]. Kotlin's `MainActivity` (templates/android) hosts it and implements the parts
//! that need the Android SDK; this crate is the Rust half of that bridge:
//!
//! * [`open_url`]: open a link or media URL in the system handler,
//! * [`request_pick`] / [`take_picked`]: the document picker, **asynchronous on purpose** (the
//!   picker destroys the activity's surface, and the glue then makes the UI thread wait for the
//!   native thread, so blocking this thread for the picker would deadlock),
//! * [`pick_async`]: the same picker as a future, for code written around async file dialogs,
//! * [`process_raw_input`]: reliable typed text (hardware keys and the soft keyboard) for egui,
//! * [`JsonFileStorage`]: an `eframe::Storage` in a file (eframe has no storage path on Android),
//! * [`clipboard_text`] and friends: the system clipboard (text, HTML, images, files),
//! * [`publish_media`]: copy a finished file into Pictures/Movies/Music/Downloads (anything the app
//!   writes into [`downloads_dir`] is published automatically once it stops changing),
//! * [`files_dir`] and [`downloads_dir`]: where the app keeps its data.
#![cfg(target_os = "android")]

mod bridge;
mod clipboard;
mod pick;
mod storage;
mod text;
mod text_core;

pub use bridge::{downloads_dir, files_dir, open_url, publish_media};
pub use clipboard::{clipboard_files, clipboard_html, clipboard_image, clipboard_text, set_clipboard_files, set_clipboard_html, set_clipboard_image, set_clipboard_text, ClipImage};
pub use storage::JsonFileStorage;
pub use text::process_raw_input;
pub use pick::{pick_async, request_pick, take_picked, PickFuture};

pub use winit::platform::android::activity::AndroidApp;

/// The key under which MainActivity posts a document opened from outside the app ("Open with",
/// share): collect it with [`take_picked`]. Keep in step with `OPEN_KEY` in MainActivity.kt.
pub const OPEN_KEY: u32 = 0x7fff_0001;

/// Starts the app: the body of the app crate's `android_main`.
///
/// `creator` builds the egui app (the same job as the closure given to `eframe::run_native` on
/// desktop).
pub fn run<F>(app: AndroidApp, title: &str, creator: F)
where
  F: FnOnce(&eframe::CreationContext<'_>) -> Box<dyn eframe::App> + 'static,
{
  // Android sets no HOME and its default temporary directory (/data/local/tmp) isn't writable by
  // apps, so point both at the app's own folders before anything else reads them.
  if let Some(files) = app.internal_data_path() {
    let cache = files.parent().map(|p| p.join("cache")).unwrap_or_else(|| files.join("cache"));
    let _ = std::fs::create_dir_all(&files);
    let _ = std::fs::create_dir_all(&cache);
    // Safety: set before any other thread is started.
    std::env::set_var("HOME", &files);
    std::env::set_var("TMPDIR", &cache);
  }
  android_logger::init_once(android_logger::Config::default().with_tag("CraftAndroid").with_max_level(log::LevelFilter::Info));
  // Safety: the pointers come straight from the glue and stay valid for the activity's lifetime.
  unsafe { bridge::init(app.vm_as_ptr(), app.activity_as_ptr(), app.internal_data_path()) };
  // Reference the JNI entry point so the linker keeps it in the app's shared library.
  std::hint::black_box(bridge::Java_ai_craft_android_MainActivity_nativeOnPicked as *const ());
  bridge::start_publish_watcher();
  text::init(&app);
  std::hint::black_box(text::Java_ai_craft_android_MainActivity_nativeOnTypedKey as *const ());
  std::hint::black_box(text::Java_ai_craft_android_MainActivity_nativeOnTypedText as *const ());

  let options = eframe::NativeOptions { android_app: Some(app), ..Default::default() };
  let result = eframe::run_native(
    title,
    options,
    Box::new(move |cc| {
      pick::install_repaint(cc.egui_ctx.clone());
      Ok(creator(cc))
    }),
  );
  if let Err(err) = result {
    log::error!("{title} stopped: {err}");
  }
}
