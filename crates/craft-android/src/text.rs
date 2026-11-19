//! Typed text on Android: the JNI entry points `MainActivity` calls, the soft keyboard's IME
//! buffer, and [`process_raw_input`], which merges all the routes into egui's input each frame.
//! The decisions live in `text_core` (pure, tested on any host).
//!
//! Why three routes: winit turns hardware-style key events into text using the input device's key
//! character map, and some devices' maps answer only when Alt is held; winit also ignores text a
//! soft keyboard commits through the IME. So `MainActivity` also reports each key press (resolved
//! with Android's own APIs, retrying without Alt) and this module reads the IME buffer.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui;
use jni::objects::{JObject, JString};
use jni::sys::{jboolean, jint};
use jni::JNIEnv;
use winit::platform::android::activity::input::{ImeOptions, InputType, TextInputAction, TextInputState};

use crate::text_core::{sanitize_ime_text, text_for_key, TextMerger};
use crate::AndroidApp;

static APP: OnceLock<AndroidApp> = OnceLock::new();

/// Text Java captured, waiting for the next frame.
fn java_text() -> &'static Mutex<VecDeque<String>> {
  static QUEUE: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
  QUEUE.get_or_init(Default::default)
}

fn merger() -> &'static Mutex<TextMerger> {
  static MERGER: OnceLock<Mutex<TextMerger>> = OnceLock::new();
  MERGER.get_or_init(Default::default)
}

/// Called once from `run`: asks the soft keyboard for plain, immediate characters (no suggestions,
/// no composing), so that each key arrives as text we can use at once.
pub(crate) fn init(app: &AndroidApp) {
  let _ = APP.set(app.clone());
  app.set_ime_editor_info(
    InputType::TYPE_CLASS_TEXT | InputType::TYPE_TEXT_VARIATION_VISIBLE_PASSWORD | InputType::TYPE_TEXT_FLAG_NO_SUGGESTIONS,
    TextInputAction::Unspecified,
    ImeOptions::IME_FLAG_NO_FULLSCREEN,
  );
}

fn push_java_text(text: String) {
  java_text().lock().unwrap_or_else(|e| e.into_inner()).push_back(text);
  crate::pick::wake();
}

/// `MainActivity.nativeOnTypedKey(...)`: a key went down. Java reports what it can tell about the
/// key; `text_for_key` decides whether it types anything.
#[no_mangle]
pub extern "system" fn Java_ai_craft_android_MainActivity_nativeOnTypedKey(_env: JNIEnv, _this: JObject, key_code: jint, unicode: jint, unicode_no_alt: jint, shift: jboolean, caps_lock: jboolean, ctrl_or_meta: jboolean) {
  if let Some(text) = text_for_key(key_code, unicode, unicode_no_alt, shift != 0, caps_lock != 0, ctrl_or_meta != 0) {
    push_java_text(text);
  }
}

/// `MainActivity.nativeOnTypedText(text)`: a multi-character key event (some keyboards send these).
#[no_mangle]
pub extern "system" fn Java_ai_craft_android_MainActivity_nativeOnTypedText(mut env: JNIEnv, _this: JObject, text: JString) {
  let text: Option<String> = env.get_string(&text).ok().map(String::from);
  if let Some(text) = text.map(|t| sanitize_ime_text(&t)).filter(|t| !t.is_empty()) {
    push_java_text(text);
  }
}

/// Text the soft keyboard committed through the IME since the last call. The buffer is emptied so
/// that every commit is new text, and the keyboard sees an empty field (it then sends Backspace
/// as a key press, which winit does handle).
fn take_ime_text() -> Option<String> {
  let app = APP.get()?;
  let state = app.text_input_state();
  if state.text.is_empty() {
    return None;
  }
  app.set_text_input_state(TextInputState::default());
  Some(sanitize_ime_text(&state.text)).filter(|t| !t.is_empty())
}

/// Call from `eframe::App::raw_input_hook`: makes typed text arrive once, whichever route it took.
///
/// * winit's own text events are de-duplicated against the characters Java reported;
/// * characters only Java saw are added as text events;
/// * text from the soft keyboard's IME is added as text events;
/// * while a text field has focus, frames keep coming a few times a second, because IME text is
///   found by polling.
pub fn process_raw_input(ctx: &egui::Context, raw: &mut egui::RawInput) {
  let now = Instant::now();
  {
    let mut merger = merger().lock().unwrap_or_else(|e| e.into_inner());
    raw.events.retain(|event| match event {
      egui::Event::Text(text) => merger.from_winit(text, now),
      _ => true,
    });
    let typed: Vec<String> = java_text().lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
    for text in typed {
      if merger.from_java(&text, now) {
        raw.events.push(egui::Event::Text(text));
      }
    }
  }
  if let Some(text) = take_ime_text() {
    raw.events.push(egui::Event::Text(text));
  }
  if ctx.egui_wants_keyboard_input() {
    ctx.request_repaint_after(Duration::from_millis(50));
  }
}
