//! Screen capture through xdg-desktop-portal, spoken directly over D-Bus.
//!
//! On GNOME Wayland the portal is the only supported way to grab the screen.
//! We ask for a non-interactive, full-desktop screenshot and do the region
//! selection ourselves on the frozen image.

use gtk4::gio::{self, prelude::*};
use gtk4::glib;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

const BUS: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";

thread_local! {
    static COUNTER: Cell<u32> = const { Cell::new(0) };
}

// Note: GLib registers our application id with the portal by itself (the
// host app registry), so the permission below is remembered per app as long
// as `org.snapgo.Snapgo.desktop` is installed.

/// Take a screenshot of the whole desktop. `done` receives the path of the
/// PNG the portal wrote.
#[allow(deprecated)] // the id-based API suits a one-shot subscription
pub fn screenshot<F>(conn: &gio::DBusConnection, done: F)
where
    F: FnOnce(Result<PathBuf, String>) + 'static,
{
    let n = COUNTER.with(|c| {
        c.set(c.get() + 1);
        c.get()
    });
    let token = format!("snapgo_{}_{}", std::process::id(), n);
    let sender = conn
        .unique_name()
        .map(|s| s.trim_start_matches(':').replace('.', "_"))
        .unwrap_or_default();
    let request_path = format!("{PATH}/request/{sender}/{token}");

    let done: Rc<RefCell<Option<F>>> = Rc::new(RefCell::new(Some(done)));
    let sub: Rc<RefCell<Option<gio::SignalSubscriptionId>>> = Rc::new(RefCell::new(None));

    // Subscribe before calling, so the response can never be missed.
    let id = conn.signal_subscribe(
        Some(BUS),
        Some("org.freedesktop.portal.Request"),
        Some("Response"),
        Some(&request_path),
        None,
        gio::DBusSignalFlags::NONE,
        {
            let done = done.clone();
            let sub = sub.clone();
            move |conn, _sender, _path, _iface, _signal, params| {
                if let Some(id) = sub.borrow_mut().take() {
                    conn.signal_unsubscribe(id);
                }
                let Some(done) = done.borrow_mut().take() else {
                    return;
                };
                done(parse_response(params));
            }
        },
    );
    *sub.borrow_mut() = Some(id);

    let opts = glib::VariantDict::new(None);
    opts.insert("handle_token", token.as_str());
    opts.insert("interactive", false);
    opts.insert("modal", false);
    let args = glib::Variant::tuple_from_iter(["".to_variant(), opts.end()]);

    let conn2 = conn.clone();
    conn.call(
        Some(BUS),
        PATH,
        "org.freedesktop.portal.Screenshot",
        "Screenshot",
        Some(&args),
        None,
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
        move |res| {
            if let Err(e) = res {
                if let Some(id) = sub.borrow_mut().take() {
                    conn2.signal_unsubscribe(id);
                }
                if let Some(done) = done.borrow_mut().take() {
                    done(Err(format!("screenshot portal call failed: {e}")));
                }
            }
        },
    );
}

fn parse_response(params: &glib::Variant) -> Result<PathBuf, String> {
    let (code, results) = params
        .get::<(u32, HashMap<String, glib::Variant>)>()
        .ok_or_else(|| format!("unexpected portal response: {params}"))?;
    match code {
        0 => {}
        1 => return Err("screenshot was cancelled or denied".into()),
        _ => return Err("the screenshot portal reported an error".into()),
    }
    let uri = results
        .get("uri")
        .and_then(|v| v.str().map(str::to_owned))
        .ok_or("portal response has no uri")?;
    gio::File::for_uri(&uri)
        .path()
        .ok_or_else(|| format!("portal returned a non-local uri: {uri}"))
}
