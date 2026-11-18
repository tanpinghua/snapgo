//! First-run screenshot permission.
//!
//! GNOME asks the user once whether an app may take screenshots, but it only
//! lets the *focused* app raise that prompt. snapgo normally has no window
//! when it captures, so the very first capture is refused. This small window
//! gives snapgo focus, triggers the prompt, and gets out of the way.

use crate::portal;
use gtk4 as gtk;
use gtk4::{glib, prelude::*};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Show the permission window. `done(true)` runs once screenshots work (the
/// window is already gone); `done(false)` if the user closes the window.
pub fn ask(app: &gtk::Application, done: impl FnOnce(bool) + 'static) {
    let done: Rc<RefCell<Option<Box<dyn FnOnce(bool)>>>> =
        Rc::new(RefCell::new(Some(Box::new(done))));

    let icon = gtk::Image::from_icon_name(app.application_id().as_deref().unwrap_or("snapgo"));
    icon.set_pixel_size(64);
    let title = gtk::Label::new(Some("Allow snapgo to take screenshots"));
    title.add_css_class("title-3");
    let text = gtk::Label::new(Some(
        "GNOME asks for this once. Choose “Allow” in the prompt and snapgo \
         starts capturing straight away.",
    ));
    text.set_wrap(true);
    text.set_max_width_chars(44);
    text.set_justify(gtk::Justification::Center);
    let status = gtk::Label::new(None);
    status.set_wrap(true);
    status.set_max_width_chars(44);
    status.set_justify(gtk::Justification::Center);
    status.add_css_class("dim-label");
    let retry = gtk::Button::with_label("Ask again");
    retry.add_css_class("suggested-action");
    retry.set_halign(gtk::Align::Center);
    retry.set_sensitive(false);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 14);
    content.set_margin_top(28);
    content.set_margin_bottom(28);
    content.set_margin_start(32);
    content.set_margin_end(32);
    content.append(&icon);
    content.append(&title);
    content.append(&text);
    content.append(&status);
    content.append(&retry);

    let window = gtk::Window::builder()
        .application(app)
        .title("snapgo")
        .resizable(false)
        .child(&content)
        .build();

    let request: Rc<dyn Fn()> = {
        let (app, window, status, retry, done) = (
            app.clone(),
            window.clone(),
            status.clone(),
            retry.clone(),
            done.clone(),
        );
        Rc::new(move || {
            let Some(conn) = app.dbus_connection() else {
                status.set_text("No connection to the desktop session.");
                return;
            };
            retry.set_sensitive(false);
            status.set_text("Waiting for your answer…");
            let (window, status, retry, done) =
                (window.clone(), status.clone(), retry.clone(), done.clone());
            portal::screenshot(&conn, move |res| match res {
                Ok(path) => {
                    // This capture shows the permission window; discard it.
                    let _ = std::fs::remove_file(&path);
                    let done = done.take();
                    window.destroy();
                    // Let the window finish disappearing before the real capture.
                    glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
                        if let Some(done) = done {
                            done(true);
                        }
                    });
                }
                Err(e) => {
                    status.set_text(&format!(
                        "Screenshots are not allowed yet ({e}). If you chose “Deny”, \
                         allow screenshots for snapgo in Settings, then ask again."
                    ));
                    retry.set_sensitive(true);
                }
            });
        })
    };

    // Ask as soon as the window has focus.
    let asked = Cell::new(false);
    let r = request.clone();
    window.connect_is_active_notify(move |w| {
        if w.is_active() && !asked.replace(true) {
            r();
        }
    });
    retry.connect_clicked(move |_| request());

    window.connect_close_request(move |_| {
        if let Some(done) = done.take() {
            done(false);
        }
        glib::Propagation::Proceed
    });
    window.present();
}
