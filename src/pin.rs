//! Pinned images: a small frameless window that keeps a capture on screen,
//! above other windows, until it is dismissed.
//!
//! Wayland gives applications no way to ask for "always on top" or to place
//! their own windows, so a pin runs as a separate snapgo process on XWayland,
//! where the window manager honours both requests. Without XWayland the pin
//! still opens, as an ordinary window.

use crate::overlay::new_screenshot_path;
use gtk4 as gtk;
use gtk4::{gdk, gio, glib, prelude::*};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

const CSS: &str = "
window.snapgo-pin { background: #000000; }
.snapgo-pin-frame { border: 1px solid rgba(53, 132, 228, 0.9); }
";

pub struct PinArgs {
    pub file: PathBuf,
    /// The file is a temporary hand-over from the overlay; delete it once read.
    pub temp: bool,
    /// On-screen size in logical pixels (default: the image size).
    pub size: Option<(i32, i32)>,
    /// Top-left corner in screen coordinates.
    pub at: Option<(i32, i32)>,
}

/// Start a pin window for `file` in its own process.
pub fn spawn(file: &Path, size: (i32, i32), at: (i32, i32)) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    cmd.arg("--pin-temp")
        .arg(file)
        .arg("--size")
        .arg(format!("{}x{}", size.0, size.1))
        .arg("--at")
        .arg(format!("{},{}", at.0, at.1));
    if std::env::var_os("DISPLAY").is_some() {
        cmd.env("GDK_BACKEND", "x11,wayland");
    }
    let mut child = cmd.spawn()?;
    // Reap the child if we outlive it; it carries on fine if we exit first.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Run the pin window (the `--pin` entry point).
pub fn run(app_id: &str, args: PinArgs) -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id(app_id)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let args = Rc::new(args);
    app.connect_activate(move |app| {
        if let Err(e) = build(app, &args) {
            eprintln!("snapgo: {e}");
        }
    });
    app.run_with_args::<&str>(&[])
}

fn build(app: &gtk::Application, args: &PinArgs) -> Result<(), String> {
    let texture = gdk::Texture::from_filename(&args.file)
        .map_err(|e| format!("{}: {e}", args.file.display()));
    if args.temp {
        let _ = std::fs::remove_file(&args.file);
    }
    let texture = texture?;
    let (w, h) = args.size.unwrap_or((texture.width(), texture.height()));
    let (w, h) = (w.max(1), h.max(1));

    if let Some(display) = gdk::Display::default() {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(CSS);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let picture = gtk::Picture::for_paintable(&texture);
    picture.set_can_shrink(true);
    picture.set_content_fit(gtk::ContentFit::Fill);
    let frame = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    frame.add_css_class("snapgo-pin-frame");
    frame.set_can_target(false);
    let root = gtk::Overlay::new();
    root.set_child(Some(&picture));
    root.add_overlay(&frame);

    let window = gtk::Window::builder()
        .application(app)
        .decorated(false)
        .title("snapgo pin")
        // Shown for the pin in the dock, the overview and Alt+Tab.
        .icon_name(app.application_id().as_deref().unwrap_or("org.snapgo.Snapgo"))
        .default_width(w)
        .default_height(h)
        .child(&root)
        .build();
    window.add_css_class("snapgo-pin");
    window.set_tooltip_text(Some(
        "Drag to move · scroll to zoom · Ctrl+scroll for opacity\n\
         Ctrl+C copy and close · Ctrl+S save and close\n\
         Double-click or Esc to close",
    ));

    // Drag anywhere to move.
    let drag = gtk::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    let win = window.clone();
    drag.connect_drag_update(move |g, ox, oy| {
        if ox.hypot(oy) < 4.0 {
            return;
        }
        let toplevel = win.surface().and_then(|s| s.downcast::<gdk::Toplevel>().ok());
        if let (Some(toplevel), Some(device), Some((x, y))) = (toplevel, g.device(), g.start_point())
        {
            toplevel.begin_move(&device, gdk::BUTTON_PRIMARY as i32, x, y, g.current_event_time());
        }
        g.reset();
    });
    window.add_controller(drag);

    // Double-click closes.
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_PRIMARY);
    let win = window.clone();
    click.connect_pressed(move |_, n, _, _| {
        if n == 2 {
            win.close();
        }
    });
    window.add_controller(click);

    // Scroll zooms, Ctrl+scroll changes opacity.
    let zoom = Rc::new(Cell::new(1.0_f64));
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    let win = window.clone();
    scroll.connect_scroll(move |c, _, dy| {
        if c.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
            win.set_opacity((win.opacity() - dy * 0.08).clamp(0.2, 1.0));
        } else {
            let z = (zoom.get() * 1.1_f64.powf(-dy)).clamp(0.2, 5.0);
            zoom.set(z);
            win.set_default_size(
                ((w as f64 * z).round() as i32).max(16),
                ((h as f64 * z).round() as i32).max(16),
            );
        }
        glib::Propagation::Stop
    });
    window.add_controller(scroll);

    let keys = gtk::EventControllerKey::new();
    let win = window.clone();
    let tex = texture.clone();
    keys.connect_key_pressed(move |_, key, _, mods| {
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let ch = key.to_unicode().map(|c| c.to_ascii_lowercase());
        if key == gdk::Key::Escape {
            win.close();
        } else if ctrl && ch == Some('c') {
            // Copy and close. The clipboard is served by this process, so it
            // lives on, windowless, until something else is copied.
            let clipboard = WidgetExt::display(&win).clipboard();
            clipboard.set_texture(&tex);
            if let Some(app) = win.application() {
                let guard = RefCell::new(Some(app.hold()));
                clipboard.connect_changed(move |cb| {
                    if !cb.is_local() {
                        guard.take();
                    }
                });
            }
            win.close();
        } else if ctrl && ch == Some('s') {
            match new_screenshot_path().and_then(|p| {
                tex.save_to_png(&p).map_err(|e| e.to_string())?;
                Ok(p)
            }) {
                // Saved: say so and close the pin.
                Ok(p) => {
                    println!("{}", p.display());
                    notify(&win, "Screenshot saved", &p.display().to_string());
                    win.close();
                }
                // Not saved: keep the pin, so the picture is not lost.
                Err(e) => {
                    eprintln!("snapgo: saving failed: {e}");
                    notify(&win, "Screenshot could not be saved", &e);
                }
            }
        } else {
            return glib::Propagation::Proceed;
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys);

    let at = args.at;
    window.connect_map(move |win| {
        let Some(xid) = x11_window_id(win) else {
            return;
        };
        // Let capture overlays find this pin (see `set_pins_above`).
        let entry = registry_dir().join(format!("{}-{xid}", std::process::id()));
        let _ = std::fs::create_dir_all(registry_dir());
        let _ = std::fs::write(&entry, b"");
        win.connect_unmap(move |_| {
            let _ = std::fs::remove_file(&entry);
        });
        // The window manager must have seen the window first, so ask once it
        // is mapped, and once more in case that raced.
        for delay in [30, 250] {
            glib::timeout_add_local_once(Duration::from_millis(delay), move || {
                if let Err(e) = x11_place(xid, at) {
                    eprintln!("snapgo: could not keep the pin on top: {e}");
                }
            });
        }
    });
    window.present();
    Ok(())
}

/// Show a desktop notification, delivered even if the app exits right after.
fn notify(window: &gtk::Window, title: &str, body: &str) {
    let Some(app) = window.application() else {
        return;
    };
    let note = gio::Notification::new(title);
    note.set_body(Some(body));
    app.send_notification(None, &note);
    if let Some(conn) = app.dbus_connection() {
        let _ = conn.flush_sync(gio::Cancellable::NONE);
    }
}

fn x11_window_id(window: &gtk::Window) -> Option<u32> {
    let surface = window.surface()?.downcast::<gdk4_x11::X11Surface>().ok()?;
    Some(surface.xid() as u32)
}

/// One empty file per open pin, named `<pid>-<x11 window id>`.
fn registry_dir() -> PathBuf {
    glib::user_runtime_dir().join("snapgo-pins")
}

/// Put every open pin back on top (`true`), or tuck them all underneath
/// (`false`) so a capture overlay is not covered by them. The overlay shows
/// the frozen screen, pins included, so they do not visibly disappear.
pub fn set_pins_above(above: bool) {
    let Ok(entries) = std::fs::read_dir(registry_dir()) else {
        return;
    };
    let mut xids = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let ids = name
            .split_once('-')
            .and_then(|(pid, xid)| Some((pid.parse::<u32>().ok()?, xid.parse::<u32>().ok()?)));
        match ids {
            Some((pid, xid)) if Path::new(&format!("/proc/{pid}")).exists() => xids.push(xid),
            // Left behind by a pin that did not exit cleanly.
            _ => {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    if xids.is_empty() {
        return; // no pins: don't even wake XWayland
    }
    if let Err(e) = x11_restack(&xids, above) {
        eprintln!("snapgo: could not restack pinned windows: {e}");
    }
}

type X11Result<T> = Result<T, Box<dyn std::error::Error>>;

struct WmState<'c> {
    conn: &'c x11rb::rust_connection::RustConnection,
    root: u32,
    net_wm_state: u32,
}

impl<'c> WmState<'c> {
    fn new(conn: &'c x11rb::rust_connection::RustConnection, screen: usize) -> X11Result<Self> {
        use x11rb::connection::Connection;
        Ok(Self {
            conn,
            root: conn.setup().roots[screen].root,
            net_wm_state: Self::atom(conn, b"_NET_WM_STATE")?,
        })
    }

    fn atom(conn: &x11rb::rust_connection::RustConnection, name: &[u8]) -> X11Result<u32> {
        use x11rb::protocol::xproto::ConnectionExt;
        Ok(conn.intern_atom(false, name)?.reply()?.atom)
    }

    /// Send an EWMH request about `xid` to the window manager.
    fn send(&self, xid: u32, message: u32, data: [u32; 5]) -> X11Result<()> {
        use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt, EventMask};
        let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        let event = ClientMessageEvent::new(32, xid, message, data);
        self.conn.send_event(false, self.root, mask, event)?;
        Ok(())
    }

    /// Add or remove up to two `_NET_WM_STATE` flags (source: pager, so it is
    /// honoured for windows of other processes too).
    fn set(&self, xid: u32, add: bool, first: &[u8], second: Option<&[u8]>) -> X11Result<()> {
        let first = Self::atom(self.conn, first)?;
        let second = match second {
            Some(name) => Self::atom(self.conn, name)?,
            None => 0,
        };
        self.send(xid, self.net_wm_state, [add as u32, first, second, 2, 0])
    }
}

/// Keep a new pin above other windows, on every workspace, and move it to `at`.
fn x11_place(xid: u32, at: Option<(i32, i32)>) -> X11Result<()> {
    use x11rb::connection::Connection;
    let (conn, screen) = x11rb::connect(None)?;
    let wm = WmState::new(&conn, screen)?;
    wm.set(xid, true, b"_NET_WM_STATE_ABOVE", Some(b"_NET_WM_STATE_STICKY"))?;
    if let Some((x, y)) = at {
        // _NET_MOVERESIZE_WINDOW: static gravity, x and y present, from a pager
        // (window managers ignore placement requests from plain applications).
        let flags = 10 | (1 << 8) | (1 << 9) | (2 << 12);
        let message = WmState::atom(&conn, b"_NET_MOVERESIZE_WINDOW")?;
        wm.send(xid, message, [flags, x as u32, y as u32, 0, 0])?;
    }
    conn.flush()?;
    Ok(())
}

fn x11_restack(xids: &[u32], above: bool) -> X11Result<()> {
    use x11rb::protocol::xproto::ConnectionExt;
    let (conn, screen) = x11rb::connect(None)?;
    let wm = WmState::new(&conn, screen)?;
    for &xid in xids {
        // "Below" rather than merely "not above": a window that just lost
        // its above state could still land on top of the overlay.
        wm.set(xid, above, b"_NET_WM_STATE_ABOVE", None)?;
        wm.set(xid, !above, b"_NET_WM_STATE_BELOW", None)?;
    }
    // Round trip, so the requests have reached the server before we go on.
    conn.get_input_focus()?.reply()?;
    Ok(())
}
