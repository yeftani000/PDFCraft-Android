//! The system clipboard, through `MainActivity`'s ClipboardManager methods.
//!
//! Android only lets the foreground app read the clipboard (from Android 10), so reads made while
//! the app is in the background simply find nothing.

use std::path::PathBuf;

use jni::objects::{JByteArray, JObject, JObjectArray, JString, JValue};
use jni::sys::jint;
use jni::JNIEnv;

use crate::bridge::with_activity;

/// An image from the clipboard: straight (not premultiplied) RGBA, `width * height * 4` bytes.
#[derive(Clone, Debug)]
pub struct ClipImage {
  pub width: usize,
  pub height: usize,
  pub rgba: Vec<u8>,
}

fn read_string(env: &mut JNIEnv, object: JObject) -> jni::errors::Result<Option<String>> {
  if object.is_null() {
    return Ok(None);
  }
  let text: String = env.get_string(&JString::from(object))?.into();
  Ok(Some(text))
}

/// Plain text on the clipboard (a rich item is flattened to text by Android).
pub fn clipboard_text() -> Option<String> {
  with_activity(|env, activity| {
    let object = env.call_method(activity, "getClipboardText", "()Ljava/lang/String;", &[])?.l()?;
    read_string(env, object)
  })
  .ok()
  .flatten()
}

/// Replaces the clipboard with `text`.
pub fn set_clipboard_text(text: &str) -> Result<(), String> {
  with_activity(|env, activity| {
    let text = env.new_string(text)?;
    env.call_method(activity, "setClipboardText", "(Ljava/lang/String;)Z", &[JValue::Object(&text)])?.z()
  })
  .and_then(accepted)
}

/// HTML on the clipboard, if the current item has any.
pub fn clipboard_html() -> Option<String> {
  with_activity(|env, activity| {
    let object = env.call_method(activity, "getClipboardHtml", "()Ljava/lang/String;", &[])?.l()?;
    read_string(env, object)
  })
  .ok()
  .flatten()
}

/// Replaces the clipboard with `html`, with `alt` (or the HTML itself) as its plain-text form.
pub fn set_clipboard_html(html: &str, alt: Option<&str>) -> Result<(), String> {
  with_activity(|env, activity| {
    let html_text = env.new_string(html)?;
    let alt_text = env.new_string(alt.unwrap_or(html))?;
    env.call_method(activity, "setClipboardHtml", "(Ljava/lang/String;Ljava/lang/String;)Z", &[JValue::Object(&html_text), JValue::Object(&alt_text)])?.z()
  })
  .and_then(accepted)
}

/// An image on the clipboard (a bitmap or an image URI), decoded to RGBA.
pub fn clipboard_image() -> Option<ClipImage> {
  let bytes: Vec<u8> = with_activity(|env, activity| {
    let object = env.call_method(activity, "getClipboardImage", "()[B", &[])?.l()?;
    if object.is_null() {
      return Ok(Vec::new());
    }
    env.convert_byte_array(JByteArray::from(object))
  })
  .ok()?;
  parse_image(&bytes)
}

/// Replaces the clipboard with an image (`rgba` is straight RGBA, `width * height * 4` bytes).
pub fn set_clipboard_image(width: usize, height: usize, rgba: &[u8]) -> Result<(), String> {
  if width == 0 || height == 0 || rgba.len() != width * height * 4 {
    return Err("the image data doesn't match its size".to_owned());
  }
  with_activity(|env, activity| {
    let data = env.byte_array_from_slice(rgba)?;
    env.call_method(activity, "setClipboardImage", "(II[B)Z", &[JValue::Int(width as jint), JValue::Int(height as jint), JValue::Object(&data)])?.z()
  })
  .and_then(accepted)
}

/// Files on the clipboard, copied into the app's cache.
pub fn clipboard_files() -> Vec<PathBuf> {
  with_activity(|env, activity| {
    let object = env.call_method(activity, "getClipboardFiles", "()[Ljava/lang/String;", &[])?.l()?;
    if object.is_null() {
      return Ok(Vec::new());
    }
    let array = JObjectArray::from(object);
    let count = env.get_array_length(&array)?;
    let mut paths = Vec::new();
    for i in 0..count {
      let element = env.get_object_array_element(&array, i)?;
      if let Some(path) = read_string(env, element)? {
        paths.push(PathBuf::from(path));
      }
    }
    Ok(paths)
  })
  .unwrap_or_default()
}

/// Replaces the clipboard with these files (they must be inside the app's own folders).
pub fn set_clipboard_files(paths: &[PathBuf]) -> Result<(), String> {
  let strings: Vec<String> = paths.iter().filter_map(|p| p.to_str().map(str::to_owned)).collect();
  with_activity(|env, activity| {
    let string_class = env.find_class("java/lang/String")?;
    let array = env.new_object_array(strings.len() as jint, &string_class, JObject::null())?;
    for (i, path) in strings.iter().enumerate() {
      let path = env.new_string(path)?;
      env.set_object_array_element(&array, i as jint, path)?;
    }
    env.call_method(activity, "setClipboardFiles", "([Ljava/lang/String;)Z", &[JValue::Object(&array)])?.z()
  })
  .and_then(accepted)
}

fn accepted(ok: bool) -> Result<(), String> {
  if ok {
    Ok(())
  } else {
    Err("the clipboard couldn't be changed".to_owned())
  }
}

/// Java returns `width (4 bytes, big-endian) + height (4 bytes) + RGBA pixels`; empty means none.
fn parse_image(bytes: &[u8]) -> Option<ClipImage> {
  if bytes.len() < 8 {
    return None;
  }
  let width = u32::from_be_bytes(bytes[0..4].try_into().ok()?) as usize;
  let height = u32::from_be_bytes(bytes[4..8].try_into().ok()?) as usize;
  let rgba = &bytes[8..];
  (width > 0 && height > 0 && rgba.len() == width.checked_mul(height)?.checked_mul(4)?).then(|| ClipImage { width, height, rgba: rgba.to_vec() })
}

#[cfg(test)]
mod tests {
  use super::*;

  fn encoded(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let mut bytes = width.to_be_bytes().to_vec();
    bytes.extend(height.to_be_bytes());
    bytes.extend(pixels);
    bytes
  }

  #[test]
  fn a_well_formed_image_parses() {
    let image = parse_image(&encoded(2, 1, &[1, 2, 3, 4, 5, 6, 7, 8])).unwrap();
    assert_eq!((image.width, image.height), (2, 1));
    assert_eq!(image.rgba, vec![1, 2, 3, 4, 5, 6, 7, 8]);
  }

  #[test]
  fn malformed_images_are_rejected() {
    assert!(parse_image(&[]).is_none());
    assert!(parse_image(&[0; 7]).is_none());
    assert!(parse_image(&encoded(2, 2, &[0; 8])).is_none(), "too few pixels for the size");
    assert!(parse_image(&encoded(0, 5, &[])).is_none());
    assert!(parse_image(&encoded(u32::MAX, u32::MAX, &[0; 4])).is_none(), "size overflow");
  }
}
