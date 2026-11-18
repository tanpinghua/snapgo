//! Annotation model: shapes, hit-testing and rendering.
//!
//! Everything here works in *image pixel* coordinates (the physical pixels of
//! the captured screenshot), so the exported PNG is always pixel-exact no
//! matter what the display scale is.

use gtk4::cairo::{self, Context, Format, ImageSurface};
use gtk4::pango;
use std::cell::RefCell;
use std::collections::HashMap;
use std::f64::consts::PI;

/// Blur strength (Gaussian sigma, image pixels) per pixel of item width, so
/// the blur tool follows the size setting like every other tool.
pub const BLUR_PER_WIDTH: f64 = 2.0;

pub type Color = (f64, f64, f64);

pub const PALETTE: [Color; 6] = [
    (0.90, 0.14, 0.14), // red
    (1.00, 0.75, 0.00), // yellow
    (0.18, 0.76, 0.34), // green
    (0.11, 0.53, 0.93), // blue
    (1.00, 1.00, 1.00), // white
    (0.08, 0.08, 0.08), // black
];

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn dist(self, o: Pt) -> f64 {
        (self.x - o.x).hypot(self.y - o.y)
    }
    pub fn offset(self, dx: f64, dy: f64) -> Pt {
        Pt::new(self.x + dx, self.y + dy)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }
    pub fn from_points(a: Pt, b: Pt) -> Self {
        Self {
            x: a.x.min(b.x),
            y: a.y.min(b.y),
            w: (a.x - b.x).abs(),
            h: (a.y - b.y).abs(),
        }
    }
    pub fn x2(&self) -> f64 {
        self.x + self.w
    }
    pub fn y2(&self) -> f64 {
        self.y + self.h
    }
    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.x && p.x <= self.x2() && p.y >= self.y && p.y <= self.y2()
    }
    pub fn grow(&self, d: f64) -> Rect {
        Rect::new(self.x - d, self.y - d, self.w + 2.0 * d, self.h + 2.0 * d)
    }
    /// Snap to whole pixels, never smaller than 1×1.
    pub fn round(&self) -> Rect {
        let (x, y) = (self.x.round(), self.y.round());
        Rect::new(
            x,
            y,
            (self.x2().round() - x).max(1.0),
            (self.y2().round() - y).max(1.0),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Pen,
    Marker,
    Text,
    Number,
    Blur,
}

impl Tool {
    pub const ALL: [Tool; 10] = [
        Tool::Select,
        Tool::Arrow,
        Tool::Line,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Pen,
        Tool::Marker,
        Tool::Text,
        Tool::Number,
        Tool::Blur,
    ];

    /// (tooltip, hotkey)
    pub fn info(self) -> (&'static str, char) {
        match self {
            Tool::Select => ("Select / move", 'v'),
            Tool::Arrow => ("Arrow", 'a'),
            Tool::Line => ("Line", 'l'),
            Tool::Rect => ("Rectangle", 'r'),
            Tool::Ellipse => ("Ellipse", 'e'),
            Tool::Pen => ("Pen", 'p'),
            Tool::Marker => ("Highlighter", 'h'),
            Tool::Text => ("Text", 't'),
            Tool::Number => ("Numbered marker", 'n'),
            Tool::Blur => ("Blur", 'b'),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Arrow { a: Pt, b: Pt },
    Line { a: Pt, b: Pt },
    Rect { a: Pt, b: Pt },
    Ellipse { a: Pt, b: Pt },
    Pen { pts: Vec<Pt> },
    Marker { pts: Vec<Pt> },
    Text { pos: Pt, text: String },
    Number { pos: Pt, n: u32 },
    Blur { a: Pt, b: Pt },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub shape: Shape,
    pub color: Color,
    /// Stroke width in image pixels; text and number sizes derive from it.
    pub width: f64,
}

pub fn font_px(width: f64) -> f64 {
    width * 6.0
}

fn num_radius(width: f64) -> f64 {
    width * 4.0
}

fn seg_dist(p: Pt, a: Pt, b: Pt) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-9 {
        return p.dist(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    p.dist(Pt::new(a.x + t * dx, a.y + t * dy))
}

fn text_layout(cr: &Context, text: &str, px: f64) -> pango::Layout {
    let layout = pangocairo::functions::create_layout(cr);
    let mut fd = pango::FontDescription::new();
    fd.set_family("Sans");
    fd.set_weight(pango::Weight::Bold);
    fd.set_absolute_size(px * pango::SCALE as f64);
    layout.set_font_description(Some(&fd));
    layout.set_text(text);
    layout
}

/// Pixel size of `text` rendered at `px`.
pub fn text_extent(text: &str, px: f64) -> (f64, f64) {
    let Ok(surf) = ImageSurface::create(Format::ARgb32, 1, 1) else {
        return (0.0, 0.0);
    };
    let Ok(cr) = Context::new(&surf) else {
        return (0.0, 0.0);
    };
    let (w, h) = text_layout(&cr, text, px).pixel_size();
    (w as f64, h as f64)
}

pub fn draw_arrow(cr: &Context, a: Pt, b: Pt, w: f64) {
    let len = a.dist(b);
    if len < 1.0 {
        return;
    }
    let ang = (b.y - a.y).atan2(b.x - a.x);
    let head = (w * 4.5).max(8.0).min(len);
    let spread = 0.48;
    cr.move_to(a.x, a.y);
    cr.line_to(b.x - head * 0.7 * ang.cos(), b.y - head * 0.7 * ang.sin());
    let _ = cr.stroke();
    cr.move_to(b.x, b.y);
    cr.line_to(
        b.x - head * (ang - spread).cos(),
        b.y - head * (ang - spread).sin(),
    );
    cr.line_to(
        b.x - head * (ang + spread).cos(),
        b.y - head * (ang + spread).sin(),
    );
    cr.close_path();
    let _ = cr.fill();
}

fn path(cr: &Context, pts: &[Pt]) {
    if let Some(first) = pts.first() {
        cr.move_to(first.x, first.y);
        // A lone point still needs a segment so the round cap paints a dot.
        cr.line_to(first.x, first.y);
        for p in &pts[1..] {
            cr.line_to(p.x, p.y);
        }
    }
}

impl Item {
    /// Draw onto `cr` (image coordinates). `blur` supplies blurred copies of
    /// the screenshot for the blur tool.
    pub fn draw(&self, cr: &Context, blur: &BlurCache) {
        let _ = cr.save();
        let (r, g, b) = self.color;
        cr.set_source_rgb(r, g, b);
        cr.set_line_width(self.width);
        cr.set_line_cap(cairo::LineCap::Round);
        cr.set_line_join(cairo::LineJoin::Round);
        match &self.shape {
            Shape::Line { a, b } => {
                cr.move_to(a.x, a.y);
                cr.line_to(b.x, b.y);
                let _ = cr.stroke();
            }
            Shape::Arrow { a, b } => draw_arrow(cr, *a, *b, self.width),
            Shape::Rect { a, b } => {
                let r = Rect::from_points(*a, *b);
                cr.set_line_join(cairo::LineJoin::Miter);
                cr.rectangle(r.x, r.y, r.w, r.h);
                let _ = cr.stroke();
            }
            Shape::Ellipse { a, b } => {
                let r = Rect::from_points(*a, *b);
                if r.w >= 1.0 && r.h >= 1.0 {
                    let _ = cr.save();
                    cr.translate(r.x + r.w / 2.0, r.y + r.h / 2.0);
                    cr.scale(r.w / 2.0, r.h / 2.0);
                    cr.arc(0.0, 0.0, 1.0, 0.0, 2.0 * PI);
                    let _ = cr.restore();
                    let _ = cr.stroke();
                }
            }
            Shape::Pen { pts } => {
                path(cr, pts);
                let _ = cr.stroke();
            }
            Shape::Marker { pts } => {
                cr.set_source_rgba(r, g, b, 0.4);
                cr.set_line_width(self.width * 5.0);
                path(cr, pts);
                let _ = cr.stroke();
            }
            Shape::Text { pos, text } => {
                let layout = text_layout(cr, text, font_px(self.width));
                cr.move_to(pos.x, pos.y);
                pangocairo::functions::show_layout(cr, &layout);
            }
            Shape::Number { pos, n } => {
                let rad = num_radius(self.width);
                cr.arc(pos.x, pos.y, rad, 0.0, 2.0 * PI);
                let _ = cr.fill();
                let luma = 0.299 * r + 0.587 * g + 0.114 * b;
                let fg = if luma > 0.6 { 0.05 } else { 1.0 };
                cr.set_source_rgb(fg, fg, fg);
                let layout = text_layout(cr, &n.to_string(), rad * 1.15);
                let (w, h) = layout.pixel_size();
                cr.move_to(pos.x - w as f64 / 2.0, pos.y - h as f64 / 2.0);
                pangocairo::functions::show_layout(cr, &layout);
            }
            Shape::Blur { a, b } => {
                let r = Rect::from_points(*a, *b);
                cr.rectangle(r.x, r.y, r.w, r.h);
                cr.clip();
                if let Some((soft, scale)) = blur.get(self.width * BLUR_PER_WIDTH) {
                    cr.scale(scale, scale);
                    let _ = cr.set_source_surface(&soft, 0.0, 0.0);
                    cr.source().set_filter(cairo::Filter::Bilinear);
                    cr.source().set_extend(cairo::Extend::Pad);
                    let _ = cr.paint();
                }
            }
        }
        let _ = cr.restore();
    }

    pub fn bounds(&self) -> Rect {
        let half = self.width / 2.0;
        match &self.shape {
            Shape::Arrow { a, b } => Rect::from_points(*a, *b).grow(half * 2.5),
            Shape::Line { a, b } | Shape::Rect { a, b } | Shape::Ellipse { a, b } => {
                Rect::from_points(*a, *b).grow(half)
            }
            Shape::Blur { a, b } => Rect::from_points(*a, *b),
            Shape::Pen { pts } | Shape::Marker { pts } => {
                let grow = if matches!(self.shape, Shape::Marker { .. }) {
                    half * 5.0
                } else {
                    half
                };
                let mut r = pts.first().map_or(Rect::new(0.0, 0.0, 0.0, 0.0), |p| {
                    Rect::new(p.x, p.y, 0.0, 0.0)
                });
                for p in pts {
                    let (x2, y2) = (r.x2().max(p.x), r.y2().max(p.y));
                    r.x = r.x.min(p.x);
                    r.y = r.y.min(p.y);
                    r.w = x2 - r.x;
                    r.h = y2 - r.y;
                }
                r.grow(grow)
            }
            Shape::Text { pos, text } => {
                let (w, h) = text_extent(text, font_px(self.width));
                Rect::new(pos.x, pos.y, w, h)
            }
            Shape::Number { pos, .. } => {
                let rad = num_radius(self.width);
                Rect::new(pos.x - rad, pos.y - rad, rad * 2.0, rad * 2.0)
            }
        }
    }

    /// Is `p` on this item, within `tol` pixels?
    pub fn hit(&self, p: Pt, tol: f64) -> bool {
        let reach = self.width / 2.0 + tol;
        match &self.shape {
            Shape::Arrow { a, b } | Shape::Line { a, b } => seg_dist(p, *a, *b) <= reach,
            Shape::Rect { a, b } => {
                let r = Rect::from_points(*a, *b);
                r.grow(reach).contains(p)
                    && !(r.w > 2.0 * reach && r.h > 2.0 * reach && r.grow(-reach).contains(p))
            }
            Shape::Ellipse { a, b } => {
                let r = Rect::from_points(*a, *b);
                let (rx, ry) = (r.w / 2.0, r.h / 2.0);
                if rx < 1.0 || ry < 1.0 {
                    return r.grow(reach).contains(p);
                }
                let (dx, dy) = (p.x - (r.x + rx), p.y - (r.y + ry));
                let d = ((dx / rx).powi(2) + (dy / ry).powi(2)).sqrt();
                ((d - 1.0) * rx.min(ry)).abs() <= reach
            }
            Shape::Pen { pts } => hit_path(pts, p, reach),
            Shape::Marker { pts } => hit_path(pts, p, self.width * 2.5 + tol),
            Shape::Text { .. } | Shape::Blur { .. } => self.bounds().grow(tol).contains(p),
            Shape::Number { pos, .. } => p.dist(*pos) <= num_radius(self.width) + tol,
        }
    }

    pub fn translated(&self, dx: f64, dy: f64) -> Item {
        let mut it = self.clone();
        match &mut it.shape {
            Shape::Arrow { a, b }
            | Shape::Line { a, b }
            | Shape::Rect { a, b }
            | Shape::Ellipse { a, b }
            | Shape::Blur { a, b } => {
                *a = a.offset(dx, dy);
                *b = b.offset(dx, dy);
            }
            Shape::Pen { pts } | Shape::Marker { pts } => {
                for p in pts.iter_mut() {
                    *p = p.offset(dx, dy);
                }
            }
            Shape::Text { pos, .. } | Shape::Number { pos, .. } => *pos = pos.offset(dx, dy),
        }
        it
    }
}

fn hit_path(pts: &[Pt], p: Pt, reach: f64) -> bool {
    match pts {
        [] => false,
        [only] => p.dist(*only) <= reach,
        _ => pts.windows(2).any(|s| seg_dist(p, s[0], s[1]) <= reach),
    }
}

/// Blurred copies of the screenshot, made on demand and kept per strength.
pub struct BlurCache {
    bg: ImageSurface,
    cache: RefCell<HashMap<u32, Option<(ImageSurface, f64)>>>,
}

impl BlurCache {
    pub fn new(bg: &ImageSurface) -> Self {
        Self {
            bg: bg.clone(),
            cache: RefCell::new(HashMap::new()),
        }
    }

    /// The screenshot blurred with roughly the given Gaussian `sigma` (image
    /// pixels), as a reduced-size surface plus the factor to scale it back up.
    pub fn get(&self, sigma: f64) -> Option<(ImageSurface, f64)> {
        let key = sigma.round().max(1.0) as u32;
        self.cache
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| blurred(&self.bg, key as f64).ok())
            .clone()
    }
}

/// Blur `bg`: shrink it, run three box-blur passes (which approximate a
/// Gaussian) on the small copy, and let the caller scale it back up smoothly.
/// Working on the small copy keeps this fast even for 4K screenshots.
fn blurred(bg: &ImageSurface, sigma: f64) -> Result<(ImageSurface, f64), cairo::Error> {
    // At least 2: quarter the pixels to blur, even for the lightest setting.
    let scale = (sigma / 2.0).round().clamp(2.0, 8.0);
    let w = (bg.width() as f64 / scale).ceil().max(1.0) as i32;
    let h = (bg.height() as f64 / scale).ceil().max(1.0) as i32;
    let mut small = ImageSurface::create(Format::Rgb24, w, h)?;
    {
        let cr = Context::new(&small)?;
        cr.scale(1.0 / scale, 1.0 / scale);
        cr.set_source_surface(bg, 0.0, 0.0)?;
        cr.source().set_filter(cairo::Filter::Good);
        cr.source().set_extend(cairo::Extend::Pad);
        cr.paint()?;
    }
    small.flush();
    // Three passes of a box of width 2r+1 give sigma^2 = r(r+1).
    let s = sigma / scale;
    let radius = ((-1.0 + (1.0 + 4.0 * s * s).sqrt()) / 2.0).round().max(1.0) as usize;
    let stride = small.stride() as usize;
    {
        let mut data = small.data().map_err(|_| cairo::Error::SurfaceFinished)?;
        for _ in 0..3 {
            box_blur(&mut data, w as usize, h as usize, stride, radius);
        }
    }
    small.mark_dirty();
    Ok((small, scale))
}

/// One horizontal and one vertical box-blur pass over 4-byte pixels.
fn box_blur(data: &mut [u8], w: usize, h: usize, stride: usize, r: usize) {
    let mut line = vec![0u8; w.max(h) * 4];
    let mut out = vec![0u8; w.max(h) * 4];
    for y in 0..h {
        let row = &mut data[y * stride..y * stride + w * 4];
        blur_line(row, &mut out[..w * 4], w, r);
        row.copy_from_slice(&out[..w * 4]);
    }
    for x in 0..w {
        for y in 0..h {
            let o = y * stride + x * 4;
            line[y * 4..y * 4 + 4].copy_from_slice(&data[o..o + 4]);
        }
        blur_line(&line[..h * 4], &mut out[..h * 4], h, r);
        for y in 0..h {
            let o = y * stride + x * 4;
            data[o..o + 4].copy_from_slice(&out[y * 4..y * 4 + 4]);
        }
    }
}

/// Sliding-window average of `n` pixels, edges clamped.
fn blur_line(src: &[u8], dst: &mut [u8], n: usize, r: usize) {
    let window = (2 * r + 1) as u32;
    let at = |i: isize, c: usize| src[(i.clamp(0, n as isize - 1) as usize) * 4 + c] as u32;
    for c in 0..4 {
        let mut sum: u32 = (-(r as isize)..=r as isize).map(|i| at(i, c)).sum();
        for x in 0..n {
            dst[x * 4 + c] = (sum / window) as u8;
            let x = x as isize;
            sum = sum + at(x + r as isize + 1, c) - at(x - r as isize, c);
        }
    }
}

/// Render the final image: the `sel` region of `bg` with all items on top.
pub fn render(
    bg: &ImageSurface,
    blur: &BlurCache,
    items: &[Item],
    sel: Rect,
) -> Result<ImageSurface, cairo::Error> {
    let sel = sel.round();
    let out = ImageSurface::create(Format::ARgb32, sel.w as i32, sel.h as i32)?;
    {
        let cr = Context::new(&out)?;
        cr.set_source_rgb(0.0, 0.0, 0.0);
        cr.paint()?;
        cr.translate(-sel.x, -sel.y);
        cr.set_source_surface(bg, 0.0, 0.0)?;
        cr.paint()?;
        for it in items {
            it.draw(&cr, blur);
        }
    }
    out.flush();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(shape: Shape) -> Item {
        Item {
            shape,
            color: PALETTE[0],
            width: 4.0,
        }
    }

    #[test]
    fn rect_normalizes_and_rounds() {
        let r = Rect::from_points(Pt::new(10.4, 20.6), Pt::new(2.0, 3.0));
        assert_eq!(r, Rect::new(2.0, 3.0, 8.4, 17.6));
        assert_eq!(r.round(), Rect::new(2.0, 3.0, 8.0, 18.0));
        assert_eq!(Rect::new(5.2, 5.2, 0.1, 0.1).round().w, 1.0);
    }

    #[test]
    fn hit_testing() {
        let line = item(Shape::Line {
            a: Pt::new(0.0, 0.0),
            b: Pt::new(100.0, 0.0),
        });
        assert!(line.hit(Pt::new(50.0, 3.0), 2.0));
        assert!(!line.hit(Pt::new(50.0, 10.0), 2.0));

        let rect = item(Shape::Rect {
            a: Pt::new(0.0, 0.0),
            b: Pt::new(100.0, 100.0),
        });
        assert!(rect.hit(Pt::new(1.0, 50.0), 2.0), "border hits");
        assert!(!rect.hit(Pt::new(50.0, 50.0), 2.0), "hollow inside");

        let ell = item(Shape::Ellipse {
            a: Pt::new(0.0, 0.0),
            b: Pt::new(100.0, 60.0),
        });
        assert!(ell.hit(Pt::new(50.0, 1.0), 2.0));
        assert!(!ell.hit(Pt::new(50.0, 30.0), 2.0));

        let blur = item(Shape::Blur {
            a: Pt::new(0.0, 0.0),
            b: Pt::new(100.0, 100.0),
        });
        assert!(blur.hit(Pt::new(50.0, 50.0), 2.0), "blur is solid");
    }

    #[test]
    fn translate_moves_every_point() {
        let pen = item(Shape::Pen {
            pts: vec![Pt::new(1.0, 1.0), Pt::new(2.0, 3.0)],
        });
        let moved = pen.translated(10.0, -1.0);
        assert_eq!(
            moved.shape,
            Shape::Pen {
                pts: vec![Pt::new(11.0, 0.0), Pt::new(12.0, 2.0)]
            }
        );
    }

    /// Renders one of each shape. Set SNAPGO_TEST_OUT=/some/file.png to keep
    /// the result for eyeballing.
    #[test]
    fn render_all_shapes() {
        let mut bg = ImageSurface::create(Format::ARgb32, 640, 400).unwrap();
        {
            let cr = Context::new(&bg).unwrap();
            cr.set_source_rgb(0.95, 0.95, 0.97);
            cr.paint().unwrap();
            cr.set_source_rgb(0.1, 0.1, 0.1);
            for i in 0..18 {
                let layout = text_layout(&cr, "secret text 1234 5678 lorem ipsum dolor", 16.0);
                cr.move_to(10.0, 8.0 + i as f64 * 22.0);
                pangocairo::functions::show_layout(&cr, &layout);
            }
        }
        let blur = BlurCache::new(&bg);
        let p = Pt::new;
        let shapes = vec![
            Shape::Blur { a: p(20.0, 60.0), b: p(300.0, 130.0) },
            Shape::Marker { pts: vec![p(20.0, 20.0), p(200.0, 20.0)] },
            Shape::Rect { a: p(330.0, 30.0), b: p(460.0, 110.0) },
            Shape::Ellipse { a: p(480.0, 30.0), b: p(620.0, 110.0) },
            Shape::Arrow { a: p(340.0, 300.0), b: p(520.0, 180.0) },
            Shape::Line { a: p(340.0, 340.0), b: p(600.0, 340.0) },
            Shape::Pen { pts: (0..60).map(|i| p(40.0 + i as f64 * 4.0, 250.0 + (i as f64 / 4.0).sin() * 20.0)).collect() },
            Shape::Text { pos: p(40.0, 320.0), text: "Hello 你好".into() },
            Shape::Number { pos: p(560.0, 240.0), n: 1 },
        ];
        let items: Vec<Item> = shapes
            .into_iter()
            .enumerate()
            .map(|(i, shape)| Item {
                shape,
                color: PALETTE[i % 4],
                width: 4.0,
            })
            .collect();

        // The blur tool follows the size setting: a light and a heavy one.
        let mut items = items;
        for (y, width) in [(140.0, 1.5), (180.0, 10.0)] {
            items.push(Item {
                shape: Shape::Blur {
                    a: p(20.0, y),
                    b: p(300.0, y + 35.0),
                },
                color: PALETTE[0],
                width,
            });
        }

        let sel = Rect::new(10.0, 10.0, 620.0, 380.0);
        let mut out = render(&bg, &blur, &items, sel).unwrap();
        assert_eq!((out.width(), out.height()), (620, 380));

        // The blurred area must differ from the original, the far corner must not.
        let stride = out.stride() as usize;
        let px = |data: &[u8], x: usize, y: usize| -> [u8; 4] {
            let o = y * stride + x * 4;
            [data[o], data[o + 1], data[o + 2], data[o + 3]]
        };
        if let Ok(path) = std::env::var("SNAPGO_TEST_OUT") {
            let mut f = std::fs::File::create(path).unwrap();
            out.write_to_png(&mut f).unwrap();
        }
        drop(blur); // it shares the background surface
        let bg_stride = bg.stride() as usize;
        let bg_data = bg.data().unwrap();
        let out_data = out.data().unwrap();
        let mut differs = 0;
        for y in 60..110 {
            for x in 20..280 {
                let o = (y + 10) * bg_stride + (x + 10) * 4;
                if px(&out_data, x, y)[..3] != bg_data[o..o + 3] {
                    differs += 1;
                }
            }
        }
        assert!(differs > 1000, "blur changed only {differs} pixels");
        assert_eq!(px(&out_data, 615, 375)[3], 255, "output is opaque");
    }
}
