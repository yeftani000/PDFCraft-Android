//! PdfCraft for Android: the entry point the Kotlin GameActivity loads (`libpdfcraft_android.so`).
//!
//! The app is `pdfcraft_ui_egui::PdfCraftApp`, built the way the web entry point builds it (no
//! native services) and wrapped in [`AndroidShell`], which adds what Android needs around it:
//!
//! * **Settings.** eframe has no storage directory on Android, so the shell keeps the app's
//!   settings in a file of its own: restored at start, saved every few seconds and whenever the app
//!   loses focus (Android can end a backgrounded app without warning).
//! * **Clipboard.** Text the app copies goes to the system clipboard, and a paste (Ctrl+V on a
//!   hardware keyboard) pastes what is on it. The app's image and file clipboard features use the
//!   `arboard` stand-in, which talks to the same clipboard.
//! * **Typing.** Characters reach egui by three routes (winit's key translation, Java's resolved
//!   characters, the soft keyboard's IME); the shell merges them so each is typed once.
//! * **Opening.** "Open with PdfCraft" documents are opened by path, like documents chosen in the
//!   app, so saving writes back to the original (see MainActivity.kt).
//!
//! File pickers are `rfd::AsyncFileDialog` calls that resolve to `craft-rfd-android`.
#![cfg(target_os = "android")]

use std::time::{Duration, Instant};

use craft_android::{AndroidApp, JsonFileStorage};
use eframe::egui;
use pdfcraft_ui_egui::PdfCraftApp;

/// The key `PdfCraftApp::save` stores its settings under.
const SETTINGS_KEY: &str = "pdfcraft";
/// How often settings are written (if they changed).
const SAVE_EVERY: Duration = Duration::from_secs(5);

struct AndroidShell {
    app: PdfCraftApp,
    storage: JsonFileStorage,
    last_save: Instant,
    was_focused: bool,
}

impl AndroidShell {
    fn new() -> Self {
        let mut storage = JsonFileStorage::open(craft_android::files_dir().join("settings.json"));
        let mut app = PdfCraftApp::new();
        if let Some(json) = storage.get_string(SETTINGS_KEY) {
            app.restore(&json);
        }
        // `restore` may have changed nothing worth keeping, but make sure the file exists.
        storage.flush();
        Self { app, storage, last_save: Instant::now(), was_focused: true }
    }

    fn save_now(&mut self) {
        eframe::App::save(&mut self.app, &mut self.storage);
        self.storage.flush();
        self.last_save = Instant::now();
    }

    /// Writes settings every [`SAVE_EVERY`], and at once when the app loses focus.
    fn autosave(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let lost_focus = self.was_focused && !focused;
        self.was_focused = focused;
        if lost_focus || self.last_save.elapsed() >= SAVE_EVERY {
            self.save_now();
        }
        // An idle app draws no frames; wake up in time for the next save.
        ctx.request_repaint_after(SAVE_EVERY);
    }

    /// Opens documents handed over by "Open with" / Share.
    fn open_requested_documents(&mut self) {
        for paths in craft_android::take_picked(craft_android::OPEN_KEY) {
            for path in paths {
                let _ = self.app.open_path(&path.to_string_lossy());
            }
        }
    }
}

impl eframe::App for AndroidShell {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.app.raw_input_hook(ctx, raw);
        paste_from_system(raw);
        // Typed text from every route (see craft_android::process_raw_input), exactly once.
        craft_android::process_raw_input(ctx, raw);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.app.save(storage);
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        self.save_now();
        self.app.on_exit(gl);
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.logic(ctx, frame);
        self.open_requested_documents();
        self.autosave(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.app.ui(ui, frame);
        copy_to_system(ui.ctx());
    }
}

/// A paste (egui turns Ctrl+V into `Event::Paste`) carries the system clipboard's text rather than
/// whatever egui last copied itself.
fn paste_from_system(raw: &mut egui::RawInput) {
    if !raw.events.iter().any(|e| matches!(e, egui::Event::Paste(_))) {
        return;
    }
    if let Some(text) = craft_android::clipboard_text() {
        for event in &mut raw.events {
            if let egui::Event::Paste(pasted) = event {
                pasted.clone_from(&text);
            }
        }
    }
}

/// Text and images the app copied this frame go to the system clipboard.
fn copy_to_system(ctx: &egui::Context) {
    let commands = ctx.output(|o| o.commands.clone());
    for command in commands {
        match command {
            egui::OutputCommand::CopyText(text) => {
                if let Err(err) = craft_android::set_clipboard_text(&text) {
                    log::warn!("couldn't copy text: {err}");
                }
            }
            egui::OutputCommand::CopyImage(image) => {
                let rgba: Vec<u8> = image.pixels.iter().flat_map(|pixel| pixel.to_srgba_unmultiplied()).collect();
                if let Err(err) = craft_android::set_clipboard_image(image.size[0], image.size[1], &rgba) {
                    log::warn!("couldn't copy an image: {err}");
                }
            }
            _ => {}
        }
    }
}

fn build_app(_cc: &eframe::CreationContext<'_>) -> Box<dyn eframe::App> {
    Box::new(AndroidShell::new())
}

#[no_mangle]
pub fn android_main(app: AndroidApp) {
    // As in the desktop `main`: have aws-lc (signatures, TLS) detect the CPU's features now.
    pdfcraft_engine::sign::init_crypto();
    craft_android::run(app, "PdfCraft", build_app);
}
