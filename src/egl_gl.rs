use crate::ipc::{FilterMode, FitMode};
use crate::pipewire::{DmaBufFrame, PixelFormat, PsmDrmFormatModifiers, MAX_MODIFIERS_PER_FMT};
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use wayland_egl::WlEglSurface;

type EGLDisplay = *mut c_void;
type EGLConfig = *mut c_void;
type EGLContext = *mut c_void;
type EGLSurface = *mut c_void;
type EGLImageKHR = *mut c_void;
type EGLBoolean = c_uint;
type EGLint = i32;
type EGLuint64KHR = u64;
type GLenum = c_uint;
type GLuint = c_uint;
type GLint = c_int;
type GLsizei = c_int;

const EGL_FALSE: EGLBoolean = 0;
const EGL_NONE: EGLint = 0x3038;
const EGL_RED_SIZE: EGLint = 0x3024;
const EGL_GREEN_SIZE: EGLint = 0x3023;
const EGL_BLUE_SIZE: EGLint = 0x3022;
const EGL_ALPHA_SIZE: EGLint = 0x3021;
const EGL_SURFACE_TYPE: EGLint = 0x3033;
const EGL_WINDOW_BIT: EGLint = 0x0004;
const EGL_RENDERABLE_TYPE: EGLint = 0x3040;
const EGL_OPENGL_ES2_BIT: EGLint = 0x0004;
const EGL_CONTEXT_CLIENT_VERSION: EGLint = 0x3098;
const EGL_EXTENSIONS: EGLint = 0x3055;
const EGL_OPENGL_ES_API: c_uint = 0x30A0;
const EGL_PLATFORM_WAYLAND_EXT: c_uint = 0x31D8;

const EGL_WIDTH: EGLint = 0x3057;
const EGL_HEIGHT: EGLint = 0x3056;
const EGL_LINUX_DMA_BUF_EXT: c_uint = 0x3270;
const EGL_LINUX_DRM_FOURCC_EXT: EGLint = 0x3271;
const EGL_DMA_BUF_PLANE0_FD_EXT: EGLint = 0x3272;
const EGL_DMA_BUF_PLANE0_OFFSET_EXT: EGLint = 0x3273;
const EGL_DMA_BUF_PLANE0_PITCH_EXT: EGLint = 0x3274;
const EGL_DMA_BUF_PLANE1_FD_EXT: EGLint = 0x3275;
const EGL_DMA_BUF_PLANE1_OFFSET_EXT: EGLint = 0x3276;
const EGL_DMA_BUF_PLANE1_PITCH_EXT: EGLint = 0x3277;
const EGL_DMA_BUF_PLANE2_FD_EXT: EGLint = 0x3278;
const EGL_DMA_BUF_PLANE2_OFFSET_EXT: EGLint = 0x3279;
const EGL_DMA_BUF_PLANE2_PITCH_EXT: EGLint = 0x327A;
const EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT: EGLint = 0x3443;
const EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT: EGLint = 0x3444;
const EGL_DMA_BUF_PLANE1_MODIFIER_LO_EXT: EGLint = 0x3445;
const EGL_DMA_BUF_PLANE1_MODIFIER_HI_EXT: EGLint = 0x3446;
const EGL_DMA_BUF_PLANE2_MODIFIER_LO_EXT: EGLint = 0x3447;
const EGL_DMA_BUF_PLANE2_MODIFIER_HI_EXT: EGLint = 0x3448;
const EGL_DMA_BUF_PLANE3_FD_EXT: EGLint = 0x3440;
const EGL_DMA_BUF_PLANE3_OFFSET_EXT: EGLint = 0x3441;
const EGL_DMA_BUF_PLANE3_PITCH_EXT: EGLint = 0x3442;
const EGL_DMA_BUF_PLANE3_MODIFIER_LO_EXT: EGLint = 0x3449;
const EGL_DMA_BUF_PLANE3_MODIFIER_HI_EXT: EGLint = 0x344A;

const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;
const DRM_FORMAT_ARGB8888: u32 = 0x3432_5241;
const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258;
const DRM_FORMAT_ABGR8888: u32 = 0x3432_4241;
const DRM_FORMAT_XBGR8888: u32 = 0x3432_4258;

const GL_NO_ERROR: GLenum = 0;
const GL_COLOR_BUFFER_BIT: GLenum = 0x0000_4000;
const GL_TEXTURE_2D: GLenum = 0x0DE1;
const GL_TEXTURE_EXTERNAL_OES: GLenum = 0x8D65;
const GL_TEXTURE0: GLenum = 0x84C0;
const GL_TEXTURE_MIN_FILTER: GLenum = 0x2801;
const GL_TEXTURE_MAG_FILTER: GLenum = 0x2800;
const GL_TEXTURE_WRAP_S: GLenum = 0x2802;
const GL_TEXTURE_WRAP_T: GLenum = 0x2803;
const GL_CLAMP_TO_EDGE: GLint = 0x812F;
const GL_NEAREST: GLint = 0x2600;
const GL_LINEAR: GLint = 0x2601;
const GL_RGBA: GLenum = 0x1908;
const GL_UNSIGNED_BYTE: GLenum = 0x1401;
const GL_UNPACK_ALIGNMENT: GLenum = 0x0CF5;
const GL_VERTEX_SHADER: GLenum = 0x8B31;
const GL_FRAGMENT_SHADER: GLenum = 0x8B30;
const GL_COMPILE_STATUS: GLenum = 0x8B81;
const GL_LINK_STATUS: GLenum = 0x8B82;
const GL_EXTENSIONS: GLenum = 0x1F03;
const GL_TRIANGLES: GLenum = 0x0004;
const GL_FLOAT: GLenum = 0x1406;

extern "C" {
    fn eglGetDisplay(display_id: *mut c_void) -> EGLDisplay;
    fn eglInitialize(dpy: EGLDisplay, major: *mut EGLint, minor: *mut EGLint) -> EGLBoolean;
    fn eglTerminate(dpy: EGLDisplay) -> EGLBoolean;
    fn eglBindAPI(api: c_uint) -> EGLBoolean;
    fn eglChooseConfig(
        dpy: EGLDisplay,
        attrib_list: *const EGLint,
        configs: *mut EGLConfig,
        config_size: EGLint,
        num_config: *mut EGLint,
    ) -> EGLBoolean;
    fn eglCreateContext(
        dpy: EGLDisplay,
        config: EGLConfig,
        share_context: EGLContext,
        attrib_list: *const EGLint,
    ) -> EGLContext;
    fn eglDestroyContext(dpy: EGLDisplay, ctx: EGLContext) -> EGLBoolean;
    fn eglCreateWindowSurface(
        dpy: EGLDisplay,
        config: EGLConfig,
        win: *mut c_void,
        attrib_list: *const EGLint,
    ) -> EGLSurface;
    fn eglDestroySurface(dpy: EGLDisplay, surface: EGLSurface) -> EGLBoolean;
    fn eglMakeCurrent(
        dpy: EGLDisplay,
        draw: EGLSurface,
        read: EGLSurface,
        ctx: EGLContext,
    ) -> EGLBoolean;
    fn eglSwapInterval(dpy: EGLDisplay, interval: EGLint) -> EGLBoolean;
    fn eglSwapBuffers(dpy: EGLDisplay, surface: EGLSurface) -> EGLBoolean;
    fn eglQueryString(dpy: EGLDisplay, name: EGLint) -> *const c_char;
    fn eglGetProcAddress(procname: *const c_char) -> *const c_void;
    fn eglGetError() -> EGLint;

    fn glGetError() -> GLenum;
    fn glGetString(name: GLenum) -> *const u8;
    fn glViewport(x: GLint, y: GLint, width: GLsizei, height: GLsizei);
    fn glClearColor(red: f32, green: f32, blue: f32, alpha: f32);
    fn glClear(mask: GLenum);
    fn glGenTextures(n: GLsizei, textures: *mut GLuint);
    fn glDeleteTextures(n: GLsizei, textures: *const GLuint);
    fn glActiveTexture(texture: GLenum);
    fn glBindTexture(target: GLenum, texture: GLuint);
    fn glTexParameteri(target: GLenum, pname: GLenum, param: GLint);
    fn glPixelStorei(pname: GLenum, param: GLint);
    fn glTexImage2D(
        target: GLenum,
        level: GLint,
        internalformat: GLint,
        width: GLsizei,
        height: GLsizei,
        border: GLint,
        format: GLenum,
        type_: GLenum,
        pixels: *const c_void,
    );
    fn glTexSubImage2D(
        target: GLenum,
        level: GLint,
        xoffset: GLint,
        yoffset: GLint,
        width: GLsizei,
        height: GLsizei,
        format: GLenum,
        type_: GLenum,
        pixels: *const c_void,
    );
    fn glCreateShader(type_: GLenum) -> GLuint;
    fn glShaderSource(
        shader: GLuint,
        count: GLsizei,
        string: *const *const c_char,
        length: *const GLint,
    );
    fn glCompileShader(shader: GLuint);
    fn glGetShaderiv(shader: GLuint, pname: GLenum, params: *mut GLint);
    fn glGetShaderInfoLog(
        shader: GLuint,
        buf_size: GLsizei,
        length: *mut GLsizei,
        info_log: *mut c_char,
    );
    fn glDeleteShader(shader: GLuint);
    fn glCreateProgram() -> GLuint;
    fn glAttachShader(program: GLuint, shader: GLuint);
    fn glBindAttribLocation(program: GLuint, index: GLuint, name: *const c_char);
    fn glLinkProgram(program: GLuint);
    fn glGetProgramiv(program: GLuint, pname: GLenum, params: *mut GLint);
    fn glDeleteProgram(program: GLuint);
    fn glUseProgram(program: GLuint);
    fn glGetUniformLocation(program: GLuint, name: *const c_char) -> GLint;
    fn glUniform1i(location: GLint, v0: GLint);
    fn glUniform4f(location: GLint, v0: f32, v1: f32, v2: f32, v3: f32);
    fn glEnableVertexAttribArray(index: GLuint);
    fn glVertexAttribPointer(
        index: GLuint,
        size: GLint,
        type_: GLenum,
        normalized: u8,
        stride: GLsizei,
        pointer: *const c_void,
    );
    fn glDrawArrays(mode: GLenum, first: GLint, count: GLsizei);
}

type FnGetPlatformDisplayExt =
    unsafe extern "C" fn(c_uint, *mut c_void, *const EGLint) -> EGLDisplay;
type FnCreateImageKhr =
    unsafe extern "C" fn(EGLDisplay, EGLContext, c_uint, *mut c_void, *const EGLint) -> EGLImageKHR;
type FnDestroyImageKhr = unsafe extern "C" fn(EGLDisplay, EGLImageKHR) -> EGLBoolean;
type FnQueryDmaBufFormatsExt =
    unsafe extern "C" fn(EGLDisplay, EGLint, *mut EGLint, *mut EGLint) -> EGLBoolean;
type FnQueryDmaBufModifiersExt = unsafe extern "C" fn(
    EGLDisplay,
    EGLint,
    EGLint,
    *mut EGLuint64KHR,
    *mut EGLBoolean,
    *mut EGLint,
) -> EGLBoolean;
type FnEglImageTargetTexture2DOes = unsafe extern "C" fn(GLenum, EGLImageKHR);

const VERT_SRC: &str = r#"
attribute vec2 a_pos;
attribute vec2 a_uv;
uniform vec4 u_uv_rect;
uniform int u_y_invert;
varying vec2 v_uv;
void main() {
    float u = mix(u_uv_rect.x, u_uv_rect.z, a_uv.x);
    float v = mix(u_uv_rect.y, u_uv_rect.w, a_uv.y);
    if (u_y_invert != 0) {
        v = 1.0 - v;
    }
    v_uv = vec2(u, v);
    gl_Position = vec4(a_pos, 0.0, 1.0);
}
"#;

const FRAG_SHM_SRC: &str = r#"
precision mediump float;
varying vec2 v_uv;
uniform sampler2D u_tex;
uniform int u_swizzle;
void main() {
    vec4 c = texture2D(u_tex, v_uv);
    if (u_swizzle == 1) {
        gl_FragColor = vec4(c.rgb, 1.0);
    } else if (u_swizzle == 2) {
        gl_FragColor = vec4(c.bgr, c.a);
    } else if (u_swizzle == 3) {
        gl_FragColor = vec4(c.bgr, 1.0);
    } else {
        gl_FragColor = c;
    }
}
"#;

const FRAG_EXT_SRC: &str = r#"
#extension GL_OES_EGL_image_external : require
precision mediump float;
varying vec2 v_uv;
uniform samplerExternalOES u_tex;
void main() {
    vec4 c = texture2D(u_tex, v_uv);
    gl_FragColor = vec4(c.rgb, 1.0);
}
"#;

const QUAD_VERTS: [f32; 24] = [
    -1.0, -1.0, 0.0, 1.0, // bottom-left
    1.0, -1.0, 1.0, 1.0, // bottom-right
    -1.0, 1.0, 0.0, 0.0, // top-left
    -1.0, 1.0, 0.0, 0.0, // top-left
    1.0, -1.0, 1.0, 1.0, // bottom-right
    1.0, 1.0, 1.0, 0.0, // top-right
];

struct ShaderProg {
    id: GLuint,
    u_tex: GLint,
    u_uv_rect: GLint,
    u_y_invert: GLint,
    u_swizzle: GLint,
}

pub struct EglRenderer {
    dpy: EGLDisplay,
    ctx: EGLContext,
    surf: EGLSurface,
    wl_egl: WlEglSurface,
    surf_w: i32,
    surf_h: i32,

    shm_prog: ShaderProg,
    ext_prog: Option<ShaderProg>,

    shm_tex: GLuint,
    shm_tex_dims: (u32, u32),
    ext_tex: GLuint,
    current_image: EGLImageKHR,

    create_image: Option<FnCreateImageKhr>,
    destroy_image: Option<FnDestroyImageKhr>,
    query_formats: Option<FnQueryDmaBufFormatsExt>,
    query_modifiers: Option<FnQueryDmaBufModifiersExt>,
    image_target_tex: Option<FnEglImageTargetTexture2DOes>,
    has_modifiers_ext: bool,
}

fn load_proc<T>(name: &str) -> Option<T> {
    let cname = CString::new(name).ok()?;
    let ptr = unsafe { eglGetProcAddress(cname.as_ptr()) };
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { std::mem::transmute_copy(&ptr) })
    }
}

fn compile_shader(kind: GLenum, src: &str) -> Result<GLuint, String> {
    let csrc = CString::new(src).map_err(|e| e.to_string())?;
    unsafe {
        let sh = glCreateShader(kind);
        let ptr = csrc.as_ptr();
        glShaderSource(sh, 1, &ptr, std::ptr::null());
        glCompileShader(sh);
        let mut ok = 0;
        glGetShaderiv(sh, GL_COMPILE_STATUS, &mut ok);
        if ok == 0 {
            let mut buf = [0i8; 512];
            let mut len = 0;
            glGetShaderInfoLog(sh, 511, &mut len, buf.as_mut_ptr());
            glDeleteShader(sh);
            let msg = CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned();
            return Err(format!("GLSL compile error: {msg}"));
        }
        Ok(sh)
    }
}

fn build_program(vert_src: &str, frag_src: &str) -> Result<ShaderProg, String> {
    let vs = compile_shader(GL_VERTEX_SHADER, vert_src)?;
    let fs = match compile_shader(GL_FRAGMENT_SHADER, frag_src) {
        Ok(s) => s,
        Err(e) => {
            unsafe { glDeleteShader(vs) };
            return Err(e);
        }
    };
    unsafe {
        let prog = glCreateProgram();
        glAttachShader(prog, vs);
        glAttachShader(prog, fs);
        let a_pos = CString::new("a_pos").unwrap();
        let a_uv = CString::new("a_uv").unwrap();
        glBindAttribLocation(prog, 0, a_pos.as_ptr());
        glBindAttribLocation(prog, 1, a_uv.as_ptr());
        glLinkProgram(prog);
        glDeleteShader(vs);
        glDeleteShader(fs);

        let mut ok = 0;
        glGetProgramiv(prog, GL_LINK_STATUS, &mut ok);
        if ok == 0 {
            glDeleteProgram(prog);
            return Err("Failed linking GLSL program".to_string());
        }

        let u_tex = glGetUniformLocation(prog, CString::new("u_tex").unwrap().as_ptr());
        let u_uv_rect = glGetUniformLocation(prog, CString::new("u_uv_rect").unwrap().as_ptr());
        let u_y_invert = glGetUniformLocation(prog, CString::new("u_y_invert").unwrap().as_ptr());
        let u_swizzle = glGetUniformLocation(prog, CString::new("u_swizzle").unwrap().as_ptr());
        Ok(ShaderProg {
            id: prog,
            u_tex,
            u_uv_rect,
            u_y_invert,
            u_swizzle,
        })
    }
}

impl EglRenderer {
    pub fn new(
        wl_display_ptr: *mut c_void,
        wl_egl: WlEglSurface,
        width: i32,
        height: i32,
    ) -> Result<Self, String> {
        let dpy = unsafe {
            if let Some(get_plat) = load_proc::<FnGetPlatformDisplayExt>("eglGetPlatformDisplayEXT")
            {
                let d = get_plat(EGL_PLATFORM_WAYLAND_EXT, wl_display_ptr, std::ptr::null());
                if !d.is_null() {
                    d
                } else {
                    eglGetDisplay(wl_display_ptr)
                }
            } else {
                eglGetDisplay(wl_display_ptr)
            }
        };
        if dpy.is_null() {
            return Err("eglGetDisplay failed for Wayland connection".to_string());
        }

        let (mut maj, mut min) = (0, 0);
        if unsafe { eglInitialize(dpy, &mut maj, &mut min) } == EGL_FALSE {
            return Err(format!("eglInitialize failed (0x{:x})", unsafe {
                eglGetError()
            }));
        }
        if unsafe { eglBindAPI(EGL_OPENGL_ES_API) } == EGL_FALSE {
            return Err("eglBindAPI(EGL_OPENGL_ES_API) failed".to_string());
        }

        let cfg_attribs = [
            EGL_SURFACE_TYPE,
            EGL_WINDOW_BIT,
            EGL_RENDERABLE_TYPE,
            EGL_OPENGL_ES2_BIT,
            EGL_RED_SIZE,
            8,
            EGL_GREEN_SIZE,
            8,
            EGL_BLUE_SIZE,
            8,
            EGL_ALPHA_SIZE,
            0,
            EGL_NONE,
        ];
        let mut cfg: EGLConfig = std::ptr::null_mut();
        let mut n_cfg = 0;
        if unsafe { eglChooseConfig(dpy, cfg_attribs.as_ptr(), &mut cfg, 1, &mut n_cfg) }
            == EGL_FALSE
            || n_cfg < 1
        {
            return Err("No matching 24-bit EGL window config found".to_string());
        }

        let ctx_attribs = [EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE];
        let ctx = unsafe { eglCreateContext(dpy, cfg, std::ptr::null_mut(), ctx_attribs.as_ptr()) };
        if ctx.is_null() {
            return Err("eglCreateContext(GLES2) failed".to_string());
        }

        let surf = unsafe {
            eglCreateWindowSurface(dpy, cfg, wl_egl.ptr() as *mut c_void, std::ptr::null())
        };
        if surf.is_null() {
            unsafe { eglDestroyContext(dpy, ctx) };
            return Err("eglCreateWindowSurface failed".to_string());
        }

        if unsafe { eglMakeCurrent(dpy, surf, surf, ctx) } == EGL_FALSE {
            unsafe {
                eglDestroySurface(dpy, surf);
                eglDestroyContext(dpy, ctx);
            }
            return Err("eglMakeCurrent failed".to_string());
        }

        /* Non-blocking swap; frame pacing is driven explicitly via wl_surface.frame() */
        unsafe {
            let _ = eglSwapInterval(dpy, 0);
            glPixelStorei(GL_UNPACK_ALIGNMENT, 4);
        }

        let egl_exts = unsafe {
            let p = eglQueryString(dpy, EGL_EXTENSIONS);
            if p.is_null() {
                String::new()
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        };
        let gl_exts = unsafe {
            let p = glGetString(GL_EXTENSIONS);
            if p.is_null() {
                String::new()
            } else {
                CStr::from_ptr(p as *const c_char)
                    .to_string_lossy()
                    .into_owned()
            }
        };

        let has_dmabuf_ext = egl_exts.contains("EGL_EXT_image_dma_buf_import");
        let has_modifiers_ext = egl_exts.contains("EGL_EXT_image_dma_buf_import_modifiers");
        let has_oes_ext = gl_exts.contains("GL_OES_EGL_image_external");

        let shm_prog = build_program(VERT_SRC, FRAG_SHM_SRC)?;
        let ext_prog = if has_dmabuf_ext && has_oes_ext {
            build_program(VERT_SRC, FRAG_EXT_SRC).ok()
        } else {
            None
        };

        let mut tex_ids = [0u32; 2];
        unsafe {
            glGenTextures(2, tex_ids.as_mut_ptr());
            for (target, tid) in [
                (GL_TEXTURE_2D, tex_ids[0]),
                (GL_TEXTURE_EXTERNAL_OES, tex_ids[1]),
            ] {
                if target == GL_TEXTURE_EXTERNAL_OES && ext_prog.is_none() {
                    continue;
                }
                glBindTexture(target, tid);
                glTexParameteri(target, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
                glTexParameteri(target, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
                glTexParameteri(target, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
                glTexParameteri(target, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
            }
        }

        let r = Self {
            dpy,
            ctx,
            surf,
            wl_egl,
            surf_w: width.max(1),
            surf_h: height.max(1),
            shm_prog,
            ext_prog,
            shm_tex: tex_ids[0],
            shm_tex_dims: (0, 0),
            ext_tex: tex_ids[1],
            current_image: std::ptr::null_mut(),
            create_image: if has_dmabuf_ext {
                load_proc("eglCreateImageKHR")
            } else {
                None
            },
            destroy_image: if has_dmabuf_ext {
                load_proc("eglDestroyImageKHR")
            } else {
                None
            },
            query_formats: if has_modifiers_ext {
                load_proc("eglQueryDmaBufFormatsEXT")
            } else {
                None
            },
            query_modifiers: if has_modifiers_ext {
                load_proc("eglQueryDmaBufModifiersEXT")
            } else {
                None
            },
            image_target_tex: if has_oes_ext {
                load_proc("glEGLImageTargetTexture2DOES")
            } else {
                None
            },
            has_modifiers_ext,
        };

        Ok(r)
    }

    pub fn resize(&mut self, width: i32, height: i32) {
        let w = width.max(1);
        let h = height.max(1);
        if (w, h) != (self.surf_w, self.surf_h) {
            self.wl_egl.resize(w, h, 0, 0);
            self.surf_w = w;
            self.surf_h = h;
        }
    }

    pub fn dmabuf_capable(&self) -> bool {
        self.ext_prog.is_some() && self.create_image.is_some() && self.image_target_tex.is_some()
    }

    pub fn query_dmabuf_formats(&self) -> Vec<PsmDrmFormatModifiers> {
        if !self.dmabuf_capable() {
            return Vec::new();
        }
        let wanted = [
            (DRM_FORMAT_XRGB8888, PixelFormat::Bgrx as u32),
            (DRM_FORMAT_ARGB8888, PixelFormat::Bgra as u32),
            (DRM_FORMAT_XBGR8888, PixelFormat::Rgbx as u32),
            (DRM_FORMAT_ABGR8888, PixelFormat::Rgba as u32),
        ];

        let mut supported_drm: Option<Vec<i32>> = None;
        if let Some(qfmt) = self.query_formats {
            let mut count = 0;
            if unsafe { qfmt(self.dpy, 0, std::ptr::null_mut(), &mut count) } != EGL_FALSE
                && count > 0
            {
                let mut list = vec![0i32; count as usize];
                if unsafe { qfmt(self.dpy, count, list.as_mut_ptr(), &mut count) } != EGL_FALSE {
                    list.truncate(count as usize);
                    supported_drm = Some(list);
                }
            }
        }

        let mut out = Vec::new();
        for (drm_fmt, spa_kind) in wanted {
            if let Some(ref sup) = supported_drm {
                if !sup.contains(&(drm_fmt as i32)) {
                    continue;
                }
            }
            let mut entry = PsmDrmFormatModifiers {
                drm_format: drm_fmt,
                spa_format_kind: spa_kind,
                modifier_count: 0,
                modifiers: [0u64; MAX_MODIFIERS_PER_FMT],
            };

            if let Some(qmod) = self.query_modifiers {
                let mut mcount = 0;
                if unsafe {
                    qmod(
                        self.dpy,
                        drm_fmt as i32,
                        0,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        &mut mcount,
                    )
                } != EGL_FALSE
                    && mcount > 0
                {
                    let mut mods = vec![0u64; mcount as usize];
                    let mut ext_only = vec![0u32; mcount as usize];
                    if unsafe {
                        qmod(
                            self.dpy,
                            drm_fmt as i32,
                            mcount,
                            mods.as_mut_ptr(),
                            ext_only.as_mut_ptr(),
                            &mut mcount,
                        )
                    } != EGL_FALSE
                    {
                        for &m in mods
                            .iter()
                            .take((mcount as usize).min(MAX_MODIFIERS_PER_FMT - 1))
                        {
                            entry.modifiers[entry.modifier_count as usize] = m;
                            entry.modifier_count += 1;
                        }
                    }
                }
            }

            if entry.modifier_count == 0 {
                entry.modifiers[0] = 0; /* DRM_FORMAT_MOD_LINEAR */
                entry.modifier_count = 1;
            } else if (entry.modifier_count as usize) < MAX_MODIFIERS_PER_FMT
                && !entry.modifiers[..entry.modifier_count as usize]
                    .contains(&DRM_FORMAT_MOD_INVALID)
            {
                entry.modifiers[entry.modifier_count as usize] = DRM_FORMAT_MOD_INVALID;
                entry.modifier_count += 1;
            }

            out.push(entry);
        }
        out
    }

    pub fn clear_black_and_swap(&self) {
        unsafe {
            glViewport(0, 0, self.surf_w, self.surf_h);
            glClearColor(0.0, 0.0, 0.0, 1.0);
            glClear(GL_COLOR_BUFFER_BIT);
            let _ = eglSwapBuffers(self.dpy, self.surf);
        }
    }

    pub fn upload_shm(
        &mut self,
        width: u32,
        height: u32,
        pixels: &[u8],
        filter: FilterMode,
    ) -> Result<(), String> {
        let needed = (width as usize) * (height as usize) * 4;
        if width == 0 || height == 0 || pixels.len() < needed {
            return Err("Invalid SHM frame dimensions".to_string());
        }
        let gl_filter = match filter {
            FilterMode::Linear => GL_LINEAR,
            FilterMode::Nearest => GL_NEAREST,
        };
        unsafe {
            glActiveTexture(GL_TEXTURE0);
            glBindTexture(GL_TEXTURE_2D, self.shm_tex);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, gl_filter);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, gl_filter);
            if self.shm_tex_dims == (width, height) {
                glTexSubImage2D(
                    GL_TEXTURE_2D,
                    0,
                    0,
                    0,
                    width as i32,
                    height as i32,
                    GL_RGBA,
                    GL_UNSIGNED_BYTE,
                    pixels.as_ptr() as *const c_void,
                );
            } else {
                glTexImage2D(
                    GL_TEXTURE_2D,
                    0,
                    GL_RGBA as i32,
                    width as i32,
                    height as i32,
                    0,
                    GL_RGBA,
                    GL_UNSIGNED_BYTE,
                    pixels.as_ptr() as *const c_void,
                );
                self.shm_tex_dims = (width, height);
            }
        }
        Ok(())
    }

    pub fn import_dmabuf(
        &mut self,
        dmabuf: &DmaBufFrame,
        filter: FilterMode,
    ) -> Result<(), String> {
        let (Some(create_img), Some(img_tex)) = (self.create_image, self.image_target_tex) else {
            return Err("DMA-BUF EGL extensions unavailable".to_string());
        };
        if dmabuf.planes.is_empty() || dmabuf.width == 0 || dmabuf.height == 0 {
            return Err("Empty DMA-BUF plane list".to_string());
        }

        let plane_keys: [(EGLint, EGLint, EGLint, EGLint, EGLint); 4] = [
            (
                EGL_DMA_BUF_PLANE0_FD_EXT,
                EGL_DMA_BUF_PLANE0_OFFSET_EXT,
                EGL_DMA_BUF_PLANE0_PITCH_EXT,
                EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT,
                EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT,
            ),
            (
                EGL_DMA_BUF_PLANE1_FD_EXT,
                EGL_DMA_BUF_PLANE1_OFFSET_EXT,
                EGL_DMA_BUF_PLANE1_PITCH_EXT,
                EGL_DMA_BUF_PLANE1_MODIFIER_LO_EXT,
                EGL_DMA_BUF_PLANE1_MODIFIER_HI_EXT,
            ),
            (
                EGL_DMA_BUF_PLANE2_FD_EXT,
                EGL_DMA_BUF_PLANE2_OFFSET_EXT,
                EGL_DMA_BUF_PLANE2_PITCH_EXT,
                EGL_DMA_BUF_PLANE2_MODIFIER_LO_EXT,
                EGL_DMA_BUF_PLANE2_MODIFIER_HI_EXT,
            ),
            (
                EGL_DMA_BUF_PLANE3_FD_EXT,
                EGL_DMA_BUF_PLANE3_OFFSET_EXT,
                EGL_DMA_BUF_PLANE3_PITCH_EXT,
                EGL_DMA_BUF_PLANE3_MODIFIER_LO_EXT,
                EGL_DMA_BUF_PLANE3_MODIFIER_HI_EXT,
            ),
        ];

        let mut attribs = Vec::with_capacity(32);
        attribs.extend_from_slice(&[
            EGL_WIDTH,
            dmabuf.width as EGLint,
            EGL_HEIGHT,
            dmabuf.height as EGLint,
            EGL_LINUX_DRM_FOURCC_EXT,
            dmabuf.drm_format as EGLint,
        ]);

        let use_mod = self.has_modifiers_ext && dmabuf.modifier != DRM_FORMAT_MOD_INVALID;
        let mod_lo = (dmabuf.modifier & 0xffff_ffff) as EGLint;
        let mod_hi = (dmabuf.modifier >> 32) as EGLint;

        for (i, plane) in dmabuf.planes.iter().take(4).enumerate() {
            let (k_fd, k_off, k_pitch, k_mlo, k_mhi) = plane_keys[i];
            attribs.extend_from_slice(&[
                k_fd,
                plane.fd,
                k_off,
                plane.offset as EGLint,
                k_pitch,
                plane.stride as EGLint,
            ]);
            if use_mod {
                attribs.extend_from_slice(&[k_mlo, mod_lo, k_mhi, mod_hi]);
            }
        }
        attribs.push(EGL_NONE);

        unsafe {
            while glGetError() != GL_NO_ERROR {}
            let img = create_img(
                self.dpy,
                std::ptr::null_mut(),
                EGL_LINUX_DMA_BUF_EXT,
                std::ptr::null_mut(),
                attribs.as_ptr(),
            );
            if img.is_null() {
                return Err(format!(
                    "eglCreateImageKHR rejected DMA-BUF (0x{:x})",
                    eglGetError()
                ));
            }

            let gl_filter = match filter {
                FilterMode::Linear => GL_LINEAR,
                FilterMode::Nearest => GL_NEAREST,
            };
            glActiveTexture(GL_TEXTURE0);
            glBindTexture(GL_TEXTURE_EXTERNAL_OES, self.ext_tex);
            glTexParameteri(GL_TEXTURE_EXTERNAL_OES, GL_TEXTURE_MIN_FILTER, gl_filter);
            glTexParameteri(GL_TEXTURE_EXTERNAL_OES, GL_TEXTURE_MAG_FILTER, gl_filter);
            img_tex(GL_TEXTURE_EXTERNAL_OES, img);

            let gerr = glGetError();
            if gerr != GL_NO_ERROR {
                if let Some(destroy_img) = self.destroy_image {
                    let _ = destroy_img(self.dpy, img);
                }
                return Err(format!("glEGLImageTargetTexture2DOES failed (0x{gerr:x})"));
            }

            if !self.current_image.is_null() {
                if let Some(destroy_img) = self.destroy_image {
                    let _ = destroy_img(self.dpy, self.current_image);
                }
            }
            self.current_image = img;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_and_swap(
        &self,
        is_dmabuf: bool,
        src_w: u32,
        src_h: u32,
        crop_uv: (f32, f32, f32, f32),
        fit: FitMode,
        swizzle: i32,
        y_invert: bool,
    ) {
        let prog = if is_dmabuf {
            match self.ext_prog.as_ref() {
                Some(p) => p,
                None => return,
            }
        } else {
            &self.shm_prog
        };

        let (cu0, cv0, cu1, cv1) = crop_uv;
        let eff_src_w = ((src_w as f32) * (cu1 - cu0).abs()).max(1.0);
        let eff_src_h = ((src_h as f32) * (cv1 - cv0).abs()).max(1.0);
        let dst_w = self.surf_w.max(1) as f32;
        let dst_h = self.surf_h.max(1) as f32;

        let mut vp_x = 0i32;
        let mut vp_y = 0i32;
        let mut vp_w = self.surf_w;
        let mut vp_h = self.surf_h;
        let mut uv_rect = (cu0, cv0, cu1, cv1);

        match fit {
            FitMode::Stretch => {}
            FitMode::Contain => {
                let scale = (dst_w / eff_src_w).min(dst_h / eff_src_h);
                vp_w = ((eff_src_w * scale).round() as i32).clamp(1, self.surf_w);
                vp_h = ((eff_src_h * scale).round() as i32).clamp(1, self.surf_h);
                vp_x = (self.surf_w - vp_w) / 2;
                vp_y = (self.surf_h - vp_h) / 2;
            }
            FitMode::Cover => {
                let src_aspect = eff_src_w / eff_src_h;
                let dst_aspect = dst_w / dst_h;
                if src_aspect > dst_aspect {
                    let keep = dst_aspect / src_aspect;
                    let pad = (1.0 - keep) * 0.5 * (cu1 - cu0);
                    uv_rect.0 = cu0 + pad;
                    uv_rect.2 = cu1 - pad;
                } else if dst_aspect > src_aspect {
                    let keep = src_aspect / dst_aspect;
                    let pad = (1.0 - keep) * 0.5 * (cv1 - cv0);
                    uv_rect.1 = cv0 + pad;
                    uv_rect.3 = cv1 - pad;
                }
            }
        }

        unsafe {
            glViewport(0, 0, self.surf_w, self.surf_h);
            glClearColor(0.0, 0.0, 0.0, 1.0);
            glClear(GL_COLOR_BUFFER_BIT);

            glViewport(vp_x, vp_y, vp_w, vp_h);
            glUseProgram(prog.id);
            glActiveTexture(GL_TEXTURE0);
            if is_dmabuf {
                glBindTexture(GL_TEXTURE_EXTERNAL_OES, self.ext_tex);
            } else {
                glBindTexture(GL_TEXTURE_2D, self.shm_tex);
                if prog.u_swizzle >= 0 {
                    glUniform1i(prog.u_swizzle, swizzle);
                }
            }
            if prog.u_tex >= 0 {
                glUniform1i(prog.u_tex, 0);
            }
            if prog.u_uv_rect >= 0 {
                glUniform4f(prog.u_uv_rect, uv_rect.0, uv_rect.1, uv_rect.2, uv_rect.3);
            }
            if prog.u_y_invert >= 0 {
                glUniform1i(prog.u_y_invert, i32::from(y_invert));
            }

            let stride = (4 * std::mem::size_of::<f32>()) as GLsizei;
            let base = QUAD_VERTS.as_ptr() as *const u8;
            glEnableVertexAttribArray(0);
            glEnableVertexAttribArray(1);
            glVertexAttribPointer(0, 2, GL_FLOAT, 0, stride, base as *const c_void);
            glVertexAttribPointer(
                1,
                2,
                GL_FLOAT,
                0,
                stride,
                base.add(2 * std::mem::size_of::<f32>()) as *const c_void,
            );
            glDrawArrays(GL_TRIANGLES, 0, 6);

            let _ = eglSwapBuffers(self.dpy, self.surf);
        }
    }
}

impl Drop for EglRenderer {
    fn drop(&mut self) {
        unsafe {
            let _ = eglMakeCurrent(self.dpy, self.surf, self.surf, self.ctx);
            if !self.current_image.is_null() {
                if let Some(destroy_img) = self.destroy_image {
                    let _ = destroy_img(self.dpy, self.current_image);
                }
            }
            let tex_ids = [self.shm_tex, self.ext_tex];
            glDeleteTextures(2, tex_ids.as_ptr());
            glDeleteProgram(self.shm_prog.id);
            if let Some(ref ep) = self.ext_prog {
                glDeleteProgram(ep.id);
            }
            let _ = eglMakeCurrent(
                self.dpy,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            let _ = eglDestroySurface(self.dpy, self.surf);
            let _ = eglDestroyContext(self.dpy, self.ctx);
            let _ = eglTerminate(self.dpy);
        }
    }
}
