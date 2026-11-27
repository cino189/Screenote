#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn distance_to(self, other: Self) -> f32 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectBounds {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

impl RectBounds {
    pub fn from_points(a: Point, b: Point) -> Self {
        Self {
            min_x: a.x.min(b.x),
            min_y: a.y.min(b.y),
            max_x: a.x.max(b.x),
            max_y: a.y.max(b.y),
        }
    }

    pub fn width(&self) -> f32 {
        (self.max_x - self.min_x).max(0.0)
    }

    pub fn height(&self) -> f32 {
        (self.max_y - self.min_y).max(0.0)
    }

    pub fn contains(&self, pt: Point, pad: f32) -> bool {
        pt.x >= self.min_x - pad
            && pt.x <= self.max_x + pad
            && pt.y >= self.min_y - pad
            && pt.y <= self.max_y + pad
    }

    pub fn clamp_to_image(&self, img_w: f32, img_h: f32) -> Self {
        let min_x = self.min_x.clamp(0.0, img_w);
        let min_y = self.min_y.clamp(0.0, img_h);
        let max_x = self.max_x.clamp(min_x, img_w);
        let max_y = self.max_y.clamp(min_y, img_h);
        Self {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorRgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl ColorRgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

pub const PALETTE: &[(&str, ColorRgba)] = &[
    ("Red", ColorRgba::rgb(239, 68, 68)),
    ("Orange", ColorRgba::rgb(249, 115, 22)),
    ("Yellow", ColorRgba::rgb(234, 179, 8)),
    ("Green", ColorRgba::rgb(34, 197, 94)),
    ("Cyan", ColorRgba::rgb(6, 182, 212)),
    ("Blue", ColorRgba::rgb(59, 130, 246)),
    ("Magenta", ColorRgba::rgb(236, 72, 153)),
    ("White", ColorRgba::rgb(255, 255, 255)),
    ("Black", ColorRgba::rgb(24, 24, 27)),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Arrow,
    Rectangle,
    Ellipse,
    Text,
    Pixelate,
    Crop,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Arrow => "Arrow",
            Self::Rectangle => "Rect",
            Self::Ellipse => "Ellipse",
            Self::Text => "Text",
            Self::Pixelate => "Pixelate",
            Self::Crop => "Crop",
        }
    }

    pub fn shortcut_hint(self) -> &'static str {
        match self {
            Self::Select => "V",
            Self::Arrow => "A",
            Self::Rectangle => "R",
            Self::Ellipse => "E",
            Self::Text => "T",
            Self::Pixelate => "P",
            Self::Crop => "C",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeKind {
    Arrow,
    Rectangle,
    Ellipse,
    Pixelate,
}

impl ShapeKind {
    pub const ALL: [Self; 4] = [
        Self::Arrow,
        Self::Rectangle,
        Self::Ellipse,
        Self::Pixelate,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Arrow => "Arrow",
            Self::Rectangle => "Rectangle",
            Self::Ellipse => "Ellipse",
            Self::Pixelate => "Pixelate",
        }
    }

    pub fn corresponding_tool(self) -> Tool {
        match self {
            Self::Arrow => Tool::Arrow,
            Self::Rectangle => Tool::Rectangle,
            Self::Ellipse => Tool::Ellipse,
            Self::Pixelate => Tool::Pixelate,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub id: u64,
    pub kind: AnnotationKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AnnotationKind {
    Arrow {
        start: Point,
        end: Point,
        color: ColorRgba,
        stroke_width: f32,
    },
    Rectangle {
        start: Point,
        end: Point,
        color: ColorRgba,
        stroke_width: f32,
        filled: bool,
    },
    Ellipse {
        start: Point,
        end: Point,
        color: ColorRgba,
        stroke_width: f32,
        filled: bool,
    },
    Text {
        pos: Point,
        text: String,
        color: ColorRgba,
        font_size: f32,
        backdrop: bool,
    },
    Pixelate {
        start: Point,
        end: Point,
        block_size: u32,
        blur: bool,
    },
}

pub fn text_backdrop_padding(font_size: f32) -> (f32, f32) {
    let size = font_size.max(8.0);
    ((size * 0.35).max(6.0), (size * 0.22).max(4.0))
}

impl Annotation {
    pub fn bounds_with_text_size(&self, measured_text: Option<(f32, f32)>) -> RectBounds {
        match &self.kind {
            AnnotationKind::Arrow { start, end, .. }
            | AnnotationKind::Rectangle { start, end, .. }
            | AnnotationKind::Ellipse { start, end, .. }
            | AnnotationKind::Pixelate { start, end, .. } => RectBounds::from_points(*start, *end),
            AnnotationKind::Text {
                pos,
                text,
                font_size,
                backdrop,
                ..
            } => {
                let size = font_size.max(8.0);
                let (text_w, text_h) = measured_text.unwrap_or_else(|| {
                    let lines: Vec<&str> = text.lines().collect();
                    let line_count = lines.len().max(1) as f32;
                    let max_chars =
                        lines.iter().map(|l| l.chars().count()).max().unwrap_or(1) as f32;
                    let w = (max_chars * (size * 0.48)).max(size * 0.5);
                    let h = line_count * (size * 1.22);
                    (w, h)
                });
                if *backdrop {
                    let (pad_x, pad_y) = text_backdrop_padding(size);
                    RectBounds {
                        min_x: pos.x - pad_x,
                        min_y: pos.y - pad_y,
                        max_x: pos.x + text_w + pad_x,
                        max_y: pos.y + text_h + pad_y,
                    }
                } else {
                    RectBounds {
                        min_x: pos.x - 2.0,
                        min_y: pos.y - 2.0,
                        max_x: pos.x + text_w + 2.0,
                        max_y: pos.y + text_h + 2.0,
                    }
                }
            }
        }
    }

    pub fn hit_test_with_bounds(
        &self,
        pt: Point,
        base_tolerance: f32,
        bounds: RectBounds,
    ) -> bool {
        match &self.kind {
            AnnotationKind::Arrow {
                start,
                end,
                stroke_width,
                ..
            } => {
                let tol = base_tolerance.max(*stroke_width * 0.75 + 4.0);
                point_to_segment_distance(pt, *start, *end) <= tol
            }
            AnnotationKind::Rectangle {
                start,
                end,
                stroke_width,
                filled,
                ..
            } => {
                let rect = RectBounds::from_points(*start, *end);
                let tol = base_tolerance.max(*stroke_width * 0.75 + 4.0);
                if *filled {
                    rect.contains(pt, tol * 0.5)
                } else {
                    if !rect.contains(pt, tol) {
                        return false;
                    }
                    let inner = RectBounds {
                        min_x: rect.min_x + tol,
                        min_y: rect.min_y + tol,
                        max_x: rect.max_x - tol,
                        max_y: rect.max_y - tol,
                    };
                    if inner.max_x > inner.min_x && inner.max_y > inner.min_y {
                        !inner.contains(pt, 0.0)
                    } else {
                        true
                    }
                }
            }
            AnnotationKind::Ellipse {
                start,
                end,
                stroke_width,
                filled,
                ..
            } => {
                let rect = RectBounds::from_points(*start, *end);
                let tol = base_tolerance.max(*stroke_width * 0.75 + 4.0);
                let rx = rect.width() * 0.5;
                let ry = rect.height() * 0.5;
                if rx < 1.0 || ry < 1.0 {
                    return rect.contains(pt, tol);
                }
                let cx = rect.min_x + rx;
                let cy = rect.min_y + ry;
                let nx = (pt.x - cx) / rx;
                let ny = (pt.y - cy) / ry;
                let norm_dist = (nx * nx + ny * ny).sqrt();
                if *filled {
                    norm_dist <= 1.0 + (tol / rx.min(ry))
                } else {
                    let band = tol / rx.min(ry);
                    (norm_dist - 1.0).abs() <= band
                }
            }
            AnnotationKind::Pixelate { .. } | AnnotationKind::Text { .. } => {
                bounds.contains(pt, base_tolerance * 0.5)
            }
        }
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        match &mut self.kind {
            AnnotationKind::Arrow { start, end, .. }
            | AnnotationKind::Rectangle { start, end, .. }
            | AnnotationKind::Ellipse { start, end, .. }
            | AnnotationKind::Pixelate { start, end, .. } => {
                start.x += dx;
                start.y += dy;
                end.x += dx;
                end.y += dy;
            }
            AnnotationKind::Text { pos, .. } => {
                pos.x += dx;
                pos.y += dy;
            }
        }
    }

    pub fn handles_with_bounds(&self, bounds: RectBounds) -> Vec<Point> {
        match &self.kind {
            AnnotationKind::Arrow { start, end, .. } => {
                vec![*start, *end]
            }
            AnnotationKind::Rectangle { .. }
            | AnnotationKind::Ellipse { .. }
            | AnnotationKind::Pixelate { .. }
            | AnnotationKind::Text { .. } => {
                vec![
                    Point::new(bounds.min_x, bounds.min_y),
                    Point::new(bounds.max_x, bounds.min_y),
                    Point::new(bounds.max_x, bounds.max_y),
                    Point::new(bounds.min_x, bounds.max_y),
                ]
            }
        }
    }

    pub fn color(&self) -> Option<ColorRgba> {
        match &self.kind {
            AnnotationKind::Arrow { color, .. }
            | AnnotationKind::Rectangle { color, .. }
            | AnnotationKind::Ellipse { color, .. }
            | AnnotationKind::Text { color, .. } => Some(*color),
            AnnotationKind::Pixelate { .. } => None,
        }
    }

    pub fn set_color(&mut self, new_color: ColorRgba) -> bool {
        match &mut self.kind {
            AnnotationKind::Arrow { color, .. }
            | AnnotationKind::Rectangle { color, .. }
            | AnnotationKind::Ellipse { color, .. }
            | AnnotationKind::Text { color, .. } => {
                if *color != new_color {
                    *color = new_color;
                    true
                } else {
                    false
                }
            }
            AnnotationKind::Pixelate { .. } => false,
        }
    }

    pub fn shape_kind(&self) -> Option<ShapeKind> {
        match &self.kind {
            AnnotationKind::Arrow { .. } => Some(ShapeKind::Arrow),
            AnnotationKind::Rectangle { .. } => Some(ShapeKind::Rectangle),
            AnnotationKind::Ellipse { .. } => Some(ShapeKind::Ellipse),
            AnnotationKind::Pixelate { .. } => Some(ShapeKind::Pixelate),
            AnnotationKind::Text { .. } => None,
        }
    }

    pub fn convert_shape(
        &mut self,
        target: ShapeKind,
        default_color: ColorRgba,
        default_stroke: f32,
        default_filled: bool,
        default_block_size: u32,
        default_blur: bool,
    ) -> bool {
        if self.shape_kind() == Some(target) {
            return false;
        }
        let (start, end, cur_color, cur_stroke, cur_filled) = match &self.kind {
            AnnotationKind::Arrow {
                start,
                end,
                color,
                stroke_width,
            } => (*start, *end, *color, *stroke_width, default_filled),
            AnnotationKind::Rectangle {
                start,
                end,
                color,
                stroke_width,
                filled,
            }
            | AnnotationKind::Ellipse {
                start,
                end,
                color,
                stroke_width,
                filled,
            } => (*start, *end, *color, *stroke_width, *filled),
            AnnotationKind::Pixelate { start, end, .. } => (
                *start,
                *end,
                default_color,
                default_stroke,
                default_filled,
            ),
            AnnotationKind::Text { .. } => return false,
        };

        self.kind = match target {
            ShapeKind::Arrow => AnnotationKind::Arrow {
                start,
                end,
                color: cur_color,
                stroke_width: cur_stroke,
            },
            ShapeKind::Rectangle => AnnotationKind::Rectangle {
                start,
                end,
                color: cur_color,
                stroke_width: cur_stroke,
                filled: cur_filled,
            },
            ShapeKind::Ellipse => AnnotationKind::Ellipse {
                start,
                end,
                color: cur_color,
                stroke_width: cur_stroke,
                filled: cur_filled,
            },
            ShapeKind::Pixelate => AnnotationKind::Pixelate {
                start,
                end,
                block_size: default_block_size,
                blur: default_blur,
            },
        };
        true
    }
}

pub fn constrain_point(start: Point, raw_end: Point, tool: Tool) -> Point {
    let dx = raw_end.x - start.x;
    let dy = raw_end.y - start.y;
    match tool {
        Tool::Arrow => {
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-3 {
                return raw_end;
            }
            let angle = dy.atan2(dx);
            let step = std::f32::consts::FRAC_PI_4;
            let snapped = (angle / step).round() * step;
            Point::new(start.x + len * snapped.cos(), start.y + len * snapped.sin())
        }
        Tool::Rectangle | Tool::Ellipse | Tool::Pixelate | Tool::Crop => {
            let side = dx.abs().max(dy.abs());
            Point::new(start.x + side * dx.signum(), start.y + side * dy.signum())
        }
        _ => raw_end,
    }
}

fn point_to_segment_distance(p: Point, a: Point, b: Point) -> f32 {
    let vx = b.x - a.x;
    let vy = b.y - a.y;
    let len_sq = vx * vx + vy * vy;
    if len_sq < 1e-6 {
        return p.distance_to(a);
    }
    let t = (((p.x - a.x) * vx + (p.y - a.y) * vy) / len_sq).clamp(0.0, 1.0);
    let proj = Point::new(a.x + t * vx, a.y + t * vy);
    p.distance_to(proj)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DocumentSnapshot {
    pub annotations: Vec<Annotation>,
    pub crop: Option<RectBounds>,
}

#[derive(Debug, Default)]
pub struct History {
    undo_stack: Vec<DocumentSnapshot>,
    redo_stack: Vec<DocumentSnapshot>,
}

impl History {
    pub fn record(&mut self, snapshot: DocumentSnapshot) {
        self.undo_stack.push(snapshot);
        if self.undo_stack.len() > 100 {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub fn undo(&mut self, current: DocumentSnapshot) -> Option<DocumentSnapshot> {
        let prev = self.undo_stack.pop()?;
        self.redo_stack.push(current);
        Some(prev)
    }

    pub fn redo(&mut self, current: DocumentSnapshot) -> Option<DocumentSnapshot> {
        let next = self.redo_stack.pop()?;
        self.undo_stack.push(current);
        Some(next)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_constrain_snaps_angles_and_squares() {
        let start = Point::new(100.0, 100.0);
        // Slightly off horizontal -> snaps to 0 degrees (y == 100.0)
        let snapped_line = constrain_point(start, Point::new(200.0, 108.0), Tool::Arrow);
        assert!((snapped_line.y - 100.0).abs() < 1e-3);

        // Rectangular drag -> constrained to equal width & height
        let snapped_rect = constrain_point(start, Point::new(160.0, 130.0), Tool::Rectangle);
        assert_eq!(snapped_rect, Point::new(160.0, 160.0));
    }

    #[test]
    fn hit_test_and_undo_redo_work_as_expected() {
        let rect_ann = Annotation {
            id: 1,
            kind: AnnotationKind::Rectangle {
                start: Point::new(10.0, 10.0),
                end: Point::new(110.0, 90.0),
                color: PALETTE[0].1,
                stroke_width: 4.0,
                filled: false,
            },
        };
        // Edge hit succeeds, center of unfilled rectangle does not hit
        let b = rect_ann.bounds_with_text_size(None);
        assert!(rect_ann.hit_test_with_bounds(Point::new(10.0, 50.0), 4.0, b));
        assert!(!rect_ann.hit_test_with_bounds(Point::new(60.0, 50.0), 4.0, b));

        let mut history = History::default();
        let empty = DocumentSnapshot::default();
        history.record(empty.clone());

        let with_one = DocumentSnapshot {
            annotations: vec![rect_ann],
            crop: None,
        };

        let undone = history.undo(with_one.clone()).expect("undo available");
        assert_eq!(undone, empty);
        let redone = history.redo(undone).expect("redo available");
        assert_eq!(redone, with_one);
    }

    #[test]
    fn text_bounds_and_shape_conversion_work_accurately() {
        let mut ann = Annotation {
            id: 7,
            kind: AnnotationKind::Text {
                pos: Point::new(50.0, 40.0),
                text: "larger text".to_string(),
                color: PALETTE[6].1,
                font_size: 24.0,
                backdrop: true,
            },
        };
        let (pad_x, pad_y) = text_backdrop_padding(24.0);
        let with_bg = ann.bounds_with_text_size(Some((110.0, 28.0)));
        assert!((with_bg.min_x - (50.0 - pad_x)).abs() < 1e-4);
        assert!((with_bg.max_x - (50.0 + 110.0 + pad_x)).abs() < 1e-4);
        assert!((with_bg.min_y - (40.0 - pad_y)).abs() < 1e-4);
        assert!((with_bg.max_y - (40.0 + 28.0 + pad_y)).abs() < 1e-4);
        assert_eq!(ann.handles_with_bounds(with_bg).len(), 4);

        if let AnnotationKind::Text { backdrop, .. } = &mut ann.kind {
            *backdrop = false;
        }
        let no_bg = ann.bounds_with_text_size(Some((110.0, 28.0)));
        assert!(no_bg.width() < with_bg.width());

        let mut shape = Annotation {
            id: 8,
            kind: AnnotationKind::Rectangle {
                start: Point::new(10.0, 20.0),
                end: Point::new(90.0, 80.0),
                color: PALETTE[0].1,
                stroke_width: 5.0,
                filled: true,
            },
        };
        assert!(shape.convert_shape(
            ShapeKind::Ellipse,
            PALETTE[1].1,
            4.0,
            false,
            14,
            false
        ));
        assert_eq!(
            shape.kind,
            AnnotationKind::Ellipse {
                start: Point::new(10.0, 20.0),
                end: Point::new(90.0, 80.0),
                color: PALETTE[0].1,
                stroke_width: 5.0,
                filled: true,
            }
        );
    }
}
