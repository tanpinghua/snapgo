# snapgo

A fast screenshot and annotation tool for GNOME on Wayland.

Press a hotkey, the screen freezes, drag a region, annotate it in place, press
Enter — the image is on your clipboard.

## Why

- **Built for GNOME Wayland.** Capture goes through the desktop screenshot
  portal, the overlay is a native GTK 4 window per monitor, and the output is
  always in real screen pixels (HiDPI and fractional scaling included).
- **A clipboard that works.** After copying, snapgo stays alive without a
  window until something else replaces the clipboard, so pasting never fails.
- **Annotations stay editable.** Every arrow, box, text or blur can be moved,
  recoloured, resized or deleted afterwards; text can be re-edited.
- **Keyboard-first.** Every tool, colour and action has a single-key shortcut.

## Install

### Ubuntu / Debian

Download the latest `.deb` from the
[releases page](https://github.com/tanpinghua/snapgo/releases), then:

```sh
sudo apt install ./snapgo_*_amd64.deb
```

Packages are built on Ubuntu 24.04 and install on that release or newer.

The first capture shows GNOME's one-time "Allow snapgo to take screenshots?"
prompt. Then give it a hotkey, as described next.

### Use the Print Screen key

GNOME uses <kbd>Print Screen</kbd> for its own screenshot tool, so free the key
first, then give it to snapgo. Open *Settings → Keyboard → View and Customise
Shortcuts*, then:

1. Under **Screenshots**, click *Take a screenshot interactively* and press
   <kbd>Backspace</kbd> to disable it.
2. Under **Custom Shortcuts**, press **+** and fill in:
   - Name: `snapgo`
   - Command: `snapgo`
   - Shortcut: press <kbd>Print Screen</kbd>

Any other key combination works the same way; only step 2 is needed if the key
is not already in use.

To undo it, delete the custom shortcut and click *Reset* on *Take a screenshot
interactively*.

### From source

```sh
sudo apt install libgtk-4-dev          # build dependency
./install.sh --shortcut '<Shift><Super>s'
```

To use <kbd>Print Screen</kbd> here, free it from GNOME's own tool first:

```sh
gsettings set org.gnome.shell.keybindings show-screenshot-ui "[]"
./install.sh --shortcut 'Print'
```

This builds a release binary and installs it, the desktop file and the icon
under `~/.local` (no root needed). With `--shortcut` it also binds a GNOME
keyboard shortcut. The desktop file is what lets GNOME remember the screenshot
permission.

## Use

| Action | Keys |
| --- | --- |
| Select a region | drag (a plain click selects the whole screen) |
| Adjust the region | drag its handles; with the select tool, drag inside to move |
| Copy and close | `Enter` or `Ctrl+C` |
| Save to `Pictures/Screenshots` and close | `Ctrl+S` |
| Pin the result on screen and close | `Ctrl+P` |
| Cancel | `Esc` |
| Tools | `V` select · `A` arrow · `L` line · `R` rectangle · `E` ellipse · `P` pen · `H` highlighter · `T` text · `N` numbered marker · `B` blur |
| Colour | `1`–`6` |
| Size | mouse wheel, or `[` and `]` |
| Undo / redo | `Ctrl+Z` / `Ctrl+Shift+Z` |
| Delete the selected item | `Delete` |
| Re-edit a text | double-click it (or click it with the text tool) |

With the select tool, click an item to select it; colour and size changes then
apply to that item.

### Pinned images

`Ctrl+P` (or the pin button) keeps the result on screen as a small frameless
window, exactly where it was captured, above other windows and on every
workspace.

| Action | How |
| --- | --- |
| Move | drag |
| Zoom | mouse wheel |
| Opacity | `Ctrl` + mouse wheel |
| Copy to the clipboard and close | `Ctrl+C` |
| Save to `Pictures/Screenshots` and close | `Ctrl+S` |
| Close | double-click or `Esc` |

Wayland has no "always on top" for applications, so pins run through XWayland,
where GNOME honours it. `snapgo --pin file.png` pins any image.

`snapgo --image file.png` opens an existing image in the same overlay.
`SNAPGO_DEBUG=1` prints progress messages.

## Layout

| File | Role |
| --- | --- |
| `src/main.rs` | entry point, single-instance application |
| `src/portal.rs` | screen capture via xdg-desktop-portal (D-Bus) |
| `src/overlay.rs` | overlay windows, input handling, toolbar, copy/save |
| `src/canvas.rs` | the widget that paints screenshot, selection and items |
| `src/pin.rs` | pinned image windows |
| `src/model.rs` | shapes, hit-testing, final rendering (unit-tested) |
| `src/icons.rs` | toolbar icons |

## Packaging

```sh
cargo install cargo-deb
cargo deb                      # writes target/debian/snapgo_<version>_amd64.deb
```

To publish a release: set `version` in `Cargo.toml`, commit, then push a
matching tag (`git tag v0.2.0 && git push origin v0.2.0`). GitHub Actions
builds the `.deb` and attaches it to a new GitHub Release.

## License

MIT — see [LICENSE](LICENSE).
