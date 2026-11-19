use std::ffi::c_void;
use std::os::fd::{AsFd, AsRawFd, RawFd};
use wayland_client::protocol::{
    wl_callback, wl_compositor, wl_output, wl_region, wl_registry, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_egl::WlEglSurface;
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, Layer},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity},
};

struct BoundOutput {
    global_name: u32,
    wl: wl_output::WlOutput,
    name: String,
    scale: i32,
    phys_w: i32,
    phys_h: i32,
}

struct DisplayState {
    compositor: Option<wl_compositor::WlCompositor>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    xdg_wm_base: Option<xdg_wm_base::XdgWmBase>,
    outputs: Vec<BoundOutput>,

    target_name: String,
    target_global_name: Option<u32>,
    target_scale: i32,

    logical_w: i32,
    logical_h: i32,
    configured: bool,
    closed: bool,
    resized: bool,
    frame_ready: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for DisplayState {
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
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(5), qh, ()));
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "xdg_wm_base" => {
                    state.xdg_wm_base = Some(registry.bind(name, version.min(5), qh, ()));
                }
                "wl_output" if version >= 2 => {
                    let idx = state.outputs.len();
                    let wl = registry.bind(name, version.min(4), qh, idx);
                    state.outputs.push(BoundOutput {
                        global_name: name,
                        wl,
                        name: String::new(),
                        scale: 1,
                        phys_w: 1920,
                        phys_h: 1080,
                    });
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                if state.target_global_name == Some(name) {
                    state.closed = true;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for DisplayState {
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

impl Dispatch<wl_surface::WlSurface, ()> for DisplayState {
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

impl Dispatch<wl_region::WlRegion, ()> for DisplayState {
    fn event(
        _: &mut Self,
        _: &wl_region::WlRegion,
        _: wl_region::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for DisplayState {
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

impl Dispatch<wl_output::WlOutput, usize> for DisplayState {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(out) = state.outputs.get_mut(*idx) else {
            return;
        };
        match event {
            wl_output::Event::Name { name } => out.name = name,
            wl_output::Event::Scale { factor } => out.scale = factor.max(1),
            wl_output::Event::Mode {
                flags: wayland_client::WEnum::Value(flags),
                width,
                height,
                ..
            } => {
                if flags.contains(wl_output::Mode::Current) {
                    out.phys_w = width.max(1);
                    out.phys_h = height.max(1);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for DisplayState {
    fn event(
        state: &mut Self,
        layer_surf: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer_surf.ack_configure(serial);
                if width > 0 && height > 0 {
                    let nw = width as i32;
                    let nh = height as i32;
                    if (nw, nh) != (state.logical_w, state.logical_h) {
                        state.logical_w = nw;
                        state.logical_h = nh;
                        state.resized = true;
                    }
                }
                state.configured = true;
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.closed = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for DisplayState {
    fn event(
        _: &mut Self,
        wm_base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for DisplayState {
    fn event(
        state: &mut Self,
        xdg_surf: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surf.ack_configure(serial);
            state.configured = true;
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for DisplayState {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => {
                if width > 0 && height > 0 && (width, height) != (state.logical_w, state.logical_h)
                {
                    state.logical_w = width;
                    state.logical_h = height;
                    state.resized = true;
                }
            }
            xdg_toplevel::Event::Close => {
                state.closed = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for DisplayState {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.frame_ready = true;
        }
    }
}

enum SurfaceRole {
    Layer(zwlr_layer_surface_v1::ZwlrLayerSurfaceV1),
    Xdg {
        xdg_surface: xdg_surface::XdgSurface,
        toplevel: xdg_toplevel::XdgToplevel,
    },
}

pub struct PresentationWindow {
    conn: Connection,
    queue: EventQueue<DisplayState>,
    qh: QueueHandle<DisplayState>,
    state: DisplayState,
    surface: wl_surface::WlSurface,
    _role: SurfaceRole,
    pub surface_kind: &'static str,
}

impl PresentationWindow {
    pub fn create(target_connector: &str) -> Result<(Self, WlEglSurface, *mut c_void), String> {
        let conn = Connection::connect_to_env()
            .map_err(|e| format!("Failed connecting to Wayland display: {e}"))?;
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();

        let mut state = DisplayState {
            compositor: None,
            layer_shell: None,
            xdg_wm_base: None,
            outputs: Vec::new(),
            target_name: target_connector.to_string(),
            target_global_name: None,
            target_scale: 1,
            logical_w: 1920,
            logical_h: 1080,
            configured: false,
            closed: false,
            resized: false,
            frame_ready: true,
        };

        conn.display().get_registry(&qh, ());
        queue
            .roundtrip(&mut state)
            .map_err(|e| format!("Wayland registry roundtrip failed: {e}"))?;
        queue
            .roundtrip(&mut state)
            .map_err(|e| format!("Wayland output metadata roundtrip failed: {e}"))?;

        let compositor = state
            .compositor
            .clone()
            .ok_or_else(|| "Compositor missing wl_compositor".to_string())?;

        let target = state
            .outputs
            .iter()
            .find(|o| o.name == state.target_name)
            .ok_or_else(|| format!("Destination output '{}' is not present", state.target_name))?;

        let target_wl = target.wl.clone();
        state.target_global_name = Some(target.global_name);
        state.target_scale = target.scale.max(1);
        state.logical_w = (target.phys_w / state.target_scale).max(1);
        state.logical_h = (target.phys_h / state.target_scale).max(1);

        let surface = compositor.create_surface(&qh, ());
        if Proxy::version(&surface) >= 3 {
            surface.set_buffer_scale(state.target_scale);
        }

        /* Empty input region -> clicks and scroll pass straight through on layer-shell & xdg-shell */
        let empty_region = compositor.create_region(&qh, ());
        surface.set_input_region(Some(&empty_region));
        empty_region.destroy();

        let (role, surface_kind) = if let Some(ref ls) = state.layer_shell {
            let layer_surf = ls.get_layer_surface(
                &surface,
                Some(&target_wl),
                Layer::Overlay,
                "portal-screen-mirror".to_string(),
                &qh,
                (),
            );
            layer_surf.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
            layer_surf.set_size(0, 0);
            layer_surf.set_exclusive_zone(-1);
            layer_surf.set_keyboard_interactivity(KeyboardInteractivity::None);
            (SurfaceRole::Layer(layer_surf), "layer-shell")
        } else if let Some(ref wm) = state.xdg_wm_base {
            let xdg_surface = wm.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg_surface.get_toplevel(&qh, ());
            toplevel.set_title("Portal Screen Mirror".to_string());
            toplevel.set_app_id("portal-screen-mirror".to_string());
            toplevel.set_fullscreen(Some(&target_wl));
            (
                SurfaceRole::Xdg {
                    xdg_surface,
                    toplevel,
                },
                "xdg-fullscreen",
            )
        } else {
            return Err(
                "Compositor implements neither zwlr_layer_shell_v1 nor xdg_wm_base".to_string(),
            );
        };

        /* Hidden until first captured frame arrives; wait for initial Configure only on map */
        let phys_w = (state.logical_w * state.target_scale).max(1);
        let phys_h = (state.logical_h * state.target_scale).max(1);
        let wl_egl = WlEglSurface::new(surface.id(), phys_w, phys_h)
            .map_err(|e| format!("Failed creating WlEglSurface: {e}"))?;
        let display_ptr = conn.backend().display_ptr() as *mut c_void;

        Ok((
            Self {
                conn,
                queue,
                qh,
                state,
                surface,
                _role: role,
                surface_kind,
            },
            wl_egl,
            display_ptr,
        ))
    }

    pub fn map_and_wait_configure(&mut self) -> Result<(i32, i32), String> {
        self.surface.commit();
        let _ = self.conn.flush();
        while !self.state.configured && !self.state.closed {
            self.queue
                .blocking_dispatch(&mut self.state)
                .map_err(|e| format!("Wayland configure dispatch failed: {e}"))?;
        }
        if self.state.closed {
            return Err("Presentation surface closed during initialization".to_string());
        }
        self.state.resized = false;
        Ok(self.physical_size())
    }

    pub fn physical_size(&self) -> (i32, i32) {
        (
            (self.state.logical_w * self.state.target_scale).max(1),
            (self.state.logical_h * self.state.target_scale).max(1),
        )
    }

    pub fn take_resize(&mut self) -> Option<(i32, i32)> {
        if self.state.resized {
            self.state.resized = false;
            Some(self.physical_size())
        } else {
            None
        }
    }

    pub fn is_closed(&self) -> bool {
        self.state.closed
    }

    pub fn frame_ready(&self) -> bool {
        self.state.frame_ready
    }

    pub fn request_frame_callback(&mut self) {
        self.state.frame_ready = false;
        self.surface.frame(&self.qh, ());
    }

    pub fn wayland_fd(&self) -> RawFd {
        self.conn.as_fd().as_raw_fd()
    }

    pub fn dispatch_nonblocking(&mut self) -> Result<(), String> {
        let _ = self.conn.flush();
        if let Some(guard) = self.conn.prepare_read() {
            let _ = guard.read();
        }
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(|e| format!("Wayland dispatch failed: {e}"))?;
        let _ = self.conn.flush();
        Ok(())
    }
}

impl Drop for PresentationWindow {
    fn drop(&mut self) {
        match &self._role {
            SurfaceRole::Layer(ls) => ls.destroy(),
            SurfaceRole::Xdg {
                xdg_surface,
                toplevel,
            } => {
                toplevel.destroy();
                xdg_surface.destroy();
            }
        }
        self.surface.destroy();
        let _ = self.conn.flush();
    }
}
