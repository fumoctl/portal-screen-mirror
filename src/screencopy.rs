use crate::outputs::OutputInfo;
use crate::pipewire::PixelFormat;
use crate::selector::RegionRect;
use std::ffi::CString;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
};

const DRM_FORMAT_ARGB2101010: u32 = 0x3033_5241;
const DRM_FORMAT_XRGB2101010: u32 = 0x3033_5258;
const DRM_FORMAT_ABGR2101010: u32 = 0x3033_4241;
const DRM_FORMAT_XBGR2101010: u32 = 0x3033_4258;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RawShmKind {
    Direct(PixelFormat),
    Argb2101010,
    Abgr2101010,
}

fn map_wl_shm_format(raw: u32) -> Option<(wl_shm::Format, RawShmKind)> {
    let wl_fmt = match wl_shm::Format::try_from(raw) {
        Ok(f) => f,
        Err(_) => return None,
    };
    match raw {
        0 | 0x3432_5241 => Some((wl_fmt, RawShmKind::Direct(PixelFormat::Bgra))),
        1 | 0x3432_5258 => Some((wl_fmt, RawShmKind::Direct(PixelFormat::Bgrx))),
        0x3432_4241 => Some((wl_fmt, RawShmKind::Direct(PixelFormat::Rgba))),
        0x3432_4258 => Some((wl_fmt, RawShmKind::Direct(PixelFormat::Rgbx))),
        DRM_FORMAT_ARGB2101010 | DRM_FORMAT_XRGB2101010 => Some((wl_fmt, RawShmKind::Argb2101010)),
        DRM_FORMAT_ABGR2101010 | DRM_FORMAT_XBGR2101010 => Some((wl_fmt, RawShmKind::Abgr2101010)),
        _ => None,
    }
}

struct ScreencopySlot {
    raw_fmt: u32,
    kind: RawShmKind,
    width: u32,
    height: u32,
    stride: u32,
    byte_len: usize,
    fd: RawFd,
    ptr: *mut u8,
    pool: wl_shm_pool::WlShmPool,
    buffer: wl_buffer::WlBuffer,
}

impl ScreencopySlot {
    #[allow(clippy::too_many_arguments)]
    fn create(
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<ScState>,
        raw_fmt: u32,
        wl_fmt: wl_shm::Format,
        kind: RawShmKind,
        width: u32,
        height: u32,
        stride: u32,
    ) -> Option<Self> {
        let byte_len = (stride as usize).checked_mul(height as usize)?;
        if byte_len == 0 {
            return None;
        }
        let cname = CString::new("psm-wlr-screencopy").ok()?;
        let fd = unsafe { libc::memfd_create(cname.as_ptr(), libc::MFD_CLOEXEC) };
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
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            wl_fmt,
            qh,
            (),
        );
        Some(Self {
            raw_fmt,
            kind,
            width,
            height,
            stride,
            byte_len,
            fd,
            ptr: ptr as *mut u8,
            pool,
            buffer,
        })
    }

    fn as_bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.byte_len) }
    }
}

impl Drop for ScreencopySlot {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.pool.destroy();
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.byte_len);
            libc::close(self.fd);
        }
    }
}

struct ScState {
    shm: Option<wl_shm::WlShm>,
    mgr: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    mgr_ver: u32,
    xdg_mgr: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    outputs: Vec<(u32, wl_output::WlOutput, String)>,

    target_name: String,
    target_global: Option<u32>,
    output_gone: bool,

    slot: Option<ScreencopySlot>,
    active_frame: Option<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1>,
    buffer_selected: bool,
    copied_this_frame: bool,
    first_frame: bool,
    flags_y_invert: bool,
    frame_ready: bool,
    frame_failed: bool,
    fatal_err: Option<String>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for ScState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => match interface.as_str() {
                "wl_shm" => {
                    state.shm = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "zwlr_screencopy_manager_v1" => {
                    let ver = version.min(3);
                    state.mgr_ver = ver;
                    state.mgr = Some(registry.bind(name, ver, qh, ()));
                }
                "zxdg_output_manager_v1" => {
                    state.xdg_mgr = Some(registry.bind(name, version.min(3), qh, ()));
                }
                "wl_output" if version >= 2 => {
                    let idx = state.outputs.len();
                    let wl = registry.bind(name, version.min(4), qh, idx);
                    state.outputs.push((name, wl, String::new()));
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                if state.target_global == Some(name) {
                    state.output_gone = true;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<zxdg_output_manager_v1::ZxdgOutputManagerV1, ()> for ScState {
    fn event(
        _: &mut Self,
        _: &zxdg_output_manager_v1::ZxdgOutputManagerV1,
        _: zxdg_output_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, usize> for ScState {
    fn event(
        state: &mut Self,
        _: &zxdg_output_v1::ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_output_v1::Event::Name { name } = event {
            if let Some(entry) = state.outputs.get_mut(*idx) {
                if entry.2.is_empty() {
                    entry.2 = name;
                }
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, usize> for ScState {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            if let Some(entry) = state.outputs.get_mut(*idx) {
                entry.2 = name;
            }
        }
    }
}

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for ScState {
    fn event(
        state: &mut Self,
        frame: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                if state.buffer_selected || state.copied_this_frame {
                    return;
                }
                let raw_fmt: u32 = format.into();
                let Some((wl_fmt, kind)) = map_wl_shm_format(raw_fmt) else {
                    return;
                };
                if width == 0 || height == 0 || stride < width * 4 {
                    return;
                }
                let reuse = state.slot.as_ref().is_some_and(|s| {
                    (s.raw_fmt, s.width, s.height, s.stride) == (raw_fmt, width, height, stride)
                });
                if !reuse {
                    let Some(ref shm) = state.shm else {
                        return;
                    };
                    state.slot = ScreencopySlot::create(
                        shm, qh, raw_fmt, wl_fmt, kind, width, height, stride,
                    );
                }
                let Some(ref slot) = state.slot else {
                    state.fatal_err =
                        Some("Failed allocating wlr-screencopy SHM buffer".to_string());
                    return;
                };
                state.buffer_selected = true;
                if state.mgr_ver < 3 {
                    state.copied_this_frame = true;
                    if !state.first_frame && state.mgr_ver >= 2 {
                        frame.copy_with_damage(&slot.buffer);
                    } else {
                        frame.copy(&slot.buffer);
                    }
                }
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => {
                if state.copied_this_frame {
                    return;
                }
                let Some(slot) = state.slot.as_ref().filter(|_| state.buffer_selected) else {
                    state.fatal_err = Some(
                        "Compositor offered no supported wlr-screencopy SHM format".to_string(),
                    );
                    return;
                };
                state.copied_this_frame = true;
                if !state.first_frame && state.mgr_ver >= 2 {
                    frame.copy_with_damage(&slot.buffer);
                } else {
                    frame.copy(&slot.buffer);
                }
            }
            zwlr_screencopy_frame_v1::Event::Flags {
                flags: WEnum::Value(fl),
            } => {
                state.flags_y_invert = fl.contains(zwlr_screencopy_frame_v1::Flags::YInvert);
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => {
                state.first_frame = false;
                state.frame_ready = true;
                frame.destroy();
                state.active_frame = None;
            }
            zwlr_screencopy_frame_v1::Event::Failed => {
                state.frame_failed = true;
                frame.destroy();
                state.active_frame = None;
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_shm::WlShm, ()> for ScState {
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
impl Dispatch<wl_shm_pool::WlShmPool, ()> for ScState {
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
impl Dispatch<wl_buffer::WlBuffer, ()> for ScState {
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
impl Dispatch<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, ()> for ScState {
    fn event(
        _: &mut Self,
        _: &zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
        _: zwlr_screencopy_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

pub struct WlrScreencopyRegion {
    conn: Connection,
    queue: EventQueue<ScState>,
    qh: QueueHandle<ScState>,
    state: ScState,
    target_wl: wl_output::WlOutput,
    overlay_cursor: i32,
    lx: i32,
    ly: i32,
    lw: i32,
    lh: i32,
    packed_scratch: Vec<u8>,
}

impl WlrScreencopyRegion {
    pub fn try_open(
        source_output: &OutputInfo,
        region: RegionRect,
        show_cursor: bool,
    ) -> Result<Option<Self>, String> {
        let ow = source_output.effective_width();
        let oh = source_output.effective_height();
        if ow < 2 || oh < 2 {
            return Ok(None);
        }

        let lx = (region.x - source_output.x).clamp(0, ow - 2);
        let ly = (region.y - source_output.y).clamp(0, oh - 2);
        let lw = (region.x + region.width - (source_output.x + lx)).clamp(2, ow - lx);
        let lh = (region.y + region.height - (source_output.y + ly)).clamp(2, oh - ly);

        let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();

        let mut state = ScState {
            shm: None,
            mgr: None,
            mgr_ver: 0,
            xdg_mgr: None,
            outputs: Vec::new(),
            target_name: source_output.name.clone(),
            target_global: None,
            output_gone: false,
            slot: None,
            active_frame: None,
            buffer_selected: false,
            copied_this_frame: false,
            first_frame: true,
            flags_y_invert: false,
            frame_ready: false,
            frame_failed: false,
            fatal_err: None,
        };

        conn.display().get_registry(&qh, ());
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;

        if state.mgr.is_none() || state.shm.is_none() {
            return Ok(None);
        }

        let mut xdg_outs = Vec::new();
        if let Some(ref xdg_mgr) = state.xdg_mgr {
            for (idx, (_, wl, _)) in state.outputs.iter().enumerate() {
                xdg_outs.push(xdg_mgr.get_xdg_output(wl, &qh, idx));
            }
        }

        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;

        for xdg in xdg_outs {
            xdg.destroy();
        }
        if let Some(xdg_mgr) = state.xdg_mgr.take() {
            xdg_mgr.destroy();
        }
        for (idx, entry) in state.outputs.iter_mut().enumerate() {
            if entry.2.is_empty() {
                entry.2 = format!("WL-{idx}");
            }
        }

        let Some((global_id, target_wl, _)) = state
            .outputs
            .iter()
            .find(|(_, _, name)| name == &state.target_name)
            .cloned()
        else {
            return Ok(None);
        };
        state.target_global = Some(global_id);

        let mut cap = Self {
            conn,
            queue,
            qh,
            state,
            target_wl,
            overlay_cursor: i32::from(show_cursor),
            lx,
            ly,
            lw,
            lh,
            packed_scratch: Vec::new(),
        };
        cap.request_frame();
        Ok(Some(cap))
    }

    pub fn wayland_fd(&self) -> RawFd {
        self.conn.as_fd().as_raw_fd()
    }

    pub fn in_flight(&self) -> bool {
        self.state.active_frame.is_some()
    }

    pub fn request_frame(&mut self) {
        if self.state.active_frame.is_some() || self.state.output_gone {
            return;
        }
        let Some(ref mgr) = self.state.mgr else {
            return;
        };
        self.state.buffer_selected = false;
        self.state.copied_this_frame = false;
        self.state.flags_y_invert = false;
        self.state.frame_ready = false;
        self.state.frame_failed = false;
        let fr = mgr.capture_output_region(
            self.overlay_cursor,
            &self.target_wl,
            self.lx,
            self.ly,
            self.lw,
            self.lh,
            &self.qh,
            (),
        );
        self.state.active_frame = Some(fr);
        let _ = self.conn.flush();
    }

    #[allow(clippy::type_complexity)]
    pub fn dispatch_and_take_frame(
        &mut self,
    ) -> Result<Option<(u32, u32, PixelFormat, bool, &[u8])>, String> {
        let _ = self.conn.flush();
        if let Some(guard) = self.conn.prepare_read() {
            let _ = guard.read();
        }
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(|e| format!("wlr-screencopy Wayland error: {e}"))?;
        let _ = self.conn.flush();

        if let Some(ref err) = self.state.fatal_err {
            return Err(err.clone());
        }
        if self.state.output_gone {
            return Err("Captured source output disconnected".to_string());
        }
        if self.state.frame_failed {
            self.state.frame_failed = false;
            return Ok(None);
        }
        if !self.state.frame_ready {
            return Ok(None);
        }
        self.state.frame_ready = false;

        let Some(ref slot) = self.state.slot else {
            return Ok(None);
        };
        let w = slot.width;
        let h = slot.height;
        let stride = slot.stride as usize;
        let row_bytes = (w as usize) * 4;
        let total = row_bytes * (h as usize);
        let y_inv = self.state.flags_y_invert;
        let raw = slot.as_bytes();

        match slot.kind {
            RawShmKind::Direct(fmt) if stride == row_bytes => {
                Ok(Some((w, h, fmt, y_inv, &raw[..total])))
            }
            RawShmKind::Direct(fmt) => {
                self.packed_scratch.resize(total, 0);
                for y in 0..(h as usize) {
                    let src = &raw[y * stride..y * stride + row_bytes];
                    let dst = &mut self.packed_scratch[y * row_bytes..(y + 1) * row_bytes];
                    dst.copy_from_slice(src);
                }
                Ok(Some((w, h, fmt, y_inv, &self.packed_scratch)))
            }
            RawShmKind::Argb2101010 | RawShmKind::Abgr2101010 => {
                let is_argb = slot.kind == RawShmKind::Argb2101010;
                self.packed_scratch.resize(total, 0);
                for y in 0..(h as usize) {
                    let row = &raw[y * stride..y * stride + row_bytes];
                    let dst = &mut self.packed_scratch[y * row_bytes..(y + 1) * row_bytes];
                    for x in 0..(w as usize) {
                        let v = u32::from_le_bytes([
                            row[x * 4],
                            row[x * 4 + 1],
                            row[x * 4 + 2],
                            row[x * 4 + 3],
                        ]);
                        let c0 = ((v & 0x3ff) >> 2) as u8;
                        let c1 = (((v >> 10) & 0x3ff) >> 2) as u8;
                        let c2 = (((v >> 20) & 0x3ff) >> 2) as u8;
                        if is_argb {
                            /* [9:0]=B, [19:10]=G, [29:20]=R -> RGBA */
                            dst[x * 4] = c2;
                            dst[x * 4 + 1] = c1;
                            dst[x * 4 + 2] = c0;
                        } else {
                            /* [9:0]=R, [19:10]=G, [29:20]=B -> RGBA */
                            dst[x * 4] = c0;
                            dst[x * 4 + 1] = c1;
                            dst[x * 4 + 2] = c2;
                        }
                        dst[x * 4 + 3] = 255;
                    }
                }
                Ok(Some((w, h, PixelFormat::Rgba, y_inv, &self.packed_scratch)))
            }
        }
    }
}

impl Drop for WlrScreencopyRegion {
    fn drop(&mut self) {
        if let Some(fr) = self.state.active_frame.take() {
            fr.destroy();
        }
        self.state.slot.take();
        for (_, wl, _) in self.state.outputs.drain(..) {
            if Proxy::version(&wl) >= 3 {
                wl.release();
            }
        }
        if let Some(mgr) = self.state.mgr.take() {
            mgr.destroy();
        }
        let _ = self.conn.flush();
    }
}
