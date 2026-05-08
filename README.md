<p align="center">
  <img src="https://img.shields.io/badge/rust-%23000000.svg?style=for-the-badge&logo=rust&logoColor=white" />
  <img src="https://img.shields.io/badge/Nix-5277C3?style=for-the-badge&logo=nixos&logoColor=white" />
  <img src="https://img.shields.io/badge/Wayland-8BC0A0?style=for-the-badge&logo=wayland&logoColor=white" />
  <img src="https://img.shields.io/badge/AV1-00E5FF?style=for-the-badge&logo=av1&logoColor=white" />
</p>

<p align="center">
  <a href="https://github.com/M4jor-Tom/yasr.rs/actions">
    <img src="https://github.com/M4jor-Tom/yasr.rs/actions/workflows/ci.yml/badge.svg" />
  </a>
</p>

# yasr — Yet Another Screen Recorder

A lightweight, modern screen recorder for **Wayland** that captures your screen and encodes it into web-compatible video using the best available codec — **AV1 via libsvtav1** by default.

Built in Rust. No Electron. No GNOME Shell dependency. Just `ffmpeg` and a portal.

https://github.com/user-attachments/assets/b2499d5a-7e04-4c0b-b849-a13956feb2d8


## Features

- **AV1-first** — auto-selects `libsvtav1` for modern, lightweight, web-ready output
- **Two interfaces** — CLI (`yasr-cli`) for scripting and TUI (`yasr-tui`) for interactive use
- **Auto FPS** — probes your actual capture speed and tunes the output rate
- **Smart codec fallback** — VAAPI → NVENC → VP9 → x264 if AV1 isn't available
- **Nix flake** — one-command build and run, no system pollution
- **Small** — written in Rust with minimal dependencies

## Quick start

```shell
# record with default settings
nix run . -- output.webm

# list your monitors
nix run .#yasr-cli -- --list-monitors

# record a specific monitor at 15 FPS with verbose ffmpeg output
nix run .#yasr-cli -- -m 0 -f 15 -v recording.webm
```

## Usage

### CLI (`yasr-cli`)

```
Yet Another Screen Recorder

Usage: yasr-cli [OPTIONS] [OUTPUT]

Arguments:
  [OUTPUT]  Output file path              [default: output.webm]

Options:
  -m, --monitor <MONITOR>  Monitor index (0-based)
  -f, --fps <FPS>          Target FPS, or 'auto' to detect  [default: auto]
      --codec <CODEC>      Video codec (auto-detected by default)
  -b, --bitrate <BITRATE>  Video bitrate (e.g. 2M, 500k)
      --list-monitors      List available displays and exit
      --list-codecs        List available encoders and exit
  -v, --verbose            Show verbose ffmpeg output
  -h, --help               Print help
  -V, --version            Print version
```

### TUI (`yasr-tui`)

```
Yet Another Screen Recorder — TUI

Usage: yasr-tui [OPTIONS] [OUTPUT]

Options:
  -f, --fps <FPS>          Target FPS, or 'auto' to detect  [default: auto]
      --codec <CODEC>      Video codec (auto-detected by default)
  -b, --bitrate <BITRATE>  Video bitrate (e.g. 2M, 500k)
  -v, --verbose            Show verbose ffmpeg output
  -h, --help               Print help
  -V, --version            Print version
```

| Key | Action |
|-----|--------|
| `Space` | Start / stop recording |
| `←` `→` | Select monitor (when not recording) |
| `e` | Toggle ffmpeg log window |
| `↑` `↓` | Scroll log window |
| `Esc` | Close log window |
| `q` | Quit |

### Container format

The output container is derived from the file extension:

| Extension | Container |
|-----------|-----------|
| `.webm`   | WebM      |
| `.mp4`    | MP4       |
| *(other)* | MP4       |

## Build & install

### Nix

```shell
# run directly (no install)
nix run github:M4jor-Tom/yasr.rs -- --help

# or from a local checkout
git clone https://github.com/M4jor-Tom/yasr.rs
cd yasr.rs
nix run . -- -m 0 output.webm

# build and install to your profile
nix profile install .
```

### Cargo

```shell
cargo build --release
target/release/yasr-cli --help
target/release/yasr-tui
```

> **Note:** The TUI requires a terminal that supports raw mode (most do). Pipe `ffmpeg` warnings to the log window by default; pass `-v` to see full encoder chatter.

## How it works

1. **Capture** — uses [`xcap`](https://crates.io/crates/xcap) which talks to `xdg-desktop-portal` (PipeWire under the hood) to grab raw BGRA frames
2. **Encode** — pipes frames to `ffmpeg` via stdin; no temp files, no GStreamer, no intermediate containers
3. **Codec priority** — `libsvtav1` (AV1) → `hevc_vaapi` / `h264_vaapi` → `hevc_nvenc` / `h264_nvenc` → `libvpx-vp9` → `libx264`

## Requirements

- **Linux** with a **Wayland** compositor (niri, Sway, Hyprland, GNOME, KDE, etc.)
- **xdg-desktop-portal** running (typically auto-started on modern Wayland sessions)
- **ffmpeg** in PATH (bundled via Nix flake; on non-Nix systems `apt install ffmpeg` or equivalent)
- Screen capture requires PipeWire and `xdg-desktop-portal-wlr` (wlroots-based compositors) or the portal backend matching your desktop

## Why AV1?

AV1 is the most modern, royalty-free video codec. It offers:
- **~30% better compression** than VP9 at the same quality
- **Royalty-free** — no licensing concerns
- **Web-native** — supported in all major browsers (Chrome, Firefox, Safari)
- `libsvtav1` at preset 10 achieves fast encoding suitable for real-time screen capture

## Project structure

```
src/
├── lib.rs              # shared library
├── capture.rs          # monitor enumeration, FPS probing
├── encode.rs           # codec detection, ffmpeg argument builder
├── vaapi.rs            # VAAPI device path discovery
└── bin/
    ├── yasr-cli.rs     # CLI entry point
    └── yasr-tui.rs     # TUI entry point (ratatui)
```

## License

MIT
