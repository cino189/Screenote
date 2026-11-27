mod annotation;
mod capture;
mod cli;
mod clipboard;
mod renderer;
mod ui;

use anyhow::Result;
use clap::Parser;
use eframe::egui;
// Link winit directly so Cargo enables its `wayland-csd-adwaita` headerbar feature
use winit as _;

use crate::capture::{CaptureOutcome, load_or_capture};
use crate::cli::Cli;
use crate::clipboard::run_clipboard_server_from_stdin;
use crate::ui::ScreenoteApp;

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.serve_clipboard {
        return run_clipboard_server_from_stdin();
    }

    let image = match load_or_capture(cli.input_source().map(|p| p.as_path()), cli.fullscreen)? {
        CaptureOutcome::Image(img) => img,
        CaptureOutcome::Cancelled => return Ok(()),
    };

    let init_w = ((image.width() as f32) * 0.8 + 64.0).clamp(900.0, 1520.0);
    let init_h = ((image.height() as f32) * 0.8 + 120.0).clamp(580.0, 960.0);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Screenote")
            .with_app_id("io.github.screenote")
            .with_inner_size([init_w, init_h])
            .with_min_inner_size([380.0, 280.0]),
        ..eframe::NativeOptions::default()
    };

    let output_path = cli.output;
    eframe::run_native(
        "Screenote",
        native_options,
        Box::new(move |cc| Ok(Box::new(ScreenoteApp::new(cc, image, output_path)))),
    )
    .map_err(|e| anyhow::anyhow!("GUI runtime error: {e}"))?;

    Ok(())
}
