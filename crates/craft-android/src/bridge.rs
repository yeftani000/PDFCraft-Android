//! The JNI side of `MainActivity.kt` (package `ai.craft.android`, the same for every app).
//!
//! Rust runs on the native glue's own thread, never the Java UI thread. Calls into Java are plain
//! JNI method calls on the activity; Java calls back through `nativeOnPicked`.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use jni::objects::{GlobalRef, JObject, JObjectArray, JString, JValue};
use jni::sys::jint;
use jni::{JNIEnv, JavaVM};

struct Bridge {
  vm: JavaVM,
  activity: GlobalRef,
  files: PathBuf,
}

static BRIDGE: OnceLock<Bridge> = OnceLock::new();

/// # Safety
/// `vm` and `activity` must be the `JavaVM` and `Activity` pointers handed out by `AndroidApp`.
pub(crate) unsafe fn init(vm: *mut c_void, activity: *mut c_void, internal_data_path: Option<PathBuf>) {
  let Ok(vm) = JavaVM::from_raw(vm.cast()) else {
    log::error!("The JavaVM pointer was invalid; Android features are disabled");
    return;
  };
  let activity = {
    let Ok(env) = vm.attach_current_thread() else {
      log::error!("Couldn't attach the native thread to the JVM");
      return;
    };
    match env.new_global_ref(JObject::from_raw(activity.cast())) {
      Ok(global) => global,
      Err(err) => {
        log::error!("Couldn't keep a reference to the activity: {err}");
        return;
      },
    }
  };
  let files = internal_data_path.unwrap_or_else(|| PathBuf::from("/data/local/tmp"));
  let _ = BRIDGE.set(Bridge { vm, activity, files });
}

/// Runs `f` with a JNI environment for this thread and the activity; a Java exception becomes an
/// error (and is cleared, so later calls still work).
pub(crate) fn with_activity<R>(f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<R>) -> Result<R, String> {
  let bridge = BRIDGE.get().ok_or_else(|| "the Android bridge isn't initialised".to_owned())?;
  let mut env = bridge.vm.attach_current_thread().map_err(|e| e.to_string())?;
  let result = f(&mut env, bridge.activity.as_obj());
  if env.exception_check().unwrap_or(false) {
    let _ = env.exception_describe();
    let _ = env.exception_clear();
    return Err("a Java exception was thrown".to_owned());
  }
  result.map_err(|e| e.to_string())
}

/// The app's private files directory.
pub fn files_dir() -> PathBuf {
  BRIDGE.get().map(|b| b.files.clone()).unwrap_or_else(|| PathBuf::from("."))
}

/// A folder to write downloads into before [`publish_media`] moves them to the gallery.
pub fn downloads_dir() -> Option<PathBuf> {
  let dir = files_dir().join("downloads");
  std::fs::create_dir_all(&dir).ok()?;
  Some(dir)
}

/// Opens a web link or media URL in whichever app handles it.
pub fn open_url(url: &str) -> Result<(), String> {
  let opened = with_activity(|env, activity| {
    let url = env.new_string(url)?;
    env.call_method(activity, "openUrl", "(Ljava/lang/String;)Z", &[JValue::Object(&url)])?.z()
  })?;
  if opened {
    Ok(())
  } else {
    Err("no app can open this link".to_owned())
  }
}

/// Copies a finished file into Pictures, Movies, Music or Downloads (in a folder named after the
/// app) so it shows in the gallery; Java does it on a background thread and then deletes the
/// private copy.
pub fn publish_media(path: &Path) {
  let Some(path) = path.to_str() else {
    return;
  };
  if let Err(err) = with_activity(|env, activity| {
    let path = env.new_string(path)?;
    env.call_method(activity, "publishMedia", "(Ljava/lang/String;)V", &[JValue::Object(&path)])?.v()
  }) {
    log::warn!("Couldn't publish the file to the gallery: {err}");
  }
}

pub(crate) fn start_pick(key: u32, mime_types: &[&str], multiple: bool) -> Result<(), String> {
  with_activity(|env, activity| {
    let string_class = env.find_class("java/lang/String")?;
    let array = env.new_object_array(mime_types.len() as jint, &string_class, JObject::null())?;
    for (i, mime) in mime_types.iter().enumerate() {
      let mime = env.new_string(mime)?;
      env.set_object_array_element(&array, i as jint, mime)?;
    }
    env.call_method(activity, "requestPick", "(I[Ljava/lang/String;Z)V", &[JValue::Int(key as jint), JValue::Object(&array), JValue::Bool(multiple.into())])?.v()
  })
}

/// `MainActivity.nativeOnPicked(key: Int, paths: Array<String>)`: the user finished (or cancelled)
/// choosing files. The paths are copies in the app's cache, named like the originals.
#[no_mangle]
pub extern "system" fn Java_ai_craft_android_MainActivity_nativeOnPicked(mut env: JNIEnv, _this: JObject, key: jint, paths: JObjectArray) {
  let mut picked = Vec::new();
  let count = env.get_array_length(&paths).unwrap_or(0);
  for i in 0..count {
    let Ok(element) = env.get_object_array_element(&paths, i) else {
      continue;
    };
    let element = JString::from(element);
    let text: Option<String> = env.get_string(&element).ok().map(String::from);
    if let Some(text) = text {
      picked.push(PathBuf::from(text));
    }
  }
  crate::pick::post_picked(key as u32, picked);
}

/// Starts a thread that publishes whatever the app saves into [`downloads_dir`] (see
/// [`publish_media`]) once a file has stopped changing. Apps that write a file and never say when
/// they're done (like those using a save dialog) need nothing else to get their output into the
/// gallery or the Downloads folder.
pub(crate) fn start_publish_watcher() {
  use std::collections::HashSet;
  use std::time::{Duration, SystemTime};
  std::thread::spawn(|| {
    let mut seen: HashSet<(PathBuf, u64, SystemTime)> = HashSet::new();
    loop {
      std::thread::sleep(Duration::from_secs(3));
      let Some(dir) = downloads_dir() else {
        continue;
      };
      let Ok(entries) = std::fs::read_dir(&dir) else {
        continue;
      };
      for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
          continue;
        };
        let Ok(modified) = meta.modified() else {
          continue;
        };
        let settled = modified.elapsed().map(|age| age >= Duration::from_secs(3)).unwrap_or(false);
        if meta.is_file() && meta.len() > 0 && settled && seen.insert((entry.path(), meta.len(), modified)) {
          publish_media(&entry.path());
        }
      }
    }
  });
}
