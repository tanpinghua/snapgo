//! snapgo — a fast screenshot and annotation tool for GNOME on Wayland.

mod canvas;
mod icons;
mod model;
mod overlay;
mod pin;
mod portal;

use gtk4 as gtk;
use gtk4::{cairo, gio, glib, prelude::*};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const APP_ID: &str = "org.snapgo.Snapgo";

const HELP: &str = "\
snapgo — screenshot and annotate

Usage: snapgo [OPTIONS]

With no options, freezes the screen and lets you select and annotate a region.

Options:
      --image <PNG>  Annotate an existing image instead of capturing the screen
      --pin <PNG>    Pin an image on screen, above other windows
                     (optional: --size WxH, --at X,Y)
  -h, --help         Show this help
  -V, --version      Show the version

In the overlay:
  Drag               select a region (click: whole screen)
  Enter, Ctrl+C      copy to the clipboard and close
  Ctrl+S             save to Pictures/Screenshots and close
  Ctrl+P             pin the result on screen and close
  Esc                cancel
  V A L R E P H T N B   select, arrow, line, rectangle, ellipse, pen,
                        highlighter, text, numbered marker, pixelate
  1-6                colour          [ ]  or mouse wheel: size
  Ctrl+Z / Ctrl+Shift+Z   undo / redo      Delete: remove selected item
";

/// Progress messages on stderr, enabled with SNAPGO_DEBUG=1.
fn debug(msg: &str) {
    if std::env::var_os("SNAPGO_DEBUG").is_some() {
        eprintln!("snapgo: {msg}");
    }
}

fn load_png(path: &Path) -> Result<cairo::ImageSurface, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    cairo::ImageSurface::create_from_png(&mut file).map_err(|e| format!("{}: {e}", path.display()))
}

fn main() -> glib::ExitCode {
    // GTK's Vulkan renderer logs a warning on every resize; OpenGL is quiet
    // and starts faster. An explicit GSK_RENDERER still wins.
    if std::env::var_os("GSK_RENDERER").is_none() {
        // SAFETY: nothing else is running yet; no other thread reads the
        // environment at this point.
        unsafe { std::env::set_var("GSK_RENDERER", "gl") };
    }

    let mut image: Option<PathBuf> = None;
    let mut pin: Option<(PathBuf, bool)> = None;
    let mut size: Option<(i32, i32)> = None;
    let mut at: Option<(i32, i32)> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return glib::ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("snapgo {}", env!("CARGO_PKG_VERSION"));
                return glib::ExitCode::SUCCESS;
            }
            "--image" => match args.next() {
                Some(p) => image = Some(PathBuf::from(p)),
                None => {
                    eprintln!("snapgo: --image needs a file");
                    return glib::ExitCode::FAILURE;
                }
            },
            "--pin" | "--pin-temp" => match args.next() {
                Some(p) => pin = Some((PathBuf::from(p), arg == "--pin-temp")),
                None => {
                    eprintln!("snapgo: {arg} needs a file");
                    return glib::ExitCode::FAILURE;
                }
            },
            "--size" | "--at" => {
                let sep = if arg == "--size" { 'x' } else { ',' };
                let pair = args.next().and_then(|v| {
                    let (a, b) = v.split_once(sep)?;
                    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
                });
                match (pair, arg.as_str()) {
                    (Some(p), "--size") => size = Some(p),
                    (Some(p), _) => at = Some(p),
                    (None, _) => {
                        eprintln!("snapgo: {arg} needs two numbers, like 640{sep}480");
                        return glib::ExitCode::FAILURE;
                    }
                }
            }
            other => {
                eprintln!("snapgo: unknown option '{other}' (try --help)");
                return glib::ExitCode::FAILURE;
            }
        }
    }

    // Normally single-instance: a second launch while the first is still
    // serving the clipboard simply starts a new capture in that process.
    // --image must run in this process, since it carries an argument.
    if let Some((file, temp)) = pin {
        return pin::run(
            APP_ID,
            pin::PinArgs {
                file,
                temp,
                size,
                at,
            },
        );
    }

    let flags = if image.is_some() {
        gio::ApplicationFlags::NON_UNIQUE
    } else {
        gio::ApplicationFlags::empty()
    };
    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(flags)
        .build();

    let busy = Rc::new(Cell::new(false));
    app.connect_activate(move |app| {
        if busy.replace(true) {
            return; // an overlay is already open
        }
        let guard = app.hold();
        let busy = busy.clone();
        let finish = move || {
            busy.set(false);
            drop(guard);
        };

        if let Some(path) = &image {
            match load_png(path) {
                Ok(shot) => overlay::start(app, shot, finish),
                Err(e) => {
                    eprintln!("snapgo: {e}");
                    finish();
                }
            }
            return;
        }

        let Some(conn) = app.dbus_connection() else {
            eprintln!("snapgo: no session bus connection");
            finish();
            return;
        };
        let app = app.clone();
        debug("asking the portal for a screenshot");
        portal::screenshot(&conn, move |res| {
            debug(&format!("portal answered: {res:?}"));
            let shot = res.and_then(|path| {
                let shot = load_png(&path);
                // The portal wrote this file just for us; don't leave it behind.
                let _ = std::fs::remove_file(&path);
                shot
            });
            match shot {
                Ok(shot) => overlay::start(&app, shot, finish),
                Err(e) => {
                    eprintln!("snapgo: {e}");
                    finish();
                }
            }
        });
    });

    app.run_with_args::<&str>(&[])
}
