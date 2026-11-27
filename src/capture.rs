use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ashpd::desktop::ResponseError;
use ashpd::desktop::file_chooser::{FileFilter, SelectedFiles};
use ashpd::desktop::screenshot::Screenshot;
use ashpd::{Error as AshpdError, PortalError};
use chrono::Local;
use image::RgbaImage;

pub enum CaptureOutcome {
    Image(RgbaImage),
    Cancelled,
}

/// Load an image from file/stdin if provided, otherwise request a native Wayland
/// screenshot via `org.freedesktop.portal.Screenshot` (works across GNOME, KDE,
/// Sway, Hyprland, COSMIC, etc.).
pub fn load_or_capture(input_source: Option<&Path>, fullscreen: bool) -> Result<CaptureOutcome> {
    if let Some(path) = input_source {
        let img = if path == Path::new("-") {
            let mut bytes = Vec::new();
            std::io::stdin()
                .read_to_end(&mut bytes)
                .context("Failed reading image bytes from stdin")?;
            image::load_from_memory(&bytes)
                .context("Failed decoding image from stdin")?
                .into_rgba8()
        } else {
            image::open(path)
                .with_context(|| format!("Failed opening image file {}", path.display()))?
                .into_rgba8()
        };
        return Ok(CaptureOutcome::Image(img));
    }

    futures_lite::future::block_on(capture_via_portal(!fullscreen))
}

async fn capture_via_portal(interactive: bool) -> Result<CaptureOutcome> {
    let request = Screenshot::request()
        .interactive(interactive)
        .modal(true)
        .send()
        .await;

    let response = match request {
        Ok(req) => match req.response() {
            Ok(resp) => resp,
            Err(e) if is_cancelled(&e) => return Ok(CaptureOutcome::Cancelled),
            Err(e) => return Err(e).context("Screenshot portal request failed"),
        },
        Err(e) if is_cancelled(&e) => return Ok(CaptureOutcome::Cancelled),
        Err(e) => {
            return Err(e).context(
                "Could not contact xdg-desktop-portal Screenshot service. Ensure xdg-desktop-portal is running.",
            );
        }
    };

    let uri = response.uri();
    let file_path = uri
        .to_file_path()
        .map_err(|()| anyhow::anyhow!("Portal returned a non-file URI: {uri}"))?;

    let img = image::open(&file_path)
        .with_context(|| format!("Failed reading portal screenshot at {}", file_path.display()))?
        .into_rgba8();

    // Clean up the raw unannotated temporary screenshot created by the portal
    // so the user's Pictures folder is not cluttered when they only copy to clipboard.
    let _ = std::fs::remove_file(&file_path);

    Ok(CaptureOutcome::Image(img))
}

pub fn default_screenshots_dir() -> PathBuf {
    if let Some(pic_dir) = dirs::picture_dir() {
        let screenshots = pic_dir.join("Screenshots");
        if screenshots.is_dir() {
            return screenshots;
        }
        return pic_dir;
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn default_filename() -> String {
    format!("screenote-{}.png", Local::now().format("%Y-%m-%d_%H-%M-%S"))
}

/// Open the native desktop Save File dialog via `org.freedesktop.portal.FileChooser`.
pub fn save_file_dialog(suggested_name: &str, initial_dir: Option<&Path>) -> Result<Option<PathBuf>> {
    futures_lite::future::block_on(async {
        let folder = initial_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(default_screenshots_dir);

        let make_builder = || {
            let filter = FileFilter::new("PNG Image")
                .glob("*.png")
                .mimetype("image/png");
            SelectedFiles::save_file()
                .title("Save Annotated Screenshot")
                .accept_label("Save")
                .modal(true)
                .current_name(suggested_name)
                .filter(filter.clone())
                .current_filter(filter)
        };

        let builder = if folder.is_dir() {
            make_builder()
                .current_folder(&folder)
                .unwrap_or_else(|_| make_builder())
        } else {
            make_builder()
        };

        let response = match builder.send().await {
            Ok(req) => match req.response() {
                Ok(resp) => resp,
                Err(e) if is_cancelled(&e) => return Ok(None),
                Err(e) => return Err(e).context("Save dialog failed"),
            },
            Err(e) if is_cancelled(&e) => return Ok(None),
            Err(e) => return Err(e).context("Failed opening FileChooser portal"),
        };

        let Some(uri) = response.uris().first() else {
            return Ok(None);
        };

        let mut path = uri
            .to_file_path()
            .map_err(|()| anyhow::anyhow!("Selected save location is not a local path: {uri}"))?;

        if path.extension().is_none() {
            path.set_extension("png");
        }

        Ok(Some(path))
    })
}

fn is_cancelled(err: &AshpdError) -> bool {
    matches!(
        err,
        AshpdError::Response(ResponseError::Cancelled)
            | AshpdError::Portal(PortalError::Cancelled(_))
    )
}
