use crate::ipc::{self, FilterMode, FitMode, IpcRequest, MirrorStatus};
use crate::outputs::{query_outputs, OutputInfo};
use eframe::egui::{
    self, Align, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Sense, Stroke,
    StrokeKind, Vec2,
};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const BG_DARK: Color32 = Color32::from_rgb(20, 21, 31);
const CARD_BG: Color32 = Color32::from_rgb(30, 33, 48);
const CARD_ACTIVE_BG: Color32 = Color32::from_rgb(31, 42, 64);
const CARD_BORDER: Color32 = Color32::from_rgb(49, 54, 77);
const TEXT_MAIN: Color32 = Color32::from_rgb(205, 214, 244);
const TEXT_MUTED: Color32 = Color32::from_rgb(147, 153, 178);
const ACCENT_BLUE: Color32 = Color32::from_rgb(137, 180, 250);
const ACCENT_LAVENDER: Color32 = Color32::from_rgb(203, 166, 247);
const ACCENT_RED: Color32 = Color32::from_rgb(243, 139, 168);
const ACCENT_GREEN: Color32 = Color32::from_rgb(166, 227, 161);
const DARK_INK: Color32 = Color32::from_rgb(17, 17, 27);

const FPS_PRESETS: &[(&str, &str)] = &[
    ("auto", "Auto (Match display)"),
    ("30", "30 FPS"),
    ("60", "60 FPS"),
    ("120", "120 FPS"),
    ("144", "144 FPS"),
    ("240", "240 FPS"),
    ("0", "Uncapped"),
];

pub struct GuiInitArgs {
    pub to: Option<String>,
    pub fit: FitMode,
    pub filter: FilterMode,
    pub max_fps: String,
    pub no_cursor: bool,
    pub shm: bool,
}

pub fn run_gui(init: GuiInitArgs) -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Portal Screen Mirror")
            .with_app_id("portal-screen-mirror")
            .with_inner_size([410.0, 565.0])
            .with_min_inner_size([365.0, 490.0])
            .with_icon(build_window_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "Portal Screen Mirror",
        options,
        Box::new(|cc| Ok(Box::new(MirrorGuiApp::new(cc, init)))),
    )
    .map_err(|e| format!("GUI error: {e}"))
}

fn build_window_icon() -> Arc<egui::IconData> {
    let size = 64u32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            let cx = (x as f32 - 31.5).abs();
            let cy = (y as f32 - 31.5).abs();
            if cx > 29.0 || cy > 29.0 {
                continue;
            }
            rgba[idx] = 28;
            rgba[idx + 1] = 31;
            rgba[idx + 2] = 46;
            rgba[idx + 3] = 255;

            let in_back = (11..=39).contains(&x)
                && (14..=36).contains(&y)
                && (x <= 13 || x >= 37 || y <= 16 || y >= 34);
            let in_front = (24..=52).contains(&x)
                && (26..=48).contains(&y)
                && (x <= 26 || x >= 50 || y <= 28 || y >= 46);
            if in_front {
                rgba[idx] = 203;
                rgba[idx + 1] = 166;
                rgba[idx + 2] = 247;
            } else if in_back {
                rgba[idx] = 137;
                rgba[idx + 1] = 180;
                rgba[idx + 2] = 250;
            }
        }
    }
    Arc::new(egui::IconData {
        rgba,
        width: size,
        height: size,
    })
}

fn configure_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG_DARK;
    visuals.window_fill = BG_DARK;
    visuals.override_text_color = Some(TEXT_MAIN);
    visuals.widgets.noninteractive.bg_fill = CARD_BG;
    visuals.widgets.noninteractive.weak_bg_fill = CARD_BG;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, CARD_BORDER);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(10);

    visuals.widgets.inactive.bg_fill = CARD_BG;
    visuals.widgets.inactive.weak_bg_fill = CARD_BG;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, CARD_BORDER);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(10);

    visuals.widgets.hovered.bg_fill = Color32::from_rgb(40, 44, 64);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(40, 44, 64);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT_BLUE);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(10);

    visuals.widgets.active.bg_fill = Color32::from_rgb(48, 54, 78);
    visuals.widgets.active.weak_bg_fill = Color32::from_rgb(48, 54, 78);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT_BLUE);
    visuals.widgets.active.corner_radius = CornerRadius::same(10);

    visuals.widgets.open.bg_fill = Color32::from_rgb(38, 42, 62);
    visuals.widgets.open.weak_bg_fill = Color32::from_rgb(38, 42, 62);
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, ACCENT_BLUE);
    visuals.widgets.open.corner_radius = CornerRadius::same(10);

    visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(137, 180, 250, 55);
    visuals.selection.stroke = Stroke::new(1.0, ACCENT_BLUE);

    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.visuals = visuals.clone();
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(12.0, 8.0);
        style.spacing.combo_height = 240.0;
    });
}

struct MirrorGuiApp {
    outputs: Vec<OutputInfo>,
    selected_output: String,
    fit: FitMode,
    filter: FilterMode,
    max_fps: String,
    show_cursor: bool,
    force_shm: bool,

    status: Option<MirrorStatus>,
    synced_pid: Option<u32>,
    pending_child: Option<Child>,
    last_error: Option<String>,
    last_poll: Instant,
}

impl MirrorGuiApp {
    fn new(cc: &eframe::CreationContext<'_>, init: GuiInitArgs) -> Self {
        configure_visuals(&cc.egui_ctx);
        let mut app = Self {
            outputs: Vec::new(),
            selected_output: init.to.unwrap_or_default(),
            fit: init.fit,
            filter: init.filter,
            max_fps: normalize_fps_preset(&init.max_fps),
            show_cursor: !init.no_cursor,
            force_shm: init.shm,
            status: None,
            synced_pid: None,
            pending_child: None,
            last_error: None,
            last_poll: Instant::now() - Duration::from_secs(2),
        };
        app.refresh_outputs();
        app.poll_daemon();
        app
    }

    fn refresh_outputs(&mut self) {
        match query_outputs() {
            Ok(list) => {
                self.outputs = list;
                let valid = self.outputs.iter().any(|o| o.name == self.selected_output);
                if !valid {
                    /* `query_outputs` sorts external outputs before internal panels */
                    self.selected_output = self
                        .outputs
                        .first()
                        .map(|o| o.name.clone())
                        .unwrap_or_default();
                }
            }
            Err(e) => {
                self.last_error = Some(e);
            }
        }
    }

    fn selected_output_info(&self) -> Option<&OutputInfo> {
        self.outputs.iter().find(|o| o.name == self.selected_output)
    }

    fn poll_daemon(&mut self) {
        self.last_poll = Instant::now();

        if let Some(ref mut child) = self.pending_child {
            match child.try_wait() {
                Ok(Some(exit)) => {
                    let mut err_txt = String::new();
                    if let Some(mut stderr) = child.stderr.take() {
                        let _ = stderr.read_to_string(&mut err_txt);
                    }
                    self.pending_child = None;
                    if !exit.success() {
                        let msg = err_txt
                            .lines()
                            .rev()
                            .find(|l| !l.trim().is_empty())
                            .unwrap_or("Mirror process exited with an error")
                            .trim_start_matches("portal-screen-mirror: error: ")
                            .to_string();
                        self.last_error = Some(msg);
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    self.pending_child = None;
                }
            }
        }

        match ipc::send_request(&IpcRequest::Status) {
            Ok(resp) if resp.ok => {
                if let Some(st) = resp.status {
                    if self.synced_pid != Some(st.pid) {
                        self.synced_pid = Some(st.pid);
                        self.selected_output = st.destination.clone();
                        self.fit = st.fit;
                        self.filter = st.filter;
                        self.max_fps = normalize_fps_preset(&st.max_fps_spec);
                    }
                    if st.backend != "starting" {
                        self.pending_child = None;
                    }
                    self.status = Some(st);
                }
            }
            _ => {
                self.status = None;
                self.synced_pid = None;
            }
        }
    }

    fn push_live_settings(&mut self) {
        if self.status.is_some() {
            match ipc::send_request(&IpcRequest::Set {
                fit: Some(self.fit),
                filter: Some(self.filter),
                max_fps: Some(self.max_fps.clone()),
            }) {
                Ok(resp) if resp.ok => {
                    if let Some(st) = resp.status {
                        self.status = Some(st);
                    }
                }
                Ok(resp) => self.last_error = Some(resp.message),
                Err(_) => {}
            }
        }
    }

    fn stop_active(&mut self) {
        let _ = ipc::send_request(&IpcRequest::Stop);
        if let Some(mut child) = self.pending_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        for _ in 0..12 {
            if !ipc::socket_path().exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        self.status = None;
        self.synced_pid = None;
    }

    fn launch_mode(&mut self, subcmd: &str) {
        self.last_error = None;
        if self.selected_output.is_empty() {
            self.last_error = Some("No destination output available".to_string());
            return;
        }

        self.stop_active();

        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                self.last_error = Some(format!("Cannot locate executable: {e}"));
                return;
            }
        };

        let fit_str = match self.fit {
            FitMode::Contain => "contain",
            FitMode::Cover => "cover",
            FitMode::Stretch => "stretch",
        };
        let filter_str = match self.filter {
            FilterMode::Linear => "linear",
            FilterMode::Nearest => "nearest",
        };

        let mut cmd = Command::new(exe);
        cmd.arg(subcmd)
            .arg("--to")
            .arg(&self.selected_output)
            .arg("--fit")
            .arg(fit_str)
            .arg("--filter")
            .arg(filter_str)
            .arg("--max-fps")
            .arg(&self.max_fps)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        if !self.show_cursor {
            cmd.arg("--no-cursor");
        }
        if self.force_shm {
            cmd.arg("--shm");
        }

        /* Detach into its own session so closing the GUI window leaves the mirror alive */
        unsafe {
            cmd.pre_exec(|| {
                let _ = libc::setsid();
                Ok(())
            });
        }

        match cmd.spawn() {
            Ok(child) => {
                self.pending_child = Some(child);
            }
            Err(e) => {
                self.last_error = Some(format!("Failed spawning mirror daemon: {e}"));
            }
        }
    }
}

fn normalize_fps_preset(spec: &str) -> String {
    let s = spec.trim().to_ascii_lowercase();
    if s.is_empty() || s == "auto" {
        "auto".to_string()
    } else if matches!(s.as_str(), "0" | "uncapped" | "max" | "unlimited") {
        "0".to_string()
    } else {
        s
    }
}

fn format_output_label(o: &OutputInfo) -> String {
    let tag = if o.is_internal { " [internal]" } else { "" };
    if o.description.is_empty() {
        format!(
            "{} ({}×{} @ {} Hz){}",
            o.name, o.physical_width, o.physical_height, o.refresh_hz, tag
        )
    } else {
        let short_desc: String = o.description.chars().take(28).collect();
        let ellipsis = if o.description.chars().count() > 28 {
            "…"
        } else {
            ""
        };
        format!(
            "{} — {}{} ({} Hz){}",
            o.name, short_desc, ellipsis, o.refresh_hz, tag
        )
    }
}

#[derive(Clone, Copy)]
enum ButtonIcon {
    Stop,
    Cancel,
    Portal,
    Region,
}

fn draw_button_icon(painter: &egui::Painter, center: egui::Pos2, icon: ButtonIcon, fg: Color32) {
    let stroke = Stroke::new(1.7, fg);
    match icon {
        ButtonIcon::Stop => {
            let r = egui::Rect::from_center_size(center, Vec2::new(10.0, 10.0));
            painter.rect_stroke(r, CornerRadius::same(2), stroke, StrokeKind::Inside);
        }
        ButtonIcon::Cancel => {
            let d = 4.5;
            painter.line_segment(
                [center + Vec2::new(-d, -d), center + Vec2::new(d, d)],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(-d, d), center + Vec2::new(d, -d)],
                stroke,
            );
        }
        ButtonIcon::Portal => {
            let r =
                egui::Rect::from_center_size(center - Vec2::new(0.0, 1.5), Vec2::new(14.0, 9.5));
            painter.rect_stroke(r, CornerRadius::same(2), stroke, StrokeKind::Inside);
            painter.line_segment(
                [center + Vec2::new(-4.0, 5.5), center + Vec2::new(4.0, 5.5)],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(0.0, 3.2), center + Vec2::new(0.0, 5.5)],
                stroke,
            );
        }
        ButtonIcon::Region => {
            let r = egui::Rect::from_center_size(center, Vec2::new(10.0, 10.0));
            painter.line_segment(
                [
                    egui::pos2(r.min.x - 2.5, r.min.y),
                    egui::pos2(r.max.x, r.min.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(r.min.x, r.min.y - 2.5),
                    egui::pos2(r.min.x, r.max.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(r.min.x, r.max.y),
                    egui::pos2(r.max.x + 2.5, r.max.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(r.max.x, r.min.y),
                    egui::pos2(r.max.x, r.max.y + 2.5),
                ],
                stroke,
            );
        }
    }
}

fn solid_action_button(
    ui: &mut egui::Ui,
    icon: ButtonIcon,
    label: &str,
    fill: Color32,
    fg: Color32,
    height: f32,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    if ui.is_rect_visible(rect) {
        let bg = if resp.is_pointer_button_down_on() {
            fill.gamma_multiply(0.82)
        } else if resp.hovered() {
            fill.gamma_multiply(1.08)
        } else {
            fill
        };
        ui.painter().rect_filled(rect, CornerRadius::same(10), bg);

        let font = FontId::new(14.0, FontFamily::Proportional);
        let galley = ui.painter().layout_no_wrap(label.to_string(), font, fg);
        let icon_gap = 22.0;
        let total_w = icon_gap + galley.size().x;
        let start_x = rect.center().x - total_w * 0.5;
        draw_button_icon(
            ui.painter(),
            egui::pos2(start_x + 7.0, rect.center().y),
            icon,
            fg,
        );
        ui.painter().galley(
            egui::pos2(start_x + icon_gap, rect.center().y - galley.size().y * 0.5),
            galley,
            fg,
        );
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

impl eframe::App for MirrorGuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.last_poll.elapsed() >= Duration::from_millis(350) {
            self.poll_daemon();
        }
        ctx.request_repaint_after(Duration::from_millis(400));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(BG_DARK)
                    .inner_margin(Margin::same(16)),
            )
            .show(ui, |ui| {
                /* Header */
                ui.horizontal(|ui| {
                    let (icon_rect, _) =
                        ui.allocate_exact_size(Vec2::new(22.0, 22.0), Sense::hover());
                    let p = ui.painter();
                    let back = egui::Rect::from_min_size(
                        icon_rect.min + Vec2::new(1.0, 2.0),
                        Vec2::new(13.0, 10.0),
                    );
                    let front = egui::Rect::from_min_size(
                        icon_rect.min + Vec2::new(7.0, 8.0),
                        Vec2::new(13.0, 10.0),
                    );
                    p.rect_stroke(
                        back,
                        CornerRadius::same(2),
                        Stroke::new(1.8, ACCENT_BLUE),
                        StrokeKind::Inside,
                    );
                    p.rect_filled(front, CornerRadius::same(2), BG_DARK);
                    p.rect_stroke(
                        front,
                        CornerRadius::same(2),
                        Stroke::new(1.8, ACCENT_LAVENDER),
                        StrokeKind::Inside,
                    );

                    ui.label(
                        RichText::new("Portal Screen Mirror")
                            .size(17.0)
                            .strong()
                            .color(TEXT_MAIN),
                    );

                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        let refresh_btn = ui
                            .button(RichText::new("↻").size(15.0).color(TEXT_MUTED))
                            .on_hover_text("Refresh connected Wayland displays");
                        if refresh_btn.clicked() {
                            self.refresh_outputs();
                        }
                    });
                });

                ui.add_space(6.0);

                /* Status Banner + Stop Button */
                let is_waiting = self.pending_child.is_some()
                    && self.status.as_ref().is_none_or(|s| s.backend == "starting");

                if let Some(st) = self.status.clone().filter(|_| !is_waiting) {
                    egui::Frame::NONE
                        .fill(CARD_ACTIVE_BG)
                        .stroke(Stroke::new(1.0, ACCENT_BLUE.gamma_multiply(0.55)))
                        .corner_radius(CornerRadius::same(12))
                        .inner_margin(Margin::symmetric(14, 11))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let (dot, _) =
                                    ui.allocate_exact_size(Vec2::new(9.0, 9.0), Sense::hover());
                                ui.painter().circle_filled(dot.center(), 4.5, ACCENT_GREEN);
                                ui.label(
                                    RichText::new(format!("Mirroring to {}", st.destination))
                                        .size(14.5)
                                        .strong()
                                        .color(TEXT_MAIN),
                                );
                            });
                            let fps_badge = if st.effective_fps_cap == 0 {
                                "uncapped".to_string()
                            } else {
                                format!("{} Hz", st.effective_fps_cap)
                            };
                            let res_badge = if st.source_width > 0 && st.source_height > 0 {
                                format!("{}×{}", st.source_width, st.source_height)
                            } else {
                                "connecting".to_string()
                            };
                            ui.label(
                                RichText::new(format!(
                                    "{} · {} · {} · {}",
                                    st.mode, st.backend, res_badge, fps_badge
                                ))
                                .size(12.0)
                                .color(TEXT_MUTED),
                            );
                        });

                    ui.add_space(4.0);
                    if solid_action_button(
                        ui,
                        ButtonIcon::Stop,
                        "Stop Mirroring",
                        ACCENT_RED,
                        DARK_INK,
                        40.0,
                    )
                    .clicked()
                    {
                        self.stop_active();
                    }
                } else if is_waiting {
                    egui::Frame::NONE
                        .fill(CARD_ACTIVE_BG)
                        .stroke(Stroke::new(1.0, ACCENT_LAVENDER.gamma_multiply(0.55)))
                        .corner_radius(CornerRadius::same(12))
                        .inner_margin(Margin::symmetric(14, 11))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new("Waiting for source selection…")
                                    .size(14.0)
                                    .strong()
                                    .color(TEXT_MAIN),
                            );
                            ui.label(
                                RichText::new(
                                    "Pick a screen/window in the Portal dialog or drag a region",
                                )
                                .size(12.0)
                                .color(TEXT_MUTED),
                            );
                        });

                    ui.add_space(4.0);
                    if solid_action_button(
                        ui,
                        ButtonIcon::Cancel,
                        "Cancel",
                        ACCENT_RED,
                        DARK_INK,
                        38.0,
                    )
                    .clicked()
                    {
                        self.stop_active();
                    }
                } else {
                    egui::Frame::NONE
                        .fill(CARD_BG)
                        .stroke(Stroke::new(1.0, CARD_BORDER))
                        .corner_radius(CornerRadius::same(12))
                        .inner_margin(Margin::symmetric(14, 11))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            let subtitle = if self.outputs.len() < 2 {
                                "1 display detected — connect HDMI/DP/USB-C or test locally"
                            } else {
                                "Ready — select a destination display and capture source"
                            };
                            ui.label(RichText::new("Idle").size(14.0).strong().color(TEXT_MAIN));
                            ui.label(RichText::new(subtitle).size(12.0).color(TEXT_MUTED));
                        });
                }

                if let Some(err) = self.last_error.clone() {
                    ui.add_space(2.0);
                    egui::Frame::NONE
                        .fill(Color32::from_rgb(52, 26, 36))
                        .stroke(Stroke::new(1.0, ACCENT_RED.gamma_multiply(0.6)))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new(err).size(12.0).color(ACCENT_RED));
                        });
                }

                ui.add_space(8.0);

                /* 1. Destination screen */
                ui.label(
                    RichText::new("1. Destination screen")
                        .size(13.0)
                        .strong()
                        .color(TEXT_MUTED),
                );
                let current_out_label = self
                    .selected_output_info()
                    .map(format_output_label)
                    .unwrap_or_else(|| {
                        if self.selected_output.is_empty() {
                            "No outputs detected".to_string()
                        } else {
                            self.selected_output.clone()
                        }
                    });

                egui::ComboBox::from_id_salt("dest_output_combo")
                    .width(ui.available_width())
                    .selected_text(RichText::new(current_out_label).size(13.5))
                    .show_ui(ui, |ui| {
                        for out in &self.outputs {
                            let label = format_output_label(out);
                            ui.selectable_value(&mut self.selected_output, out.name.clone(), label);
                        }
                    });

                ui.add_space(6.0);

                /* 2. Frame rate */
                ui.label(
                    RichText::new("2. Frame rate")
                        .size(13.0)
                        .strong()
                        .color(TEXT_MUTED),
                );
                let dest_hz = self
                    .selected_output_info()
                    .map(|o| o.refresh_hz)
                    .unwrap_or(60);
                let fps_display = match self.max_fps.as_str() {
                    "auto" => format!("Auto (Match display — {dest_hz} Hz)"),
                    "0" => "Uncapped".to_string(),
                    other => format!("{other} FPS"),
                };
                let prev_fps = self.max_fps.clone();
                egui::ComboBox::from_id_salt("fps_combo")
                    .width(ui.available_width())
                    .selected_text(RichText::new(fps_display).size(13.5))
                    .show_ui(ui, |ui| {
                        for &(val, base_label) in FPS_PRESETS {
                            let lbl = if val == "auto" {
                                format!("{base_label} ({dest_hz} Hz)")
                            } else {
                                base_label.to_string()
                            };
                            ui.selectable_value(&mut self.max_fps, val.to_string(), lbl);
                        }
                    });
                if self.max_fps != prev_fps {
                    self.push_live_settings();
                }

                ui.add_space(6.0);

                /* 3. Scaling & Filter */
                ui.label(
                    RichText::new("3. Scaling & filter")
                        .size(13.0)
                        .strong()
                        .color(TEXT_MUTED),
                );
                let prev_fit = self.fit;
                let prev_filter = self.filter;
                ui.columns(2, |cols| {
                    let fit_lbl = match self.fit {
                        FitMode::Contain => "Fit: Contain",
                        FitMode::Cover => "Fit: Cover",
                        FitMode::Stretch => "Fit: Stretch",
                    };
                    egui::ComboBox::from_id_salt("fit_combo")
                        .width(cols[0].available_width())
                        .selected_text(RichText::new(fit_lbl).size(13.0))
                        .show_ui(&mut cols[0], |ui| {
                            ui.selectable_value(
                                &mut self.fit,
                                FitMode::Contain,
                                "Contain (Letterbox)",
                            );
                            ui.selectable_value(&mut self.fit, FitMode::Cover, "Cover (Fill)");
                            ui.selectable_value(&mut self.fit, FitMode::Stretch, "Stretch");
                        });

                    let filt_lbl = match self.filter {
                        FilterMode::Linear => "Filter: Linear",
                        FilterMode::Nearest => "Filter: Nearest",
                    };
                    egui::ComboBox::from_id_salt("filter_combo")
                        .width(cols[1].available_width())
                        .selected_text(RichText::new(filt_lbl).size(13.0))
                        .show_ui(&mut cols[1], |ui| {
                            ui.selectable_value(
                                &mut self.filter,
                                FilterMode::Linear,
                                "Linear (Smooth)",
                            );
                            ui.selectable_value(
                                &mut self.filter,
                                FilterMode::Nearest,
                                "Nearest (Crisp text)",
                            );
                        });
                });
                if self.fit != prev_fit || self.filter != prev_filter {
                    self.push_live_settings();
                }

                ui.horizontal(|ui| {
                    ui.checkbox(
                        &mut self.show_cursor,
                        RichText::new("Show cursor").size(12.5).color(TEXT_MUTED),
                    );
                    ui.add_space(8.0);
                    ui.checkbox(
                        &mut self.force_shm,
                        RichText::new("Force SHM").size(12.5).color(TEXT_MUTED),
                    )
                    .on_hover_text("Bypass GPU DMA-BUF and force PipeWire MemFd SHM buffers");
                });

                ui.add_space(8.0);

                /* 4. Capture source */
                ui.label(
                    RichText::new("4. Capture source")
                        .size(13.0)
                        .strong()
                        .color(TEXT_MUTED),
                );

                if solid_action_button(
                    ui,
                    ButtonIcon::Portal,
                    "Portal Picker (Screen or Window)",
                    ACCENT_BLUE,
                    DARK_INK,
                    42.0,
                )
                .clicked()
                {
                    self.launch_mode("portal");
                }

                if solid_action_button(
                    ui,
                    ButtonIcon::Region,
                    "Rectangular Region",
                    ACCENT_LAVENDER,
                    DARK_INK,
                    42.0,
                )
                .clicked()
                {
                    self.launch_mode("region");
                }
            });
    }
}
