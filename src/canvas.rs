//! The overlay canvas: a custom widget that paints the frozen screenshot,
//! the dimmed surround, the selection frame and all annotations.
//!
//! The screenshot is a GPU texture and the dim is plain colour nodes; only the
//! selection area goes through cairo, so redraws stay cheap on large screens.

use crate::model::{Color, Item, Pt, Rect, Tool, PALETTE};
use gtk4 as gtk;
use gtk4::subclass::prelude::*;
use gtk4::{cairo, gdk, glib, graphene, pango, prelude::*};
use std::cell::RefCell;
use std::rc::Rc;

pub const ACCENT: Color = (0.21, 0.52, 0.89);
/// Extra room around the selection for handles and the size label.
const PAD: f64 = 40.0;
pub const HANDLE: f64 = 4.5;

pub struct Images {
    pub bg: cairo::ImageSurface,
    pub pix: cairo::ImageSurface,
    pub texture: gdk::Texture,
}

pub struct State {
    /// Dropped when the overlay closes, to release the screenshot memory.
    pub images: Option<Images>,
    pub img_w: f64,
    pub img_h: f64,
    /// Selected region, image pixels.
    pub sel: Option<Rect>,
    pub items: Vec<Item>,
    pub selected: Option<usize>,
    /// Shape currently being dragged out.
    pub draft: Option<Item>,
    /// Item hidden because its text is being edited in place.
    pub hidden: Option<usize>,
    pub undo: Vec<Vec<Item>>,
    pub redo: Vec<Vec<Item>>,
    pub tool: Tool,
    pub color: Color,
    /// Stroke size in logical pixels (1..=12).
    pub size: f64,
}

impl State {
    pub fn new(images: Images) -> Self {
        let (img_w, img_h) = (images.bg.width() as f64, images.bg.height() as f64);
        Self {
            images: Some(images),
            img_w,
            img_h,
            sel: None,
            items: Vec::new(),
            selected: None,
            draft: None,
            hidden: None,
            undo: Vec::new(),
            redo: Vec::new(),
            tool: Tool::Arrow,
            color: PALETTE[0],
            size: 3.0,
        }
    }

    /// Record the current items so the next change can be undone.
    pub fn checkpoint(&mut self) {
        self.undo.push(self.items.clone());
        self.redo.clear();
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.items, prev));
            self.selected = None;
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.items, next));
            self.selected = None;
        }
    }

    pub fn full(&self) -> Rect {
        Rect::new(0.0, 0.0, self.img_w, self.img_h)
    }

    /// Topmost item under `p`.
    pub fn item_at(&self, p: Pt, tol: f64) -> Option<usize> {
        (0..self.items.len())
            .rev()
            .find(|&i| Some(i) != self.hidden && self.items[i].hit(p, tol))
    }
}

/// The eight resize handles of `sel` as (hx, hy, position); hx/hy are -1, 0, 1.
pub fn handles(sel: Rect) -> [(i8, i8, Pt); 8] {
    let (x1, y1, x2, y2) = (sel.x, sel.y, sel.x2(), sel.y2());
    let (xm, ym) = ((x1 + x2) / 2.0, (y1 + y2) / 2.0);
    [
        (-1, -1, Pt::new(x1, y1)),
        (0, -1, Pt::new(xm, y1)),
        (1, -1, Pt::new(x2, y1)),
        (-1, 0, Pt::new(x1, ym)),
        (1, 0, Pt::new(x2, ym)),
        (-1, 1, Pt::new(x1, y2)),
        (0, 1, Pt::new(xm, y2)),
        (1, 1, Pt::new(x2, y2)),
    ]
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Canvas {
        pub state: RefCell<Option<Rc<RefCell<State>>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Canvas {
        const NAME: &'static str = "SnapgoCanvas";
        type Type = super::Canvas;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Canvas {}

    impl WidgetImpl for Canvas {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (w, h) = (widget.width() as f64, widget.height() as f64);
            let Some(state) = self.state.borrow().clone() else {
                return;
            };
            let st = state.borrow();
            let Some(images) = st.images.as_ref() else {
                return;
            };
            let rect = |x: f64, y: f64, w: f64, h: f64| {
                graphene::Rect::new(x as f32, y as f32, w.max(0.0) as f32, h.max(0.0) as f32)
            };
            snapshot.append_texture(&images.texture, &rect(0.0, 0.0, w, h));

            let Some(sel) = st.sel else {
                snapshot.append_color(&gdk::RGBA::new(0.0, 0.0, 0.0, 0.3), &rect(0.0, 0.0, w, h));
                draw_hint(snapshot, w, h);
                return;
            };

            // Widget pixels per image pixel.
            let k = w / st.img_w;
            let (x, y, sw, sh) = (sel.x * k, sel.y * k, sel.w * k, sel.h * k);
            let dim = gdk::RGBA::new(0.0, 0.0, 0.0, 0.5);
            snapshot.append_color(&dim, &rect(0.0, 0.0, w, y));
            snapshot.append_color(&dim, &rect(0.0, y + sh, w, h - y - sh));
            snapshot.append_color(&dim, &rect(0.0, y, x, sh));
            snapshot.append_color(&dim, &rect(x + sw, y, w - x - sw, sh));

            let cr = snapshot.append_cairo(&rect(x - PAD, y - PAD, sw + 2.0 * PAD, sh + 2.0 * PAD));

            // Annotations, clipped to the selection, in image coordinates.
            let _ = cr.save();
            cr.scale(k, k);
            cr.rectangle(sel.x, sel.y, sel.w, sel.h);
            cr.clip();
            for (i, item) in st.items.iter().enumerate() {
                if Some(i) != st.hidden {
                    item.draw(&cr, &images.pix);
                }
            }
            if let Some(draft) = &st.draft {
                draft.draw(&cr, &images.pix);
            }
            let _ = cr.restore();

            // Marker around the selected item.
            if let Some(item) = st.selected.and_then(|i| st.items.get(i)) {
                let b = item.bounds();
                let (bx, by) = (b.x * k - 4.0, b.y * k - 4.0);
                cr.rectangle(bx, by, b.w * k + 8.0, b.h * k + 8.0);
                cr.set_line_width(1.0);
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.9);
                let _ = cr.stroke_preserve();
                cr.set_dash(&[4.0, 4.0], 0.0);
                cr.set_source_rgb(ACCENT.0, ACCENT.1, ACCENT.2);
                let _ = cr.stroke();
                cr.set_dash(&[], 0.0);
            }

            // Selection frame and handles.
            cr.set_source_rgb(ACCENT.0, ACCENT.1, ACCENT.2);
            cr.set_line_width(1.5);
            cr.rectangle(x, y, sw, sh);
            let _ = cr.stroke();
            for (_, _, p) in handles(Rect::new(x, y, sw, sh)) {
                cr.arc(p.x, p.y, HANDLE, 0.0, std::f64::consts::TAU);
                cr.set_source_rgb(1.0, 1.0, 1.0);
                let _ = cr.fill_preserve();
                cr.set_source_rgb(ACCENT.0, ACCENT.1, ACCENT.2);
                let _ = cr.stroke();
            }

            // Size label, in real (image) pixels.
            let label = format!("{} × {}", sel.w as i64, sel.h as i64);
            let layout = label_layout(&cr, &label, 12.0);
            let (lw, lh) = layout.pixel_size();
            let (lw, lh) = (lw as f64 + 12.0, lh as f64 + 6.0);
            let (lx, ly) = if y >= lh + 10.0 {
                (x, y - lh - 8.0)
            } else {
                (x + 8.0, y + 8.0)
            };
            rounded(&cr, lx, ly, lw, lh, 5.0);
            cr.set_source_rgba(0.1, 0.1, 0.12, 0.85);
            let _ = cr.fill();
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.move_to(lx + 6.0, ly + 3.0);
            pangocairo::functions::show_layout(&cr, &layout);
        }
    }
}

fn label_layout(cr: &cairo::Context, text: &str, px: f64) -> pango::Layout {
    let layout = pangocairo::functions::create_layout(cr);
    let mut fd = pango::FontDescription::new();
    fd.set_family("Sans");
    fd.set_absolute_size(px * pango::SCALE as f64);
    layout.set_font_description(Some(&fd));
    layout.set_text(text);
    layout
}

fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::FRAC_PI_2;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, FRAC_PI_2, 2.0 * FRAC_PI_2);
    cr.arc(x + r, y + r, r, 2.0 * FRAC_PI_2, 3.0 * FRAC_PI_2);
    cr.close_path();
}

fn draw_hint(snapshot: &gtk::Snapshot, w: f64, h: f64) {
    let text = "Drag to select a region  ·  Click for the whole screen  ·  Esc to cancel";
    let (bw, bh) = (w.min(640.0), 44.0);
    let (bx, by) = ((w - bw) / 2.0, h * 0.12);
    let cr = snapshot.append_cairo(&graphene::Rect::new(
        bx as f32, by as f32, bw as f32, bh as f32,
    ));
    let layout = label_layout(&cr, text, 14.0);
    let (lw, lh) = layout.pixel_size();
    let pw = (lw as f64 + 32.0).min(bw);
    rounded(&cr, bx + (bw - pw) / 2.0, by, pw, bh, 12.0);
    cr.set_source_rgba(0.1, 0.1, 0.12, 0.85);
    let _ = cr.fill();
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.move_to(bx + (bw - lw as f64) / 2.0, by + (bh - lh as f64) / 2.0);
    pangocairo::functions::show_layout(&cr, &layout);
}

glib::wrapper! {
    pub struct Canvas(ObjectSubclass<imp::Canvas>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Canvas {
    pub fn new(state: Rc<RefCell<State>>) -> Self {
        let canvas: Self = glib::Object::new();
        canvas.imp().state.replace(Some(state));
        canvas.set_hexpand(true);
        canvas.set_vexpand(true);
        canvas
    }
}
