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

```sh
sudo apt install libgtk-4-dev          # build dependency (Ubuntu/Debian)
./install.sh --shortcut '<Shift><Super>s'
```

This builds a release binary, installs it to `~/.local/bin/snapgo`, installs
the desktop file, and (with `--shortcut`) binds a GNOME keyboard shortcut.
Leave `--shortcut` out to set one yourself in *Settings → Keyboard → Custom
Shortcuts*.

The first capture shows GNOME's one-time "Allow snapgo to take screenshots?"
prompt. The desktop file is what lets GNOME remember the answer.

## Use

| Action | Keys |
| --- | --- |
| Select a region | drag (a plain click selects the whole screen) |
| Adjust the region | drag its handles; with the select tool, drag inside to move |
| Copy and close | `Enter` or `Ctrl+C` |
| Save to `Pictures/Screenshots` and close | `Ctrl+S` |
| Cancel | `Esc` |
| Tools | `V` select · `A` arrow · `L` line · `R` rectangle · `E` ellipse · `P` pen · `H` highlighter · `T` text · `N` numbered marker · `B` pixelate |
| Colour | `1`–`6` |
| Size | mouse wheel, or `[` and `]` |
| Undo / redo | `Ctrl+Z` / `Ctrl+Shift+Z` |
| Delete the selected item | `Delete` |
| Re-edit a text | double-click it (or click it with the text tool) |

With the select tool, click an item to select it; colour and size changes then
apply to that item.

`snapgo --image file.png` opens an existing image in the same overlay.
`SNAPGO_DEBUG=1` prints progress messages.

## Layout

| File | Role |
| --- | --- |
| `src/main.rs` | entry point, single-instance application |
| `src/portal.rs` | screen capture via xdg-desktop-portal (D-Bus) |
| `src/overlay.rs` | overlay windows, input handling, toolbar, copy/save |
| `src/canvas.rs` | the widget that paints screenshot, selection and items |
| `src/model.rs` | shapes, hit-testing, final rendering (unit-tested) |
| `src/icons.rs` | toolbar icons |
