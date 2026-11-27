use std::io::Cursor;

use anyhow::{Context, Result};
use cosmic_text::{Attrs, Buffer, Color as CosmicColor, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};
use image::{ImageFormat, Rgba, RgbaImage};
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Rect,
    Stroke, Transform,
};

use crate::annotation::{
    Annotation, AnnotationKind, ColorRgba, Point, RectBounds, text_backdrop_padding,
};

#[derive(Debug, Clone, Copy)]
pub struct ArrowGeometry {
    pub shaft_end: Point,
    pub tip: Point,
    pub left_wing: Point,
    pub neck: Point,
    pub right_wing: Point,
}

impl ArrowGeometry {
    pub fn compute(start: Point, end: Point, stroke_width: f32) -> Option<Self> {
        let dx = end.x - start.x;
        let dy = end.y - start.y;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1.0 {
            return None;
        }

        let ux = dx / len;
        let uy = dy / len;
        let nx = -uy;
        let ny = ux;

        let head_len = (stroke_width * 4.2).max(14.0).min(len * 0.85);
        let half_width = head_len * 0.44;
        let neck_dist = head_len * 0.76;

        let base_x = end.x - ux * head_len;
        let base_y = end.y - uy * head_len;

        let neck = Point::new(end.x - ux * neck_dist, end.y - uy * neck_dist);
        let left_wing = Point::new(base_x + nx * half_width, base_y + ny * half_width);
        let right_wing = Point::new(base_x - nx * half_width, base_y - ny * half_width);

        Some(Self {
            shaft_end: neck,
            tip: end,
            left_wing,
            neck,
            right_wing,
        })
    }
}

/// Sample mosaic blocks from `img` for live preview on the interactive UI canvas.
pub fn sample_pixelate_blocks(
    img: &RgbaImage,
    bounds: RectBounds,
    block_size: u32,
) -> Vec<(RectBounds, ColorRgba)> {
    let img_w = img.width();
    let img_h = img.height();
    if img_w == 0 || img_h == 0 {
        return Vec::new();
    }

    let clamped = bounds.clamp_to_image(img_w as f32, img_h as f32);
    let x0 = clamped.min_x.floor() as u32;
    let y0 = clamped.min_y.floor() as u32;
    let x1 = clamped.max_x.ceil().min(img_w as f32) as u32;
    let y1 = clamped.max_y.ceil().min(img_h as f32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return Vec::new();
    }

    let block = block_size.max(4);
    let sample_step = (block / 4).max(1);
    let cols = (x1 - x0).div_ceil(block) as usize;
    let rows = (y1 - y0).div_ceil(block) as usize;
    let mut out = Vec::with_capacity(cols * rows);

    let mut by = y0;
    while by < y1 {
        let bh = block.min(y1 - by);
        let mut bx = x0;
        while bx < x1 {
            let bw = block.min(x1 - bx);
            let (mut r_sum, mut g_sum, mut b_sum, mut count) = (0u64, 0u64, 0u64, 0u64);

            let mut sy = by;
            while sy < by + bh {
                let mut sx = bx;
                while sx < bx + bw {
                    let p = img.get_pixel(sx, sy);
                    r_sum += u64::from(p[0]);
                    g_sum += u64::from(p[1]);
                    b_sum += u64::from(p[2]);
                    count += 1;
                    sx += sample_step;
                }
                sy += sample_step;
            }

            if let (Some(avg_r), Some(avg_g), Some(avg_b)) = (
                r_sum.checked_div(count),
                g_sum.checked_div(count),
                b_sum.checked_div(count),
            ) {
                let color = ColorRgba::rgb(avg_r as u8, avg_g as u8, avg_b as u8);
                out.push((
                    RectBounds {
                        min_x: bx as f32,
                        min_y: by as f32,
                        max_x: (bx + bw) as f32,
                        max_y: (by + bh) as f32,
                    },
                    color,
                ));
            }
            bx += block;
        }
        by += block;
    }

    out
}

pub struct Renderer {
    font_system: FontSystem,
    swash_cache: SwashCache,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
        }
    }

    /// Render the base screenshot with all annotations in z-order at 100% native resolution,
    /// applying any active crop at the end.
    pub fn render(
        &mut self,
        base_image: &RgbaImage,
        annotations: &[Annotation],
        crop: Option<RectBounds>,
    ) -> Result<RgbaImage> {
        let width = base_image.width();
        let height = base_image.height();
        let mut pixmap =
            Pixmap::new(width, height).context("Invalid image dimensions for Pixmap")?;

        // Copy straight RGBA pixels into premultiplied Pixmap buffer
        for (dst, src) in pixmap.pixels_mut().iter_mut().zip(base_image.pixels()) {
            *dst = PremultipliedColorU8::from_rgba(src[0], src[1], src[2], src[3])
                .unwrap_or(PremultipliedColorU8::TRANSPARENT);
        }

        for ann in annotations {
            self.draw_annotation(&mut pixmap, &ann.kind);
        }

        // Convert premultiplied Pixmap back to straight RgbaImage
        let mut full_img = RgbaImage::new(width, height);
        for (x, y, dst) in full_img.enumerate_pixels_mut() {
            let p = pixmap.pixels()[(y * width + x) as usize].demultiply();
            *dst = Rgba([p.red(), p.green(), p.blue(), p.alpha()]);
        }

        if let Some(crop_rect) = crop {
            let clamped = crop_rect.clamp_to_image(width as f32, height as f32);
            let cx = clamped.min_x.round() as u32;
            let cy = clamped.min_y.round() as u32;
            let cw = (clamped.max_x.round() as u32)
                .saturating_sub(cx)
                .min(width.saturating_sub(cx));
            let ch = (clamped.max_y.round() as u32)
                .saturating_sub(cy)
                .min(height.saturating_sub(cy));
            if cw >= 1 && ch >= 1 {
                return Ok(image::imageops::crop_imm(&full_img, cx, cy, cw, ch).to_image());
            }
        }

        Ok(full_img)
    }

    pub fn render_png_bytes(
        &mut self,
        base_image: &RgbaImage,
        annotations: &[Annotation],
        crop: Option<RectBounds>,
    ) -> Result<Vec<u8>> {
        let rendered = self.render(base_image, annotations, crop)?;
        let mut bytes = Vec::new();
        rendered
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .context("Failed encoding rendered image as PNG")?;
        Ok(bytes)
    }

    fn draw_annotation(&mut self, pixmap: &mut Pixmap, kind: &AnnotationKind) {
        match kind {
            AnnotationKind::Arrow {
                start,
                end,
                color,
                stroke_width,
            } => {
                if let Some(geom) = ArrowGeometry::compute(*start, *end, *stroke_width) {
                    if start.distance_to(geom.shaft_end) > 0.5 {
                        draw_line_segment(
                            pixmap,
                            *start,
                            geom.shaft_end,
                            *color,
                            *stroke_width,
                            LineCap::Round,
                        );
                    }
                    let mut pb = PathBuilder::new();
                    pb.move_to(geom.tip.x, geom.tip.y);
                    pb.line_to(geom.left_wing.x, geom.left_wing.y);
                    pb.line_to(geom.neck.x, geom.neck.y);
                    pb.line_to(geom.right_wing.x, geom.right_wing.y);
                    pb.close();
                    if let Some(path) = pb.finish() {
                        let paint = make_paint(*color);
                        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                    }
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
                if let Some(rect) = Rect::from_ltrb(b.min_x, b.min_y, b.max_x, b.max_y) {
                    let path = PathBuilder::from_rect(rect);
                    let paint = make_paint(*color);
                    if *filled {
                        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                    } else {
                        let stroke = make_stroke(*stroke_width, LineCap::Square, LineJoin::Miter);
                        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
                    }
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
                if let Some(oval) = Rect::from_ltrb(b.min_x, b.min_y, b.max_x, b.max_y)
                    && let Some(path) = PathBuilder::from_oval(oval)
                {
                    let paint = make_paint(*color);
                    if *filled {
                        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                    } else {
                        let stroke = make_stroke(*stroke_width, LineCap::Round, LineJoin::Round);
                        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
                    }
                }
            }
            AnnotationKind::Pixelate {
                start,
                end,
                block_size,
                blur,
            } => {
                let bounds = RectBounds::from_points(*start, *end);
                if *blur {
                    blur_pixmap_in_place(pixmap, bounds, *block_size);
                } else {
                    pixelate_pixmap_in_place(pixmap, bounds, *block_size);
                }
            }
            AnnotationKind::Text {
                pos,
                text,
                color,
                font_size,
                backdrop,
            } => {
                self.draw_text(pixmap, *pos, text, *color, *font_size, *backdrop);
            }
        }
    }

    fn draw_text(
        &mut self,
        pixmap: &mut Pixmap,
        pos: Point,
        text: &str,
        color: ColorRgba,
        font_size: f32,
        backdrop: bool,
    ) {
        if text.trim().is_empty() {
            return;
        }

        let size = font_size.max(8.0);
        let line_height = (size * 1.28).ceil();
        let metrics = Metrics::new(size, line_height);

        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        {
            let mut borrowed = buffer.borrow_with(&mut self.font_system);
            borrowed.set_size(None, None);
            let attrs = Attrs::new()
                .family(Family::SansSerif)
                .weight(Weight::SEMIBOLD);
            borrowed.set_text(text, &attrs, Shaping::Advanced, None);
            borrowed.shape_until_scroll(true);
        }

        // Measure actual shaped bounds for optional backdrop pill
        let mut max_w = 0.0f32;
        let mut max_y = 0.0f32;
        for run in buffer.layout_runs() {
            max_w = max_w.max(run.line_w);
            max_y = max_y.max(run.line_top + run.line_height);
        }

        if backdrop && max_w > 0.0 && max_y > 0.0 {
            let (pad_x, pad_y) = text_backdrop_padding(size);
            let is_dark_text = luminance(color) < 0.35;
            let bg_color = if is_dark_text {
                ColorRgba::rgba(248, 250, 252, 235)
            } else {
                ColorRgba::rgba(12, 16, 26, 232)
            };
            let border_color = if is_dark_text {
                ColorRgba::rgba(15, 23, 42, 65)
            } else {
                ColorRgba::rgba(255, 255, 255, 58)
            };
            if let Some(bg_rect) = Rect::from_ltrb(
                pos.x - pad_x,
                pos.y - pad_y,
                pos.x + max_w + pad_x,
                pos.y + max_y + pad_y,
            ) {
                let path = rounded_rect_path(bg_rect, (size * 0.25).clamp(4.0, 10.0));
                let paint = make_paint(bg_color);
                pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                let border_paint = make_paint(border_color);
                let border_stroke = make_stroke(1.0, LineCap::Round, LineJoin::Round);
                pixmap.stroke_path(
                    &path,
                    &border_paint,
                    &border_stroke,
                    Transform::identity(),
                    None,
                );
            }
        }

        let pm_w = pixmap.width() as i32;
        let pm_h = pixmap.height() as i32;
        let base_x = pos.x.round() as i32;
        let base_y = pos.y.round() as i32;
        let cosmic_col = CosmicColor::rgba(color.r, color.g, color.b, color.a);
        let pixels = pixmap.pixels_mut();

        buffer.draw(
            &mut self.font_system,
            &mut self.swash_cache,
            cosmic_col,
            |gx, gy, _w, _h, c| {
                let px = base_x + gx;
                let py = base_y + gy;
                if px < 0 || py < 0 || px >= pm_w || py >= pm_h {
                    return;
                }
                let sa = c.a();
                if sa == 0 {
                    return;
                }
                let idx = (py as usize) * (pm_w as usize) + (px as usize);
                blend_source_over(&mut pixels[idx], c.r(), c.g(), c.b(), sa);
            },
        );
    }
}

fn draw_line_segment(
    pixmap: &mut Pixmap,
    start: Point,
    end: Point,
    color: ColorRgba,
    stroke_width: f32,
    cap: LineCap,
) {
    let mut pb = PathBuilder::new();
    pb.move_to(start.x, start.y);
    pb.line_to(end.x, end.y);
    if let Some(path) = pb.finish() {
        let paint = make_paint(color);
        let stroke = make_stroke(stroke_width, cap, LineJoin::Round);
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

fn make_paint(c: ColorRgba) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(c.r, c.g, c.b, c.a));
    paint.anti_alias = true;
    paint
}

fn make_stroke(width: f32, line_cap: LineCap, line_join: LineJoin) -> Stroke {
    Stroke {
        width: width.max(1.0),
        line_cap,
        line_join,
        ..Stroke::default()
    }
}

fn rounded_rect_path(rect: Rect, radius: f32) -> tiny_skia::Path {
    let r = radius.min(rect.width() * 0.5).min(rect.height() * 0.5);
    if r <= 0.5 {
        return PathBuilder::from_rect(rect);
    }
    let l = rect.left();
    let t = rect.top();
    let ri = rect.right();
    let b = rect.bottom();

    let mut pb = PathBuilder::new();
    pb.move_to(l + r, t);
    pb.line_to(ri - r, t);
    pb.quad_to(ri, t, ri, t + r);
    pb.line_to(ri, b - r);
    pb.quad_to(ri, b, ri - r, b);
    pb.line_to(l + r, b);
    pb.quad_to(l, b, l, b - r);
    pb.line_to(l, t + r);
    pb.quad_to(l, t, l + r, t);
    pb.close();
    pb.finish().unwrap_or_else(|| PathBuilder::from_rect(rect))
}

fn luminance(c: ColorRgba) -> f32 {
    (0.2126 * f32::from(c.r) + 0.7152 * f32::from(c.g) + 0.0722 * f32::from(c.b)) / 255.0
}

fn blend_source_over(dst: &mut PremultipliedColorU8, sr: u8, sg: u8, sb: u8, sa: u8) {
    let sa_u32 = u32::from(sa);
    let inv_sa = 255 - sa_u32;

    let pr = (u32::from(sr) * sa_u32 + 127) / 255;
    let pg = (u32::from(sg) * sa_u32 + 127) / 255;
    let pb = (u32::from(sb) * sa_u32 + 127) / 255;

    let out_r = (pr + (u32::from(dst.red()) * inv_sa + 127) / 255).min(255) as u8;
    let out_g = (pg + (u32::from(dst.green()) * inv_sa + 127) / 255).min(255) as u8;
    let out_b = (pb + (u32::from(dst.blue()) * inv_sa + 127) / 255).min(255) as u8;
    let out_a = (sa_u32 + (u32::from(dst.alpha()) * inv_sa + 127) / 255).min(255) as u8;

    if let Some(pm) = PremultipliedColorU8::from_rgba(
        out_r.min(out_a),
        out_g.min(out_a),
        out_b.min(out_a),
        out_a,
    ) {
        *dst = pm;
    }
}

fn pixelate_pixmap_in_place(pixmap: &mut Pixmap, bounds: RectBounds, block_size: u32) {
    let w = pixmap.width();
    let h = pixmap.height();
    let clamped = bounds.clamp_to_image(w as f32, h as f32);
    let x0 = clamped.min_x.floor() as u32;
    let y0 = clamped.min_y.floor() as u32;
    let x1 = clamped.max_x.ceil().min(w as f32) as u32;
    let y1 = clamped.max_y.ceil().min(h as f32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return;
    }

    let block = block_size.max(4);
    let pixels = pixmap.pixels_mut();

    let mut by = y0;
    while by < y1 {
        let bh = block.min(y1 - by);
        let mut bx = x0;
        while bx < x1 {
            let bw = block.min(x1 - bx);
            let (mut r_sum, mut g_sum, mut b_sum, mut a_sum, mut count) =
                (0u64, 0u64, 0u64, 0u64, 0u64);

            for y in by..(by + bh) {
                let row_off = (y * w) as usize;
                for x in bx..(bx + bw) {
                    let p = pixels[row_off + (x as usize)];
                    r_sum += u64::from(p.red());
                    g_sum += u64::from(p.green());
                    b_sum += u64::from(p.blue());
                    a_sum += u64::from(p.alpha());
                    count += 1;
                }
            }

            if let (Some(ra), Some(rr), Some(rg), Some(rb)) = (
                a_sum.checked_div(count),
                r_sum.checked_div(count),
                g_sum.checked_div(count),
                b_sum.checked_div(count),
            ) {
                let avg_a = ra as u8;
                let avg_r = (rr as u8).min(avg_a);
                let avg_g = (rg as u8).min(avg_a);
                let avg_b = (rb as u8).min(avg_a);
                if let Some(avg_px) = PremultipliedColorU8::from_rgba(avg_r, avg_g, avg_b, avg_a) {
                    for y in by..(by + bh) {
                        let row_off = (y * w) as usize;
                        for x in bx..(bx + bw) {
                            pixels[row_off + (x as usize)] = avg_px;
                        }
                    }
                }
            }

            bx += block;
        }
        by += block;
    }
}

fn blur_pixmap_in_place(pixmap: &mut Pixmap, bounds: RectBounds, strength: u32) {
    let img_w = pixmap.width();
    let img_h = pixmap.height();
    let clamped = bounds.clamp_to_image(img_w as f32, img_h as f32);
    let x0 = clamped.min_x.floor() as u32;
    let y0 = clamped.min_y.floor() as u32;
    let x1 = clamped.max_x.ceil().min(img_w as f32) as u32;
    let y1 = clamped.max_y.ceil().min(img_h as f32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return;
    }

    let rw = (x1 - x0) as usize;
    let rh = (y1 - y0) as usize;
    let radius = (strength as usize / 2).clamp(2, 64);

    let pixels = pixmap.pixels_mut();
    let mut buf = vec![[0u8; 4]; rw * rh];
    let mut tmp = vec![[0u8; 4]; rw * rh];

    for ry in 0..rh {
        let row_off = ((y0 as usize + ry) * img_w as usize) + x0 as usize;
        for rx in 0..rw {
            let p = pixels[row_off + rx];
            buf[ry * rw + rx] = [p.red(), p.green(), p.blue(), p.alpha()];
        }
    }

    // 3-pass separable box blur approximates a smooth Gaussian blur in O(W*H)
    for _ in 0..3 {
        // Horizontal pass: buf -> tmp
        for ry in 0..rh {
            let row = ry * rw;
            for rx in 0..rw {
                let sx0 = rx.saturating_sub(radius);
                let sx1 = (rx + radius).min(rw - 1);
                let count = (sx1 - sx0 + 1) as u32;
                let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
                for sx in sx0..=sx1 {
                    let c = buf[row + sx];
                    r += u32::from(c[0]);
                    g += u32::from(c[1]);
                    b += u32::from(c[2]);
                    a += u32::from(c[3]);
                }
                tmp[row + rx] = [
                    (r / count) as u8,
                    (g / count) as u8,
                    (b / count) as u8,
                    (a / count) as u8,
                ];
            }
        }
        // Vertical pass: tmp -> buf
        for rx in 0..rw {
            for ry in 0..rh {
                let sy0 = ry.saturating_sub(radius);
                let sy1 = (ry + radius).min(rh - 1);
                let count = (sy1 - sy0 + 1) as u32;
                let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
                for sy in sy0..=sy1 {
                    let c = tmp[sy * rw + rx];
                    r += u32::from(c[0]);
                    g += u32::from(c[1]);
                    b += u32::from(c[2]);
                    a += u32::from(c[3]);
                }
                buf[ry * rw + rx] = [
                    (r / count) as u8,
                    (g / count) as u8,
                    (b / count) as u8,
                    (a / count) as u8,
                ];
            }
        }
    }

    for ry in 0..rh {
        let row_off = ((y0 as usize + ry) * img_w as usize) + x0 as usize;
        for rx in 0..rw {
            let [r, g, b, a] = buf[ry * rw + rx];
            if let Some(pm) = PremultipliedColorU8::from_rgba(r.min(a), g.min(a), b.min(a), a) {
                pixels[row_off + rx] = pm;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixelate_and_blur_obscure_regions_accurately() {
        let mut img = RgbaImage::new(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                let val = if x < 8 { 0 } else { 200 };
                img.put_pixel(x, y, Rgba([val, val, val, 255]));
            }
        }

        let mut renderer = Renderer::new();
        let pix_ann = Annotation {
            id: 1,
            kind: AnnotationKind::Pixelate {
                start: Point::new(0.0, 0.0),
                end: Point::new(16.0, 16.0),
                block_size: 16,
                blur: false,
            },
        };

        let out = renderer.render(&img, &[pix_ann], None).expect("render succeeds");
        assert_eq!(out.get_pixel(1, 1), &Rgba([100, 100, 100, 255]));
        assert_eq!(out.get_pixel(14, 14), &Rgba([100, 100, 100, 255]));

        let blur_ann = Annotation {
            id: 2,
            kind: AnnotationKind::Pixelate {
                start: Point::new(0.0, 0.0),
                end: Point::new(16.0, 16.0),
                block_size: 12,
                blur: true,
            },
        };
        let blurred = renderer
            .render(&img, &[blur_ann], None)
            .expect("blur render succeeds");
        let mid = blurred.get_pixel(8, 8)[0];
        assert!(mid > 40 && mid < 160, "expected smooth transition at boundary, got {mid}");
    }

    #[test]
    fn crop_and_shapes_render_to_valid_png() {
        let img = RgbaImage::from_pixel(200, 120, Rgba([240, 240, 245, 255]));
        let annotations = vec![
            Annotation {
                id: 1,
                kind: AnnotationKind::Rectangle {
                    start: Point::new(20.0, 20.0),
                    end: Point::new(100.0, 80.0),
                    color: ColorRgba::rgb(239, 68, 68),
                    stroke_width: 4.0,
                    filled: false,
                },
            },
            Annotation {
                id: 2,
                kind: AnnotationKind::Arrow {
                    start: Point::new(30.0, 30.0),
                    end: Point::new(90.0, 70.0),
                    color: ColorRgba::rgb(59, 130, 246),
                    stroke_width: 4.0,
                },
            },
            Annotation {
                id: 3,
                kind: AnnotationKind::Ellipse {
                    start: Point::new(40.0, 30.0),
                    end: Point::new(80.0, 70.0),
                    color: ColorRgba::rgb(34, 197, 94),
                    stroke_width: 3.0,
                    filled: true,
                },
            },
            Annotation {
                id: 4,
                kind: AnnotationKind::Text {
                    pos: Point::new(20.0, 68.0),
                    text: "Hello Wayland".to_string(),
                    color: ColorRgba::rgb(255, 255, 255),
                    font_size: 14.0,
                    backdrop: true,
                },
            },
        ];

        let crop = Some(RectBounds {
            min_x: 10.0,
            min_y: 10.0,
            max_x: 110.0,
            max_y: 90.0,
        });

        let mut renderer = Renderer::new();
        let png_bytes = renderer
            .render_png_bytes(&img, &annotations, crop)
            .expect("png render succeeds");
        let decoded = image::load_from_memory(&png_bytes)
            .expect("valid PNG")
            .into_rgba8();
        assert_eq!(decoded.dimensions(), (100, 80));
        // Upper center of filled green ellipse (at 60,45 in base -> 50,35 in cropped) should be green
        assert_eq!(decoded.get_pixel(50, 35), &Rgba([34, 197, 94, 255]));
    }

    #[test]
    fn text_backdrop_toggle_changes_rendered_background() {
        let img = RgbaImage::from_pixel(160, 80, Rgba([180, 180, 190, 255]));
        let mut renderer = Renderer::new();

        let without_backdrop = Annotation {
            id: 1,
            kind: AnnotationKind::Text {
                pos: Point::new(30.0, 25.0),
                text: "Backdrop test".to_string(),
                color: ColorRgba::rgb(236, 72, 153),
                font_size: 20.0,
                backdrop: false,
            },
        };
        let mut with_backdrop = without_backdrop.clone();
        if let AnnotationKind::Text { backdrop, .. } = &mut with_backdrop.kind {
            *backdrop = true;
        }

        let out_plain = renderer
            .render(&img, &[without_backdrop], None)
            .expect("plain text render succeeds");
        let out_pill = renderer
            .render(&img, &[with_backdrop], None)
            .expect("backdrop text render succeeds");

        // Sample inside the left padding region (x = 26, y = 32): untouched when backdrop=false, darkened when backdrop=true
        assert_eq!(out_plain.get_pixel(26, 32), &Rgba([180, 180, 190, 255]));
        assert!(
            out_pill.get_pixel(26, 32)[0] < 60,
            "expected dark pill padding pixel when backdrop=true, got {:?}",
            out_pill.get_pixel(26, 32)
        );
    }
}
