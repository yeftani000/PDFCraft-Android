//! The part of `rfd` the Craft apps use, on Android: [`AsyncFileDialog`] and [`FileHandle`].
//!
//! * `pick_file` / `pick_files` open Android's document picker. The chosen documents arrive as
//!   copies in the app's cache under their original names, so [`FileHandle::path`] is a real path.
//! * `save_file` hands back a path inside the app's downloads folder (made unique, so nothing is
//!   overwritten); whatever the app writes there is published to the device's Downloads, Pictures,
//!   Movies or Music folder once the file stops changing (craft-android does that).
//! * `pick_folder` returns that same downloads folder: Android has no folder picker an app can
//!   write through, and this is where exported files end up.

use std::future::Future;
use std::path::{Path, PathBuf};

/// A file or folder the user chose (or where a save goes).
#[derive(Clone, Debug)]
pub struct FileHandle(PathBuf);

impl FileHandle {
    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn file_name(&self) -> String {
        self.0.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    pub async fn read(&self) -> Vec<u8> {
        std::fs::read(&self.0).unwrap_or_default()
    }

    pub async fn write(&self, data: &[u8]) -> std::io::Result<()> {
        std::fs::write(&self.0, data)
    }
}

impl From<PathBuf> for FileHandle {
    fn from(path: PathBuf) -> Self {
        Self(path)
    }
}

impl From<FileHandle> for PathBuf {
    fn from(handle: FileHandle) -> Self {
        handle.0
    }
}

/// Builds a picker the way `rfd::AsyncFileDialog` does. Only the options the apps use are kept.
#[derive(Clone, Debug, Default)]
pub struct AsyncFileDialog {
    title: Option<String>,
    file_name: Option<String>,
    filters: Vec<(String, Vec<String>)>,
    directory: Option<PathBuf>,
}

impl AsyncFileDialog {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn set_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    #[must_use]
    pub fn set_file_name(mut self, file_name: impl Into<String>) -> Self {
        self.file_name = Some(file_name.into());
        self
    }

    #[must_use]
    pub fn set_directory<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.directory = Some(path.as_ref().to_path_buf());
        self
    }

    #[must_use]
    pub fn add_filter(mut self, name: impl Into<String>, extensions: &[impl ToString]) -> Self {
        self.filters.push((name.into(), extensions.iter().map(ToString::to_string).collect()));
        self
    }

    /// Choose one existing file.
    pub fn pick_file(self) -> impl Future<Output = Option<FileHandle>> + Send {
        let pick = backend::pick(&self.filters, false);
        async move { pick.await.into_iter().next().map(FileHandle) }
    }

    /// Choose one or more existing files.
    pub fn pick_files(self) -> impl Future<Output = Option<Vec<FileHandle>>> + Send {
        let pick = backend::pick(&self.filters, true);
        async move {
            let files: Vec<FileHandle> = pick.await.into_iter().map(FileHandle).collect();
            (!files.is_empty()).then_some(files)
        }
    }

    /// Choose a folder: the app's downloads folder (see the module docs).
    pub fn pick_folder(self) -> impl Future<Output = Option<FileHandle>> + Send {
        async move { backend::downloads_dir().map(FileHandle) }
    }

    /// Choose where to save: a fresh name in the app's downloads folder (see the module docs).
    pub fn save_file(self) -> impl Future<Output = Option<FileHandle>> + Send {
        async move {
            let dir = backend::downloads_dir()?;
            let wanted = self.file_name.as_deref().map(sanitize).filter(|n| !n.is_empty()).unwrap_or_else(|| "document".to_owned());
            Some(FileHandle(unique_in(&dir, &wanted)))
        }
    }
}

/// A file name without directory parts.
fn sanitize(name: &str) -> String {
    name.rsplit(['/', '\\']).next().unwrap_or("").trim().to_owned()
}

/// `dir/name`, or `dir/name (2).ext` and so on if that already exists.
fn unique_in(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let path = Path::new(name);
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| name.to_owned());
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|candidate| !candidate.exists())
        .unwrap_or(first)
}

/// MIME types for an Android picker from `rfd`-style filters. Extensions may be globs written to
/// match any letter case (`[pP][dD][fF]`); anything unknown (or `*`) widens the picker to every file.
fn mime_types(filters: &[(String, Vec<String>)]) -> Vec<&'static str> {
    let mut mimes: Vec<&'static str> = Vec::new();
    for (_, extensions) in filters {
        for extension in extensions {
            let mime = match mime_for(&plain_extension(extension)) {
                Some(mime) => mime,
                None => return Vec::new(),
            };
            if !mimes.contains(&mime) {
                mimes.push(mime);
            }
        }
    }
    mimes
}

/// `[pP][dD][fF]` becomes `pdf`; a plain extension is lowercased.
fn plain_extension(glob: &str) -> String {
    let mut out = String::new();
    let mut chars = glob.trim_start_matches("*.").chars().peekable();
    while let Some(c) = chars.next() {
        if c == '[' {
            if let Some(first) = chars.next() {
                out.push(first.to_ascii_lowercase());
            }
            for c in chars.by_ref() {
                if c == ']' {
                    break;
                }
            }
        } else {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

fn mime_for(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "jp2" | "j2k" => "image/jp2",
        "svg" => "image/svg+xml",
        "txt" | "text" | "md" => "text/plain",
        "csv" => "text/csv",
        "json" => "application/json",
        "xml" => "text/xml",
        "html" | "htm" => "text/html",
        "xfdf" | "fdf" => "application/vnd.adobe.xfdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "zip" => "application/zip",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "p12" | "pfx" => "application/x-pkcs12",
        "pem" | "crt" | "cer" => "application/x-x509-ca-cert",
        _ => return None,
    })
}

#[cfg(target_os = "android")]
mod backend {
    use std::future::Future;
    use std::path::PathBuf;

    pub fn pick(filters: &[(String, Vec<String>)], multiple: bool) -> impl Future<Output = Vec<PathBuf>> + Send {
        craft_android::pick_async(&super::mime_types(filters), multiple)
    }

    pub fn downloads_dir() -> Option<PathBuf> {
        craft_android::downloads_dir()
    }
}

/// Off Android there is no picker: this exists so the apps' code can be type-checked on a desktop.
#[cfg(not(target_os = "android"))]
mod backend {
    use std::future::Future;
    use std::path::PathBuf;

    pub fn pick(filters: &[(String, Vec<String>)], _multiple: bool) -> impl Future<Output = Vec<PathBuf>> + Send {
        let _ = super::mime_types(filters);
        async { Vec::new() }
    }

    pub fn downloads_dir() -> Option<PathBuf> {
        let dir = std::env::temp_dir().join("craft-rfd-android-downloads");
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(name: &str, exts: &[&str]) -> (String, Vec<String>) {
        (name.to_owned(), exts.iter().map(|e| (*e).to_owned()).collect())
    }

    #[test]
    fn any_case_globs_become_mime_types() {
        assert_eq!(plain_extension("[pP][dD][fF]"), "pdf");
        assert_eq!(plain_extension("[jJ][pP]2"), "jp2");
        assert_eq!(plain_extension("PNG"), "png");
        assert_eq!(mime_types(&[filter("PDF", &["[pP][dD][fF]"])]), vec!["application/pdf"]);
        assert_eq!(mime_types(&[filter("Images", &["png", "jpg", "[jJ][pP][eE][gG]"])]), vec!["image/png", "image/jpeg"]);
    }

    #[test]
    fn an_unknown_or_wildcard_extension_widens_to_every_file() {
        assert!(mime_types(&[filter("Any", &["*"])]).is_empty());
        assert!(mime_types(&[filter("Mixed", &["pdf", "weirdext"])]).is_empty());
        assert!(mime_types(&[]).is_empty());
    }

    #[test]
    fn save_names_never_clobber_or_escape() {
        assert_eq!(sanitize("../../etc/passwd"), "passwd");
        assert_eq!(sanitize("a\\b\\doc.pdf"), "doc.pdf");
        let dir = std::env::temp_dir().join(format!("rfd-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unique_in(&dir, "x.pdf"), dir.join("x.pdf"));
        std::fs::write(dir.join("x.pdf"), b"1").unwrap();
        assert_eq!(unique_in(&dir, "x.pdf"), dir.join("x (2).pdf"));
        std::fs::write(dir.join("x (2).pdf"), b"1").unwrap();
        assert_eq!(unique_in(&dir, "x.pdf"), dir.join("x (3).pdf"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
