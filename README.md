# Screenote

A lightweight, native Wayland, desktop-environment-agnostic screenshot and annotation tool written in pure Rust.

Works natively across **GNOME, KDE Plasma, Sway, Hyprland, and COSMIC** via `xdg-desktop-portal` (`org.freedesktop.portal.Screenshot`)—with zero GTK/Qt C header dependencies.

![Screenote editor](docs/screenote%20sample.png)

## Features

- **Native Wayland Capture**: Uses `xdg-desktop-portal` (`ashpd`) for compositor-native fullscreen or region selection across multi-monitor setups, or `--fullscreen` for instant full-screen capture, plus file/stdin input (`grim - | screenote -`).
- **Simple, Fast Annotation Tools**:
  - **Arrow (`A`)** (hold `Shift` to snap to 45° increments)
  - **Rectangle (`R`)** & **Ellipse / Circle (`E`)** (outline or filled; hold `Shift` for 1:1 square/circle)
  - **Text (`T`)** (inline canvas editor, multi-line `Shift+Enter`, optional high-contrast backdrop pill, double-click to edit)
  - **Pixelate / Blur (`P`)** (toggle between mosaic pixelate and smooth Gaussian-like blur)
  - **Crop (`C`)** & **Select / Move / Resize (`V`)**
- **Full-Resolution Export**: Annotations are stored as resolution-independent vectors and rendered onto the original screenshot resolution with `tiny-skia` and `cosmic-text`.
- **Persistent Wayland Clipboard**: `Copy` (`Ctrl+C`) and `Copy & Close` (`Ctrl+Enter`) keep `image/png` alive on the Wayland clipboard even after the Screenote window closes.

## Compatibility & Disclaimer

> **Disclaimer:** Screenote has **only been tested on a single Fedora Linux 45 (Workstation Edition) machine** running GNOME on Wayland. While it uses standard `xdg-desktop-portal` and Wayland protocols, other Linux distributions and compositors (KDE Plasma, Sway, Hyprland, COSMIC, etc.) have not been tested.

### Tested Environment

- **OS:** Fedora Linux 45 (Workstation Edition), `x86_64`
- **Kernel:** `7.3.4-300.fc45.x86_64`
- **Desktop / Session:** GNOME Shell `51.1` on Wayland (`XDG_SESSION_TYPE=wayland`)
- **Rust Toolchain:** `rustc 1.100.0` / `cargo 1.100.0` (`edition = "2024"`)

### Required Dependencies

- **Build-time:**
  - Rust & Cargo supporting Rust 2024 edition (`rustc` ≥ 1.85; tested with `1.100.0`). No `-devel` C header packages are required.
- **Runtime:**
  - `xdg-desktop-portal` (tested: `1.22.1`) and a portal backend implementing `Screenshot` and `FileChooser` (tested: `xdg-desktop-portal-gnome 51.0`)
  - Wayland client, XKB, and EGL runtime libraries: `libwayland-client` (`1.26.0`), `libxkbcommon` (`1.13.1`), `mesa-libEGL` (`26.2.7`)
  - `wl-clipboard` (`wl-copy`, tested: `2.3.0`) — recommended on GNOME/Mutter for clipboard persistence; falls back to the built-in `wl-clipboard-rs` worker on `zwlr_data_control` compositors
  - System fonts installed (used by `cosmic-text` for text annotations)

## Install & Run

Screenote is a single self-contained binary (no external assets, schemas, or `-devel` C headers required) and installs directly into `~/.cargo/bin`:

```bash
# Install directly from the Git repository
cargo install --git https://github.com/cino189/Screenote

# Or from a local checkout
cargo install --path .

screenote
```

### CLI Usage

```bash
# Interactive Wayland capture (select region or full screen via compositor portal) -> annotate
screenote

# Capture full screen immediately -> annotate
screenote --fullscreen

# Annotate an existing image or pipe from stdin
screenote path/to/image.png
grim - | screenote -
```

## Keyboard Shortcuts

| Shortcut | Action |
| --- | --- |
| `V` | Select / Move / Resize handles |
| `A` | Arrow tool |
| `R` | Rectangle tool |
| `E` | Ellipse / Circle tool |
| `T` | Text tool |
| `P` | Pixelate / Blur tool |
| `C` | Crop canvas |
| `Delete` / `Backspace` | Delete selected annotation |
| `Ctrl+Z` / `Ctrl+Y` | Undo / Redo |
| `Ctrl+C` | Copy PNG to Wayland clipboard |
| `Ctrl+Enter` | Copy PNG to Wayland clipboard & close |
| `Ctrl+S` | Quick save to `~/Pictures/Screenshots/` |
| `Ctrl+Shift+S` | Save As… via desktop portal |
| `Ctrl+N` | Take a new screenshot |
| `Ctrl+Q` | Quit |

## Binding to a Shortcut on GNOME Wayland

1. Find the install location for your environment (`~/.cargo/bin` is the default):
   ```bash
   which screenote
   ```
2. Open **Settings → Keyboard → View and Customize Shortcuts → Custom Shortcuts** and add a shortcut pointing to the screenote install location (e.g. `/home/$USER/.cargo/bin/screenote`).
