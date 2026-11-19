use crate::outputs::OutputInfo;
use std::ffi::CString;
use std::os::fd::{BorrowedFd, RawFd};
use std::process::Command;
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm,
    wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1, wp_cursor_shape_manager_v1,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, Layer},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity},
};

#[derive(Debug, Clone, Copy)]
pub struct RegionRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

pub fn parse_geometry(spec: &str) -> Result<RegionRect, String> {
    /* Supports `X,Y WxH` (slurp format) or `X,Y,W,H` */
    let parts: Vec<&str> = spec
        .split(|c: char| c == ',' || c == 'x' || c == 'X' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() != 4 {
        return Err(format!(
            "Invalid geometry '{spec}'; expected 'X,Y,W,H' or 'X,Y WxH'"
        ));
    }
    let nums: Result<Vec<i32>, _> = parts.iter().map(|p| p.parse::<i32>()).collect();
    let nums = nums.map_err(|_| format!("Invalid integer in geometry '{spec}'"))?;
    if nums[2] < 2 || nums[3] < 2 {
        return Err("Region width and height must be at least 2 pixels".to_string());
    }
    Ok(RegionRect {
        x: nums[0],
        y: nums[1],
        width: nums[2],
        height: nums[3],
    })
}

pub fn best_output_for_region(outputs: &[OutputInfo], rect: RegionRect) -> Option<&OutputInfo> {
    let mut best: Option<(&OutputInfo, i64)> = None;
    for out in outputs {
        let ow = out.effective_width();
        let oh = out.effective_height();
        let ix0 = rect.x.max(out.x);
        let iy0 = rect.y.max(out.y);
        let ix1 = (rect.x + rect.width).min(out.x + ow);
        let iy1 = (rect.y + rect.height).min(out.y + oh);
        let area = i64::from((ix1 - ix0).max(0)) * i64::from((iy1 - iy0).max(0));
        if best.is_none_or(|(_, ba)| area > ba) {
            best = Some((out, area));
        }
    }
    best.and_then(|(o, a)| if a > 0 { Some(o) } else { outputs.first() })
}

pub fn select_region(
    outputs: &[OutputInfo],
    prefer_slurp: bool,
) -> Result<Option<RegionRect>, String> {
    if !prefer_slurp {
        if let Ok(res) = run_native_selector(outputs) {
            return Ok(res);
        }
    }
    run_slurp_fallback()
}

fn run_slurp_fallback() -> Result<Option<RegionRect>, String> {
    let output = Command::new("slurp")
        .args(["-f", "%x %y %w %h"])
        .output()
        .map_err(|_| {
            "Native layer-shell selector unavailable and `slurp` is not installed (pass --geometry X,Y,W,H)"
                .to_string()
        })?;

    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    parse_geometry(trimmed).map(Some)
}

/* ===== Native Wayland Layer-Shell Crosshair Selector ===== */

struct ShmOverlayBuf {
    fd: RawFd,
    ptr: *mut u32,
    len_pixels: usize,
    pool: wl_shm_pool::WlShmPool,
    buffer: wl_buffer::WlBuffer,
}

impl ShmOverlayBuf {
    fn create(shm: &wl_shm::WlShm, qh: &QueueHandle<SelState>, w: i32, h: i32) -> Option<Self> {
        let len_pixels = (w.max(1) as usize) * (h.max(1) as usize);
        let byte_len = len_pixels * 4;
        let name = CString::new("psm-selector").ok()?;
        let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        if unsafe { libc::ftruncate(fd, byte_len as libc::off_t) } != 0 {
            unsafe { libc::close(fd) };
            return None;
        }
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                byte_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            unsafe { libc::close(fd) };
            return None;
        }
        let bfd = unsafe { BorrowedFd::borrow_raw(fd) };
        let pool = shm.create_pool(bfd, byte_len as i32, qh, ());
        let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
        Some(Self {
            fd,
            ptr: ptr as *mut u32,
            len_pixels,
            pool,
            buffer,
        })
    }

    fn pixels_mut(&mut self) -> &mut [u32] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len_pixels) }
    }
}

impl Drop for ShmOverlayBuf {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.pool.destroy();
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.len_pixels * 4);
            libc::close(self.fd);
        }
    }
}

struct OverlaySurface {
    out_name: String,
    global_x: i32,
    global_y: i32,
    width: i32,
    height: i32,
    surface: wl_surface::WlSurface,
    layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    shm_buf: Option<ShmOverlayBuf>,
    configured: bool,
}

struct SelState {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    seat: Option<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    cursor_mgr: Option<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    cursor_dev: Option<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>,

    wl_outputs: Vec<(wl_output::WlOutput, String)>,
    overlays: Vec<OverlaySurface>,

    active_surface_idx: Option<usize>,
    cursor_global: (f64, f64),
    drag_start: Option<(f64, f64)>,
    finished: bool,
    cancelled: bool,
    result: Option<RegionRect>,
    needs_redraw: bool,
}

impl SelState {
    fn current_box(&self) -> Option<(i32, i32, i32, i32)> {
        let (sx, sy) = self.drag_start?;
        let (cx, cy) = self.cursor_global;
        let x0 = sx.min(cx).round() as i32;
        let y0 = sy.min(cy).round() as i32;
        let x1 = sx.max(cx).round() as i32;
        let y1 = sy.max(cy).round() as i32;
        Some((x0, y0, (x1 - x0).max(1), (y1 - y0).max(1)))
    }

    fn redraw_all(&mut self, qh: &QueueHandle<Self>) {
        let Some(ref shm) = self.shm else {
            return;
        };
        let sel = self.current_box();
        const SCRIM: u32 = 0x6607_0714; /* translucent dark overlay */
        const CLEAR: u32 = 0x0500_0000; /* nearly clear selection interior (alpha>0 keeps pointer events!) */
        const BORDER: u32 = 0xFF89_B4FA; /* crisp accent border (ARGB8888) */

        for ov in &mut self.overlays {
            if !ov.configured || ov.width <= 0 || ov.height <= 0 {
                continue;
            }
            let need_alloc = ov
                .shm_buf
                .as_ref()
                .is_none_or(|b| b.len_pixels != (ov.width as usize) * (ov.height as usize));
            if need_alloc {
                ov.shm_buf = ShmOverlayBuf::create(shm, qh, ov.width, ov.height);
            }
            let Some(ref mut buf) = ov.shm_buf else {
                continue;
            };
            let w = ov.width;
            let h = ov.height;
            let px = buf.pixels_mut();
            px.fill(SCRIM);

            if let Some((gx, gy, gw, gh)) = sel {
                let lx0 = (gx - ov.global_x).clamp(0, w);
                let ly0 = (gy - ov.global_y).clamp(0, h);
                let lx1 = (gx + gw - ov.global_x).clamp(0, w);
                let ly1 = (gy + gh - ov.global_y).clamp(0, h);

                for y in ly0..ly1 {
                    let row =
                        &mut px[(y as usize) * (w as usize)..((y + 1) as usize) * (w as usize)];
                    row[lx0 as usize..lx1 as usize].fill(CLEAR);
                }

                /* Draw 2px border where selection edges fall inside this output */
                let bx0 = gx - ov.global_x;
                let by0 = gy - ov.global_y;
                let bx1 = bx0 + gw;
                let by1 = by0 + gh;
                for t in 0..2 {
                    let top_y = by0 + t;
                    if top_y >= 0 && top_y < h && lx0 < lx1 {
                        let r = (top_y as usize) * (w as usize);
                        px[r + lx0 as usize..r + lx1 as usize].fill(BORDER);
                    }
                    let bot_y = by1 - 1 - t;
                    if bot_y >= 0 && bot_y < h && lx0 < lx1 {
                        let r = (bot_y as usize) * (w as usize);
                        px[r + lx0 as usize..r + lx1 as usize].fill(BORDER);
                    }
                    let left_x = bx0 + t;
                    if left_x >= 0 && left_x < w {
                        for y in ly0..ly1 {
                            px[(y as usize) * (w as usize) + left_x as usize] = BORDER;
                        }
                    }
                    let right_x = bx1 - 1 - t;
                    if right_x >= 0 && right_x < w {
                        for y in ly0..ly1 {
                            px[(y as usize) * (w as usize) + right_x as usize] = BORDER;
                        }
                    }
                }
            }

            ov.surface.attach(Some(&buf.buffer), 0, 0);
            ov.surface.damage_buffer(0, 0, w, h);
            ov.surface.commit();
        }
        self.needs_redraw = false;
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for SelState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_shm" => {
                    state.shm = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_seat" => {
                    if state.seat.is_none() {
                        state.seat = Some(registry.bind(name, version.min(7), qh, ()));
                    }
                }
                "wp_cursor_shape_manager_v1" => {
                    state.cursor_mgr = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "wl_output" if version >= 2 => {
                    let idx = state.wl_outputs.len();
                    let wl = registry.bind(name, version.min(4), qh, idx);
                    state.wl_outputs.push((wl, String::new()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, usize> for SelState {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            if let Some(entry) = state.wl_outputs.get_mut(*idx) {
                entry.1 = name;
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for SelState {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                let ptr = seat.get_pointer(qh, ());
                if let Some(ref mgr) = state.cursor_mgr {
                    state.cursor_dev = Some(mgr.get_pointer(&ptr, qh, ()));
                }
                state.pointer = Some(ptr);
            }
            if caps.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for SelState {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        const BTN_LEFT: u32 = 0x110;
        const BTN_RIGHT: u32 = 0x111;

        match event {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } => {
                if let Some(ref dev) = state.cursor_dev {
                    dev.set_shape(serial, wp_cursor_shape_device_v1::Shape::Crosshair);
                }
                if let Some((idx, ov)) = state
                    .overlays
                    .iter()
                    .enumerate()
                    .find(|(_, o)| o.surface.id() == surface.id())
                {
                    state.active_surface_idx = Some(idx);
                    state.cursor_global = (
                        f64::from(ov.global_x) + surface_x,
                        f64::from(ov.global_y) + surface_y,
                    );
                    if state.drag_start.is_some() {
                        state.needs_redraw = true;
                    }
                }
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                if let Some(idx) = state.active_surface_idx {
                    if let Some(ov) = state.overlays.get(idx) {
                        state.cursor_global = (
                            f64::from(ov.global_x) + surface_x,
                            f64::from(ov.global_y) + surface_y,
                        );
                        if state.drag_start.is_some() {
                            state.needs_redraw = true;
                        }
                    }
                }
            }
            wl_pointer::Event::Button {
                button,
                state: WEnum::Value(bstate),
                ..
            } => {
                if button == BTN_RIGHT && bstate == wl_pointer::ButtonState::Pressed {
                    state.cancelled = true;
                    state.finished = true;
                } else if button == BTN_LEFT {
                    if bstate == wl_pointer::ButtonState::Pressed {
                        state.drag_start = Some(state.cursor_global);
                        state.needs_redraw = true;
                    } else if bstate == wl_pointer::ButtonState::Released {
                        if let Some((x, y, w, h)) = state.current_box() {
                            if w >= 4 && h >= 4 {
                                state.result = Some(RegionRect {
                                    x,
                                    y,
                                    width: w,
                                    height: h,
                                });
                            } else {
                                state.cancelled = true;
                            }
                        } else {
                            state.cancelled = true;
                        }
                        state.finished = true;
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for SelState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key {
            key,
            state: WEnum::Value(wl_keyboard::KeyState::Pressed),
            ..
        } = event
        {
            /* Linux evdev KEY_ESC == 1 */
            if key == 1 {
                state.cancelled = true;
                state.finished = true;
            }
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, usize> for SelState {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer.ack_configure(serial);
                if let Some(ov) = state.overlays.get_mut(*idx) {
                    if width > 0 && height > 0 {
                        ov.width = width as i32;
                        ov.height = height as i32;
                    }
                    ov.configured = true;
                    state.needs_redraw = true;
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.cancelled = true;
                state.finished = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for SelState {
    fn event(
        _: &mut Self,
        _: &wl_compositor::WlCompositor,
        _: wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<wl_shm::WlShm, ()> for SelState {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<wl_shm_pool::WlShmPool, ()> for SelState {
    fn event(
        _: &mut Self,
        _: &wl_shm_pool::WlShmPool,
        _: wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<wl_buffer::WlBuffer, ()> for SelState {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        _: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<wl_surface::WlSurface, ()> for SelState {
    fn event(
        _: &mut Self,
        _: &wl_surface::WlSurface,
        _: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for SelState {
    fn event(
        _: &mut Self,
        _: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        _: zwlr_layer_shell_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1, ()> for SelState {
    fn event(
        _: &mut Self,
        _: &wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
        _: wp_cursor_shape_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1, ()> for SelState {
    fn event(
        _: &mut Self,
        _: &wp_cursor_shape_device_v1::WpCursorShapeDeviceV1,
        _: wp_cursor_shape_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

fn run_native_selector(outputs: &[OutputInfo]) -> Result<Option<RegionRect>, String> {
    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();

    let mut state = SelState {
        compositor: None,
        shm: None,
        layer_shell: None,
        seat: None,
        pointer: None,
        keyboard: None,
        cursor_mgr: None,
        cursor_dev: None,
        wl_outputs: Vec::new(),
        overlays: Vec::new(),
        active_surface_idx: None,
        cursor_global: (0.0, 0.0),
        drag_start: None,
        finished: false,
        cancelled: false,
        result: None,
        needs_redraw: false,
    };

    conn.display().get_registry(&qh, ());
    queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
    queue.roundtrip(&mut state).map_err(|e| e.to_string())?;

    let (Some(comp), Some(_shm), Some(ls)) = (
        state.compositor.clone(),
        state.shm.clone(),
        state.layer_shell.clone(),
    ) else {
        return Err("Missing zwlr_layer_shell_v1 or wl_shm".to_string());
    };

    if state.pointer.is_none() {
        return Err("No Wayland pointer available".to_string());
    }

    for (wl_out, name) in state.wl_outputs.clone() {
        let info = outputs.iter().find(|o| o.name == name);
        let (gx, gy, lw, lh) = info.map_or((0, 0, 1920, 1080), |o| {
            (o.x, o.y, o.effective_width(), o.effective_height())
        });
        let idx = state.overlays.len();
        let surf = comp.create_surface(&qh, ());
        let layer = ls.get_layer_surface(
            &surf,
            Some(&wl_out),
            Layer::Overlay,
            "portal-screen-mirror-selector".to_string(),
            &qh,
            idx,
        );
        layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
        layer.set_size(0, 0);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        surf.commit();

        state.overlays.push(OverlaySurface {
            out_name: name,
            global_x: gx,
            global_y: gy,
            width: lw,
            height: lh,
            surface: surf,
            layer,
            shm_buf: None,
            configured: false,
        });
    }

    if state.overlays.is_empty() {
        return Err("No Wayland outputs bound for selector".to_string());
    }

    queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
    state.redraw_all(&qh);
    let _ = conn.flush();

    while !state.finished {
        queue
            .blocking_dispatch(&mut state)
            .map_err(|e| e.to_string())?;
        if state.needs_redraw && !state.finished {
            state.redraw_all(&qh);
            let _ = conn.flush();
        }
    }

    let picked = if state.cancelled { None } else { state.result };

    for mut ov in state.overlays.drain(..) {
        let _ = &ov.out_name;
        ov.shm_buf.take();
        ov.layer.destroy();
        ov.surface.destroy();
    }
    if let Some(dev) = state.cursor_dev.take() {
        dev.destroy();
    }
    if let Some(mgr) = state.cursor_mgr.take() {
        mgr.destroy();
    }
    if let Some(p) = state.pointer.take() {
        if Proxy::version(&p) >= 3 {
            p.release();
        }
    }
    if let Some(k) = state.keyboard.take() {
        if Proxy::version(&k) >= 3 {
            k.release();
        }
    }
    let _ = queue.roundtrip(&mut state);

    Ok(picked)
}
