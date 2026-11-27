use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "screenote",
    version,
    about = "Native Wayland, desktop-environment-agnostic screenshot and annotation tool"
)]
pub struct Cli {
    /// Capture the full screen immediately instead of opening the interactive region/screen selector
    #[arg(short, long)]
    pub fullscreen: bool,

    /// Open an existing image file instead of capturing (use '-' to read from stdin)
    #[arg(short, long, value_name = "FILE")]
    pub input: Option<PathBuf>,

    /// Optional positional image path (or '-' for stdin)
    #[arg(value_name = "FILE")]
    pub file: Option<PathBuf>,

    /// Default file path when saving the annotated screenshot
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Internal background worker mode to serve Wayland clipboard data from stdin
    #[arg(long, hide = true)]
    pub serve_clipboard: bool,
}

impl Cli {
    pub fn input_source(&self) -> Option<&PathBuf> {
        self.input.as_ref().or(self.file.as_ref())
    }
}
