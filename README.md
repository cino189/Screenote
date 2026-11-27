# Screenote

A lightweight, native Wayland, desktop-environment-agnostic screenshot and annotation tool written in pure Rust.

Works natively across **GNOME, KDE Plasma, Sway, Hyprland, and COSMIC** via `xdg-desktop-portal` (`org.freedesktop.portal.Screenshot`)—with zero GTK/Qt C header dependencies.

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

## Install & Run

Screenote is a single self-contained binary (no external assets, schemas, or `-devel` C headers required) and installs directly into `~/.cargo/bin`:

```bash
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

1. Install with Cargo (`~/.cargo/bin` is in `$PATH`):
   ```bash
   cargo install --path .
   ```
2. Open **Settings → Keyboard → View and Customize Shortcuts → Custom Shortcuts** and add a shortcut pointing to `/home/$USER/.cargo/bin/screenote` (for example bound to `Super+Shift+S` or `Shift+Print`).
