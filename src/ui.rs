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
    ShapeKind, Tool, constrain_point, text_backdrop_padding,
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
    ResizeShape {
        id: u64,
        handle_index: usize,
        anchor: Point,
        recorded_undo: bool,
    },
    ResizeText {
        id: u64,
        handle_index: usize,
        anchor: Point,
        initial_diag: f32,
        initial_font_size: f32,
        recorded_undo: bool,
    },
}

#[derive(Debug, Clone)]
struct ActiveTextEditor {
    pos: Point,
    text: String,
    editing_id: Option<u64>,
    just_opened: bool,
    refocus: bool,
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
    slider_undo_recorded: bool,

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
            slider_undo_recorded: false,
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

    fn select_annotation(&mut self, id: Option<u64>) {
        self.selected_id = id;
        if let Some(sel_id) = id
            && let Some(ann) = self.annotations.iter().find(|a| a.id == sel_id)
            && let Some(col) = ann.color()
        {
            self.active_color = col;
        }
    }

    fn switch_tool(&mut self, new_tool: Tool) {
        self.commit_text_editor();
        if new_tool != Tool::Select && new_tool != self.active_tool {
            self.selected_id = None;
        }
        self.active_tool = new_tool;
    }

    fn record_slider_undo(&mut self, before: DocumentSnapshot, resp: &egui::Response) {
        if !self.slider_undo_recorded {
            self.history.record(before);
            if resp.dragged() {
                self.slider_undo_recorded = true;
            }
        }
    }

    fn measure_text_image_size(
        &self,
        ctx: &egui::Context,
        text: &str,
        font_size: f32,
    ) -> (f32, f32) {
        let zoom = self.zoom.max(0.05);
        let scaled_font = (font_size.max(8.0) * zoom).max(6.0);
        let sample = if text.is_empty() { " " } else { text };
        let galley = ctx
            .layer_painter(egui::LayerId::background())
            .layout_no_wrap(
                sample.to_string(),
                FontId::new(scaled_font, FontFamily::Proportional),
                Color32::WHITE,
            );
        (galley.size().x / zoom, galley.size().y / zoom)
    }

    fn annotation_bounds(&self, ctx: &egui::Context, ann: &Annotation) -> RectBounds {
        match &ann.kind {
            AnnotationKind::Text {
                text, font_size, ..
            } => {
                let measured = self.measure_text_image_size(ctx, text, *font_size);
                ann.bounds_with_text_size(Some(measured))
            }
            _ => ann.bounds_with_text_size(None),
        }
    }

    fn open_text_editor_for(&mut self, id: u64) {
        if let Some(ann) = self.annotations.iter().find(|a| a.id == id).cloned()
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
            self.selected_id = Some(id);
            self.text_editor = Some(ActiveTextEditor {
                pos,
                text,
                editing_id: Some(id),
                just_opened: true,
                refocus: false,
            });
        }
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
                if self.selected_id == Some(edit_id) {
                    self.selected_id = None;
                }
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
        let mut nudge = Vec2::ZERO;

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

            if self.selected_id.is_some() {
                for (key, dir) in [
                    (Key::ArrowLeft, Vec2::new(-1.0, 0.0)),
                    (Key::ArrowRight, Vec2::new(1.0, 0.0)),
                    (Key::ArrowUp, Vec2::new(0.0, -1.0)),
                    (Key::ArrowDown, Vec2::new(0.0, 1.0)),
                ] {
                    if i.consume_key(Modifiers::SHIFT, key) {
                        nudge += dir * 10.0;
                    } else if i.consume_key(Modifiers::NONE, key) {
                        nudge += dir;
                    }
                }
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
            self.switch_tool(tool);
        }
        if nudge != Vec2::ZERO
            && let Some(sel) = self.selected_id
        {
            self.record_undo();
            if let Some(ann) = self.annotations.iter_mut().find(|a| a.id == sel) {
                ann.translate(nudge.x, nudge.y);
            }
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

    fn draw_top_toolbar(&mut self, root_ui: &mut egui::Ui, ctx: &egui::Context) {
        if !ctx.input(|i| i.pointer.any_down()) {
            self.slider_undo_recorded = false;
        }

        egui::Panel::top("screenote_toolbar")
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(24, 26, 32))
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(42, 46, 58))),
            )
            .show(root_ui, |ui| {
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
                            self.switch_tool(tool);
                        }
                    }

                    let selected_kind = self
                        .selected_id
                        .and_then(|id| self.annotations.iter().find(|a| a.id == id))
                        .map(|a| a.kind.clone());

                    let selected_color = self
                        .selected_id
                        .and_then(|id| self.annotations.iter().find(|a| a.id == id))
                        .and_then(Annotation::color);

                    let show_palette = match self.active_tool {
                        Tool::Arrow | Tool::Rectangle | Tool::Ellipse | Tool::Text => true,
                        Tool::Select => selected_color.is_some(),
                        Tool::Pixelate | Tool::Crop => false,
                    };

                    // 2. Color palette swatches (wraps cleanly as a group onto a new line when narrow)
                    if show_palette {
                        let palette_width = (PALETTE.len() as f32) * 28.0 + 14.0;
                        if ui.available_size_before_wrap().x < palette_width
                            && ui.cursor().min.x > row_start_x + 4.0
                        {
                            ui.end_row();
                        } else {
                            draw_toolbar_divider(ui);
                        }

                        let highlighted_color = selected_color.unwrap_or(self.active_color);
                        for (name, col) in PALETTE {
                            let is_selected = highlighted_color == *col;
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
                                if let Some(ed) = &mut self.text_editor {
                                    ed.refocus = true;
                                }
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

                    // 3. Contextual tool / selected-annotation modifiers
                    let params_needed_width = match self.active_tool {
                        Tool::Arrow => 185.0,
                        Tool::Rectangle | Tool::Ellipse => {
                            if self.shape_filled {
                                65.0
                            } else {
                                235.0
                            }
                        }
                        Tool::Text => 265.0,
                        Tool::Pixelate => 275.0,
                        Tool::Crop => {
                            if self.crop.is_some() {
                                95.0
                            } else {
                                0.0
                            }
                        }
                        Tool::Select => match &selected_kind {
                            None => 170.0,
                            Some(AnnotationKind::Arrow { .. }) => 355.0,
                            Some(
                                AnnotationKind::Rectangle { filled, .. }
                                | AnnotationKind::Ellipse { filled, .. },
                            ) => {
                                if *filled {
                                    230.0
                                } else {
                                    405.0
                                }
                            }
                            Some(AnnotationKind::Text { .. }) => 380.0,
                            Some(AnnotationKind::Pixelate { .. }) => 440.0,
                        },
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
                            let before = self.snapshot();
                            ui.label("Width:");
                            let resp = ui.add(
                                egui::Slider::new(&mut self.stroke_width, 1.0..=20.0)
                                    .step_by(1.0)
                                    .suffix("px"),
                            );
                            if resp.changed()
                                && let Some(sel_id) = self.selected_id
                                && self.annotations.iter().any(|a| {
                                    a.id == sel_id && matches!(a.kind, AnnotationKind::Arrow { .. })
                                })
                            {
                                self.record_slider_undo(before, &resp);
                                if let Some(ann) =
                                    self.annotations.iter_mut().find(|a| a.id == sel_id)
                                    && let AnnotationKind::Arrow { stroke_width, .. } =
                                        &mut ann.kind
                                {
                                    *stroke_width = self.stroke_width;
                                }
                            }
                        }
                        Tool::Rectangle | Tool::Ellipse => {
                            let before = self.snapshot();
                            let fill_resp = ui.checkbox(&mut self.shape_filled, "Fill");
                            if fill_resp.changed()
                                && let Some(sel_id) = self.selected_id
                                && let Some(ann) =
                                    self.annotations.iter_mut().find(|a| a.id == sel_id)
                                && let AnnotationKind::Rectangle { filled, .. }
                                | AnnotationKind::Ellipse { filled, .. } = &mut ann.kind
                            {
                                *filled = self.shape_filled;
                                self.history.record(before.clone());
                            }
                            if !self.shape_filled {
                                ui.label("Width:");
                                let width_resp = ui.add(
                                    egui::Slider::new(&mut self.stroke_width, 1.0..=20.0)
                                        .step_by(1.0)
                                        .suffix("px"),
                                );
                                if width_resp.changed()
                                    && let Some(sel_id) = self.selected_id
                                    && self.annotations.iter().any(|a| {
                                        a.id == sel_id
                                            && matches!(
                                                a.kind,
                                                AnnotationKind::Rectangle { .. }
                                                    | AnnotationKind::Ellipse { .. }
                                            )
                                    })
                                {
                                    self.record_slider_undo(before, &width_resp);
                                    if let Some(ann) =
                                        self.annotations.iter_mut().find(|a| a.id == sel_id)
                                        && let AnnotationKind::Rectangle { stroke_width, .. }
                                        | AnnotationKind::Ellipse { stroke_width, .. } =
                                            &mut ann.kind
                                    {
                                        *stroke_width = self.stroke_width;
                                    }
                                }
                            }
                        }
                        Tool::Text => {
                            let before = self.snapshot();
                            ui.label("Size:");
                            let size_resp = ui.add(
                                egui::Slider::new(&mut self.font_size, 12.0..=96.0)
                                    .step_by(2.0)
                                    .suffix("px"),
                            );
                            if size_resp.changed()
                                && let Some(sel_id) = self.selected_id
                                && self.annotations.iter().any(|a| {
                                    a.id == sel_id && matches!(a.kind, AnnotationKind::Text { .. })
                                })
                            {
                                self.record_slider_undo(before.clone(), &size_resp);
                                if let Some(ann) =
                                    self.annotations.iter_mut().find(|a| a.id == sel_id)
                                    && let AnnotationKind::Text { font_size, .. } = &mut ann.kind
                                {
                                    *font_size = self.font_size;
                                }
                            }
                            let bg_resp = ui.checkbox(&mut self.text_backdrop, "Backdrop");
                            if bg_resp.changed() {
                                if let Some(ed) = &mut self.text_editor {
                                    ed.refocus = true;
                                }
                                if let Some(sel_id) = self.selected_id
                                    && let Some(ann) =
                                        self.annotations.iter_mut().find(|a| a.id == sel_id)
                                    && let AnnotationKind::Text { backdrop, .. } = &mut ann.kind
                                {
                                    *backdrop = self.text_backdrop;
                                    self.history.record(before);
                                }
                            }
                        }
                        Tool::Pixelate => {
                            let before = self.snapshot();
                            let blur_resp = ui.checkbox(&mut self.pixelate_blur, "Smooth Blur");
                            if blur_resp.changed()
                                && let Some(sel_id) = self.selected_id
                                && let Some(ann) =
                                    self.annotations.iter_mut().find(|a| a.id == sel_id)
                                && let AnnotationKind::Pixelate { blur, .. } = &mut ann.kind
                            {
                                *blur = self.pixelate_blur;
                                self.history.record(before.clone());
                            }
                            ui.label(if self.pixelate_blur { "Radius:" } else { "Block:" });
                            let size_resp = ui.add(
                                egui::Slider::new(&mut self.pixelate_size, 4..=48).suffix("px"),
                            );
                            if size_resp.changed()
                                && let Some(sel_id) = self.selected_id
                                && self.annotations.iter().any(|a| {
                                    a.id == sel_id
                                        && matches!(a.kind, AnnotationKind::Pixelate { .. })
                                })
                            {
                                self.record_slider_undo(before, &size_resp);
                                if let Some(ann) =
                                    self.annotations.iter_mut().find(|a| a.id == sel_id)
                                    && let AnnotationKind::Pixelate { block_size, .. } =
                                        &mut ann.kind
                                {
                                    *block_size = self.pixelate_size;
                                }
                            }
                        }
                        Tool::Crop => {
                            if self.crop.is_some() && ui.button("Reset Crop").clicked() {
                                self.record_undo();
                                self.crop = None;
                            }
                        }
                        Tool::Select => {
                            self.draw_selected_annotation_modifiers(ui);
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

    fn draw_selected_annotation_modifiers(&mut self, ui: &mut egui::Ui) {
        let Some(sel_id) = self.selected_id else {
            ui.label(
                egui::RichText::new("Click an annotation to edit")
                    .size(12.0)
                    .color(Color32::from_rgb(148, 163, 184)),
            );
            return;
        };

        let Some(current_shape) = self
            .annotations
            .iter()
            .find(|a| a.id == sel_id)
            .and_then(Annotation::shape_kind)
        else {
            self.draw_selected_text_modifiers(ui, sel_id);
            return;
        };

        // Shape converter for geometric annotations (Arrow <-> Rectangle <-> Ellipse <-> Pixelate)
        let mut chosen_shape = current_shape;
        ui.label("Shape:");
        egui::ComboBox::from_id_salt("selected_shape_kind")
            .width(92.0)
            .selected_text(chosen_shape.label())
            .show_ui(ui, |ui| {
                for candidate in ShapeKind::ALL {
                    ui.selectable_value(&mut chosen_shape, candidate, candidate.label());
                }
            });
        if chosen_shape != current_shape {
            let before = self.snapshot();
            let def_col = self.active_color;
            let def_stroke = self.stroke_width;
            let def_fill = self.shape_filled;
            let def_block = self.pixelate_size;
            let def_blur = self.pixelate_blur;
            if let Some(ann) = self.annotations.iter_mut().find(|a| a.id == sel_id)
                && ann.convert_shape(
                    chosen_shape,
                    def_col,
                    def_stroke,
                    def_fill,
                    def_block,
                    def_blur,
                )
            {
                self.history.record(before);
            }
        }

        let before = self.snapshot();
        let Some(kind_snapshot) = self
            .annotations
            .iter()
            .find(|a| a.id == sel_id)
            .map(|a| a.kind.clone())
        else {
            return;
        };

        match kind_snapshot {
            AnnotationKind::Arrow {
                mut stroke_width, ..
            } => {
                ui.label("Width:");
                let resp = ui.add(
                    egui::Slider::new(&mut stroke_width, 1.0..=20.0)
                        .step_by(1.0)
                        .suffix("px"),
                );
                if resp.changed() {
                    self.record_slider_undo(before, &resp);
                    self.stroke_width = stroke_width;
                    if let Some(ann) = self.annotations.iter_mut().find(|a| a.id == sel_id)
                        && let AnnotationKind::Arrow {
                            stroke_width: sw, ..
                        } = &mut ann.kind
                    {
                        *sw = stroke_width;
                    }
                }
            }
            AnnotationKind::Rectangle {
                mut stroke_width,
                mut filled,
                ..
            }
            | AnnotationKind::Ellipse {
                mut stroke_width,
                mut filled,
                ..
            } => {
                if ui.checkbox(&mut filled, "Fill").changed() {
                    self.shape_filled = filled;
                    if let Some(ann) = self.annotations.iter_mut().find(|a| a.id == sel_id)
                        && let AnnotationKind::Rectangle { filled: f, .. }
                        | AnnotationKind::Ellipse { filled: f, .. } = &mut ann.kind
                    {
                        *f = filled;
                    }
                    self.history.record(before.clone());
                }
                if !filled {
                    ui.label("Width:");
                    let resp = ui.add(
                        egui::Slider::new(&mut stroke_width, 1.0..=20.0)
                            .step_by(1.0)
                            .suffix("px"),
                    );
                    if resp.changed() {
                        self.record_slider_undo(before, &resp);
                        self.stroke_width = stroke_width;
                        if let Some(ann) = self.annotations.iter_mut().find(|a| a.id == sel_id)
                            && let AnnotationKind::Rectangle {
                                stroke_width: sw, ..
                            }
                            | AnnotationKind::Ellipse {
                                stroke_width: sw, ..
                            } = &mut ann.kind
                        {
                            *sw = stroke_width;
                        }
                    }
                }
            }
            AnnotationKind::Pixelate {
                mut block_size,
                mut blur,
                ..
            } => {
                if ui.checkbox(&mut blur, "Smooth Blur").changed() {
                    self.pixelate_blur = blur;
                    if let Some(ann) = self.annotations.iter_mut().find(|a| a.id == sel_id)
                        && let AnnotationKind::Pixelate { blur: b, .. } = &mut ann.kind
                    {
                        *b = blur;
                    }
                    self.history.record(before.clone());
                }
                ui.label(if blur { "Radius:" } else { "Block:" });
                let resp = ui.add(egui::Slider::new(&mut block_size, 4..=48).suffix("px"));
                if resp.changed() {
                    self.record_slider_undo(before, &resp);
                    self.pixelate_size = block_size;
                    if let Some(ann) = self.annotations.iter_mut().find(|a| a.id == sel_id)
                        && let AnnotationKind::Pixelate {
                            block_size: bs, ..
                        } = &mut ann.kind
                    {
                        *bs = block_size;
                    }
                }
            }
            AnnotationKind::Text { .. } => {}
        }

        if ui
            .button("Delete")
            .on_hover_text("Delete selected annotation (Del)")
            .clicked()
        {
            self.selected_id = None;
            self.record_undo();
            self.annotations.retain(|a| a.id != sel_id);
        }
    }

    fn draw_selected_text_modifiers(&mut self, ui: &mut egui::Ui, sel_id: u64) {
        let before = self.snapshot();
        let Some((mut font_size, mut backdrop)) = self
            .annotations
            .iter()
            .find(|a| a.id == sel_id)
            .and_then(|a| match a.kind {
                AnnotationKind::Text {
                    font_size,
                    backdrop,
                    ..
                } => Some((font_size, backdrop)),
                _ => None,
            })
        else {
            return;
        };

        ui.label("Size:");
        let size_resp = ui.add(
            egui::Slider::new(&mut font_size, 12.0..=96.0)
                .step_by(1.0)
                .suffix("px"),
        );
        if size_resp.changed() {
            self.record_slider_undo(before.clone(), &size_resp);
            self.font_size = font_size;
            if let Some(ann) = self.annotations.iter_mut().find(|a| a.id == sel_id)
                && let AnnotationKind::Text {
                    font_size: fs, ..
                } = &mut ann.kind
            {
                *fs = font_size;
            }
        }

        if ui.checkbox(&mut backdrop, "Backdrop").changed() {
            self.text_backdrop = backdrop;
            if let Some(ann) = self.annotations.iter_mut().find(|a| a.id == sel_id)
                && let AnnotationKind::Text { backdrop: bg, .. } = &mut ann.kind
            {
                *bg = backdrop;
            }
            self.history.record(before);
        }

        if ui
            .button("Edit")
            .on_hover_text("Edit text content (or double-click on canvas)")
            .clicked()
        {
            self.open_text_editor_for(sel_id);
        }

        if ui
            .button("Delete")
            .on_hover_text("Delete selected annotation (Del)")
            .clicked()
        {
            self.selected_id = None;
            self.record_undo();
            self.annotations.retain(|a| a.id != sel_id);
        }
    }

    fn draw_bottom_bar(&mut self, root_ui: &mut egui::Ui) {
        egui::Panel::bottom("screenote_statusbar")
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(20, 22, 27))
                    .inner_margin(egui::Margin::symmetric(12, 4))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(36, 40, 50))),
            )
            .show(root_ui, |ui| {
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

    fn draw_canvas(&mut self, root_ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::from_rgb(15, 17, 22)))
            .show(root_ui, |ui| {
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

                if response.dragged_by(egui::PointerButton::Middle)
                    || (space_held && response.dragged_by(egui::PointerButton::Primary))
                {
                    self.pan += response.drag_delta();
                    self.fit_to_view = false;
                    ctx.set_cursor_icon(CursorIcon::Grabbing);
                } else if space_held && response.hovered() {
                    ctx.set_cursor_icon(CursorIcon::Grab);
                } else if response.hovered() {
                    let cursor = match self.active_tool {
                        Tool::Select => self.select_hover_cursor(ctx, response.hover_pos(), &to_image),
                        Tool::Text => CursorIcon::Text,
                        _ => CursorIcon::Crosshair,
                    };
                    ctx.set_cursor_icon(cursor);
                }

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

                // Selection outline & resize handles (hugs actual bounds tightly)
                if let Some(sel_id) = self.selected_id
                    && self
                        .text_editor
                        .as_ref()
                        .is_none_or(|ed| ed.editing_id != Some(sel_id))
                    && let Some(sel_ann) = self.annotations.iter().find(|a| a.id == sel_id)
                {
                    let b = self.annotation_bounds(ctx, sel_ann);
                    let sel_rect = Rect::from_min_max(
                        to_screen(Point::new(b.min_x, b.min_y)),
                        to_screen(Point::new(b.max_x, b.max_y)),
                    );
                    let corner_radius = match &sel_ann.kind {
                        AnnotationKind::Text {
                            font_size,
                            backdrop: true,
                            ..
                        } => {
                            let scaled_font = (*font_size * self.zoom).max(6.0);
                            CornerRadius::same((scaled_font * 0.25).clamp(2.0, 8.0) as u8)
                        }
                        _ => CornerRadius::same(2),
                    };
                    painter.rect_stroke(
                        sel_rect,
                        corner_radius,
                        Stroke::new(1.2_f32, Color32::from_rgb(96, 165, 250)),
                        StrokeKind::Outside,
                    );
                    for handle in sel_ann.handles_with_bounds(b) {
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
                self.show_inline_text_editor(ctx, canvas_rect, &to_screen);
            });
    }

    fn select_hover_cursor(
        &self,
        ctx: &egui::Context,
        hover_pos: Option<Pos2>,
        to_image: &dyn Fn(Pos2) -> Point,
    ) -> CursorIcon {
        match &self.drag_op {
            Some(DragOperation::MoveAnnotation { .. }) => return CursorIcon::Grabbing,
            Some(
                DragOperation::ResizeShape { handle_index, .. }
                | DragOperation::ResizeText { handle_index, .. },
            ) => {
                return if handle_index % 2 == 0 {
                    CursorIcon::ResizeNwSe
                } else {
                    CursorIcon::ResizeNeSw
                };
            }
            _ => {}
        }

        let Some(pos) = hover_pos else {
            return CursorIcon::Default;
        };
        let img_pt = to_image(pos);
        let hit_tol = (8.0 / self.zoom).max(4.0);

        if let Some(sel_id) = self.selected_id
            && let Some(sel_ann) = self.annotations.iter().find(|a| a.id == sel_id)
        {
            let b = self.annotation_bounds(ctx, sel_ann);
            if let Some((idx, _)) = sel_ann
                .handles_with_bounds(b)
                .iter()
                .enumerate()
                .find(|(_, h)| h.distance_to(img_pt) <= hit_tol * 1.4)
            {
                return if matches!(sel_ann.kind, AnnotationKind::Arrow { .. }) {
                    CursorIcon::Crosshair
                } else if idx % 2 == 0 {
                    CursorIcon::ResizeNwSe
                } else {
                    CursorIcon::ResizeNeSw
                };
            }
        }

        let any_hit = self.annotations.iter().rev().any(|ann| {
            let b = self.annotation_bounds(ctx, ann);
            ann.hit_test_with_bounds(img_pt, hit_tol, b)
        });
        if any_hit {
            CursorIcon::Grab
        } else {
            CursorIcon::Default
        }
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
            let hit_text_id = self
                .annotations
                .iter()
                .rev()
                .find(|a| {
                    matches!(a.kind, AnnotationKind::Text { .. })
                        && a.hit_test_with_bounds(img_pt, hit_tol, self.annotation_bounds(ctx, a))
                })
                .map(|a| a.id);
            if let Some(id) = hit_text_id {
                self.open_text_editor_for(id);
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
                // If clicking an existing text annotation, select it so its Size/Backdrop/Color can be tweaked;
                // clicking empty space starts a new text annotation.
                let hit_existing_text = self
                    .annotations
                    .iter()
                    .rev()
                    .find(|a| {
                        matches!(a.kind, AnnotationKind::Text { .. })
                            && a.hit_test_with_bounds(pt, hit_tol, self.annotation_bounds(ctx, a))
                    })
                    .map(|a| a.id);
                if let Some(existing_id) = hit_existing_text {
                    self.select_annotation(Some(existing_id));
                    if let Some(ann) = self.annotations.iter().find(|a| a.id == existing_id)
                        && let AnnotationKind::Text {
                            font_size,
                            backdrop,
                            ..
                        } = ann.kind
                    {
                        self.font_size = font_size;
                        self.text_backdrop = backdrop;
                    }
                } else {
                    self.selected_id = None;
                    self.text_editor = Some(ActiveTextEditor {
                        pos: pt,
                        text: String::new(),
                        editing_id: None,
                        just_opened: true,
                        refocus: false,
                    });
                }
            }
            return;
        }

        if response.clicked_by(egui::PointerButton::Primary)
            && self.active_tool == Tool::Select
            && let Some(pos) = response.interact_pointer_pos()
        {
            let img_pt = to_image(pos);
            let found = self
                .annotations
                .iter()
                .rev()
                .find(|a| a.hit_test_with_bounds(img_pt, hit_tol, self.annotation_bounds(ctx, a)))
                .map(|a| a.id);
            self.select_annotation(found);
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
                        && let Some(sel_ann) =
                            self.annotations.iter().find(|a| a.id == sel_id).cloned()
                    {
                        let b = self.annotation_bounds(ctx, &sel_ann);
                        let handles = sel_ann.handles_with_bounds(b);
                        if let Some((idx, handle_pt)) = handles
                            .iter()
                            .enumerate()
                            .find(|(_, h)| h.distance_to(img_pt) <= hit_tol * 1.4)
                        {
                            if let AnnotationKind::Text { font_size, .. } = sel_ann.kind {
                                let anchor = handles[(idx + 2) % 4];
                                self.drag_op = Some(DragOperation::ResizeText {
                                    id: sel_id,
                                    handle_index: idx,
                                    anchor,
                                    initial_diag: anchor.distance_to(*handle_pt).max(8.0),
                                    initial_font_size: font_size,
                                    recorded_undo: false,
                                });
                            } else {
                                let anchor = if matches!(sel_ann.kind, AnnotationKind::Arrow { .. })
                                {
                                    handles[1 - idx]
                                } else {
                                    handles[(idx + 2) % 4]
                                };
                                self.drag_op = Some(DragOperation::ResizeShape {
                                    id: sel_id,
                                    handle_index: idx,
                                    anchor,
                                    recorded_undo: false,
                                });
                            }
                            return;
                        }
                    }

                    // Otherwise hit-test annotations top-to-bottom
                    let hit_id = self
                        .annotations
                        .iter()
                        .rev()
                        .find(|a| {
                            a.hit_test_with_bounds(img_pt, hit_tol, self.annotation_bounds(ctx, a))
                        })
                        .map(|a| a.id);
                    if let Some(id) = hit_id {
                        self.select_annotation(Some(id));
                        self.drag_op = Some(DragOperation::MoveAnnotation {
                            id,
                            last_pos: img_pt,
                            recorded_undo: false,
                        });
                    } else {
                        self.select_annotation(None);
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
            let active_op = self.drag_op.clone();
            match active_op {
                Some(DragOperation::Create { tool, start, .. }) => {
                    let next_pt = if shift_held {
                        constrain_point(start, raw_pt, tool)
                    } else {
                        raw_pt
                    };
                    if let Some(DragOperation::Create { current, .. }) = &mut self.drag_op {
                        *current = next_pt;
                    }
                }
                Some(DragOperation::MoveAnnotation {
                    id,
                    last_pos,
                    recorded_undo,
                }) => {
                    let dx = raw_pt.x - last_pos.x;
                    let dy = raw_pt.y - last_pos.y;
                    if dx.abs() > 0.01 || dy.abs() > 0.01 {
                        if !recorded_undo {
                            self.history.record(before);
                        }
                        if let Some(ann) = self.annotations.iter_mut().find(|a| a.id == id) {
                            ann.translate(dx, dy);
                        }
                        if let Some(DragOperation::MoveAnnotation {
                            last_pos: lp,
                            recorded_undo: ru,
                            ..
                        }) = &mut self.drag_op
                        {
                            *lp = raw_pt;
                            *ru = true;
                        }
                    }
                }
                Some(DragOperation::ResizeShape {
                    id,
                    handle_index,
                    anchor,
                    recorded_undo,
                }) => {
                    if !recorded_undo {
                        self.history.record(before);
                        if let Some(DragOperation::ResizeShape {
                            recorded_undo: ru, ..
                        }) = &mut self.drag_op
                        {
                            *ru = true;
                        }
                    }
                    if let Some(ann) = self.annotations.iter_mut().find(|a| a.id == id) {
                        let tool_kind = ann
                            .shape_kind()
                            .map_or(Tool::Rectangle, ShapeKind::corresponding_tool);
                        let target_pt = if shift_held {
                            constrain_point(anchor, raw_pt, tool_kind)
                        } else {
                            raw_pt
                        };
                        match &mut ann.kind {
                            AnnotationKind::Arrow { start, end, .. } => {
                                if handle_index == 0 {
                                    *start = target_pt;
                                    *end = anchor;
                                } else {
                                    *start = anchor;
                                    *end = target_pt;
                                }
                            }
                            AnnotationKind::Rectangle { start, end, .. }
                            | AnnotationKind::Ellipse { start, end, .. }
                            | AnnotationKind::Pixelate { start, end, .. } => {
                                *start = anchor;
                                *end = target_pt;
                            }
                            AnnotationKind::Text { .. } => {}
                        }
                    }
                }
                Some(DragOperation::ResizeText {
                    id,
                    handle_index,
                    anchor,
                    initial_diag,
                    initial_font_size,
                    recorded_undo,
                }) => {
                    let new_diag = anchor.distance_to(raw_pt).max(4.0);
                    let scale = new_diag / initial_diag.max(4.0);
                    let new_size = (initial_font_size * scale).clamp(10.0, 144.0).round();

                    if let Some((text_str, backdrop)) = self
                        .annotations
                        .iter()
                        .find(|a| a.id == id)
                        .and_then(|a| match &a.kind {
                            AnnotationKind::Text { text, backdrop, .. } => {
                                Some((text.clone(), *backdrop))
                            }
                            _ => None,
                        })
                    {
                        if !recorded_undo {
                            self.history.record(before);
                            if let Some(DragOperation::ResizeText {
                                recorded_undo: ru, ..
                            }) = &mut self.drag_op
                            {
                                *ru = true;
                            }
                        }

                        let (tw, th) = self.measure_text_image_size(ctx, &text_str, new_size);
                        let (pad_x, pad_y) = if backdrop {
                            text_backdrop_padding(new_size)
                        } else {
                            (2.0, 2.0)
                        };
                        let box_w = tw + pad_x * 2.0;
                        let box_h = th + pad_y * 2.0;
                        let (min_x, min_y) = match handle_index {
                            0 => (anchor.x - box_w, anchor.y - box_h),
                            1 => (anchor.x, anchor.y - box_h),
                            2 => (anchor.x, anchor.y),
                            _ => (anchor.x - box_w, anchor.y),
                        };
                        let new_pos = Point::new(min_x + pad_x, min_y + pad_y);
                        self.font_size = new_size;
                        if let Some(ann) = self.annotations.iter_mut().find(|a| a.id == id)
                            && let AnnotationKind::Text {
                                pos, font_size: fs, ..
                            } = &mut ann.kind
                        {
                            *pos = new_pos;
                            *fs = new_size;
                        }
                    }
                }
                None => {}
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
                    let (pad_x, pad_y) = text_backdrop_padding(*font_size);
                    let pad = Vec2::new(pad_x * self.zoom, pad_y * self.zoom);
                    let (bg, border) = text_backdrop_colors(*color);
                    let bg_rect = Rect::from_min_size(screen_pos - pad, galley.size() + pad * 2.0);
                    let corner = CornerRadius::same((scaled_font * 0.25).clamp(2.0, 8.0) as u8);
                    painter.rect_filled(bg_rect, corner, bg);
                    painter.rect_stroke(
                        bg_rect,
                        corner,
                        Stroke::new(1.0_f32, border),
                        StrokeKind::Inside,
                    );
                }
                painter.galley(screen_pos, galley, to_egui_color(*color));
            }
        }
    }

    fn show_inline_text_editor(
        &mut self,
        ctx: &egui::Context,
        canvas_rect: Rect,
        to_screen: &dyn Fn(Point) -> Pos2,
    ) {
        let Some(mut editor) = self.text_editor.take() else {
            return;
        };

        let anchor = to_screen(editor.pos);
        let scaled_font = (self.font_size * self.zoom).clamp(10.0, 96.0);
        let (pad_x, pad_y) = if self.text_backdrop {
            let (px, py) = text_backdrop_padding(self.font_size);
            ((px * self.zoom).max(4.0), (py * self.zoom).max(3.0))
        } else {
            (4.0, 3.0)
        };
        let frame_pos = anchor - Vec2::new(pad_x, pad_y);

        let sample_text = if editor.text.is_empty() {
            "Type…"
        } else {
            editor.text.as_str()
        };
        let measured_w = ctx
            .layer_painter(egui::LayerId::background())
            .layout_no_wrap(
                sample_text.to_string(),
                FontId::new(scaled_font, FontFamily::Proportional),
                Color32::WHITE,
            )
            .size()
            .x;
        let desired_text_w = (measured_w + 10.0).max(52.0);

        let frame_fill = if self.text_backdrop {
            text_backdrop_colors(self.active_color).0
        } else {
            Color32::TRANSPARENT
        };

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

        let area_resp = egui::Area::new(egui::Id::unique("screenote_inline_text"))
            .fixed_pos(frame_pos)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(0.0, 4.0);
                egui::Frame::new()
                    .fill(frame_fill)
                    .corner_radius(CornerRadius::same(
                        (scaled_font * 0.25).clamp(2.0, 8.0) as u8,
                    ))
                    .inner_margin(egui::Margin {
                        left: pad_x.round() as i8,
                        right: pad_x.round() as i8,
                        top: pad_y.round() as i8,
                        bottom: pad_y.round() as i8,
                    })
                    .stroke(Stroke::new(1.4_f32, Color32::from_rgb(59, 130, 246)))
                    .show(ui, |ui| {
                        let edit = egui::TextEdit::multiline(&mut editor.text)
                            .frame(egui::Frame::NONE)
                            .margin(egui::Margin::ZERO)
                            .font(FontId::new(scaled_font, FontFamily::Proportional))
                            .text_color(to_egui_color(self.active_color))
                            .desired_rows(1)
                            .desired_width(desired_text_w)
                            .hint_text("Type…");
                        let resp = ui.add(edit);
                        if editor.just_opened || editor.refocus {
                            resp.request_focus();
                            editor.just_opened = false;
                            editor.refocus = false;
                        }
                    });

                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(15, 18, 26, 215))
                    .corner_radius(CornerRadius::same(4))
                    .inner_margin(egui::Margin::symmetric(6, 2))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new("Enter: apply · Shift+Enter: newline · Esc: cancel")
                                .size(10.0)
                                .color(Color32::from_rgb(160, 174, 192)),
                        );
                    });
            });

        if cancel {
            return;
        }
        let clicked_outside_on_canvas = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer.interact_pos().is_some_and(|p| {
                    canvas_rect.contains(p) && !area_resp.response.rect.contains(p)
                })
        });
        if commit || clicked_outside_on_canvas {
            self.text_editor = Some(editor);
            self.commit_text_editor();
        } else {
            self.text_editor = Some(editor);
        }
    }
}

impl eframe::App for ScreenoteApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_shortcuts(&ctx);
        self.draw_top_toolbar(ui, &ctx);
        self.draw_bottom_bar(ui);
        self.draw_canvas(ui, &ctx);
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

fn text_backdrop_colors(c: ColorRgba) -> (Color32, Color32) {
    let lum =
        (0.2126 * f32::from(c.r) + 0.7152 * f32::from(c.g) + 0.0722 * f32::from(c.b)) / 255.0;
    if lum < 0.35 {
        (
            Color32::from_rgba_unmultiplied(248, 250, 252, 235),
            Color32::from_rgba_unmultiplied(15, 23, 42, 65),
        )
    } else {
        (
            Color32::from_rgba_unmultiplied(12, 16, 26, 232),
            Color32::from_rgba_unmultiplied(255, 255, 255, 58),
        )
    }
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
