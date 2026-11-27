use std::io::{Read, Write};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use wl_clipboard_rs::copy::{MimeType, Options, Source};

/// Copy PNG image bytes to the Wayland clipboard in a way that persists
/// even after the Screenote GUI window closes.
pub fn copy_png_to_clipboard(png_bytes: &[u8]) -> Result<()> {
    // 1. Prefer `wl-copy --type image/png` when available: it handles both
    //    `zwlr_data_control` (wlroots/KDE/COSMIC) AND `wl_data_device` popup
    //    surfaces (GNOME Mutter), forking a tiny background paste server.
    if try_wl_copy_binary(png_bytes).is_ok() {
        return Ok(());
    }

    // 2. Pure-Rust fallback: spawn a detached `screenote --serve-clipboard`
    //    worker using `wl-clipboard-rs` in foreground mode so clipboard
    //    ownership outlives the GUI window without external binaries.
    if try_spawn_self_clipboard_worker(png_bytes).is_ok() {
        return Ok(());
    }

    // 3. In-process fallback (stays alive while GUI remains open).
    let opts = Options::new();
    opts.copy(
        Source::Bytes(png_bytes.to_vec().into_boxed_slice()),
        MimeType::Specific("image/png".to_string()),
    )
    .context("Failed to copy PNG image to Wayland clipboard")?;

    Ok(())
}

fn try_wl_copy_binary(png_bytes: &[u8]) -> Result<()> {
    let mut child = Command::new("wl-copy")
        .arg("--type")
        .arg("image/png")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to spawn wl-copy")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(png_bytes)
            .context("Failed to write PNG data to wl-copy")?;
    }

    let status = child.wait().context("Failed waiting for wl-copy")?;
    anyhow::ensure!(status.success(), "wl-copy exited with status {status}");
    Ok(())
}

fn try_spawn_self_clipboard_worker(png_bytes: &[u8]) -> Result<()> {
    let exe = std::env::current_exe().context("Could not determine current executable path")?;
    let mut child = Command::new(exe)
        .arg("--serve-clipboard")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to spawn internal clipboard worker")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(png_bytes)
            .context("Failed writing PNG bytes to clipboard worker")?;
    }

    Ok(())
}

/// Entry point for the detached `--serve-clipboard` background worker.
/// Blocks serving Wayland paste requests until another application claims the clipboard.
pub fn run_clipboard_server_from_stdin() -> Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .read_to_end(&mut bytes)
        .context("Failed to read clipboard PNG payload from stdin")?;
    if bytes.is_empty() {
        return Ok(());
    }

    let mut opts = Options::new();
    opts.foreground(true);
    opts.copy(
        Source::Bytes(bytes.into_boxed_slice()),
        MimeType::Specific("image/png".to_string()),
    )
    .context("Wayland clipboard worker failed")?;

    Ok(())
}
