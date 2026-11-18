//! The capture overlay: one fullscreen window per monitor showing the frozen
//! screenshot, with region selection, the annotation toolbar and all input
//! handling.

use crate::canvas::{handles, Canvas, Images, State};
use crate::icons::{self, Icon};
use crate::model::{self, Color, Item, Pt, Rect, Shape, Tool, PALETTE};
use gtk4 as gtk;
use gtk4::{cairo, gdk, gio, glib, prelude::*};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

const CSS: &str = "
window.snapgo { background: black; }
.snapgo-bar {
  background: rgba(30, 30, 34, 0.96);
  border: 1px solid rgba(255, 255, 255, 0.14);
  border-radius: 10px;
  padding: 4px;
}
.snapgo-bar button {
  min-width: 30px; min-height: 30px;
  padding: 0; margin: 0 1px;
  background: none; border: none; box-shadow: none; outline: none;
  border-radius: 6px;
}
.snapgo-bar button:hover { background: rgba(255, 255, 255, 0.13); }
.snapgo-bar button:checked { background: #3584e4; }
.snapgo-bar button.swatch { min-width: 24px; }
.snapgo-bar separator { background: rgba(255, 255, 255, 0.16); min-width: 1px; margin: 5px 4px; }
.snapgo-bar label { color: #eeeeee; font-size: 12px; min-width: 20px; }
";

pub struct Session {
    app: gtk::Application,
    ctxs: RefCell<Vec<Rc<Ctx>>>,
    on_close: RefCell<Option<Box<dyn FnOnce()>>>,
}

impl Session {
    /// The overlay that keyboard actions apply to: the one holding the
    /// selection, whichever monitor happens to have keyboard focus.
    fn active(&self, fallback: &Rc<Ctx>) -> Rc<Ctx> {
        self.ctxs
            .borrow()
            .iter()
            .find(|c| c.state.borrow().sel.is_some())
            .cloned()
            .unwrap_or_else(|| fallback.clone())
    }

    /// Only one monitor carries a selection at a time.
    fn clear_others(&self, keep: &Ctx) {
        let others: Vec<Rc<Ctx>> = self
            .ctxs
            .borrow()
            .iter()
            .filter(|c| !std::ptr::eq(Rc::as_ptr(c), keep))
            .cloned()
            .collect();
        for c in others {
            c.commit_text();
            c.state.borrow_mut().sel = None;
            c.toolbar.set_visible(false);
            c.canvas.queue_draw();
        }
    }

    pub fn close(&self) {
        for c in self.ctxs.take() {
            c.release();
        }
        if let Some(f) = self.on_close.take() {
            f();
        }
    }
}

enum Drag {
    None,
    NewSel { moved: bool },
    MoveSel { orig: Rect },
    ResizeSel { orig: Rect, hx: i8, hy: i8 },
    MoveItem { idx: usize, orig: Item, moved: bool },
    Draw,
}

struct TextEdit {
    entry: gtk::Text,
    pos: Pt,
    replace: Option<usize>,
}

pub struct Ctx {
    session: Rc<Session>,
    window: gtk::Window,
    root: gtk::Overlay,
    canvas: Canvas,
    fixed: gtk::Fixed,
    toolbar: gtk::Box,
    size_label: gtk::Label,
    state: Rc<RefCell<State>>,
    drag: RefCell<Drag>,
    /// Press position of the current drag, image coordinates.
    press: Cell<Pt>,
    last_click: Cell<Option<(Instant, Pt)>>,
    scroll_acc: Cell<f64>,
    text: RefCell<Option<TextEdit>>,
    tool_buttons: RefCell<Vec<(Tool, gtk::ToggleButton)>>,
    swatches: RefCell<Vec<gtk::DrawingArea>>,
    text_css: gtk::CssProvider,
    text_class: String,
}

/// Wrap a cairo image surface (RGB24 or ARGB32) as a GDK texture.
fn texture_from_surface(surf: &mut cairo::ImageSurface) -> Option<gdk::Texture> {
    surf.flush();
    let format = match surf.format() {
        cairo::Format::Rgb24 => gdk::MemoryFormat::B8g8r8x8,
        cairo::Format::ARgb32 => gdk::MemoryFormat::B8g8r8a8Premultiplied,
        _ => return None,
    };
    let (w, h, stride) = (surf.width(), surf.height(), surf.stride());
    let data = surf.data().ok()?.to_vec();
    let bytes = glib::Bytes::from_owned(data);
    Some(gdk::MemoryTexture::new(w, h, format, &bytes, stride as usize).upcast())
}

fn make_images(shot: &cairo::ImageSurface, crop: Rect) -> Option<Images> {
    let mut bg = cairo::ImageSurface::create(cairo::Format::Rgb24, crop.w as i32, crop.h as i32).ok()?;
    {
        let cr = cairo::Context::new(&bg).ok()?;
        cr.set_source_rgb(0.0, 0.0, 0.0);
        cr.paint().ok()?;
        cr.set_source_surface(shot, -crop.x, -crop.y).ok()?;
        cr.paint().ok()?;
    }
    let pix = model::pixelated(&bg).ok()?;
    let texture = texture_from_surface(&mut bg)?;
    Some(Images { bg, pix, texture })
}

fn install_css(display: &gdk::Display) {
    thread_local!(static DONE: Cell<bool> = const { Cell::new(false) });
    if DONE.replace(true) {
        return;
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    gtk::style_context_add_provider_for_display(
        display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// Where each monitor sits inside the full-desktop screenshot.
fn monitor_crops(display: &gdk::Display, iw: f64, ih: f64) -> Vec<(Option<gdk::Monitor>, Rect)> {
    let monitors: Vec<gdk::Monitor> = display
        .monitors()
        .iter::<gdk::Monitor>()
        .flatten()
        .collect();
    let full = Rect::new(0.0, 0.0, iw, ih);
    if monitors.len() <= 1 {
        return vec![(monitors.into_iter().next(), full)];
    }
    let geos: Vec<gdk::Rectangle> = monitors.iter().map(|m| m.geometry()).collect();
    let ux = geos.iter().map(|g| g.x()).min().unwrap_or(0) as f64;
    let uy = geos.iter().map(|g| g.y()).min().unwrap_or(0) as f64;
    let ux2 = geos.iter().map(|g| g.x() + g.width()).max().unwrap_or(1) as f64;
    let scale = iw / (ux2 - ux).max(1.0);
    monitors
        .into_iter()
        .zip(geos)
        .filter_map(|(m, g)| {
            let x = ((g.x() as f64 - ux) * scale).round().clamp(0.0, iw - 1.0);
            let y = ((g.y() as f64 - uy) * scale).round().clamp(0.0, ih - 1.0);
            let w = (g.width() as f64 * scale).round().min(iw - x);
            let h = (g.height() as f64 * scale).round().min(ih - y);
            (w >= 1.0 && h >= 1.0).then(|| (Some(m), Rect::new(x, y, w, h)))
        })
        .collect()
}

/// Open the overlay on every monitor. `on_close` runs once it is dismissed.
pub fn start(app: &gtk::Application, shot: cairo::ImageSurface, on_close: impl FnOnce() + 'static) {
    let session = Rc::new(Session {
        app: app.clone(),
        ctxs: RefCell::new(Vec::new()),
        on_close: RefCell::new(Some(Box::new(on_close))),
    });
    let Some(display) = gdk::Display::default() else {
        eprintln!("snapgo: no display");
        session.close();
        return;
    };
    install_css(&display);
    let crops = monitor_crops(&display, shot.width() as f64, shot.height() as f64);
    for (i, (monitor, crop)) in crops.into_iter().enumerate() {
        match make_images(&shot, crop) {
            Some(images) => {
                let ctx = Ctx::build(&session, &display, monitor.as_ref(), images, i);
                session.ctxs.borrow_mut().push(ctx);
            }
            None => eprintln!("snapgo: could not prepare the image for monitor {i}"),
        }
    }
    if session.ctxs.borrow().is_empty() {
        session.close();
    }
}

fn icon_widget(icon: Icon) -> gtk::DrawingArea {
    let da = gtk::DrawingArea::new();
    da.set_content_width(icons::SIZE);
    da.set_content_height(icons::SIZE);
    da.set_halign(gtk::Align::Center);
    da.set_valign(gtk::Align::Center);
    da.set_draw_func(move |_, cr, _, _| icons::draw(cr, icon));
    da
}

fn resize_cursor(hx: i8, hy: i8) -> &'static str {
    match (hx, hy) {
        (-1, -1) => "nw-resize",
        (0, -1) => "n-resize",
        (1, -1) => "ne-resize",
        (-1, 0) => "w-resize",
        (1, 0) => "e-resize",
        (-1, 1) => "sw-resize",
        (0, 1) => "s-resize",
        _ => "se-resize",
    }
}

impl Ctx {
    fn build(
        session: &Rc<Session>,
        display: &gdk::Display,
        monitor: Option<&gdk::Monitor>,
        images: Images,
        index: usize,
    ) -> Rc<Ctx> {
        let window = gtk::Window::builder()
            .application(&session.app)
            .decorated(false)
            .title("snapgo")
            .build();
        window.add_css_class("snapgo");

        let state = Rc::new(RefCell::new(State::new(images)));
        let canvas = Canvas::new(state.clone());
        let fixed = gtk::Fixed::new();
        let root = gtk::Overlay::new();
        root.set_child(Some(&canvas));
        root.add_overlay(&fixed);
        window.set_child(Some(&root));

        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        toolbar.add_css_class("snapgo-bar");
        toolbar.set_cursor_from_name(Some("default"));
        toolbar.set_visible(false);
        fixed.put(&toolbar, 0.0, 0.0);

        let text_css = gtk::CssProvider::new();
        gtk::style_context_add_provider_for_display(
            display,
            &text_css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );

        let ctx = Rc::new(Ctx {
            session: session.clone(),
            window: window.clone(),
            root: root.clone(),
            canvas,
            fixed,
            toolbar,
            size_label: gtk::Label::new(None),
            state,
            drag: RefCell::new(Drag::None),
            press: Cell::new(Pt::default()),
            last_click: Cell::new(None),
            scroll_acc: Cell::new(0.0),
            text: RefCell::new(None),
            tool_buttons: RefCell::new(Vec::new()),
            swatches: RefCell::new(Vec::new()),
            text_css,
            text_class: format!("snapgo-text-{index}"),
        });
        ctx.build_toolbar();

        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        let c = ctx.clone();
        drag.connect_drag_begin(move |_, x, y| c.drag_begin(x, y));
        let c = ctx.clone();
        drag.connect_drag_update(move |_, ox, oy| c.drag_update(ox, oy));
        let c = ctx.clone();
        drag.connect_drag_end(move |_, _, _| c.drag_end());
        root.add_controller(drag);

        let motion = gtk::EventControllerMotion::new();
        let c = ctx.clone();
        motion.connect_motion(move |_, x, y| c.update_cursor(x, y));
        root.add_controller(motion);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let c = ctx.clone();
        scroll.connect_scroll(move |_, _, dy| {
            let acc = c.scroll_acc.get() + dy;
            let steps = acc.trunc();
            c.scroll_acc.set(acc - steps);
            if steps != 0.0 {
                c.session.active(&c).change_size(-steps);
            }
            glib::Propagation::Stop
        });
        root.add_controller(scroll);

        let keys = gtk::EventControllerKey::new();
        let c = ctx.clone();
        keys.connect_key_pressed(move |_, key, _, mods| c.key_pressed(key, mods));
        window.add_controller(keys);

        let s = session.clone();
        window.connect_close_request(move |_| {
            let s = s.clone();
            glib::idle_add_local_once(move || s.close());
            glib::Propagation::Stop
        });

        match monitor {
            Some(m) => window.fullscreen_on_monitor(m),
            None => window.fullscreen(),
        }
        window.present();
        ctx
    }

    fn build_toolbar(self: &Rc<Self>) {
        let bar = &self.toolbar;
        let button = |icon: Icon, tip: &str| {
            let b = gtk::Button::new();
            b.set_child(Some(&icon_widget(icon)));
            b.set_tooltip_text(Some(tip));
            b.set_focusable(false);
            b
        };
        let separator = || gtk::Separator::new(gtk::Orientation::Vertical);

        let mut group: Option<gtk::ToggleButton> = None;
        for tool in Tool::ALL {
            let (name, hotkey) = tool.info();
            let b = gtk::ToggleButton::new();
            b.set_child(Some(&icon_widget(Icon::Tool(tool))));
            b.set_tooltip_text(Some(&format!("{name} ({})", hotkey.to_ascii_uppercase())));
            b.set_focusable(false);
            match &group {
                Some(g) => b.set_group(Some(g)),
                None => group = Some(b.clone()),
            }
            b.set_active(self.state.borrow().tool == tool);
            let c = self.clone();
            b.connect_toggled(move |b| {
                if b.is_active() {
                    c.tool_chosen(tool);
                }
            });
            bar.append(&b);
            self.tool_buttons.borrow_mut().push((tool, b));
        }

        bar.append(&separator());
        for (i, color) in PALETTE.into_iter().enumerate() {
            let da = gtk::DrawingArea::new();
            da.set_content_width(18);
            da.set_content_height(18);
            da.set_halign(gtk::Align::Center);
            da.set_valign(gtk::Align::Center);
            let state = self.state.clone();
            da.set_draw_func(move |_, cr, w, h| {
                let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
                let current = state.borrow().color == color;
                cr.arc(cx, cy, if current { 5.5 } else { 7.0 }, 0.0, std::f64::consts::TAU);
                cr.set_source_rgb(color.0, color.1, color.2);
                let _ = cr.fill_preserve();
                cr.set_line_width(1.0);
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.45);
                let _ = cr.stroke();
                if current {
                    cr.arc(cx, cy, 8.0, 0.0, std::f64::consts::TAU);
                    cr.set_line_width(1.6);
                    cr.set_source_rgb(1.0, 1.0, 1.0);
                    let _ = cr.stroke();
                }
            });
            let b = gtk::Button::new();
            b.add_css_class("swatch");
            b.set_child(Some(&da));
            b.set_tooltip_text(Some(&format!("Colour ({})", i + 1)));
            b.set_focusable(false);
            let c = self.clone();
            b.connect_clicked(move |_| c.set_color(color));
            bar.append(&b);
            self.swatches.borrow_mut().push(da);
        }

        bar.append(&separator());
        self.size_label
            .set_tooltip_text(Some("Size: scroll the mouse wheel, or [ and ]"));
        bar.append(&self.size_label);
        self.refresh_controls();

        bar.append(&separator());
        let c = self.clone();
        let b = button(Icon::Undo, "Undo (Ctrl+Z)");
        b.connect_clicked(move |_| c.undo_redo(false));
        bar.append(&b);
        let c = self.clone();
        let b = button(Icon::Redo, "Redo (Ctrl+Shift+Z)");
        b.connect_clicked(move |_| c.undo_redo(true));
        bar.append(&b);

        bar.append(&separator());
        let c = self.clone();
        let b = button(Icon::Save, "Save to Pictures/Screenshots (Ctrl+S)");
        b.connect_clicked(move |_| c.save());
        bar.append(&b);
        let c = self.clone();
        let b = button(Icon::Copy, "Copy to clipboard (Enter)");
        b.connect_clicked(move |_| c.copy());
        bar.append(&b);
        let c = self.clone();
        let b = button(Icon::Close, "Cancel (Esc)");
        b.connect_clicked(move |_| c.session.close());
        bar.append(&b);
    }

    /// Widget pixels per image pixel.
    fn k(&self) -> f64 {
        let w = self.canvas.width() as f64;
        let img_w = self.state.borrow().img_w;
        if w > 0.0 && img_w > 0.0 {
            w / img_w
        } else {
            1.0
        }
    }

    fn to_img(&self, x: f64, y: f64) -> Pt {
        let k = self.k();
        let st = self.state.borrow();
        Pt::new((x / k).clamp(0.0, st.img_w), (y / k).clamp(0.0, st.img_h))
    }

    fn over_toolbar(&self, x: f64, y: f64) -> bool {
        self.toolbar.is_visible()
            && self.toolbar.compute_bounds(&self.root).is_some_and(|b| {
                b.contains_point(&gtk::graphene::Point::new(x as f32, y as f32))
            })
    }

    // ---- pointer -------------------------------------------------------

    fn drag_begin(self: &Rc<Self>, x: f64, y: f64) {
        self.drag.replace(Drag::None);
        if self.over_toolbar(x, y) {
            return;
        }
        let now = Instant::now();
        let here = Pt::new(x, y);
        let double = self.last_click.get().is_some_and(|(t, q)| {
            now.duration_since(t) < Duration::from_millis(400) && q.dist(here) < 6.0
        });
        self.last_click.set(Some((now, here)));

        let was_editing = self.text.borrow().is_some();
        self.commit_text();
        if self.state.borrow().sel.is_none() {
            self.session.clear_others(self);
        }

        let p = self.to_img(x, y);
        self.press.set(p);
        let k = self.k();
        let tol = 6.0 / k;

        enum Next {
            Drag(Drag),
            NewText,
            EditText(usize),
        }
        let next = {
            let mut st = self.state.borrow_mut();
            let tool = st.tool;
            let handle = st
                .sel
                .and_then(|sel| handles(sel).into_iter().find(|h| h.2.dist(p) <= 9.0 / k));
            let hit = st.item_at(p, tol);
            let hit_text = hit.filter(|&i| matches!(st.items[i].shape, Shape::Text { .. }));
            match (st.sel, handle) {
                (None, _) => Next::Drag(Drag::NewSel { moved: false }),
                (Some(orig), Some((hx, hy, _))) => Next::Drag(Drag::ResizeSel { orig, hx, hy }),
                (Some(orig), None) => {
                    let inside = orig.contains(p);
                    if let (Some(idx), true) = (hit_text, tool == Tool::Text || double) {
                        Next::EditText(idx)
                    } else if tool == Tool::Select {
                        st.selected = hit;
                        match hit {
                            Some(idx) => Next::Drag(Drag::MoveItem {
                                idx,
                                orig: st.items[idx].clone(),
                                moved: false,
                            }),
                            None if inside => Next::Drag(Drag::MoveSel { orig }),
                            None => Next::Drag(Drag::NewSel { moved: false }),
                        }
                    } else if inside {
                        st.selected = None;
                        let (color, width) = (st.color, st.size / k);
                        let two = |shape: Shape| Item { shape, color, width };
                        let draft = match tool {
                            Tool::Arrow => Some(two(Shape::Arrow { a: p, b: p })),
                            Tool::Line => Some(two(Shape::Line { a: p, b: p })),
                            Tool::Rect => Some(two(Shape::Rect { a: p, b: p })),
                            Tool::Ellipse => Some(two(Shape::Ellipse { a: p, b: p })),
                            Tool::Blur => Some(two(Shape::Blur { a: p, b: p })),
                            Tool::Pen => Some(two(Shape::Pen { pts: vec![p] })),
                            Tool::Marker => Some(two(Shape::Marker { pts: vec![p] })),
                            Tool::Number => {
                                let n = st
                                    .items
                                    .iter()
                                    .filter(|i| matches!(i.shape, Shape::Number { .. }))
                                    .count() as u32
                                    + 1;
                                st.checkpoint();
                                st.items.push(two(Shape::Number { pos: p, n }));
                                None
                            }
                            Tool::Text | Tool::Select => None,
                        };
                        match draft {
                            Some(d) => {
                                st.draft = Some(d);
                                Next::Drag(Drag::Draw)
                            }
                            // A click that just finished one text does not
                            // immediately open another.
                            None if tool == Tool::Text && !was_editing => Next::NewText,
                            None => Next::Drag(Drag::None),
                        }
                    } else if st.items.is_empty() {
                        Next::Drag(Drag::NewSel { moved: false })
                    } else {
                        Next::Drag(Drag::None)
                    }
                }
            }
        };
        match next {
            Next::Drag(d) => {
                self.drag.replace(d);
            }
            Next::NewText => self.start_text(p, None),
            Next::EditText(idx) => self.start_text(p, Some(idx)),
        }
        self.canvas.queue_draw();
    }

    fn drag_update(&self, ox: f64, oy: f64) {
        let k = self.k();
        let start = self.press.get();
        let (dx, dy) = (ox / k, oy / k);
        let mut reposition = false;
        {
            let mut st = self.state.borrow_mut();
            let (iw, ih) = (st.img_w, st.img_h);
            let p = Pt::new(
                (start.x + dx).clamp(0.0, iw),
                (start.y + dy).clamp(0.0, ih),
            );
            let far = ox.hypot(oy) >= 4.0;
            match &mut *self.drag.borrow_mut() {
                Drag::None => return,
                Drag::NewSel { moved } => {
                    if !*moved && !far {
                        return;
                    }
                    *moved = true;
                    st.sel = Some(Rect::from_points(start, p).round());
                    self.toolbar.set_visible(false);
                }
                Drag::MoveSel { orig } => {
                    let x = (orig.x + dx).clamp(0.0, (iw - orig.w).max(0.0));
                    let y = (orig.y + dy).clamp(0.0, (ih - orig.h).max(0.0));
                    st.sel = Some(Rect::new(x.round(), y.round(), orig.w, orig.h));
                    reposition = true;
                }
                Drag::ResizeSel { orig, hx, hy } => {
                    let (mut a, mut b) = (Pt::new(orig.x, orig.y), Pt::new(orig.x2(), orig.y2()));
                    match *hx {
                        -1 => a.x = p.x,
                        1 => b.x = p.x,
                        _ => {}
                    }
                    match *hy {
                        -1 => a.y = p.y,
                        1 => b.y = p.y,
                        _ => {}
                    }
                    st.sel = Some(Rect::from_points(a, b).round());
                    reposition = true;
                }
                Drag::MoveItem { idx, orig, moved } => {
                    if !*moved {
                        if !far {
                            return;
                        }
                        st.checkpoint();
                        *moved = true;
                    }
                    if let Some(slot) = st.items.get_mut(*idx) {
                        *slot = orig.translated(dx, dy);
                    }
                }
                Drag::Draw => {
                    if let Some(d) = st.draft.as_mut() {
                        match &mut d.shape {
                            Shape::Arrow { b, .. }
                            | Shape::Line { b, .. }
                            | Shape::Rect { b, .. }
                            | Shape::Ellipse { b, .. }
                            | Shape::Blur { b, .. } => *b = p,
                            Shape::Pen { pts } | Shape::Marker { pts } => pts.push(p),
                            Shape::Text { .. } | Shape::Number { .. } => {}
                        }
                    }
                }
            }
        }
        if reposition {
            self.position_toolbar();
        }
        self.canvas.queue_draw();
    }

    fn drag_end(&self) {
        let drag = self.drag.replace(Drag::None);
        {
            let mut st = self.state.borrow_mut();
            match drag {
                Drag::NewSel { moved: false } if st.sel.is_none() => {
                    // A plain click selects the whole screen.
                    st.sel = Some(st.full());
                }
                Drag::Draw => {
                    if let Some(d) = st.draft.take() {
                        let keep = match &d.shape {
                            Shape::Arrow { a, b }
                            | Shape::Line { a, b }
                            | Shape::Rect { a, b }
                            | Shape::Ellipse { a, b }
                            | Shape::Blur { a, b } => a.dist(*b) >= 3.0,
                            _ => true,
                        };
                        if keep {
                            st.checkpoint();
                            st.items.push(d);
                        }
                    }
                }
                _ => {}
            }
        }
        self.position_toolbar();
        self.canvas.queue_draw();
    }

    fn update_cursor(&self, x: f64, y: f64) {
        if !matches!(*self.drag.borrow(), Drag::None) {
            return;
        }
        let p = self.to_img(x, y);
        let k = self.k();
        let st = self.state.borrow();
        let name = match st.sel {
            None => "crosshair",
            Some(sel) => {
                if let Some((hx, hy, _)) =
                    handles(sel).into_iter().find(|h| h.2.dist(p) <= 9.0 / k)
                {
                    resize_cursor(hx, hy)
                } else if st.tool == Tool::Select {
                    if st.item_at(p, 6.0 / k).is_some() {
                        "pointer"
                    } else if sel.contains(p) {
                        "move"
                    } else {
                        "crosshair"
                    }
                } else if !sel.contains(p) {
                    "default"
                } else if st.tool == Tool::Text {
                    "text"
                } else {
                    "crosshair"
                }
            }
        };
        self.root.set_cursor_from_name(Some(name));
    }

    fn position_toolbar(&self) {
        let Some(sel) = self.state.borrow().sel else {
            self.toolbar.set_visible(false);
            return;
        };
        let k = self.k();
        let (w, h) = (self.canvas.width() as f64, self.canvas.height() as f64);
        self.toolbar.set_visible(true);
        let tw = self.toolbar.measure(gtk::Orientation::Horizontal, -1).1 as f64;
        let th = self.toolbar.measure(gtk::Orientation::Vertical, -1).1 as f64;
        let x = (sel.x2() * k - tw).clamp(8.0, (w - tw - 8.0).max(8.0));
        let mut y = sel.y2() * k + 10.0;
        if y + th > h - 8.0 {
            y = sel.y * k - th - 10.0;
            if y < 8.0 {
                // No room outside: tuck it into the bottom of the selection.
                y = sel.y2() * k - th - 10.0;
            }
        }
        self.fixed.move_(&self.toolbar, x.round(), y.round());
    }

    // ---- tools, colour, size ------------------------------------------

    fn tool_chosen(&self, tool: Tool) {
        self.commit_text();
        {
            let mut st = self.state.borrow_mut();
            st.tool = tool;
            if tool != Tool::Select {
                st.selected = None;
            }
        }
        self.canvas.queue_draw();
    }

    fn set_tool(&self, tool: Tool) {
        let button = self
            .tool_buttons
            .borrow()
            .iter()
            .find(|(t, _)| *t == tool)
            .map(|(_, b)| b.clone());
        if let Some(b) = button {
            b.set_active(true);
        }
    }

    fn refresh_controls(&self) {
        self.size_label
            .set_text(&format!("{}", self.state.borrow().size as i32));
        for s in self.swatches.borrow().iter() {
            s.queue_draw();
        }
    }

    fn set_color(&self, color: Color) {
        let editing = self.text.borrow().is_some();
        {
            let mut st = self.state.borrow_mut();
            st.color = color;
            if !editing {
                if let Some(idx) = st.selected.filter(|&i| i < st.items.len()) {
                    if st.items[idx].color != color {
                        st.checkpoint();
                        st.items[idx].color = color;
                    }
                }
            }
        }
        if editing {
            self.update_text_css();
        }
        self.refresh_controls();
        self.canvas.queue_draw();
    }

    fn change_size(&self, delta: f64) {
        let editing = self.text.borrow().is_some();
        let k = self.k();
        {
            let mut st = self.state.borrow_mut();
            let size = (st.size + delta).clamp(1.0, 12.0);
            if size == st.size {
                return;
            }
            st.size = size;
            if !editing {
                if let Some(idx) = st.selected.filter(|&i| i < st.items.len()) {
                    st.checkpoint();
                    st.items[idx].width = size / k;
                }
            }
        }
        if editing {
            self.update_text_css();
        }
        self.refresh_controls();
        self.canvas.queue_draw();
    }

    fn undo_redo(&self, redo: bool) {
        self.commit_text();
        {
            let mut st = self.state.borrow_mut();
            if redo {
                st.redo();
            } else {
                st.undo();
            }
        }
        self.canvas.queue_draw();
    }

    fn delete_selected(&self) {
        {
            let mut st = self.state.borrow_mut();
            if let Some(idx) = st.selected.take().filter(|&i| i < st.items.len()) {
                st.checkpoint();
                st.items.remove(idx);
            }
        }
        self.canvas.queue_draw();
    }

    fn select_all(&self) {
        {
            let mut st = self.state.borrow_mut();
            st.sel = Some(st.full());
        }
        self.position_toolbar();
        self.canvas.queue_draw();
    }

    // ---- text ----------------------------------------------------------

    fn update_text_css(&self) {
        let st = self.state.borrow();
        let (r, g, b) = (
            (st.color.0 * 255.0) as u8,
            (st.color.1 * 255.0) as u8,
            (st.color.2 * 255.0) as u8,
        );
        // Matches model::font_px: item width is size / k image pixels, which
        // is `size` logical pixels on screen.
        let px = model::font_px(st.size);
        self.text_css.load_from_string(&format!(
            ".{} {{ color: rgb({r},{g},{b}); caret-color: rgb({r},{g},{b}); \
             font-family: Sans; font-weight: bold; font-size: {px}px; \
             background: rgba(128,128,128,0.2); border-radius: 3px; \
             outline: 1px dashed rgba(255,255,255,0.75); \
             padding: 0; margin: 0; min-height: 0; }}",
            self.text_class
        ));
        if let Some(edit) = self.text.borrow().as_ref() {
            fit_text(&edit.entry, "", px);
        }
    }

    /// Open the in-place text editor, for a new text at `p` or to re-edit
    /// the text item `replace`. A real GTK text widget is used so input
    /// methods (Chinese, Japanese, compose keys…) work.
    fn start_text(self: &Rc<Self>, p: Pt, replace: Option<usize>) {
        self.drag.replace(Drag::None);
        let k = self.k();
        let (pos, initial) = {
            let mut st = self.state.borrow_mut();
            let existing = replace.and_then(|i| st.items.get(i).cloned());
            match existing {
                Some(Item {
                    shape: Shape::Text { pos, text },
                    color,
                    width,
                }) => {
                    st.hidden = replace;
                    st.selected = None;
                    st.color = color;
                    st.size = (width * k).round().clamp(1.0, 12.0);
                    (pos, text)
                }
                _ => {
                    let half = model::font_px(st.size / k) * 0.65;
                    (Pt::new(p.x, p.y - half), String::new())
                }
            }
        };
        self.update_text_css();
        self.refresh_controls();

        let entry = gtk::Text::new();
        entry.add_css_class(&self.text_class);
        entry.set_text(&initial);
        // GtkFixed gives children their minimum size, so the editor is sized
        // explicitly to its text (plus any input-method preedit).
        let state = self.state.clone();
        entry.connect_changed(move |e| fit_text(e, "", model::font_px(state.borrow().size)));
        let state = self.state.clone();
        entry.connect_preedit_changed(move |e, preedit| {
            fit_text(e, preedit, model::font_px(state.borrow().size))
        });
        fit_text(&entry, "", model::font_px(self.state.borrow().size));
        self.fixed.put(&entry, pos.x * k, pos.y * k);
        let c = self.clone();
        entry.connect_activate(move |_| c.commit_text());
        entry.grab_focus();
        entry.set_position(-1);
        self.text.replace(Some(TextEdit {
            entry,
            pos,
            replace: replace.filter(|_| self.state.borrow().hidden.is_some()),
        }));
        self.canvas.queue_draw();
    }

    fn commit_text(&self) {
        let Some(edit) = self.text.take() else {
            return;
        };
        let text = edit.entry.text().to_string();
        self.fixed.remove(&edit.entry);
        let k = self.k();
        {
            let mut st = self.state.borrow_mut();
            st.hidden = None;
            let item = Item {
                shape: Shape::Text {
                    pos: edit.pos,
                    text: text.clone(),
                },
                color: st.color,
                width: st.size / k,
            };
            let empty = text.trim().is_empty();
            match edit.replace.filter(|&i| i < st.items.len()) {
                Some(idx) if empty => {
                    st.checkpoint();
                    st.items.remove(idx);
                }
                Some(idx) => {
                    if st.items[idx] != item {
                        st.checkpoint();
                        st.items[idx] = item;
                    }
                }
                None if !empty => {
                    st.checkpoint();
                    st.items.push(item);
                }
                None => {}
            }
        }
        self.canvas.queue_draw();
    }

    fn cancel_text(&self) {
        if let Some(edit) = self.text.take() {
            self.fixed.remove(&edit.entry);
            self.state.borrow_mut().hidden = None;
            self.canvas.queue_draw();
        }
    }

    // ---- keyboard ------------------------------------------------------

    fn key_pressed(self: &Rc<Self>, key: gdk::Key, mods: gdk::ModifierType) -> glib::Propagation {
        use glib::Propagation::{Proceed, Stop};
        // While typing, the text widget owns the keyboard; only Esc gets here
        // with a meaning.
        if self.text.borrow().is_some() {
            if key == gdk::Key::Escape {
                self.cancel_text();
                return Stop;
            }
            return Proceed;
        }
        if mods.intersects(gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK) {
            return Proceed;
        }
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let target = self.session.active(self);

        if key == gdk::Key::Escape {
            self.session.close();
        } else if key == gdk::Key::Return || key == gdk::Key::KP_Enter {
            target.copy();
        } else if key == gdk::Key::Delete || key == gdk::Key::BackSpace {
            target.delete_selected();
        } else {
            let ch = key.to_unicode().map(|c| c.to_ascii_lowercase());
            match (ctrl, ch) {
                (true, Some('c')) => target.copy(),
                (true, Some('s')) => target.save(),
                (true, Some('z')) => target.undo_redo(shift),
                (true, Some('y')) => target.undo_redo(true),
                (true, Some('a')) => target.select_all(),
                (false, Some('[')) => target.change_size(-1.0),
                (false, Some(']')) => target.change_size(1.0),
                (false, Some(c)) => {
                    let digit = c.to_digit(10).map(|d| d as usize);
                    if let Some(color) = digit.and_then(|d| PALETTE.get(d.wrapping_sub(1))) {
                        target.set_color(*color);
                    } else if let Some(tool) = Tool::ALL.into_iter().find(|t| t.info().1 == c) {
                        target.set_tool(tool);
                    } else {
                        return Proceed;
                    }
                }
                _ => return Proceed,
            }
        }
        Stop
    }

    // ---- output --------------------------------------------------------

    fn render(&self) -> Option<cairo::ImageSurface> {
        self.commit_text();
        let st = self.state.borrow();
        let images = st.images.as_ref()?;
        let sel = st.sel.unwrap_or_else(|| st.full());
        match model::render(&images.bg, &images.pix, &st.items, sel) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("snapgo: rendering failed: {e}");
                None
            }
        }
    }

    /// Copy the result to the clipboard and close the overlay.
    ///
    /// Wayland clipboards are served by the owning process, so snapgo stays
    /// alive (windowless) until something else replaces the clipboard.
    fn copy(&self) {
        let Some(texture) = self
            .render()
            .and_then(|mut surf| texture_from_surface(&mut surf))
        else {
            return;
        };
        let clipboard = WidgetExt::display(&self.window).clipboard();
        clipboard.set_texture(&texture);
        let guard = RefCell::new(Some(self.session.app.hold()));
        clipboard.connect_changed(move |cb| {
            if !cb.is_local() {
                guard.take();
            }
        });
        self.session.close();
    }

    /// Save the result as a PNG in Pictures/Screenshots and close.
    fn save(&self) {
        let Some(surf) = self.render() else {
            return;
        };
        let app = self.session.app.clone();
        let guard = app.hold();
        self.session.close();
        // Encode after the overlay is gone so closing feels instant.
        glib::timeout_add_local_once(Duration::from_millis(20), move || {
            let note = match write_png(&surf) {
                Ok(path) => {
                    println!("{}", path.display());
                    let n = gio::Notification::new("Screenshot saved");
                    n.set_body(Some(&path.display().to_string()));
                    n
                }
                Err(e) => {
                    eprintln!("snapgo: saving failed: {e}");
                    let n = gio::Notification::new("Screenshot could not be saved");
                    n.set_body(Some(&e));
                    n
                }
            };
            app.send_notification(None, &note);
            drop(guard);
        });
    }

    fn release(&self) {
        self.cancel_text();
        self.state.borrow_mut().images = None;
        gtk::style_context_remove_provider_for_display(&WidgetExt::display(&self.window), &self.text_css);
        self.window.destroy();
    }
}

/// Size the in-place editor to fit what is being typed.
fn fit_text(entry: &gtk::Text, preedit: &str, px: f64) {
    let shown = format!("{}{preedit}", entry.text());
    let (w, _) = model::text_extent(&shown, px);
    entry.set_size_request((w + px * 1.2).ceil() as i32, -1);
}

fn write_png(surf: &cairo::ImageSurface) -> Result<PathBuf, String> {
    let dir = glib::user_special_dir(glib::UserDirectory::Pictures)
        .unwrap_or_else(glib::home_dir)
        .join("Screenshots");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stamp = glib::DateTime::now_local()
        .and_then(|t| t.format("%Y%m%d-%H%M%S"))
        .map_err(|e| e.to_string())?;
    let mut path = dir.join(format!("snapgo-{stamp}.png"));
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = dir.join(format!("snapgo-{stamp}-{n}.png"));
    }
    let mut file = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    surf.write_to_png(&mut file)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}
