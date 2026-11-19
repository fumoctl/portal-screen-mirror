mod display;
mod egl_gl;
mod gui;
mod ipc;
mod outputs;
mod pipewire;
mod portal;
mod screencopy;
mod selector;

use clap::{Args, Parser, Subcommand, ValueEnum};
use display::PresentationWindow;
use egl_gl::EglRenderer;
use ipc::{FilterMode, FitMode, IpcRequest, IpcResponse, IpcServer, MirrorStatus};
use outputs::{query_outputs, select_destination, OutputInfo};
use pipewire::{CapturedFrame, PwCapture};
use portal::{open_screencast_portal, PortalCaptureKind};
use screencopy::WlrScreencopyRegion;
use selector::{best_output_for_region, parse_geometry, select_region};
use std::os::fd::RawFd;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant};

static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static SIGNAL_WAKE_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(_sig: libc::c_int) {
    STOP_REQUESTED.store(true, Ordering::Relaxed);
    let fd = SIGNAL_WAKE_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        let one: u64 = 1;
        unsafe {
            let _ = libc::write(
                fd,
                &one as *const u64 as *const libc::c_void,
                std::mem::size_of::<u64>(),
            );
        }
    }
}

fn install_signal_handlers(wake_fd: RawFd) {
    SIGNAL_WAKE_FD.store(wake_fd, Ordering::Relaxed);
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_signal as *const () as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        sa.sa_flags = libc::SA_RESTART;
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            let _ = libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
}

fn create_eventfd() -> Result<RawFd, String> {
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if fd < 0 {
        Err("Failed creating eventfd".to_string())
    } else {
        Ok(fd)
    }
}

fn drain_eventfd(fd: RawFd) {
    let mut val: u64 = 0;
    loop {
        let n = unsafe {
            libc::read(
                fd,
                &mut val as *mut u64 as *mut libc::c_void,
                std::mem::size_of::<u64>(),
            )
        };
        if n < 0 {
            break;
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ToggleMode {
    Portal,
    Region,
}

#[derive(Debug, Clone, Args)]
struct MirrorCommonArgs {
    /// Destination Wayland output connector (e.g. HDMI-A-1, DP-1); defaults to first external output
    #[arg(short = 't', long = "to", value_name = "OUTPUT")]
    to: Option<String>,

    /// Aspect-ratio scaling mode on the destination screen
    #[arg(long, value_enum, default_value_t = FitMode::Contain)]
    fit: FitMode,

    /// GPU texture sampling filter (`nearest` keeps crisp pixels for HiDPI text/code)
    #[arg(long, value_enum, default_value_t = FilterMode::Linear)]
    filter: FilterMode,

    /// Frame rate cap: `auto` (match destination display Hz), `0` (uncapped), or explicit Hz (e.g. `60`)
    #[arg(long = "max-fps", default_value = "auto", value_name = "FPS")]
    max_fps: String,

    /// Hide the mouse cursor inside the mirrored stream
    #[arg(long = "no-cursor")]
    no_cursor: bool,

    /// Force PipeWire MemFd SHM buffers instead of zero-copy GPU DMA-BUF
    #[arg(long = "shm")]
    shm: bool,
}

#[derive(Debug, Clone, Args)]
struct RegionExtraArgs {
    /// Explicit region geometry (`X,Y,W,H` or `X,Y WxH`), skipping interactive selection
    #[arg(short = 'g', long = "geometry", value_name = "RECT")]
    geometry: Option<String>,

    /// Prefer external `slurp` over the built-in native layer-shell crosshair selector
    #[arg(long)]
    slurp: bool,

    /// Force XDG ScreenCast + GPU crop even when `zwlr_screencopy_manager_v1` is available
    #[arg(long = "portal-only")]
    portal_only: bool,
}

#[derive(Debug, Parser)]
#[command(
    name = "portal-screen-mirror",
    version,
    about = "Standalone Wayland screen, window, and region mirror for any compositor via XDG ScreenCast + PipeWire + EGL"
)]
struct Cli {
    #[command(flatten)]
    default_mirror: MirrorCommonArgs,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Open the standalone egui control panel window (default when run with no subcommand)
    Gui(MirrorCommonArgs),

    /// Mirror a screen or individual window chosen via XDG Desktop Portal
    Portal(MirrorCommonArgs),

    /// Select a rectangular region and mirror it (zero-dialog wlr-screencopy or Portal + GPU crop)
    Region {
        #[command(flatten)]
        common: MirrorCommonArgs,
        #[command(flatten)]
        region: RegionExtraArgs,
    },

    /// Stop mirroring if active; otherwise start a new mirror session
    Toggle {
        /// Capture mode when starting a new session
        #[arg(short = 'm', long = "mode", value_enum, default_value_t = ToggleMode::Portal)]
        mode: ToggleMode,
        #[command(flatten)]
        common: MirrorCommonArgs,
        #[command(flatten)]
        region: RegionExtraArgs,
    },

    /// Stop the running portal-screen-mirror instance
    Stop,

    /// Query the status of the running instance
    Status {
        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Hot-update scaling, filtering, or FPS cap on the running instance
    Set {
        #[arg(long, value_enum)]
        fit: Option<FitMode>,
        #[arg(long, value_enum)]
        filter: Option<FilterMode>,
        #[arg(long = "max-fps")]
        max_fps: Option<String>,
    },

    /// List connected Wayland outputs, resolutions, and refresh rates
    Outputs {
        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },
}

fn parse_fps_spec(spec: &str, dest_hz: u32) -> Result<(String, u32), String> {
    let s = spec.trim().to_ascii_lowercase();
    if s.is_empty() || s == "auto" {
        return Ok(("auto".to_string(), dest_hz.max(1)));
    }
    if matches!(s.as_str(), "0" | "uncapped" | "unlimited" | "max") {
        return Ok(("0".to_string(), 0));
    }
    let n: u32 = s
        .parse()
        .map_err(|_| format!("Invalid --max-fps '{spec}'; use `auto`, `0`, or 1..1000"))?;
    if n > 1000 {
        return Err("--max-fps cannot exceed 1000".to_string());
    }
    Ok((n.to_string(), n))
}

struct LiveSettings {
    mode: String,
    backend: String,
    destination: String,
    dest_hz: u32,
    fit: FitMode,
    filter: FilterMode,
    max_fps_spec: String,
    effective_fps_cap: u32,
    src_w: u32,
    src_h: u32,
    last_is_dmabuf: bool,
    last_swizzle: i32,
    last_y_invert: bool,
    mapped: bool,
}

impl LiveSettings {
    fn snapshot(&self) -> MirrorStatus {
        MirrorStatus {
            running: true,
            pid: std::process::id(),
            mode: self.mode.clone(),
            backend: self.backend.clone(),
            destination: self.destination.clone(),
            fit: self.fit,
            filter: self.filter,
            max_fps_spec: self.max_fps_spec.clone(),
            effective_fps_cap: self.effective_fps_cap,
            source_width: self.src_w,
            source_height: self.src_h,
        }
    }
}

fn handle_ipc(
    ipc: &IpcServer,
    live: &mut LiveSettings,
    pw: Option<&PwCapture>,
    mut need_redraw: bool,
) -> (bool, bool) {
    let mut stop = false;
    ipc.poll_requests(|req| match req {
        IpcRequest::Status => IpcResponse {
            ok: true,
            message: "running".to_string(),
            status: Some(live.snapshot()),
        },
        IpcRequest::Stop => {
            stop = true;
            IpcResponse {
                ok: true,
                message: "stopping".to_string(),
                status: None,
            }
        }
        IpcRequest::Set {
            fit,
            filter,
            max_fps,
        } => {
            if let Some(f) = fit {
                live.fit = f;
                need_redraw = true;
            }
            if let Some(fl) = filter {
                live.filter = fl;
                need_redraw = true;
            }
            if let Some(fps_str) = max_fps {
                match parse_fps_spec(&fps_str, live.dest_hz) {
                    Ok((norm, cap)) => {
                        live.max_fps_spec = norm;
                        live.effective_fps_cap = cap;
                        if let Some(pw_stream) = pw {
                            pw_stream.set_fps(cap);
                        }
                    }
                    Err(e) => {
                        return IpcResponse {
                            ok: false,
                            message: e,
                            status: None,
                        };
                    }
                }
            }
            IpcResponse {
                ok: true,
                message: "updated".to_string(),
                status: Some(live.snapshot()),
            }
        }
    });
    (stop, need_redraw)
}

fn run_screencopy_loop(
    ipc: &IpcServer,
    win: &mut PresentationWindow,
    egl: &mut EglRenderer,
    sc: &mut WlrScreencopyRegion,
    live: &mut LiveSettings,
    wake_fd: RawFd,
) -> Result<(), String> {
    install_signal_handlers(wake_fd);
    let mut last_present = Instant::now() - Duration::from_secs(1);

    while !STOP_REQUESTED.load(Ordering::Relaxed) && !win.is_closed() {
        let timeout_ms = if !sc.in_flight() && win.frame_ready() {
            if live.effective_fps_cap > 0 {
                let min_dt = Duration::from_secs_f64(1.0 / f64::from(live.effective_fps_cap));
                let elapsed = last_present.elapsed();
                if elapsed >= min_dt {
                    sc.request_frame();
                    250
                } else {
                    (min_dt - elapsed).as_millis().clamp(1, 250) as i32
                }
            } else {
                sc.request_frame();
                250
            }
        } else {
            250
        };

        let mut pfds = [
            libc::pollfd {
                fd: win.wayland_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: sc.wayland_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: ipc.raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: wake_fd,
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        unsafe {
            let _ = libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout_ms);
        }
        if pfds[3].revents != 0 {
            drain_eventfd(wake_fd);
        }

        win.dispatch_nonblocking()?;
        let mut need_redraw = false;
        if let Some((nw, nh)) = win.take_resize() {
            egl.resize(nw, nh);
            need_redraw = true;
        }

        let (ipc_stop, redraw_after_ipc) = handle_ipc(ipc, live, None, need_redraw);
        if ipc_stop {
            break;
        }
        need_redraw = redraw_after_ipc;

        if let Some((w, h, fmt, y_inv, pixels)) = sc.dispatch_and_take_frame()? {
            if !live.mapped {
                let (pw, ph) = win.map_and_wait_configure()?;
                egl.resize(pw, nh_or(ph));
                live.mapped = true;
                eprintln!(
                    "portal-screen-mirror: mirroring region ({w}x{h}) -> {} via wlr-screencopy ({}, fps={})",
                    live.destination, live.max_fps_spec, live.effective_fps_cap
                );
            }
            egl.upload_shm(w, h, pixels, live.filter)?;
            live.src_w = w;
            live.src_h = h;
            live.last_is_dmabuf = false;
            live.last_swizzle = fmt.shader_swizzle();
            live.last_y_invert = y_inv;
            need_redraw = true;
        }

        if live.mapped && need_redraw && live.src_w > 0 && win.frame_ready() {
            win.request_frame_callback();
            egl.draw_and_swap(
                false,
                live.src_w,
                live.src_h,
                (0.0, 0.0, 1.0, 1.0),
                live.fit,
                live.last_swizzle,
                live.last_y_invert,
            );
            let _ = win.dispatch_nonblocking();
            last_present = Instant::now();
        }
    }

    if live.mapped {
        egl.clear_black_and_swap();
    }
    Ok(())
}

#[inline]
fn nh_or(v: i32) -> i32 {
    v.max(1)
}

#[allow(clippy::too_many_arguments)]
fn run_pipewire_loop(
    ipc: &IpcServer,
    win: &mut PresentationWindow,
    egl: &mut EglRenderer,
    sel: portal::PortalSelection,
    crop_uv: (f32, f32, f32, f32),
    force_shm: bool,
    live: &mut LiveSettings,
    wake_fd: RawFd,
) -> Result<(), String> {
    let drm_formats = if force_shm {
        Vec::new()
    } else {
        egl.query_dmabuf_formats()
    };
    let allow_dmabuf = !drm_formats.is_empty();

    let mut pw = PwCapture::start(
        sel.pw_fd,
        sel.node_id,
        wake_fd,
        allow_dmabuf,
        &drm_formats,
        live.effective_fps_cap,
    )?;
    /* `psm_pw_start` duplicates `pw_fd`; close our copy so the remote closes cleanly on stop */
    unsafe {
        libc::close(sel.pw_fd);
    }

    let session = sel.session;
    install_signal_handlers(wake_fd);

    let mut last_present = Instant::now() - Duration::from_secs(1);
    let mut have_frame = false;
    let mut pending_present = false;

    while !STOP_REQUESTED.load(Ordering::Relaxed) && !win.is_closed() && !session.is_closed() {
        if let Some(err) = pw.check_error() {
            return Err(format!("PipeWire stream ended: {err}"));
        }

        let mut pfds = [
            libc::pollfd {
                fd: win.wayland_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: wake_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: ipc.raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let timeout_ms = if pending_present && win.frame_ready() {
            2
        } else {
            250
        };
        unsafe {
            let _ = libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout_ms);
        }
        if pfds[1].revents != 0 {
            drain_eventfd(wake_fd);
        }

        win.dispatch_nonblocking()?;
        if let Some((nw, nh)) = win.take_resize() {
            egl.resize(nw, nh);
            if have_frame {
                pending_present = true;
            }
        }

        let (ipc_stop, redraw_after_ipc) = handle_ipc(ipc, live, Some(&pw), pending_present);
        if ipc_stop {
            break;
        }
        pending_present = redraw_after_ipc;

        let fps_gate_open = if live.effective_fps_cap > 0 {
            let min_dt = Duration::from_secs_f64(1.0 / f64::from(live.effective_fps_cap));
            last_present.elapsed() >= min_dt
        } else {
            true
        };

        if fps_gate_open {
            if let Some(frame) = pw.pop_frame() {
                if !live.mapped {
                    let (pw_w, pw_h) = win.map_and_wait_configure()?;
                    egl.resize(pw_w, pw_h);
                    live.mapped = true;
                }
                match frame {
                    CapturedFrame::DmaBuf(dmabuf) => {
                        match egl.import_dmabuf(&dmabuf, live.filter) {
                            Ok(()) => {
                                if live.backend != "dmabuf" {
                                    live.backend = "dmabuf".to_string();
                                    eprintln!(
                                        "portal-screen-mirror: mirroring ({}x{}) -> {} via zero-copy DMA-BUF ({}, fps={})",
                                        dmabuf.width, dmabuf.height, live.destination, win.surface_kind, live.max_fps_spec
                                    );
                                }
                                live.src_w = dmabuf.width;
                                live.src_h = dmabuf.height;
                                live.last_is_dmabuf = true;
                                live.last_swizzle = 0;
                                live.last_y_invert = dmabuf.y_invert;
                                have_frame = true;
                                pending_present = true;
                            }
                            Err(e) => {
                                eprintln!(
                                    "portal-screen-mirror: {e}; falling back live to PipeWire MemFd SHM"
                                );
                                pw.force_shm();
                            }
                        }
                    }
                    CapturedFrame::Shm {
                        width,
                        height,
                        format,
                        y_invert,
                        pixels,
                    } => {
                        egl.upload_shm(width, height, pixels, live.filter)?;
                        if live.backend != "shm" {
                            live.backend = "shm".to_string();
                            eprintln!(
                                "portal-screen-mirror: mirroring ({width}x{height}) -> {} via PipeWire SHM ({}, fps={})",
                                live.destination, win.surface_kind, live.max_fps_spec
                            );
                        }
                        live.src_w = width;
                        live.src_h = height;
                        live.last_is_dmabuf = false;
                        live.last_swizzle = format.shader_swizzle();
                        live.last_y_invert = y_invert;
                        have_frame = true;
                        pending_present = true;
                    }
                }
            }
        }

        if live.mapped && have_frame && pending_present && win.frame_ready() && fps_gate_open {
            win.request_frame_callback();
            egl.draw_and_swap(
                live.last_is_dmabuf,
                live.src_w,
                live.src_h,
                crop_uv,
                live.fit,
                live.last_swizzle,
                live.last_y_invert,
            );
            let _ = win.dispatch_nonblocking();
            last_present = Instant::now();
            pending_present = false;
        }
    }

    if live.mapped {
        egl.clear_black_and_swap();
    }
    Ok(())
}

struct WakeFdGuard(RawFd);
impl Drop for WakeFdGuard {
    fn drop(&mut self) {
        SIGNAL_WAKE_FD.store(-1, Ordering::Relaxed);
        if self.0 >= 0 {
            unsafe {
                libc::close(self.0);
            }
        }
    }
}

fn start_mirror(
    common: MirrorCommonArgs,
    region_opts: Option<RegionExtraArgs>,
) -> Result<(), String> {
    if ipc::send_request(&IpcRequest::Status).is_ok() {
        return Err(
            "Another portal-screen-mirror instance is already running (use `stop` or `toggle`)"
                .to_string(),
        );
    }

    let outputs = query_outputs()?;
    let dest = select_destination(&outputs, common.to.as_deref())?.clone();
    let (max_fps_spec, effective_fps_cap) = parse_fps_spec(&common.max_fps, dest.refresh_hz)?;
    let show_cursor = !common.no_cursor;

    let wake_fd = create_eventfd()?;
    let _wake_guard = WakeFdGuard(wake_fd);

    let mut live = LiveSettings {
        mode: if region_opts.is_some() {
            "region".to_string()
        } else {
            "portal".to_string()
        },
        backend: "starting".to_string(),
        destination: dest.name.clone(),
        dest_hz: dest.refresh_hz,
        fit: common.fit,
        filter: common.filter,
        max_fps_spec,
        effective_fps_cap,
        src_w: 0,
        src_h: 0,
        last_is_dmabuf: false,
        last_swizzle: 0,
        last_y_invert: false,
        mapped: false,
    };

    if let Some(reg) = region_opts {
        let rect = match reg.geometry {
            Some(ref spec) => parse_geometry(spec)?,
            None => match select_region(&outputs, reg.slurp)? {
                Some(r) => r,
                None => return Ok(()),
            },
        };

        let src_out = best_output_for_region(&outputs, rect)
            .ok_or_else(|| "Could not match selected region to a Wayland output".to_string())?;

        if !reg.portal_only {
            if let Some(mut sc) = WlrScreencopyRegion::try_open(src_out, rect, show_cursor)? {
                let ipc = IpcServer::bind()?;
                let (mut win, wl_egl, dpy_ptr) = PresentationWindow::create(&dest.name)?;
                let (init_w, init_h) = win.physical_size();
                let mut egl = EglRenderer::new(dpy_ptr, wl_egl, init_w, init_h)?;
                live.backend = "wlr-screencopy".to_string();
                return run_screencopy_loop(&ipc, &mut win, &mut egl, &mut sc, &mut live, wake_fd);
            }
        }

        let Some(sel) = open_screencast_portal(
            PortalCaptureKind::MonitorOnly {
                restore_key: Some(src_out.name.clone()),
            },
            show_cursor,
            wake_fd,
        )?
        else {
            return Ok(());
        };

        let (mx, my) = sel.stream_position.unwrap_or((src_out.x, src_out.y));
        let (mw, mh) = sel
            .stream_size
            .filter(|(w, h)| *w > 0 && *h > 0)
            .unwrap_or((src_out.effective_width(), src_out.effective_height()));
        let u0 = ((rect.x - mx) as f32 / mw.max(1) as f32).clamp(0.0, 1.0);
        let v0 = ((rect.y - my) as f32 / mh.max(1) as f32).clamp(0.0, 1.0);
        let u1 = ((rect.x + rect.width - mx) as f32 / mw.max(1) as f32).clamp(0.0, 1.0);
        let v1 = ((rect.y + rect.height - my) as f32 / mh.max(1) as f32).clamp(0.0, 1.0);
        let crop_uv = if u1 - u0 > 0.005 && v1 - v0 > 0.005 {
            (u0, v0, u1, v1)
        } else {
            (0.0, 0.0, 1.0, 1.0)
        };

        let ipc = IpcServer::bind()?;
        let (mut win, wl_egl, dpy_ptr) = PresentationWindow::create(&dest.name)?;
        let (init_w, init_h) = win.physical_size();
        let mut egl = EglRenderer::new(dpy_ptr, wl_egl, init_w, init_h)?;
        run_pipewire_loop(
            &ipc, &mut win, &mut egl, sel, crop_uv, common.shm, &mut live, wake_fd,
        )
    } else {
        let Some(sel) =
            open_screencast_portal(PortalCaptureKind::MonitorOrWindow, show_cursor, wake_fd)?
        else {
            return Ok(());
        };
        let ipc = IpcServer::bind()?;
        let (mut win, wl_egl, dpy_ptr) = PresentationWindow::create(&dest.name)?;
        let (init_w, init_h) = win.physical_size();
        let mut egl = EglRenderer::new(dpy_ptr, wl_egl, init_w, init_h)?;
        run_pipewire_loop(
            &ipc,
            &mut win,
            &mut egl,
            sel,
            (0.0, 0.0, 1.0, 1.0),
            common.shm,
            &mut live,
            wake_fd,
        )
    }
}

fn print_outputs(list: &[OutputInfo], json: bool) -> Result<(), String> {
    if json {
        let s = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
        println!("{s}");
        return Ok(());
    }
    if list.is_empty() {
        println!("No Wayland outputs found.");
        return Ok(());
    }
    for o in list {
        let tag = if o.is_internal {
            "internal"
        } else {
            "external"
        };
        println!(
            "{:<12} {:>4}x{:<4} @ {:>3} Hz  pos=({}, {})  scale={}  [{}]  {}",
            o.name,
            o.physical_width,
            o.physical_height,
            o.refresh_hz,
            o.x,
            o.y,
            o.scale,
            tag,
            o.description
        );
    }
    Ok(())
}

fn run() -> Result<ExitCode, String> {
    let cli = Cli::parse();

    match cli.command {
        None => {
            let c = cli.default_mirror;
            gui::run_gui(gui::GuiInitArgs {
                to: c.to,
                fit: c.fit,
                filter: c.filter,
                max_fps: c.max_fps,
                no_cursor: c.no_cursor,
                shm: c.shm,
            })?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Commands::Gui(c)) => {
            gui::run_gui(gui::GuiInitArgs {
                to: c.to,
                fit: c.fit,
                filter: c.filter,
                max_fps: c.max_fps,
                no_cursor: c.no_cursor,
                shm: c.shm,
            })?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Commands::Portal(common)) => {
            start_mirror(common, None)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Commands::Region { common, region }) => {
            start_mirror(common, Some(region))?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Commands::Toggle {
            mode,
            common,
            region,
        }) => {
            if ipc::send_request(&IpcRequest::Status).is_ok() {
                let _ = ipc::send_request(&IpcRequest::Stop)?;
                println!("Stopped active portal-screen-mirror session.");
                return Ok(ExitCode::SUCCESS);
            }
            match mode {
                ToggleMode::Portal => start_mirror(common, None)?,
                ToggleMode::Region => start_mirror(common, Some(region))?,
            }
            Ok(ExitCode::SUCCESS)
        }
        Some(Commands::Stop) => {
            let resp = ipc::send_request(&IpcRequest::Stop)?;
            println!("{}", resp.message);
            Ok(ExitCode::SUCCESS)
        }
        Some(Commands::Status { json }) => match ipc::send_request(&IpcRequest::Status) {
            Ok(resp) => {
                if json {
                    let out =
                        serde_json::to_string_pretty(&resp.status).map_err(|e| e.to_string())?;
                    println!("{out}");
                } else if let Some(st) = resp.status {
                    let fps_label = if st.effective_fps_cap == 0 {
                        "uncapped".to_string()
                    } else if st.max_fps_spec == "auto" {
                        format!("auto ({} Hz)", st.effective_fps_cap)
                    } else {
                        format!("{} Hz", st.effective_fps_cap)
                    };
                    println!(
                        "running (pid {}) — {} [{}] -> {} ({}x{}, fit={:?}, filter={:?}, fps={})",
                        st.pid,
                        st.mode,
                        st.backend,
                        st.destination,
                        st.source_width,
                        st.source_height,
                        st.fit,
                        st.filter,
                        fps_label
                    );
                }
                Ok(ExitCode::SUCCESS)
            }
            Err(_) => {
                if json {
                    println!("{{\"running\":false}}");
                } else {
                    println!("stopped");
                }
                Ok(ExitCode::from(1))
            }
        },
        Some(Commands::Set {
            fit,
            filter,
            max_fps,
        }) => {
            if fit.is_none() && filter.is_none() && max_fps.is_none() {
                return Err(
                    "Pass at least one of --fit, --filter, or --max-fps to `set`".to_string(),
                );
            }
            let resp = ipc::send_request(&IpcRequest::Set {
                fit,
                filter,
                max_fps,
            })?;
            if !resp.ok {
                return Err(resp.message);
            }
            println!("{}", resp.message);
            Ok(ExitCode::SUCCESS)
        }
        Some(Commands::Outputs { json }) => {
            let outs = query_outputs()?;
            print_outputs(&outs, json)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("portal-screen-mirror: error: {err}");
            ExitCode::FAILURE
        }
    }
}
