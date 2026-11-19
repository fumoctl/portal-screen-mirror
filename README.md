# Portal Screen Mirror

A standalone, compositor-agnostic Wayland **screen, window, and rectangular region mirror** written in Rust + C (`libpipewire-0.3` + `EGL/GLES2`).

Unlike compositor-specific mirroring tools (`wl-mirror`, `hyprctl output mirror`, etc.), **Portal Screen Mirror** works out-of-the-box across **any Wayland compositor** that implements `org.freedesktop.portal.ScreenCast` — with **zero compositor window rules**, **no Noctalia / shell framework dependency**, and **no GStreamer or Python runtime**.

---

## Highlights

- **Built-in Pure-Rust `egui` Control Panel + CLI/IPC Daemon**:
  - Run `portal-screen-mirror` (or launch **Portal Screen Mirror** from any `.desktop` app launcher) to open a native Wayland `egui`/`glow` control panel with destination display selection, live FPS/scaling/filter hot-updates, **Portal Picker**, **Rectangular Region**, and **Stop Mirroring**.
  - Closing the GUI window leaves your active mirror running in the background; reopening the GUI (or running `portal-screen-mirror status` / `set` / `stop`) reconnects seamlessly over Unix socket IPC.
- **Universal Presentation Surface**:
  - Pins a borderless, click-through `zwlr_layer_shell_v1` `Overlay` surface directly onto the destination `wl_output` (Umbriel, Hyprland, Niri, Sway, KDE Plasma KWin, River, Labwc, MangoWC).
  - Automatically falls back to `xdg_toplevel.set_fullscreen(target_output)` on compositors without `zwlr-layer-shell` (GNOME Mutter, Weston).
- **Zero-Copy GPU `DMA-BUF` + Self-Healing `SHM` Fallback**:
  - Negotiates `SPA_DATA_DmaBuf` with explicit DRM format modifiers (`EGL_EXT_image_dma_buf_import_modifiers` + `GL_OES_EGL_image_external`) so frames stay in VRAM end-to-end.
  - Automatically renegotiates live to triple-buffered `SPA_DATA_MemFd` SHM if a portal or GPU driver emits malformed multi-plane modifier metadata (such as broken AMD DCC planes) or rejects `eglCreateImageKHR`.
- **Three Capture Modes**:
  1. **Screen or Individual Window (`portal`, default)** — Opens your desktop's native XDG `ScreenCast` picker so you can mirror an entire workspace/monitor *or* a single private window onto a projector, TV, or lecture hall HDMI/DP/USB-C display.
  2. **Rectangular Region (`region`)** — Includes a built-in native multi-monitor `zwlr_layer_shell_v1` crosshair selector (plus `--geometry X,Y,W,H` and `--slurp` support):
     - Uses instant, zero-dialog `zwlr_screencopy_manager_v1::capture_output_region` (`copy_with_damage`, 8-bit + 10-bit `2101010` HDR unpack) when supported by the compositor.
     - Automatically falls back to XDG `ScreenCast` + GPU UV-crop shader (caching per-output `restore_token` in `$XDG_STATE_HOME/portal-screen-mirror/restore_tokens.json`) on KDE Plasma and GNOME.
- **Smart Frame Pacing (`--max-fps auto`)**:
  - Detects the destination display's active hardware refresh rate (`60 Hz` projector, `144 Hz` external monitor, etc.) while capturing from high-refresh (`240 Hz` / `300 Hz`) internal panels, pacing presentation non-blockingly via `eglSwapInterval(0)` + `wl_surface.frame()`.
- **Live Unix Socket IPC (`$XDG_RUNTIME_DIR/portal-screen-mirror.sock`)**:
  - `toggle`, `stop`, `status --json` (ideal for Waybar / Quickshell / Eww / polybar modules), and live `set` (`--fit`, `--filter`, `--max-fps`) without restarting the stream.

---

## Compositor Compatibility

| Compositor | Portal Backend | Presentation Surface | Screen / Window (`portal`) | Region (`region`) |
| :--- | :--- | :--- | :---: | :---: |
| **Hyprland** | `xdg-desktop-portal-hyprland` | `zwlr-layer-shell` (`Overlay`) | Screen + Window | Zero-dialog `wlr-screencopy` |
| **Umbriel** | `xdg-desktop-portal-umbriel` | `zwlr-layer-shell` (`Overlay`) | Screen + Window | Zero-dialog `wlr-screencopy` |
| **Niri** | `xdg-desktop-portal-gnome` | `zwlr-layer-shell` (`Overlay`) | Screen + Window | Zero-dialog `wlr-screencopy` |
| **Sway / River / Labwc** | `xdg-desktop-portal-wlr` | `zwlr-layer-shell` (`Overlay`) | Screen | Zero-dialog `wlr-screencopy` |
| **KDE Plasma (KWin)** | `xdg-desktop-portal-kde` | `zwlr-layer-shell` (`Overlay`) | Screen + Window | Portal + GPU UV crop (`restore_token`) |
| **GNOME (Mutter)** | `xdg-desktop-portal-gnome` | `xdg-toplevel` (`Fullscreen`) | Screen + Window | `--geometry` / `--slurp` + Portal UV crop |

---

## Quick Start

### Run Directly with Nix Flakes

```bash
# Open the native egui GUI control panel
nix run github:fumoctl/portal-screen-mirror

# List connected displays and refresh rates
nix run github:fumoctl/portal-screen-mirror -- outputs

# Headless CLI: mirror a screen/window to a specific connector
nix run github:fumoctl/portal-screen-mirror -- portal --to HDMI-A-1

# Headless CLI: interactive crosshair region mirror
nix run github:fumoctl/portal-screen-mirror -- region --to HDMI-A-1
```

### NixOS / Home Manager Installation

Add the input to your `flake.nix`:

```nix
{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-26.05";
    portal-screen-mirror = {
      url = "github:fumoctl/portal-screen-mirror";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
}
```

Enable via the **NixOS module** or **Home Manager module**:

```nix
# NixOS module:
{ inputs, ... }:
{
  imports = [ inputs.portal-screen-mirror.nixosModules.default ];
  programs.portal-screen-mirror.enable = true;
}

# Or Home Manager module:
{ inputs, ... }:
{
  imports = [ inputs.portal-screen-mirror.homeManagerModules.default ];
  programs.portal-screen-mirror.enable = true;
}
```

Or use the **overlay** (`inputs.portal-screen-mirror.overlays.default`) to expose `pkgs.portal-screen-mirror`.

### Building on Arch / Fedora / Debian / Ubuntu

Install `rustc`, `cargo`, `pkg-config`, and development headers for `libpipewire-0.3`, `wayland-client`, `wayland-egl`, and `libglvnd` (`egl`, `glesv2`):

```bash
# Arch Linux
sudo pacman -S rust pkgconf clang pipewire wayland libxkbcommon libglvnd

# Fedora
sudo dnf install cargo rust pkgconf-pkg-config pipewire-devel wayland-devel libxkbcommon-devel libglvnd-devel

# Debian / Ubuntu
sudo apt install cargo rustc pkg-config libpipewire-0.3-dev libwayland-dev libxkbcommon-dev libegl-dev libgles-dev

cargo build --release
install -Dm755 target/release/portal-screen-mirror ~/.local/bin/portal-screen-mirror
install -Dm644 assets/portal-screen-mirror.desktop ~/.local/share/applications/portal-screen-mirror.desktop
install -Dm644 assets/portal-screen-mirror.svg ~/.local/share/icons/hicolor/scalable/apps/portal-screen-mirror.svg
```

---

## CLI Usage

```text
portal-screen-mirror [OPTIONS] [COMMAND]

Commands:
  gui      Open the standalone egui control panel window (default when run with no subcommand)
  portal   Mirror a screen or individual window chosen via XDG Desktop Portal
  region   Select a rectangular region and mirror it (wlr-screencopy or Portal + GPU crop)
  toggle   Stop mirroring if active; otherwise start a new mirror session
  stop     Stop the running portal-screen-mirror instance
  status   Query the status of the running instance (`--json` supported)
  set      Hot-update `--fit`, `--filter`, or `--max-fps` on the running instance
  outputs  List connected Wayland outputs, resolutions, and refresh rates (`--json` supported)
```

### Common Examples

```bash
# Keybind-friendly toggle (starts Portal picker if idle, stops if already mirroring)
portal-screen-mirror toggle

# Keybind-friendly toggle for rectangular region capture
portal-screen-mirror toggle --mode region

# Crisp nearest-neighbor scaling for code/terminals on a lower-res projector
portal-screen-mirror portal --to HDMI-A-1 --fit contain --filter nearest

# Mirror an explicit region without interactive selection
portal-screen-mirror region --to HDMI-A-1 --geometry 0,0,1280,720

# Live hot-update while mirroring (zero stream interruption!)
portal-screen-mirror set --fit cover --filter nearest --max-fps 60

# Machine-readable status for status bars (Waybar / Eww / Quickshell)
portal-screen-mirror status --json

# Stop mirroring
portal-screen-mirror stop
```

### Compositor Keybind Examples

**Hyprland (`hyprland.conf`)**:
```ini
bind = SUPER, M, exec, portal-screen-mirror toggle
bind = SUPER SHIFT, M, exec, portal-screen-mirror toggle --mode region
```

**Sway (`~/.config/sway/config`)**:
```ini
bindsym $mod+m exec portal-screen-mirror toggle
bindsym $mod+Shift+m exec portal-screen-mirror toggle --mode region
```

**Niri (`~/.config/niri/config.kdl`)**:
```kdl
binds {
    Mod+M { spawn "portal-screen-mirror" "toggle"; }
    Mod+Shift+M { spawn "portal-screen-mirror" "toggle" "--mode" "region"; }
}
```

**Umbriel (`config.toml` / Nix)**:
```toml
[binds]
"Super + M" = { spawn = ["portal-screen-mirror", "toggle"] }
"Super + Shift + M" = { spawn = ["portal-screen-mirror", "toggle", "--mode", "region"] }
```

---

## Development

With Nix + `direnv` (or `nix develop`):

```bash
cd /home/fumoctl/Projects/portal-screen-mirror
nix develop

cargo check
cargo clippy -- -D warnings
cargo build --release
nix build .#default
```

## License

[MIT](./LICENSE)
