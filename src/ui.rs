use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, ColorImage, CornerRadius, CursorIcon, FontFamily, FontId, Key,
    Modifiers, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Vec2,
};
use image::RgbaImage;

use crate::annotation::{
    Annotation, AnnotationKind, ColorRgba, DocumentSnapshot, History, PALETTE, Point, RectBounds,
    Tool, constrain_point,
};
use crate::capture::{CaptureOutcome, default_filename, default_screenshots_dir, load_or_capture, save_file_dialog};
use crate::clipboard::copy_png_to_clipboard;
use crate::renderer::{ArrowGeometry, Renderer, sample_pixelate_blocks};

#[derive(Debug, Clone)]
enum DragOperation {
    Create {
        tool: Tool,
        start: Point,
        current: Point,
    },
    MoveAnnotation {
        id: u64,
        last_pos: Point,
        recorded_undo: bool,
    },
    ResizeHandle {
        id: u64,
        handle_index: usize,
        recorded_undo: bool,
    },
}

#[derive(Debug, Clone)]
struct ActiveTextEditor {
    pos: Point,
    text: String,
    editing_id: Option<u64>,
    just_opened: bool,
}

struct StatusToast {
    message: String,
    is_error: bool,
    expires_at: Instant,
}

pub struct ScreenoteApp {
    base_image: RgbaImage,
    texture: Option<TextureHandle>,
    renderer: Renderer,

    annotations: Vec<Annotation>,
    crop: Option<RectBounds>,
    history: History,
    next_id: u64,

    active_tool: Tool,
    selected_id: Option<u64>,
    active_color: ColorRgba,
    stroke_width: f32,
    font_size: f32,
    pixelate_size: u32,
    pixelate_blur: bool,
    shape_filled: bool,
    text_backdrop: bool,

    drag_op: Option<DragOperation>,
    text_editor: Option<ActiveTextEditor>,

    zoom: f32,
    pan: Vec2,
    fit_to_view: bool,

    default_output: Option<PathBuf>,
    toast: Option<StatusToast>,
}

impl ScreenoteApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        base_image: RgbaImage,
        default_output: Option<PathBuf>,
    ) -> Self {
        configure_visuals(&cc.egui_ctx);

        let mut app = Self {
            base_image,
            texture: None,
            renderer: Renderer::new(),
            annotations: Vec::new(),
            crop: None,
            history: History::default(),
            next_id: 1,
            active_tool: Tool::Arrow,
            selected_id: None,
            active_color: PALETTE[0].1,
            stroke_width: 4.0,
            font_size: 24.0,
            pixelate_size: 14,
            pixelate_blur: false,
            shape_filled: false,
            text_backdrop: true,
            drag_op: None,
            text_editor: None,
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit_to_view: true,
            default_output,
            toast: None,
        };
        app.upload_texture(&cc.egui_ctx);
        app
    }

    fn upload_texture(&mut self, ctx: &egui::Context) {
        let size = [
            self.base_image.width() as usize,
            self.base_image.height() as usize,
        ];
        let color_image = ColorImage::from_rgba_unmultiplied(size, self.base_image.as_raw());
        self.texture =
            Some(ctx.load_texture("screenshot_canvas", color_image, TextureOptions::LINEAR));
    }

    fn snapshot(&self) -> DocumentSnapshot {
        DocumentSnapshot {
            annotations: self.annotations.clone(),
            crop: self.crop,
        }
    }

    fn restore(&mut self, snap: DocumentSnapshot) {
        self.annotations = snap.annotations;
        self.crop = snap.crop;
        if let Some(sel) = self.selected_id
            && !self.annotations.iter().any(|a| a.id == sel)
        {
            self.selected_id = None;
        }
    }

    fn record_undo(&mut self) {
        self.history.record(self.snapshot());
    }

    fn perform_undo(&mut self) {
        let current = self.snapshot();
        if let Some(prev) = self.history.undo(current) {
            self.restore(prev);
        }
    }

    fn perform_redo(&mut self) {
        let current = self.snapshot();
        if let Some(next) = self.history.redo(current) {
            self.restore(next);
        }
    }

    fn set_toast(&mut self, msg: impl Into<String>, is_error: bool) {
        self.toast = Some(StatusToast {
            message: msg.into(),
            is_error,
            expires_at: Instant::now() + Duration::from_secs(4),
        });
    }

    fn commit_text_editor(&mut self) {
        let Some(editor) = self.text_editor.take() else {
            return;
        };
        let trimmed = editor.text.trim_end();
        if trimmed.is_empty() {
            if let Some(edit_id) = editor.editing_id {
                self.record_undo();
                self.annotations.retain(|a| a.id != edit_id);
            }
            return;
        }

        self.record_undo();
        if let Some(edit_id) = editor.editing_id
            && let Some(existing) = self.annotations.iter_mut().find(|a| a.id == edit_id)
        {
            existing.kind = AnnotationKind::Text {
                pos: editor.pos,
                text: trimmed.to_string(),
                color: self.active_color,
                font_size: self.font_size,
                backdrop: self.text_backdrop,
            };
            self.selected_id = Some(edit_id);
            return;
        }

        let id = self.next_id;
        self.next_id += 1;
        self.annotations.push(Annotation {
            id,
            kind: AnnotationKind::Text {
                pos: editor.pos,
                text: trimmed.to_string(),
                color: self.active_color,
                font_size: self.font_size,
                backdrop: self.text_backdrop,
            },
        });
        self.selected_id = Some(id);
    }

    fn copy_to_clipboard(&mut self, ctx: &egui::Context, close_after: bool) {
        self.commit_text_editor();
        match self
            .renderer
            .render_png_bytes(&self.base_image, &self.annotations, self.crop)
        {
            Ok(png_bytes) => match copy_png_to_clipboard(&png_bytes) {
                Ok(()) => {
                    if close_after {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    } else {
                        self.set_toast("Copied annotated PNG to Wayland clipboard", false);
                    }
                }
                Err(err) => self.set_toast(format!("Clipboard error: {err:#}"), true),
            },
            Err(err) => self.set_toast(format!("Render error: {err:#}"), true),
        }
    }

    fn quick_save(&mut self) {
        self.commit_text_editor();
        let target_path = if let Some(path) = &self.default_output {
            path.clone()
        } else {
            let mut dir = default_screenshots_dir();
            if dir.file_name().and_then(|n| n.to_str()) == Some("Pictures") {
                let sub = dir.join("Screenshots");
                if std::fs::create_dir_all(&sub).is_ok() {
                    dir = sub;
                }
            }
            dir.join(default_filename())
        };

        if let Some(parent) = target_path.parent()
            && !parent.as_os_str().is_empty()
        {
            let _ = std::fs::create_dir_all(parent);
        }

        match self
            .renderer
            .render_png_bytes(&self.base_image, &self.annotations, self.crop)
        {
            Ok(bytes) => match std::fs::write(&target_path, bytes) {
                Ok(()) => {
                    self.set_toast(format!("Saved to {}", target_path.display()), false);
                }
                Err(err) => {
                    self.set_toast(format!("Failed writing {}: {err}", target_path.display()), true);
                }
            },
            Err(err) => self.set_toast(format!("Render error: {err:#}"), true),
        }
    }

    fn save_as(&mut self) {
        self.commit_text_editor();
        let suggested = self
            .default_output
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(default_filename);
        let initial_dir = self
            .default_output
            .as_ref()
            .and_then(|p| p.parent())
            .filter(|p| !p.as_os_str().is_empty());

        match save_file_dialog(&suggested, initial_dir) {
            Ok(Some(chosen)) => {
                match self
                    .renderer
                    .render_png_bytes(&self.base_image, &self.annotations, self.crop)
                {
                    Ok(bytes) => match std::fs::write(&chosen, bytes) {
                        Ok(()) => {
                            self.default_output = Some(chosen.clone());
                            self.set_toast(format!("Saved to {}", chosen.display()), false);
                        }
                        Err(err) => {
                            self.set_toast(
                                format!("Failed writing {}: {err}", chosen.display()),
                                true,
                            );
                        }
                    },
                    Err(err) => self.set_toast(format!("Render error: {err:#}"), true),
                }
            }
            Ok(None) => {}
            Err(err) => self.set_toast(format!("Save dialog error: {err:#}"), true),
        }
    }

    fn recapture(&mut self, ctx: &egui::Context) {
        self.commit_text_editor();
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        std::thread::sleep(Duration::from_millis(220));
        let result = load_or_capture(None, false);
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);

        match result {
            Ok(CaptureOutcome::Image(img)) => {
                self.base_image = img;
                self.annotations.clear();
                self.crop = None;
                self.history = History::default();
                self.selected_id = None;
                self.fit_to_view = true;
                self.pan = Vec2::ZERO;
                self.upload_texture(ctx);
                self.set_toast("Captured new screenshot", false);
            }
            Ok(CaptureOutcome::Cancelled) => {}
            Err(err) => self.set_toast(format!("Capture failed: {err:#}"), true),
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.text_editor.is_some() {
            return;
        }

        let mut do_undo = false;
        let mut do_redo = false;
        let mut do_copy = false;
        let mut do_copy_close = false;
        let mut do_save = false;
        let mut do_save_as = false;
        let mut do_recapture = false;
        let mut do_delete = false;
        let mut switch_tool = None;

        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
                || i.consume_key(Modifiers::COMMAND, Key::Y)
            {
                do_redo = true;
            } else if i.consume_key(Modifiers::COMMAND, Key::Z) {
                do_undo = true;
            }

            if i.consume_key(Modifiers::COMMAND, Key::Enter) {
                do_copy_close = true;
            } else if i.consume_key(Modifiers::COMMAND, Key::C) {
                do_copy = true;
            }

            if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::S) {
                do_save_as = true;
            } else if i.consume_key(Modifiers::COMMAND, Key::S) {
                do_save = true;
            }

            if i.consume_key(Modifiers::COMMAND, Key::N) {
                do_recapture = true;
            }

            if i.consume_key(Modifiers::COMMAND, Key::Q) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }

            if i.consume_key(Modifiers::NONE, Key::Delete)
                || i.consume_key(Modifiers::NONE, Key::Backspace)
            {
                do_delete = true;
            }

            if i.consume_key(Modifiers::NONE, Key::Escape) {
                self.drag_op = None;
                self.selected_id = None;
            }

            for (key, tool) in [
                (Key::V, Tool::Select),
                (Key::A, Tool::Arrow),
                (Key::R, Tool::Rectangle),
                (Key::E, Tool::Ellipse),
                (Key::T, Tool::Text),
                (Key::P, Tool::Pixelate),
                (Key::C, Tool::Crop),
            ] {
                if i.consume_key(Modifiers::NONE, key) {
                    switch_tool = Some(tool);
                }
            }
        });

        if let Some(tool) = switch_tool {
            self.active_tool = tool;
        }
        if do_undo {
            self.perform_undo();
        }
        if do_redo {
            self.perform_redo();
        }
        if do_delete
            && let Some(sel) = self.selected_id.take()
        {
            self.record_undo();
            self.annotations.retain(|a| a.id != sel);
        }
        if do_copy_close {
            self.copy_to_clipboard(ctx, true);
        } else if do_copy {
            self.copy_to_clipboard(ctx, false);
        }
        if do_save_as {
            self.save_as();
        } else if do_save {
            self.quick_save();
        }
        if do_recapture {
            self.recapture(ctx);
        }
    }

    fn draw_top_toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("screenote_toolbar")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(24, 26, 32))
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(42, 46, 58))),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                    ui.set_row_height(24.0);
                    let row_start_x = ui.cursor().min.x;

                    // 1. Tool selector buttons
                    for tool in [
                        Tool::Select,
                        Tool::Arrow,
                        Tool::Rectangle,
                        Tool::Ellipse,
                        Tool::Text,
                        Tool::Pixelate,
                        Tool::Crop,
                    ] {
                        let is_active = self.active_tool == tool;
                        let btn = egui::Button::new(
                            egui::RichText::new(tool.label()).color(if is_active {
                                Color32::WHITE
                            } else {
                                Color32::from_rgb(210, 214, 224)
                            }),
                        )
                        .fill(if is_active {
                            Color32::from_rgb(59, 130, 246)
                        } else {
                            Color32::from_rgb(36, 40, 50)
                        })
                        .corner_radius(CornerRadius::same(6));

                        let resp = ui
                            .add(btn)
                            .on_hover_text(format!("{} ({})", tool.label(), tool.shortcut_hint()));
                        if resp.clicked() {
                            self.commit_text_editor();
                            self.active_tool = tool;
                        }
                    }

                    // 2. Color palette swatches (wraps cleanly as a group onto a new line when narrow)
                    if self.active_tool != Tool::Pixelate && self.active_tool != Tool::Crop {
                        let palette_width = (PALETTE.len() as f32) * 28.0 + 14.0;
                        if ui.available_size_before_wrap().x < palette_width
                            && ui.cursor().min.x > row_start_x + 4.0
                        {
                            ui.end_row();
                        } else {
                            draw_toolbar_divider(ui);
                        }

                        for (name, col) in PALETTE {
                            let is_selected = self.active_color == *col;
                            let (rect, resp) =
                                ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
                            let c32 = to_egui_color(*col);
                            ui.painter().circle_filled(rect.center(), 8.5, c32);
                            if is_selected {
                                ui.painter().circle_stroke(
                                    rect.center(),
                                    10.0,
                                    Stroke::new(2.0_f32, Color32::WHITE),
                                );
                            } else {
                                ui.painter().circle_stroke(
                                    rect.center(),
                                    8.5,
                                    Stroke::new(1.0_f32, Color32::from_white_alpha(45)),
                                );
                            }
                            if resp.on_hover_text(*name).clicked() {
                                self.active_color = *col;
                                if let Some(sel_id) = self.selected_id {
                                    let before = self.snapshot();
                                    if let Some(ann) =
                                        self.annotations.iter_mut().find(|a| a.id == sel_id)
                                        && ann.set_color(*col)
                                    {
                                        self.history.record(before);
                                    }
                                }
                            }
                        }
                    }

                    // 3. Contextual tool parameters
                    let params_needed_width = match self.active_tool {
                        Tool::Arrow => 185.0,
                        Tool::Rectangle | Tool::Ellipse => {
                            if self.shape_filled {
                                60.0
                            } else {
                                230.0
                            }
                        }
                        Tool::Text => 255.0,
                        Tool::Pixelate => 275.0,
                        Tool::Crop => {
                            if self.crop.is_some() {
                                95.0
                            } else {
                                0.0
                            }
                        }
                        Tool::Select => {
                            if self.selected_id.is_some() {
                                75.0
                            } else {
                                0.0
                            }
                        }
                    };

                    if params_needed_width > 0.0 {
                        if ui.available_size_before_wrap().x < params_needed_width
                            && ui.cursor().min.x > row_start_x + 4.0
                        {
                            ui.end_row();
                        } else {
                            draw_toolbar_divider(ui);
                        }
                    }

                    match self.active_tool {
                        Tool::Arrow => {
                            ui.label("Width:");
                            ui.add(
                                egui::Slider::new(&mut self.stroke_width, 1.0..=20.0)
                                    .step_by(1.0)
                                    .suffix("px"),
                            );
                        }
                        Tool::Rectangle | Tool::Ellipse => {
                            ui.checkbox(&mut self.shape_filled, "Fill");
                            if !self.shape_filled {
                                ui.label("Width:");
                                ui.add(
                                    egui::Slider::new(&mut self.stroke_width, 1.0..=20.0)
                                        .step_by(1.0)
                                        .suffix("px"),
                                );
                            }
                        }
                        Tool::Text => {
                            ui.label("Size:");
                            ui.add(
                                egui::Slider::new(&mut self.font_size, 12.0..=72.0)
                                    .step_by(2.0)
                                    .suffix("px"),
                            );
                            ui.checkbox(&mut self.text_backdrop, "Backdrop");
                        }
                        Tool::Pixelate => {
                            ui.checkbox(&mut self.pixelate_blur, "Smooth Blur");
                            ui.label(if self.pixelate_blur { "Radius:" } else { "Block:" });
                            ui.add(
                                egui::Slider::new(&mut self.pixelate_size, 4..=48).suffix("px"),
                            );
                        }
                        Tool::Crop => {
                            if self.crop.is_some() && ui.button("Reset Crop").clicked() {
                                self.record_undo();
                                self.crop = None;
                            }
                        }
                        Tool::Select => {
                            if self.selected_id.is_some()
                                && ui
                                    .button("Delete")
                                    .on_hover_text("Delete selected annotation (Del)")
                                    .clicked()
                                && let Some(sel) = self.selected_id.take()
                            {
                                self.record_undo();
                                self.annotations.retain(|a| a.id != sel);
                            }
                        }
                    }

                    // 4. History & export actions: right-align when wide enough on the current row;
                    //    otherwise move to a new wrapped line so buttons never overlap colors/tools.
                    let actions_width = 450.0;
                    if ui.available_size_before_wrap().x >= actions_width {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            self.draw_action_buttons_rtl(ctx, ui);
                        });
                    } else {
                        if ui.cursor().min.x > row_start_x + 4.0 {
                            ui.end_row();
                        }
                        self.draw_action_buttons_ltr(ctx, ui);
                    }
                });
            });
    }

    fn draw_action_buttons_rtl(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        if ui
            .add(copy_close_button())
            .on_hover_text("Copy PNG to Wayland clipboard and exit (Ctrl+Enter)")
            .clicked()
        {
            self.copy_to_clipboard(ctx, true);
        }
        if ui
            .button("Copy")
            .on_hover_text("Copy PNG to Wayland clipboard (Ctrl+C)")
            .clicked()
        {
            self.copy_to_clipboard(ctx, false);
        }
        if ui
            .button("Save As…")
            .on_hover_text("Choose file path via desktop portal (Ctrl+Shift+S)")
            .clicked()
        {
            self.save_as();
        }
        if ui
            .button("Save")
            .on_hover_text("Quick save to Pictures/Screenshots (Ctrl+S)")
            .clicked()
        {
            self.quick_save();
        }

        draw_toolbar_divider(ui);

        if ui
            .button("New")
            .on_hover_text("Take a new screenshot (Ctrl+N)")
            .clicked()
        {
            self.recapture(ctx);
        }
        if ui
            .add_enabled(
                !self.annotations.is_empty() || self.crop.is_some(),
                egui::Button::new("Clear"),
            )
            .on_hover_text("Clear all annotations")
            .clicked()
        {
            self.record_undo();
            self.annotations.clear();
            self.crop = None;
            self.selected_id = None;
        }
        if ui
            .add_enabled(self.history.can_redo(), egui::Button::new("Redo"))
            .on_hover_text("Redo (Ctrl+Y)")
            .clicked()
        {
            self.perform_redo();
        }
        if ui
            .add_enabled(self.history.can_undo(), egui::Button::new("Undo"))
            .on_hover_text("Undo (Ctrl+Z)")
            .clicked()
        {
            self.perform_undo();
        }
    }

    fn draw_action_buttons_ltr(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        if ui
            .add_enabled(self.history.can_undo(), egui::Button::new("Undo"))
            .on_hover_text("Undo (Ctrl+Z)")
            .clicked()
        {
            self.perform_undo();
        }
        if ui
            .add_enabled(self.history.can_redo(), egui::Button::new("Redo"))
            .on_hover_text("Redo (Ctrl+Y)")
            .clicked()
        {
            self.perform_redo();
        }
        if ui
            .add_enabled(
                !self.annotations.is_empty() || self.crop.is_some(),
                egui::Button::new("Clear"),
            )
            .on_hover_text("Clear all annotations")
            .clicked()
        {
            self.record_undo();
            self.annotations.clear();
            self.crop = None;
            self.selected_id = None;
        }
        if ui
            .button("New")
            .on_hover_text("Take a new screenshot (Ctrl+N)")
            .clicked()
        {
            self.recapture(ctx);
        }

        draw_toolbar_divider(ui);

        if ui
            .button("Save")
            .on_hover_text("Quick save to Pictures/Screenshots (Ctrl+S)")
            .clicked()
        {
            self.quick_save();
        }
        if ui
            .button("Save As…")
            .on_hover_text("Choose file path via desktop portal (Ctrl+Shift+S)")
            .clicked()
        {
            self.save_as();
        }
        if ui
            .button("Copy")
            .on_hover_text("Copy PNG to Wayland clipboard (Ctrl+C)")
            .clicked()
        {
            self.copy_to_clipboard(ctx, false);
        }
        if ui
            .add(copy_close_button())
            .on_hover_text("Copy PNG to Wayland clipboard and exit (Ctrl+Enter)")
            .clicked()
        {
            self.copy_to_clipboard(ctx, true);
        }
    }

    fn draw_bottom_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("screenote_statusbar")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(20, 22, 27))
                    .inner_margin(egui::Margin::symmetric(12, 4))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(36, 40, 50))),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let (out_w, out_h) = if let Some(c) = self.crop {
                        (c.width().round() as u32, c.height().round() as u32)
                    } else {
                        (self.base_image.width(), self.base_image.height())
                    };
                    ui.label(
                        egui::RichText::new(format!("{out_w} × {out_h} px"))
                            .size(12.0)
                            .color(Color32::from_rgb(156, 163, 175)),
                    );

                    draw_toolbar_divider(ui);

                    if ui
                        .small_button(format!("{:.0}%", self.zoom * 100.0))
                        .on_hover_text("Reset zoom to Fit Window")
                        .clicked()
                    {
                        self.fit_to_view = true;
                        self.pan = Vec2::ZERO;
                    }
                    if ui
                        .small_button("100%")
                        .on_hover_text("Actual pixel size (1:1)")
                        .clicked()
                    {
                        self.fit_to_view = false;
                        self.zoom = 1.0;
                        self.pan = Vec2::ZERO;
                    }

                    let active_toast = self
                        .toast
                        .as_ref()
                        .filter(|t| Instant::now() < t.expires_at);

                    if let Some(toast) = active_toast {
                        draw_toolbar_divider(ui);
                        let col = if toast.is_error {
                            Color32::from_rgb(248, 113, 113)
                        } else {
                            Color32::from_rgb(74, 222, 128)
                        };
                        ui.label(egui::RichText::new(&toast.message).size(12.0).color(col));
                    } else {
                        self.toast = None;
                        if ui.available_size_before_wrap().x > 420.0 {
                            draw_toolbar_divider(ui);
                            ui.label(
                                egui::RichText::new(
                                    "Hold Shift to snap angles/aspect · Middle-drag or Space+drag to pan · Ctrl+Scroll to zoom",
                                )
                                .size(11.5)
                                .color(Color32::from_rgb(120, 128, 142)),
                            );
                        }
                    }
                });
            });
    }

    fn draw_canvas(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::from_rgb(15, 17, 22)))
            .show(ctx, |ui| {
                let avail = ui.available_rect_before_wrap();
                let img_w = self.base_image.width() as f32;
                let img_h = self.base_image.height() as f32;

                if self.fit_to_view && img_w > 0.0 && img_h > 0.0 {
                    let pad = 32.0;
                    let sx = ((avail.width() - pad).max(64.0)) / img_w;
                    let sy = ((avail.height() - pad).max(64.0)) / img_h;
                    self.zoom = sx.min(sy).min(1.0);
                }

                let (canvas_rect, response) =
                    ui.allocate_exact_size(avail.size(), Sense::click_and_drag());

                // Zoom with Ctrl+Scroll or pinch, Pan with Middle-drag or Space+drag or plain scroll
                let space_held = ctx.input(|i| i.key_down(Key::Space)) && self.text_editor.is_none();
                if response.hovered() {
                    let scroll = ctx.input(|i| i.smooth_scroll_delta);
                    let zoom_delta = ctx.input(|i| i.zoom_delta());
                    if (zoom_delta - 1.0).abs() > 1e-4 {
                        self.fit_to_view = false;
                        let old_zoom = self.zoom;
                        self.zoom = (self.zoom * zoom_delta).clamp(0.1, 8.0);
                        if let Some(hover) = response.hover_pos() {
                            let rel = hover - canvas_rect.center() - self.pan;
                            self.pan -= rel * (self.zoom / old_zoom - 1.0);
                        }
                    } else if scroll != Vec2::ZERO {
                        self.pan += scroll;
                        self.fit_to_view = false;
                    }
                }

                if response.dragged_by(egui::PointerButton::Middle)
                    || (space_held && response.dragged_by(egui::PointerButton::Primary))
                {
                    self.pan += response.drag_delta();
                    self.fit_to_view = false;
                    ctx.set_cursor_icon(CursorIcon::Grabbing);
                } else if space_held && response.hovered() {
                    ctx.set_cursor_icon(CursorIcon::Grab);
                } else if response.hovered() {
                    ctx.set_cursor_icon(match self.active_tool {
                        Tool::Select => CursorIcon::Default,
                        Tool::Text => CursorIcon::Text,
                        _ => CursorIcon::Crosshair,
                    });
                }

                let zoom = self.zoom;
                let scaled_size = Vec2::new(img_w * zoom, img_h * zoom);
                let img_origin = canvas_rect.center() - scaled_size * 0.5 + self.pan;
                let img_screen_rect = Rect::from_min_size(img_origin, scaled_size);

                let to_screen = move |p: Point| -> Pos2 {
                    Pos2::new(img_origin.x + p.x * zoom, img_origin.y + p.y * zoom)
                };
                let to_image = move |pos: Pos2| -> Point {
                    Point::new(
                        ((pos.x - img_origin.x) / zoom).clamp(0.0, img_w),
                        ((pos.y - img_origin.y) / zoom).clamp(0.0, img_h),
                    )
                };

                // Canvas drop-shadow & screenshot texture
                let painter = ui.painter_at(canvas_rect);
                painter.rect_filled(
                    img_screen_rect.translate(Vec2::new(0.0, 4.0)).expand(2.0),
                    CornerRadius::same(2),
                    Color32::from_black_alpha(110),
                );
                if let Some(tex) = &self.texture {
                    painter.image(
                        tex.id(),
                        img_screen_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                painter.rect_stroke(
                    img_screen_rect,
                    CornerRadius::ZERO,
                    Stroke::new(1.0_f32, Color32::from_rgb(55, 62, 76)),
                    StrokeKind::Outside,
                );

                // Interactive pointer handling (when not panning with Space)
                if !space_held {
                    self.handle_canvas_pointer(ctx, &response, &to_image);
                }

                // Clip annotations to screenshot bounds
                let img_painter = painter.with_clip_rect(canvas_rect.intersect(img_screen_rect));
                for ann in &self.annotations {
                    if self
                        .text_editor
                        .as_ref()
                        .is_some_and(|ed| ed.editing_id == Some(ann.id))
                    {
                        continue;
                    }
                    self.paint_annotation(&img_painter, &ann.kind, &to_screen);
                }

                // Live drag preview
                if let Some(DragOperation::Create {
                    tool,
                    start,
                    current,
                }) = &self.drag_op
                {
                    if *tool == Tool::Crop {
                        paint_crop_overlay(
                            &img_painter,
                            img_screen_rect,
                            RectBounds::from_points(*start, *current),
                            &to_screen,
                        );
                    } else if let Some(preview_kind) =
                        self.make_annotation_kind(*tool, *start, *current)
                    {
                        self.paint_annotation(&img_painter, &preview_kind, &to_screen);
                    }
                } else if let Some(crop_bounds) = self.crop {
                    paint_crop_overlay(&img_painter, img_screen_rect, crop_bounds, &to_screen);
                }

                // Selection outline & resize handles
                if let Some(sel_id) = self.selected_id
                    && let Some(sel_ann) = self.annotations.iter().find(|a| a.id == sel_id)
                {
                    let b = sel_ann.bounds();
                    let sel_rect = Rect::from_min_max(
                        to_screen(Point::new(b.min_x, b.min_y)),
                        to_screen(Point::new(b.max_x, b.max_y)),
                    )
                    .expand(3.0);
                    painter.rect_stroke(
                        sel_rect,
                        CornerRadius::same(2),
                        Stroke::new(1.0_f32, Color32::from_rgb(96, 165, 250)),
                        StrokeKind::Outside,
                    );
                    for handle in sel_ann.handles() {
                        let hp = to_screen(handle);
                        painter.circle_filled(hp, 5.0, Color32::WHITE);
                        painter.circle_stroke(
                            hp,
                            5.0,
                            Stroke::new(1.8_f32, Color32::from_rgb(37, 99, 235)),
                        );
                    }
                }

                // Inline text overlay widget
                self.show_inline_text_editor(ctx, &to_screen);
            });
    }

    fn handle_canvas_pointer(
        &mut self,
        ctx: &egui::Context,
        response: &egui::Response,
        to_image: &dyn Fn(Pos2) -> Point,
    ) {
        let shift_held = ctx.input(|i| i.modifiers.shift);
        let hit_tol = (8.0 / self.zoom).max(4.0);

        if response.double_clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let img_pt = to_image(pos);
            if let Some(ann) = self
                .annotations
                .iter()
                .rev()
                .find(|a| matches!(a.kind, AnnotationKind::Text { .. }) && a.hit_test(img_pt, hit_tol))
                .cloned()
                && let AnnotationKind::Text {
                    pos,
                    text,
                    color,
                    font_size,
                    backdrop,
                } = ann.kind
            {
                self.active_color = color;
                self.font_size = font_size;
                self.text_backdrop = backdrop;
                self.text_editor = Some(ActiveTextEditor {
                    pos,
                    text,
                    editing_id: Some(ann.id),
                    just_opened: true,
                });
                return;
            }
        }

        if response.clicked_by(egui::PointerButton::Primary)
            && self.active_tool == Tool::Text
            && let Some(pos) = response.interact_pointer_pos()
        {
            if self.text_editor.is_some() {
                self.commit_text_editor();
            } else {
                let pt = to_image(pos);
                self.text_editor = Some(ActiveTextEditor {
                    pos: pt,
                    text: String::new(),
                    editing_id: None,
                    just_opened: true,
                });
            }
            return;
        }

        if response.clicked_by(egui::PointerButton::Primary)
            && self.active_tool == Tool::Select
            && let Some(pos) = response.interact_pointer_pos()
        {
            let img_pt = to_image(pos);
            self.selected_id = self
                .annotations
                .iter()
                .rev()
                .find(|a| a.hit_test(img_pt, hit_tol))
                .map(|a| a.id);
        }

        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(pos) = response.interact_pointer_pos()
        {
            if self.text_editor.is_some() {
                self.commit_text_editor();
            }
            let img_pt = to_image(pos);
            match self.active_tool {
                Tool::Select => {
                    // Check handles of currently selected annotation first
                    if let Some(sel_id) = self.selected_id
                        && let Some(sel_ann) = self.annotations.iter().find(|a| a.id == sel_id)
                        && let Some((idx, _)) = sel_ann
                            .handles()
                            .iter()
                            .enumerate()
                            .find(|(_, h)| h.distance_to(img_pt) <= hit_tol * 1.4)
                    {
                        self.drag_op = Some(DragOperation::ResizeHandle {
                            id: sel_id,
                            handle_index: idx,
                            recorded_undo: false,
                        });
                        return;
                    }

                    // Otherwise hit-test annotations top-to-bottom
                    if let Some(hit_id) = self
                        .annotations
                        .iter()
                        .rev()
                        .find(|a| a.hit_test(img_pt, hit_tol))
                        .map(|a| a.id)
                    {
                        self.selected_id = Some(hit_id);
                        self.drag_op = Some(DragOperation::MoveAnnotation {
                            id: hit_id,
                            last_pos: img_pt,
                            recorded_undo: false,
                        });
                    } else {
                        self.selected_id = None;
                    }
                }
                Tool::Text => {}
                tool => {
                    self.selected_id = None;
                    self.drag_op = Some(DragOperation::Create {
                        tool,
                        start: img_pt,
                        current: img_pt,
                    });
                }
            }
        }

        if response.dragged_by(egui::PointerButton::Primary)
            && let Some(pos) = response.interact_pointer_pos()
        {
            let raw_pt = to_image(pos);
            let before = self.snapshot();
            if let Some(op) = &mut self.drag_op {
                match op {
                    DragOperation::Create {
                        tool,
                        start,
                        current,
                    } => {
                        *current = if shift_held {
                            constrain_point(*start, raw_pt, *tool)
                        } else {
                            raw_pt
                        };
                    }
                    DragOperation::MoveAnnotation {
                        id,
                        last_pos,
                        recorded_undo,
                    } => {
                        let dx = raw_pt.x - last_pos.x;
                        let dy = raw_pt.y - last_pos.y;
                        if dx.abs() > 0.01 || dy.abs() > 0.01 {
                            if !*recorded_undo {
                                self.history.record(before);
                                *recorded_undo = true;
                            }
                            if let Some(ann) = self.annotations.iter_mut().find(|a| a.id == *id) {
                                ann.translate(dx, dy);
                            }
                            *last_pos = raw_pt;
                        }
                    }
                    DragOperation::ResizeHandle {
                        id,
                        handle_index,
                        recorded_undo,
                    } => {
                        if !*recorded_undo {
                            self.history.record(before);
                            *recorded_undo = true;
                        }
                        if let Some(ann) = self.annotations.iter_mut().find(|a| a.id == *id) {
                            ann.set_handle(*handle_index, raw_pt);
                        }
                    }
                }
            }
        }

        if response.drag_stopped_by(egui::PointerButton::Primary)
            && let Some(op) = self.drag_op.take()
            && let DragOperation::Create {
                tool,
                start,
                current,
            } = op
            && start.distance_to(current) >= 3.0
        {
            if tool == Tool::Crop {
                let bounds = RectBounds::from_points(start, current)
                    .clamp_to_image(self.base_image.width() as f32, self.base_image.height() as f32);
                if bounds.width() >= 8.0 && bounds.height() >= 8.0 {
                    self.record_undo();
                    self.crop = Some(bounds);
                    self.active_tool = Tool::Arrow;
                }
            } else if let Some(kind) = self.make_annotation_kind(tool, start, current) {
                self.record_undo();
                let id = self.next_id;
                self.next_id += 1;
                self.annotations.push(Annotation { id, kind });
                self.selected_id = Some(id);
            }
        }
    }

    fn make_annotation_kind(&self, tool: Tool, start: Point, end: Point) -> Option<AnnotationKind> {
        match tool {
            Tool::Arrow => Some(AnnotationKind::Arrow {
                start,
                end,
                color: self.active_color,
                stroke_width: self.stroke_width,
            }),
            Tool::Rectangle => Some(AnnotationKind::Rectangle {
                start,
                end,
                color: self.active_color,
                stroke_width: self.stroke_width,
                filled: self.shape_filled,
            }),
            Tool::Ellipse => Some(AnnotationKind::Ellipse {
                start,
                end,
                color: self.active_color,
                stroke_width: self.stroke_width,
                filled: self.shape_filled,
            }),
            Tool::Pixelate => Some(AnnotationKind::Pixelate {
                start,
                end,
                block_size: self.pixelate_size,
                blur: self.pixelate_blur,
            }),
            _ => None,
        }
    }

    fn paint_annotation(
        &self,
        painter: &Painter,
        kind: &AnnotationKind,
        to_screen: &dyn Fn(Point) -> Pos2,
    ) {
        match kind {
            AnnotationKind::Arrow {
                start,
                end,
                color,
                stroke_width,
            } => {
                let c32 = to_egui_color(*color);
                if let Some(geom) = ArrowGeometry::compute(*start, *end, *stroke_width) {
                    if start.distance_to(geom.shaft_end) > 0.5 {
                        painter.line_segment(
                            [to_screen(*start), to_screen(geom.shaft_end)],
                            Stroke::new((*stroke_width * self.zoom).max(1.0), c32),
                        );
                    }
                    painter.add(Shape::convex_polygon(
                        vec![
                            to_screen(geom.tip),
                            to_screen(geom.left_wing),
                            to_screen(geom.neck),
                        ],
                        c32,
                        Stroke::NONE,
                    ));
                    painter.add(Shape::convex_polygon(
                        vec![
                            to_screen(geom.tip),
                            to_screen(geom.neck),
                            to_screen(geom.right_wing),
                        ],
                        c32,
                        Stroke::NONE,
                    ));
                }
            }
            AnnotationKind::Rectangle {
                start,
                end,
                color,
                stroke_width,
                filled,
            } => {
                let b = RectBounds::from_points(*start, *end);
                let rect = Rect::from_min_max(
                    to_screen(Point::new(b.min_x, b.min_y)),
                    to_screen(Point::new(b.max_x, b.max_y)),
                );
                let c32 = to_egui_color(*color);
                if *filled {
                    painter.rect_filled(rect, CornerRadius::ZERO, c32);
                } else {
                    painter.rect_stroke(
                        rect,
                        CornerRadius::ZERO,
                        Stroke::new((*stroke_width * self.zoom).max(1.0), c32),
                        StrokeKind::Middle,
                    );
                }
            }
            AnnotationKind::Ellipse {
                start,
                end,
                color,
                stroke_width,
                filled,
            } => {
                let b = RectBounds::from_points(*start, *end);
                let rect = Rect::from_min_max(
                    to_screen(Point::new(b.min_x, b.min_y)),
                    to_screen(Point::new(b.max_x, b.max_y)),
                );
                let radius = rect.size() * 0.5;
                let c32 = to_egui_color(*color);
                if *filled {
                    painter.add(egui::epaint::EllipseShape::filled(rect.center(), radius, c32));
                } else {
                    painter.add(egui::epaint::EllipseShape::stroke(
                        rect.center(),
                        radius,
                        Stroke::new((*stroke_width * self.zoom).max(1.0), c32),
                    ));
                }
            }
            AnnotationKind::Pixelate {
                start,
                end,
                block_size,
                blur,
            } => {
                let bounds = RectBounds::from_points(*start, *end);
                let preview_block = if *blur {
                    ((*block_size) / 2).max(4)
                } else {
                    *block_size
                };
                for (cell, col) in sample_pixelate_blocks(&self.base_image, bounds, preview_block) {
                    let cell_rect = Rect::from_min_max(
                        to_screen(Point::new(cell.min_x, cell.min_y)),
                        to_screen(Point::new(cell.max_x, cell.max_y)),
                    );
                    let radius = if *blur {
                        CornerRadius::same(2)
                    } else {
                        CornerRadius::ZERO
                    };
                    painter.rect_filled(cell_rect, radius, to_egui_color(col));
                }
            }
            AnnotationKind::Text {
                pos,
                text,
                color,
                font_size,
                backdrop,
            } => {
                let screen_pos = to_screen(*pos);
                let scaled_font = (*font_size * self.zoom).max(6.0);
                let galley = painter.layout_no_wrap(
                    text.clone(),
                    FontId::new(scaled_font, FontFamily::Proportional),
                    to_egui_color(*color),
                );
                if *backdrop {
                    let pad = Vec2::new(
                        (scaled_font * 0.35).max(3.0),
                        (scaled_font * 0.22).max(2.0),
                    );
                    let lum = (0.2126 * f32::from(color.r)
                        + 0.7152 * f32::from(color.g)
                        + 0.0722 * f32::from(color.b))
                        / 255.0;
                    let bg = if lum < 0.35 {
                        Color32::from_rgba_unmultiplied(248, 250, 252, 220)
                    } else {
                        Color32::from_rgba_unmultiplied(15, 23, 42, 215)
                    };
                    let bg_rect = Rect::from_min_size(screen_pos - pad, galley.size() + pad * 2.0);
                    painter.rect_filled(
                        bg_rect,
                        CornerRadius::same((scaled_font * 0.25).clamp(2.0, 8.0) as u8),
                        bg,
                    );
                }
                painter.galley(screen_pos, galley, to_egui_color(*color));
            }
        }
    }

    fn show_inline_text_editor(
        &mut self,
        ctx: &egui::Context,
        to_screen: &dyn Fn(Point) -> Pos2,
    ) {
        let Some(mut editor) = self.text_editor.take() else {
            return;
        };

        let anchor = to_screen(editor.pos);
        let scaled_font = (self.font_size * self.zoom).clamp(13.0, 64.0);
        let mut commit = false;
        let mut cancel = false;

        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                cancel = true;
            } else if i.consume_key(Modifiers::NONE, Key::Enter)
                || i.consume_key(Modifiers::COMMAND, Key::Enter)
            {
                commit = true;
            }
        });

        let area_resp = egui::Area::new(egui::Id::new("screenote_inline_text"))
            .fixed_pos(anchor)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(20, 24, 34, 240))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(egui::Margin::same(6))
                    .stroke(Stroke::new(1.5_f32, Color32::from_rgb(59, 130, 246)))
                    .show(ui, |ui| {
                        let edit = egui::TextEdit::multiline(&mut editor.text)
                            .font(FontId::new(scaled_font, FontFamily::Proportional))
                            .text_color(to_egui_color(self.active_color))
                            .desired_rows(1)
                            .desired_width(220.0)
                            .hint_text("Type annotation…");
                        let resp = ui.add(edit);
                        if editor.just_opened {
                            resp.request_focus();
                            editor.just_opened = false;
                        }
                        ui.label(
                            egui::RichText::new("Enter: apply · Shift+Enter: newline · Esc: cancel")
                                .size(10.5)
                                .color(Color32::from_rgb(148, 163, 184)),
                        );
                    });
            });

        if cancel {
            return;
        }
        if commit
            || (ctx.input(|i| i.pointer.any_pressed())
                && !area_resp
                    .response
                    .rect
                    .contains(ctx.input(|i| i.pointer.interact_pos().unwrap_or_default())))
        {
            self.text_editor = Some(editor);
            self.commit_text_editor();
        } else {
            self.text_editor = Some(editor);
        }
    }
}

impl eframe::App for ScreenoteApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_shortcuts(ctx);
        self.draw_top_toolbar(ctx);
        self.draw_bottom_bar(ctx);
        self.draw_canvas(ctx);
    }
}

fn paint_crop_overlay(
    painter: &Painter,
    img_rect: Rect,
    crop_bounds: RectBounds,
    to_screen: &dyn Fn(Point) -> Pos2,
) {
    let crop_screen = Rect::from_min_max(
        to_screen(Point::new(crop_bounds.min_x, crop_bounds.min_y)),
        to_screen(Point::new(crop_bounds.max_x, crop_bounds.max_y)),
    )
    .intersect(img_rect);

    let dim = Color32::from_black_alpha(150);
    // Top, bottom, left, right dim bands outside the crop selection
    painter.rect_filled(
        Rect::from_min_max(img_rect.min, Pos2::new(img_rect.max.x, crop_screen.min.y)),
        CornerRadius::ZERO,
        dim,
    );
    painter.rect_filled(
        Rect::from_min_max(Pos2::new(img_rect.min.x, crop_screen.max.y), img_rect.max),
        CornerRadius::ZERO,
        dim,
    );
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(img_rect.min.x, crop_screen.min.y),
            Pos2::new(crop_screen.min.x, crop_screen.max.y),
        ),
        CornerRadius::ZERO,
        dim,
    );
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(crop_screen.max.x, crop_screen.min.y),
            Pos2::new(img_rect.max.x, crop_screen.max.y),
        ),
        CornerRadius::ZERO,
        dim,
    );

    painter.rect_stroke(
        crop_screen,
        CornerRadius::ZERO,
        Stroke::new(1.5_f32, Color32::WHITE),
        StrokeKind::Outside,
    );
    painter.text(
        crop_screen.left_top() + Vec2::new(6.0, 6.0),
        Align2::LEFT_TOP,
        format!(
            "{} × {}",
            crop_bounds.width().round() as u32,
            crop_bounds.height().round() as u32
        ),
        FontId::proportional(11.5),
        Color32::WHITE,
    );
}

fn copy_close_button() -> egui::Button<'static> {
    egui::Button::new(
        egui::RichText::new("Copy & Close")
            .strong()
            .color(Color32::WHITE),
    )
    .fill(Color32::from_rgb(37, 99, 235))
    .corner_radius(CornerRadius::same(6))
}

fn draw_toolbar_divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(6.0, 18.0), Sense::hover());
    ui.painter().line_segment(
        [
            Pos2::new(rect.center().x, rect.top() + 2.0),
            Pos2::new(rect.center().x, rect.bottom() - 2.0),
        ],
        Stroke::new(1.0_f32, Color32::from_rgb(52, 58, 72)),
    );
}

fn to_egui_color(c: ColorRgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

fn configure_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(24, 26, 32);
    visuals.window_fill = Color32::from_rgb(24, 26, 32);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(36, 40, 50);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(50, 56, 70);
    visuals.widgets.active.bg_fill = Color32::from_rgb(59, 130, 246);
    ctx.set_visuals(visuals);
}
