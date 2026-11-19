# AGENTS.md — Portal Screen Mirror

Architecture reference, protocol invariants, and workflow rules for AI coding agents and contributors working in this repository.

`portal-screen-mirror` is a standalone, compositor-agnostic **Wayland screen, window, and rectangular region mirroring tool** written in **Rust + C (`libpipewire-0.3` + `EGL/GLES2`)** with a built-in pure-Rust **`eframe`/`egui` (`glow`) Wayland GUI** and **Nix Flake** packaging (`x86_64-linux` and `aarch64-linux`). It has **zero runtime dependency** on Noctalia, GStreamer, Python, X11, or compositor-specific window rules.

---

## 1. Repository Layout & Architecture

```text
portal-screen-mirror/
├── flake.nix                       # Nix Flake: packages, apps, overlays, nixosModules, homeManagerModules, devShells
├── flake.lock                      # Locked nixpkgs input (keep synced with flake.nix edits)
├── .envrc                          # direnv `use flake` hook
├── Cargo.toml                      # Rust dependencies & feature flags
├── Cargo.lock                      # Locked crate graph (required by rustPlatform.buildRustPackage)
├── build.rs                        # Probes pkg-config libs and compiles `src/pw_bridge.c` -> `libpw_bridge.a`
├── assets/
│   ├── portal-screen-mirror.desktop # XDG desktop launcher entry (`Exec=portal-screen-mirror gui`)
│   └── portal-screen-mirror.svg    # Scalable hicolor app icon
└── src/
    ├── main.rs                     # CLI (`clap`), daemon startup sequence, single-threaded `poll()` render loops
    ├── gui.rs                      # Pure-Rust `eframe`/`egui` (`glow`) Wayland control panel & detached child manager
    ├── ipc.rs                      # Unix domain socket IPC (`$XDG_RUNTIME_DIR/portal-screen-mirror.sock`)
    ├── outputs.rs                  # Wayland output discovery (`wl_output` + `zxdg_output_manager_v1`) & destination picker
    ├── display.rs                  # Presentation surface (`zwlr_layer_shell_v1` Overlay + `xdg_toplevel` fullscreen fallback)
    ├── egl_gl.rs                   # EGL Wayland context, GLSL ES 2.0 shaders, DMA-BUF `EGLImageKHR` import & SHM upload
    ├── portal.rs                   # D-Bus `org.freedesktop.portal.ScreenCast` session, `Session::Closed` watcher, `restore_token` cache
    ├── pipewire.rs                 # Safe Rust FFI wrapper (`PwCapture`, `CapturedFrame`, RAII `DmaBufFrame` FD guard)
    ├── pw_bridge.h                 # C ABI boundary between Rust and `libpipewire-0.3`
    ├── pw_bridge.c                 # PipeWire `pw_thread_loop` + `pw_stream` (DMA-BUF modifier pods, triple-buffered MemFd SHM, live fallback)
    ├── selector.rs                 # Native multi-output `zwlr_layer_shell_v1` crosshair region selector (`slurp` & `--geometry` fallback)
    └── screencopy.rs               # `zwlr_screencopy_manager_v1` (`v1`–`v3`) zero-dialog region capture (8-bit + 10-bit `2101010`)
```

### Runtime Topology & State Paths

1. **GUI Mode (`portal-screen-mirror` / `portal-screen-mirror gui`)**:
   - Runs `gui::run_gui` (`eframe` + `glow` on Wayland).
   - Spawns the mirroring engine as a **detached child session (`setsid`)** invoking its own binary (`portal-screen-mirror portal ...` or `portal-screen-mirror region ...`) so closing the GUI window leaves an active mirror running uninterrupted.
   - Polls and live-updates the running daemon via Unix socket IPC (`Status`, `Set`, `Stop`).
2. **Daemon / CLI Mode (`portal`, `region`, `toggle`)**:
   - Acquires user source selection first (`select_region` or `open_screencast_portal`), then binds `IpcServer`, creates `PresentationWindow` + `EglRenderer`, and enters a non-blocking `libc::poll()` loop multiplexing Wayland display FD(s), `eventfd` wakeup (PipeWire frames, `Session::Closed`, `SIGINT`/`SIGTERM`/`SIGHUP`), and the IPC listener FD.
3. **Ephemeral & Persistent Files**:
   - `$XDG_RUNTIME_DIR/portal-screen-mirror.sock` (`0600`): Active daemon Unix IPC socket (auto-cleaned on `Drop` and stale-checked on startup).
   - `$XDG_RUNTIME_DIR/portal-screen-mirror.log` (`0600`): Daemon `stderr` log written when launched from the GUI.
   - `$XDG_STATE_HOME/portal-screen-mirror/restore_tokens.json` (`0600`): Per-output XDG ScreenCast `restore_token` cache for `region` mode's Portal fallback on KDE/GNOME.

---

## 2. Wayland Protocol & Presentation Invariants

### Deferred Surface Map & `ack_configure` Ordering (`src/display.rs`, `src/egl_gl.rs`)
- **Never attach or swap an EGL buffer before `ack_configure`**: `PresentationWindow::create` sets up the `wl_surface` + `zwlr_layer_surface_v1` (or `xdg_toplevel`) and `WlEglSurface`, and `EglRenderer::new` creates the EGL window surface and compiles shaders, **without** calling `wl_surface.commit()` or `eglSwapBuffers()`.
- `PresentationWindow::map_and_wait_configure()` is called **only when Frame 1 arrives** (`!live.mapped` in `main.rs`). This avoids fatal Wayland `unconfigured_buffer` protocol errors and keeps the destination monitor untouched while the user is choosing in the Portal dialog or dragging a region.

### Click-Through Layer-Shell & Fullscreen Fallback (`src/display.rs`)
- Both `zwlr_layer_shell_v1` (`Layer::Overlay`, `Anchor::Top | Bottom | Left | Right`, `exclusive_zone = -1`, `KeyboardInteractivity::None`) and the `xdg_toplevel.set_fullscreen(Some(&target_wl))` fallback (used on GNOME Mutter / Weston) set an **empty `wl_region` input region** (`surface.set_input_region(Some(&empty_region))`) so mouse clicks and scroll events pass straight through the mirror overlay.

### Non-Blocking Vsync & FPS Pacing (`src/egl_gl.rs`, `src/main.rs`)
- **`eglSwapInterval(dpy, 0)` is mandatory**: Never change swap interval to `1`. Blocking inside `eglSwapBuffers()` stalls the single-threaded `poll()` loop (freezing `portal-screen-mirror stop` / `status` IPC) whenever the destination output is occluded, workspace-hidden, or DPMS-off.
- Frame pacing is driven explicitly by `PresentationWindow::request_frame_callback()` (`wl_surface.frame()` -> `wl_callback::Event::Done`) combined with the monotonic `effective_fps_cap` gate (`--max-fps auto` resolves to `dest.refresh_hz`).

### `wlr-screencopy` v3 `BufferDone` & 10-Bit Unpack (`src/screencopy.rs`)
- In `zwlr_screencopy_manager_v1` **version 3**, compositors may emit multiple `Event::Buffer` events (including unsupported 16-bit float formats) followed by `Event::BufferDone`, and require clients to wait for `BufferDone` before invoking `copy` / `copy_with_damage`.
- Never treat an individual unsupported `Event::Buffer` candidate as fatal, and keep `buffer_selected` + `copied_this_frame` guards intact so `copy` (frame 0) / `copy_with_damage` (subsequent frames) is invoked strictly once per frame.
- Preserve `Argb2101010` / `Abgr2101010` unpacking alongside `Direct` 8-bit formats (`0`, `1`, `AR24`, `XR24`, `AB24`, `XB24`) so HDR / 10-bit panels capture cleanly.

### Native Region Selector Alpha Invariant (`src/selector.rs`)
- The selected interior cutout in `SelState::redraw_all` uses `CLEAR = 0x0500_0000` (`alpha = 5`, **not** `0`). Several Wayland compositors treat `alpha == 0` SHM pixels as click-through hit-test holes, which would drop pointer motion/release events mid-drag.

---

## 3. PipeWire, DMA-BUF & GLSL Shader Invariants

### Never Enable `PW_STREAM_FLAG_MAP_BUFFERS` (`src/pw_bridge.c`)
- `pw_stream_connect` **must omit `PW_STREAM_FLAG_MAP_BUFFERS`** whenever `SPA_DATA_DmaBuf` is advertised. Setting `MAP_BUFFERS` causes `libpipewire` to `mmap()` tiled/compressed GPU DMA-BUF FDs, failing buffer allocation on AMD/NVIDIA drivers.
- Instead, `on_add_buffer` / `on_remove_buffer` manually `mmap` and `munmap` `d->fd` **only** when `d->type == SPA_DATA_MemFd`.

### Modifier Negotiation & Self-Healing `DMA-BUF` → `SHM` Fallback
1. **Format pods (`build_format_params` in `src/pw_bridge.c`)**: Builds `SPA_FORMAT_VIDEO_modifier` choice pods with `SPA_POD_PROP_FLAG_MANDATORY | SPA_POD_PROP_FLAG_DONT_FIXATE` from `EglRenderer::query_dmabuf_formats()` ahead of fallback SHM pods, and requests `SPA_PARAM_Buffers` (`range 3, 2..4`) in `on_param_changed`.
2. **Stage 1 fallback (`src/pw_bridge.c`)**: `on_process` inspects `d->chunk->size` across all DMA-BUF planes. If `chunk->size == 0` or a multi-plane buffer reports bogus tiny plane sizes (`n_planes > 1 && chunk->size < 64`, e.g. `xdg-desktop-portal-umbriel`'s AMD DCC `chunk->size = 9` bug), `trigger_shm_fallback_locked()` renegotiates the live stream to SHM via `pw_stream_update_params`.
3. **Stage 2 fallback (`src/main.rs`)**: If `EglRenderer::import_dmabuf` (`eglCreateImageKHR` or `glEGLImageTargetTexture2DOES`) fails on any DMA-BUF frame, `run_pipewire_loop` logs the reason and calls `pw.force_shm()` without dropping the Portal D-Bus session.

### FFI File-Descriptor Ownership (`src/pw_bridge.c`, `src/pipewire.rs`, `src/main.rs`)
- `psm_pw_start` `dup()`s `pw_fd` before handing it to `pw_context_connect_fd` (which takes ownership). `run_pipewire_loop` in `src/main.rs` **must** `libc::close(sel.pw_fd)` after `PwCapture::start` so the D-Bus PipeWire remote closes cleanly on stop.
- `on_process` `dup()`s each DMA-BUF plane FD into `s->pending_frame`; if a newer frame overwrites an unconsumed `pending_frame` (or on `psm_pw_stop`), `close_frame_fds` closes the superseded FDs. Once popped into Rust (`PwCapture::pop_frame`), `DmaBufFrame::drop` closes the plane FDs after `eglCreateImageKHR`.

### Shader Crop + `Y_INVERT` Order (`src/egl_gl.rs`)
- In `VERT_SRC`, `v` **must** first interpolate `mix(u_uv_rect.y, u_uv_rect.w, a_uv.y)` in top-down screen coordinates and apply `if (u_y_invert != 0) { v = 1.0 - v; }` **after** `mix(...)`. Reversing that order flips cropped regions to the wrong vertical slice whenever `y_invert` is set.

---

## 4. GUI ↔ Daemon Coordination Traps (`src/gui.rs`, `src/main.rs`, `src/ipc.rs`)

1. **Bind `IpcServer` only AFTER interactive selection (`start_mirror` in `src/main.rs`)**:
   - Check `ipc::send_request(&IpcRequest::Status)` upfront to reject duplicate instances, run `select_region` / `open_screencast_portal`, and call `IpcServer::bind()` **only after** selection returns `Some(...)`.
   - Binding `portal-screen-mirror.sock` before selection causes GUI status polls to connect to an unserviced listener backlog and block UI frames during region/window picking.
2. **Fixed utility viewport & persistent widget IDs (`src/gui.rs`)**:
   - Keep `min_inner_size == max_inner_size` (`[410.0, 565.0]`) and `.with_resizable(false)` on `ViewportBuilder`. Tiling Wayland compositors (`Umbriel`, `Hyprland`, `Sway`, `Niri`, `KWin`) use `min_size == max_size` to float the control panel automatically instead of tiling action buttons across the whole screen.
   - Custom interactive controls (`solid_action_button`) **must** use `ui.make_persistent_id(id_salt)` rather than positional `allocate_exact_size(..., Sense::click())` auto-IDs, and preserve `ignore_clicks_until` so status-banner transitions (`Waiting` → `Mirroring`) never replay stray pointer releases onto `Stop Mirroring`.
3. **File-backed child `stderr` (never an unconsumed pipe)**:
   - `launch_mode` redirects daemon `stderr` to `$XDG_RUNTIME_DIR/portal-screen-mirror.log` (`0600`) rather than `Stdio::piped()`, avoiding `EPIPE` or 64 KiB pipe deadlocks after the GUI closes.

---

## 5. Build, Linker & Nix Flake Rules

- **`build.rs` link order (`-lpw_bridge` before `-lpipewire-0.3`)**:
  - `cc::Build::compile("pw_bridge")` **must** execute *before* `pkg_config::Config::new().probe("libpipewire-0.3")` emits `cargo:rustc-link-lib=pipewire-0.3` (probe include paths first with `.cargo_metadata(false)`). Reversing them causes GNU `ld --as-needed` to drop `-lpipewire-0.3` before scanning `libpw_bridge.a`, failing release builds with undefined `pw_*` references.
- **Crate feature discipline (`Cargo.toml`)**:
  - `zbus`: keep `default-features = false, features = ["async-io", "blocking-api"]` (`zbus 5` requires an async runtime backend even when using `zbus::blocking`).
  - `eframe`: keep `default-features = false, features = ["default_fonts", "glow", "wayland"]` so builds stay pure-Wayland + OpenGL ES without pulling X11 or `wgpu`/`naga` shader compilers.
- **Nix `dlopen` `LD_LIBRARY_PATH` wrapping (`flake.nix`)**:
  - `wayland-backend`, `winit`/`sctk`, and `glutin` load `libwayland-client.so`, `libwayland-cursor.so`, `libwayland-egl.so`, `libxkbcommon.so`, and `libEGL.so`/`libGLESv2.so` via `dlopen` at runtime. Keep `wayland`, `libxkbcommon`, `libglvnd`, and `/run/opengl-driver/lib` in both `postFixup` (`wrapProgram`) and `devShells.default.LD_LIBRARY_PATH`.
- **Untracked files & lockfiles**:
  - Always run `git add -N <path>` on new files before running `nix build` or `nix flake check`, or Nix will omit them from the flake source tree.
  - Keep `Cargo.lock` and `flake.lock` updated and formatted (`nixfmt` for `.nix`, `cargo fmt` for `.rs`).

---

## 6. Verification Checklist

Run all checks below before considering any change complete:

```bash
# 1. Stage intent for any newly created files so Nix sees them
git add -N . && git status --short

# 2. Format & lint Rust + C (zero warnings allowed)
nix develop --command bash -c 'cargo fmt --check && cargo clippy -- -D warnings && cargo build --release'

# 3. Check Nix formatting, build the sandboxed flake package, and run flake checks
nixfmt --check flake.nix
nix build --no-link --print-out-paths .#default
nix flake check

# 4. Smoke-test Wayland output discovery & IPC lifecycle (when in an active Wayland session)
nix run .#default -- outputs
nix run .#default -- status || true
```
