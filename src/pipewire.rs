use std::ffi::CStr;
use std::os::fd::RawFd;
use std::ptr::NonNull;

pub const MAX_PLANES: usize = 4;
pub const MAX_MODIFIERS_PER_FMT: usize = 64;
pub const MAX_DRM_FORMATS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PixelFormat {
    Rgba = 1,
    Rgbx = 2,
    Bgra = 3,
    Bgrx = 4,
}

impl PixelFormat {
    pub fn from_raw(v: u32) -> Option<Self> {
        match v {
            1 => Some(Self::Rgba),
            2 => Some(Self::Rgbx),
            3 => Some(Self::Bgra),
            4 => Some(Self::Bgrx),
            _ => None,
        }
    }

    pub fn shader_swizzle(self) -> i32 {
        match self {
            Self::Rgba => 0,
            Self::Rgbx => 1,
            Self::Bgra => 2,
            Self::Bgrx => 3,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PsmDrmFormatModifiers {
    pub drm_format: u32,
    pub spa_format_kind: u32,
    pub modifier_count: u32,
    pub modifiers: [u64; MAX_MODIFIERS_PER_FMT],
}

#[repr(C)]
struct PsmFrameRaw {
    is_dmabuf: bool,
    width: u32,
    height: u32,
    format: u32,
    drm_format: u32,
    modifier: u64,
    y_invert: bool,
    plane_count: u32,
    plane_fds: [i32; MAX_PLANES],
    plane_offsets: [u32; MAX_PLANES],
    plane_strides: [u32; MAX_PLANES],
    shm_pixels: *const u8,
    shm_stride: u32,
    shm_size: usize,
    seq: u64,
}

#[repr(C)]
struct PsmPwStreamOpaque {
    _priv: [u8; 0],
}

extern "C" {
    fn psm_pw_start(
        pw_fd: libc::c_int,
        node_id: u32,
        wakeup_eventfd: libc::c_int,
        allow_dmabuf: bool,
        drm_formats: *const PsmDrmFormatModifiers,
        drm_format_count: usize,
        target_fps: u32,
    ) -> *mut PsmPwStreamOpaque;

    fn psm_pw_pop_frame(s: *mut PsmPwStreamOpaque, out_frame: *mut PsmFrameRaw) -> bool;
    fn psm_pw_force_shm(s: *mut PsmPwStreamOpaque);
    fn psm_pw_set_fps(s: *mut PsmPwStreamOpaque, target_fps: u32);
    fn psm_pw_has_error(
        s: *mut PsmPwStreamOpaque,
        err_buf: *mut libc::c_char,
        err_buf_len: usize,
    ) -> bool;
    fn psm_pw_stop(s: *mut PsmPwStreamOpaque);
}

#[derive(Debug)]
pub struct DmaBufPlane {
    pub fd: RawFd,
    pub offset: u32,
    pub stride: u32,
}

#[derive(Debug)]
pub struct DmaBufFrame {
    pub width: u32,
    pub height: u32,
    pub drm_format: u32,
    pub modifier: u64,
    pub y_invert: bool,
    pub planes: Vec<DmaBufPlane>,
}

impl Drop for DmaBufFrame {
    fn drop(&mut self) {
        for plane in &self.planes {
            if plane.fd >= 0 {
                unsafe {
                    libc::close(plane.fd);
                }
            }
        }
    }
}

pub enum CapturedFrame<'a> {
    DmaBuf(DmaBufFrame),
    Shm {
        width: u32,
        height: u32,
        format: PixelFormat,
        y_invert: bool,
        pixels: &'a [u8],
    },
}

pub struct PwCapture {
    inner: NonNull<PsmPwStreamOpaque>,
}

impl PwCapture {
    pub fn start(
        pw_fd: RawFd,
        node_id: u32,
        wakeup_eventfd: RawFd,
        allow_dmabuf: bool,
        drm_formats: &[PsmDrmFormatModifiers],
        target_fps: u32,
    ) -> Result<Self, String> {
        let count = drm_formats.len().min(MAX_DRM_FORMATS);
        let ptr = unsafe {
            psm_pw_start(
                pw_fd,
                node_id,
                wakeup_eventfd,
                allow_dmabuf,
                if count > 0 {
                    drm_formats.as_ptr()
                } else {
                    std::ptr::null()
                },
                count,
                target_fps,
            )
        };
        NonNull::new(ptr)
            .map(|inner| Self { inner })
            .ok_or_else(|| "Failed to initialize PipeWire capture stream".to_string())
    }

    pub fn pop_frame(&mut self) -> Option<CapturedFrame<'_>> {
        let mut raw: PsmFrameRaw = unsafe { std::mem::zeroed() };
        let ok = unsafe { psm_pw_pop_frame(self.inner.as_ptr(), &mut raw) };
        if !ok {
            return None;
        }

        if raw.is_dmabuf {
            let n = (raw.plane_count as usize).min(MAX_PLANES);
            let mut planes = Vec::with_capacity(n);
            for i in 0..n {
                planes.push(DmaBufPlane {
                    fd: raw.plane_fds[i],
                    offset: raw.plane_offsets[i],
                    stride: raw.plane_strides[i],
                });
            }
            Some(CapturedFrame::DmaBuf(DmaBufFrame {
                width: raw.width,
                height: raw.height,
                drm_format: raw.drm_format,
                modifier: raw.modifier,
                y_invert: raw.y_invert,
                planes,
            }))
        } else {
            let format = PixelFormat::from_raw(raw.format)?;
            if raw.shm_pixels.is_null()
                || raw.shm_size < (raw.width as usize) * (raw.height as usize) * 4
            {
                return None;
            }
            let pixels = unsafe { std::slice::from_raw_parts(raw.shm_pixels, raw.shm_size) };
            Some(CapturedFrame::Shm {
                width: raw.width,
                height: raw.height,
                format,
                y_invert: raw.y_invert,
                pixels,
            })
        }
    }

    pub fn force_shm(&self) {
        unsafe {
            psm_pw_force_shm(self.inner.as_ptr());
        }
    }

    pub fn set_fps(&self, target_fps: u32) {
        unsafe {
            psm_pw_set_fps(self.inner.as_ptr(), target_fps);
        }
    }

    pub fn check_error(&self) -> Option<String> {
        let mut buf = [0i8; 256];
        let has = unsafe { psm_pw_has_error(self.inner.as_ptr(), buf.as_mut_ptr(), buf.len()) };
        if has {
            let msg = unsafe { CStr::from_ptr(buf.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            Some(msg)
        } else {
            None
        }
    }
}

impl Drop for PwCapture {
    fn drop(&mut self) {
        unsafe {
            psm_pw_stop(self.inner.as_ptr());
        }
    }
}
