#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(not(any(windows, target_os = "linux")))]
compile_error!("Orbit currently supports Windows and Linux only.");

mod app;
mod coder;
mod context;
mod diagnostics;
mod media;
mod pipeline;
mod providers;
mod runner;
mod secure_store;
mod security;
mod session;
mod storage;
mod tools;
mod ui;
mod workspace;

#[cfg(test)]
mod e2e;

use app::App;
use eframe::egui;

enum BootState {
    Ready(Box<App>),
    Failed(String),
}

struct OpenChatApp {
    inner: BootState,
}

impl OpenChatApp {
    fn boot() -> Self {
        match App::new() {
            Ok(app) => Self {
                inner: BootState::Ready(Box::new(app)),
            },
            Err(e) => {
                tracing::error!("failed to initialize app: {e:#}");
                Self {
                    inner: BootState::Failed(format!("{e:#}")),
                }
            }
        }
    }

    fn retry(&mut self) {
        *self = Self::boot();
    }
}

impl eframe::App for OpenChatApp {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let BootState::Ready(app) = &mut self.inner {
            app.kill_all_runs();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let BootState::Ready(app) = &mut self.inner {
            crate::ui::render(app, ui);
            return;
        }
        let message = match &self.inner {
            BootState::Failed(message) => message.clone(),
            BootState::Ready(_) => return,
        };
        if crate::ui::render_init_error(ui, &message) {
            self.retry();
        }
    }
}

fn init_tracing() {
    use crate::security::redact::RedactingWriter;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let log_path = crate::storage::data_dir().map(|dir| {
        let _ = std::fs::create_dir_all(&dir);
        dir.join("orbit.log")
    });
    let file = log_path.and_then(|path| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
    });
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(move || {
            let tee = Tee {
                stderr: std::io::stderr(),
                file: file.as_ref().and_then(|f| f.try_clone().ok()),
            };
            RedactingWriter::new(tee)
        })
        .init();
}

struct Tee {
    stderr: std::io::Stderr,
    file: Option<std::fs::File>,
}

impl std::io::Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::Write::write_all(&mut self.stderr, buf);
        if let Some(file) = &mut self.file {
            let _ = std::io::Write::write_all(file, buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::Write::flush(&mut self.stderr);
        if let Some(file) = &mut self.file {
            let _ = std::io::Write::flush(file);
        }
        Ok(())
    }
}

fn load_icon() -> egui::IconData {
    let bytes = include_bytes!("../assets/icon.png");
    let image = image::load_from_memory(bytes)
        .expect("failed to decode icon.png")
        .into_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}

fn main() -> eframe::Result<()> {
    // Aqui é o fruto do trabalho do Orbit
    init_tracing();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([720.0, 480.0])
            .with_icon(load_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "Orbit",
        native_options,
        Box::new(|cc| {
            crate::ui::theme::install_fonts(&cc.egui_ctx);
            Ok(Box::new(OpenChatApp::boot()))
        }),
    )
}
