use serde::{Deserialize, Serialize};
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputInfo {
    pub name: String,
    pub description: String,
    pub x: i32,
    pub y: i32,
    pub logical_width: i32,
    pub logical_height: i32,
    pub physical_width: i32,
    pub physical_height: i32,
    pub scale: i32,
    pub refresh_mhz: i32,
    pub refresh_hz: u32,
    pub is_internal: bool,
}

impl OutputInfo {
    pub fn effective_width(&self) -> i32 {
        if self.logical_width > 0 {
            self.logical_width
        } else {
            (self.physical_width / self.scale.max(1)).max(1)
        }
    }

    pub fn effective_height(&self) -> i32 {
        if self.logical_height > 0 {
            self.logical_height
        } else {
            (self.physical_height / self.scale.max(1)).max(1)
        }
    }
}

struct RawOutput {
    wl: wl_output::WlOutput,
    xdg: Option<zxdg_output_v1::ZxdgOutputV1>,
    name: String,
    description: String,
    make_model: String,
    x: i32,
    y: i32,
    logical_w: i32,
    logical_h: i32,
    phys_w: i32,
    phys_h: i32,
    scale: i32,
    cur_mhz: i32,
    pref_mhz: i32,
}

impl RawOutput {
    fn new(wl: wl_output::WlOutput) -> Self {
        Self {
            wl,
            xdg: None,
            name: String::new(),
            description: String::new(),
            make_model: String::new(),
            x: 0,
            y: 0,
            logical_w: 0,
            logical_h: 0,
            phys_w: 0,
            phys_h: 0,
            scale: 1,
            cur_mhz: 0,
            pref_mhz: 0,
        }
    }
}

struct ProbeState {
    xdg_mgr: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    outputs: Vec<RawOutput>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for ProbeState {
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
            if interface == "wl_output" && version >= 2 {
                let ver = version.min(4);
                let idx = state.outputs.len();
                let wl = registry.bind::<wl_output::WlOutput, usize, Self>(name, ver, qh, idx);
                state.outputs.push(RawOutput::new(wl));
            } else if interface == "zxdg_output_manager_v1" {
                let ver = version.min(3);
                state.xdg_mgr = Some(
                    registry.bind::<zxdg_output_manager_v1::ZxdgOutputManagerV1, (), Self>(
                        name,
                        ver,
                        qh,
                        (),
                    ),
                );
            }
        }
    }
}

impl Dispatch<zxdg_output_manager_v1::ZxdgOutputManagerV1, ()> for ProbeState {
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

impl Dispatch<wl_output::WlOutput, usize> for ProbeState {
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
            wl_output::Event::Geometry {
                x, y, make, model, ..
            } => {
                if out.x == 0 && out.y == 0 {
                    out.x = x;
                    out.y = y;
                }
                let label = format!("{} {}", make.trim(), model.trim())
                    .trim()
                    .to_string();
                if !label.is_empty() {
                    out.make_model = label;
                }
            }
            wl_output::Event::Mode {
                flags: WEnum::Value(flags),
                width,
                height,
                refresh,
            } => {
                if flags.contains(wl_output::Mode::Current) {
                    out.phys_w = width;
                    out.phys_h = height;
                    if refresh > 0 {
                        out.cur_mhz = refresh;
                    }
                } else if flags.contains(wl_output::Mode::Preferred) && out.pref_mhz == 0 {
                    if out.phys_w == 0 {
                        out.phys_w = width;
                        out.phys_h = height;
                    }
                    if refresh > 0 {
                        out.pref_mhz = refresh;
                    }
                }
            }
            wl_output::Event::Scale { factor } => {
                out.scale = factor.max(1);
            }
            wl_output::Event::Name { name } => {
                out.name = name;
            }
            wl_output::Event::Description { description } => {
                out.description = description;
            }
            _ => {}
        }
    }
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, usize> for ProbeState {
    fn event(
        state: &mut Self,
        _: &zxdg_output_v1::ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(out) = state.outputs.get_mut(*idx) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => {
                out.x = x;
                out.y = y;
            }
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                out.logical_w = width;
                out.logical_h = height;
            }
            zxdg_output_v1::Event::Name { name } => {
                if out.name.is_empty() {
                    out.name = name;
                }
            }
            zxdg_output_v1::Event::Description { description } => {
                if out.description.is_empty() {
                    out.description = description;
                }
            }
            _ => {}
        }
    }
}

pub fn query_outputs() -> Result<Vec<OutputInfo>, String> {
    let conn = Connection::connect_to_env()
        .map_err(|e| format!("Failed to connect to Wayland display: {e}"))?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();

    let mut state = ProbeState {
        xdg_mgr: None,
        outputs: Vec::new(),
    };

    conn.display().get_registry(&qh, ());
    queue
        .roundtrip(&mut state)
        .map_err(|e| format!("Wayland registry roundtrip failed: {e}"))?;

    if let Some(ref mgr) = state.xdg_mgr {
        for (idx, out) in state.outputs.iter_mut().enumerate() {
            let xdg = mgr.get_xdg_output(&out.wl, &qh, idx);
            out.xdg = Some(xdg);
        }
    }

    queue
        .roundtrip(&mut state)
        .map_err(|e| format!("Wayland output roundtrip failed: {e}"))?;

    let mut results = Vec::with_capacity(state.outputs.len());
    for (idx, out) in state.outputs.into_iter().enumerate() {
        if let Some(xdg) = out.xdg {
            xdg.destroy();
        }
        if out.wl.proxy_version() >= 3 {
            out.wl.release();
        }

        let name = if out.name.is_empty() {
            format!("WL-{idx}")
        } else {
            out.name
        };
        let desc = if !out.description.is_empty() {
            out.description
        } else {
            out.make_model
        };
        let mhz = if out.cur_mhz > 0 {
            out.cur_mhz
        } else {
            out.pref_mhz
        };
        let hz = if mhz >= 1000 {
            ((mhz as f64 / 1000.0).round() as u32).clamp(24, 500)
        } else {
            60
        };
        let logical_w = if out.logical_w > 0 {
            out.logical_w
        } else {
            (out.phys_w / out.scale.max(1)).max(0)
        };
        let logical_h = if out.logical_h > 0 {
            out.logical_h
        } else {
            (out.phys_h / out.scale.max(1)).max(0)
        };
        let phys_w = if out.phys_w > 0 {
            out.phys_w
        } else {
            logical_w * out.scale.max(1)
        };
        let phys_h = if out.phys_h > 0 {
            out.phys_h
        } else {
            logical_h * out.scale.max(1)
        };
        let is_internal =
            name.starts_with("eDP-") || name.starts_with("LVDS-") || name.starts_with("DSI-");

        results.push(OutputInfo {
            name,
            description: desc,
            x: out.x,
            y: out.y,
            logical_width: logical_w,
            logical_height: logical_h,
            physical_width: phys_w,
            physical_height: phys_h,
            scale: out.scale,
            refresh_mhz: mhz,
            refresh_hz: hz,
            is_internal,
        });
    }

    if let Some(mgr) = state.xdg_mgr {
        mgr.destroy();
    }
    let _ = queue.roundtrip(&mut ProbeState {
        xdg_mgr: None,
        outputs: Vec::new(),
    });

    results.sort_by(|a, b| {
        a.is_internal
            .cmp(&b.is_internal)
            .then_with(|| a.name.cmp(&b.name))
    });

    Ok(results)
}

trait ProxyVersionExt {
    fn proxy_version(&self) -> u32;
}

impl ProxyVersionExt for wl_output::WlOutput {
    fn proxy_version(&self) -> u32 {
        wayland_client::Proxy::version(self)
    }
}

pub fn select_destination<'a>(
    outputs: &'a [OutputInfo],
    requested: Option<&str>,
) -> Result<&'a OutputInfo, String> {
    if outputs.is_empty() {
        return Err("No Wayland outputs detected".to_string());
    }
    if let Some(req) = requested {
        let trimmed = req.trim();
        if !trimmed.is_empty() {
            return outputs.iter().find(|o| o.name == trimmed).ok_or_else(|| {
                let known: Vec<&str> = outputs.iter().map(|o| o.name.as_str()).collect();
                format!(
                    "Destination output '{trimmed}' not found (connected: {})",
                    known.join(", ")
                )
            });
        }
    }
    if outputs.len() < 2 {
        return Err(format!(
            "Only one display ('{}') is connected; connect a second display or pass --to {}",
            outputs[0].name, outputs[0].name
        ));
    }
    Ok(&outputs[0])
}
