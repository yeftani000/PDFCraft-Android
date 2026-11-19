//! Settings storage: eframe has no storage directory on Android, so apps that keep their settings
//! through `eframe::Storage` (`App::save`) use this file-backed one instead.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Key/value strings kept in one JSON file, written atomically (temporary file, then rename).
pub struct JsonFileStorage {
  path: PathBuf,
  values: BTreeMap<String, String>,
  dirty: bool,
}

impl JsonFileStorage {
  /// Loads `path` if it exists. A missing or unreadable file starts empty; an unreadable one is
  /// kept as `<name>.bad` so it isn't lost when the next save overwrites it.
  pub fn open(path: impl Into<PathBuf>) -> Self {
    let path = path.into();
    let values = match std::fs::read_to_string(&path) {
      Ok(text) => match serde_json::from_str::<BTreeMap<String, String>>(&text) {
        Ok(values) => values,
        Err(err) => {
          log::warn!("Settings file {} is unreadable ({err}); starting fresh", path.display());
          let _ = std::fs::rename(&path, path.with_extension("bad"));
          BTreeMap::new()
        },
      },
      Err(_) => BTreeMap::new(),
    };
    Self { path, values, dirty: false }
  }

  pub fn get_string(&self, key: &str) -> Option<String> {
    self.values.get(key).cloned()
  }

  /// Stores `value`; nothing is written until [`flush`](Self::flush), and only if it changed.
  pub fn set_string(&mut self, key: &str, value: String) {
    if self.values.get(key) != Some(&value) {
      self.values.insert(key.to_owned(), value);
      self.dirty = true;
    }
  }

  /// Forgets `key`.
  pub fn remove_string(&mut self, key: &str) {
    if self.values.remove(key).is_some() {
      self.dirty = true;
    }
  }

  /// Writes the file if anything changed since the last write.
  pub fn flush(&mut self) {
    if !self.dirty {
      return;
    }
    let Ok(text) = serde_json::to_string_pretty(&self.values) else {
      return;
    };
    if let Some(dir) = self.path.parent() {
      let _ = std::fs::create_dir_all(dir);
    }
    let temporary = self.path.with_extension("tmp");
    match std::fs::write(&temporary, text).and_then(|()| std::fs::rename(&temporary, &self.path)) {
      Ok(()) => self.dirty = false,
      Err(err) => log::warn!("Couldn't save settings to {}: {err}", self.path.display()),
    }
  }
}

impl eframe::Storage for JsonFileStorage {
  fn get_string(&self, key: &str) -> Option<String> {
    JsonFileStorage::get_string(self, key)
  }

  fn set_string(&mut self, key: &str, value: String) {
    JsonFileStorage::set_string(self, key, value);
  }

  fn remove_string(&mut self, key: &str) {
    JsonFileStorage::remove_string(self, key);
  }

  fn flush(&mut self) {
    JsonFileStorage::flush(self);
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn temp_file(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("craft-storage-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("settings.json")
  }

  #[test]
  fn values_survive_a_restart() {
    let path = temp_file("roundtrip");
    let mut first = JsonFileStorage::open(&path);
    first.set_string("pdfcraft", "{\"zoom\":1.5}".to_owned());
    first.set_string("other", "x".to_owned());
    first.flush();
    let second = JsonFileStorage::open(&path);
    assert_eq!(second.get_string("pdfcraft").as_deref(), Some("{\"zoom\":1.5}"));
    assert_eq!(second.get_string("other").as_deref(), Some("x"));
    assert_eq!(second.get_string("missing"), None);
  }

  #[test]
  fn an_unchanged_value_does_not_rewrite_the_file() {
    let path = temp_file("unchanged");
    let mut storage = JsonFileStorage::open(&path);
    storage.set_string("k", "v".to_owned());
    storage.flush();
    std::fs::remove_file(&path).unwrap();
    storage.set_string("k", "v".to_owned());
    storage.flush();
    assert!(!path.exists(), "setting the same value again must not write");
    storage.set_string("k", "w".to_owned());
    storage.flush();
    assert!(path.exists());
  }

  #[test]
  fn removed_keys_stay_removed() {
    let path = temp_file("remove");
    let mut storage = JsonFileStorage::open(&path);
    storage.set_string("a", "1".to_owned());
    storage.set_string("b", "2".to_owned());
    storage.flush();
    storage.remove_string("a");
    storage.remove_string("never-there");
    storage.flush();
    let reopened = JsonFileStorage::open(&path);
    assert_eq!(reopened.get_string("a"), None);
    assert_eq!(reopened.get_string("b").as_deref(), Some("2"));
  }

  #[test]
  fn a_corrupt_file_is_kept_aside_and_ignored() {
    let path = temp_file("corrupt");
    std::fs::write(&path, "{ not json").unwrap();
    let storage = JsonFileStorage::open(&path);
    assert_eq!(storage.get_string("anything"), None);
    assert!(path.with_extension("bad").exists());
  }
}
