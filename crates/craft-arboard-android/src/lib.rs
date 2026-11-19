//! The part of `arboard` the app uses, on Android, where arboard has no backend: text, HTML, images
//! and file lists on the system clipboard, through `craft-android`'s ClipboardManager bridge.
//!
//! Android only lets the foreground app read the clipboard, so reads made in the background find
//! nothing ([`Error::ContentNotAvailable`]).

use std::borrow::Cow;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum Error {
    ContentNotAvailable,
    ClipboardNotSupported,
    ClipboardOccupied,
    ConversionFailure,
    Unknown { description: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::ContentNotAvailable => f.write_str("the clipboard has nothing of that kind"),
            Error::ClipboardNotSupported => f.write_str("the clipboard isn't available"),
            Error::ClipboardOccupied => f.write_str("the clipboard is in use"),
            Error::ConversionFailure => f.write_str("the clipboard content couldn't be converted"),
            Error::Unknown { description } => write!(f, "clipboard error: {description}"),
        }
    }
}

impl std::error::Error for Error {}

fn failed(description: String) -> Error {
    Error::Unknown { description }
}

/// RGBA pixels (straight alpha), as in arboard.
#[derive(Clone, Debug)]
pub struct ImageData<'a> {
    pub width: usize,
    pub height: usize,
    pub bytes: Cow<'a, [u8]>,
}

#[derive(Debug)]
pub struct Clipboard;

impl Clipboard {
    pub fn new() -> Result<Self, Error> {
        Ok(Self)
    }

    pub fn get_text(&mut self) -> Result<String, Error> {
        backend::text().ok_or(Error::ContentNotAvailable)
    }

    pub fn set_text<'a, T: Into<Cow<'a, str>>>(&mut self, text: T) -> Result<(), Error> {
        backend::set_text(&text.into()).map_err(failed)
    }

    pub fn get_image(&mut self) -> Result<ImageData<'static>, Error> {
        backend::image().ok_or(Error::ContentNotAvailable)
    }

    pub fn set_image(&mut self, image: ImageData<'_>) -> Result<(), Error> {
        if image.width == 0 || image.height == 0 || image.bytes.len() != image.width * image.height * 4 {
            return Err(Error::ConversionFailure);
        }
        backend::set_image(image.width, image.height, &image.bytes).map_err(failed)
    }

    pub fn clear(&mut self) -> Result<(), Error> {
        backend::set_text("").map_err(failed)
    }

    /// The builder for writing other formats, as in arboard.
    pub fn set(&mut self) -> Set<'_> {
        Set(std::marker::PhantomData)
    }

    /// The builder for reading other formats, as in arboard.
    pub fn get(&mut self) -> Get<'_> {
        Get(std::marker::PhantomData)
    }
}

/// `Clipboard::set()`.
pub struct Set<'clipboard>(std::marker::PhantomData<&'clipboard mut Clipboard>);

impl Set<'_> {
    pub fn text<'a, T: Into<Cow<'a, str>>>(self, text: T) -> Result<(), Error> {
        backend::set_text(&text.into()).map_err(failed)
    }

    pub fn html<'a, T: Into<Cow<'a, str>>>(self, html: T, alt: Option<T>) -> Result<(), Error> {
        let html = html.into();
        let alt = alt.map(Into::into);
        backend::set_html(&html, alt.as_deref()).map_err(failed)
    }

    pub fn image(self, image: ImageData<'_>) -> Result<(), Error> {
        Clipboard.set_image(image)
    }

    /// Only files inside the app's own folders can be shared through the Android clipboard.
    pub fn file_list<P: AsRef<Path>>(self, files: &[P]) -> Result<(), Error> {
        let paths: Vec<PathBuf> = files.iter().map(|p| p.as_ref().to_path_buf()).collect();
        backend::set_files(&paths).map_err(failed)
    }
}

/// `Clipboard::get()`.
pub struct Get<'clipboard>(std::marker::PhantomData<&'clipboard mut Clipboard>);

impl Get<'_> {
    pub fn text(self) -> Result<String, Error> {
        Clipboard.get_text()
    }

    pub fn html(self) -> Result<String, Error> {
        backend::html().ok_or(Error::ContentNotAvailable)
    }

    pub fn image(self) -> Result<ImageData<'static>, Error> {
        Clipboard.get_image()
    }

    pub fn file_list(self) -> Result<Vec<PathBuf>, Error> {
        let files = backend::files();
        if files.is_empty() {
            Err(Error::ContentNotAvailable)
        } else {
            Ok(files)
        }
    }
}

#[cfg(target_os = "android")]
mod backend {
    use std::path::PathBuf;

    use super::ImageData;

    pub fn text() -> Option<String> {
        craft_android::clipboard_text()
    }

    pub fn set_text(text: &str) -> Result<(), String> {
        craft_android::set_clipboard_text(text)
    }

    pub fn html() -> Option<String> {
        craft_android::clipboard_html()
    }

    pub fn set_html(html: &str, alt: Option<&str>) -> Result<(), String> {
        craft_android::set_clipboard_html(html, alt)
    }

    pub fn image() -> Option<ImageData<'static>> {
        craft_android::clipboard_image().map(|i| ImageData { width: i.width, height: i.height, bytes: i.rgba.into() })
    }

    pub fn set_image(width: usize, height: usize, rgba: &[u8]) -> Result<(), String> {
        craft_android::set_clipboard_image(width, height, rgba)
    }

    pub fn files() -> Vec<PathBuf> {
        craft_android::clipboard_files()
    }

    pub fn set_files(paths: &[PathBuf]) -> Result<(), String> {
        craft_android::set_clipboard_files(paths)
    }
}

/// Off Android there is no clipboard here: this exists so the app's code can be type-checked on a
/// desktop.
#[cfg(not(target_os = "android"))]
mod backend {
    use std::path::PathBuf;

    use super::ImageData;

    const NONE: &str = "no clipboard off Android";

    pub fn text() -> Option<String> {
        None
    }

    pub fn set_text(_text: &str) -> Result<(), String> {
        Err(NONE.to_owned())
    }

    pub fn html() -> Option<String> {
        None
    }

    pub fn set_html(_html: &str, _alt: Option<&str>) -> Result<(), String> {
        Err(NONE.to_owned())
    }

    pub fn image() -> Option<ImageData<'static>> {
        None
    }

    pub fn set_image(_width: usize, _height: usize, _rgba: &[u8]) -> Result<(), String> {
        Err(NONE.to_owned())
    }

    pub fn files() -> Vec<PathBuf> {
        Vec::new()
    }

    pub fn set_files(_paths: &[PathBuf]) -> Result<(), String> {
        Err(NONE.to_owned())
    }
}
