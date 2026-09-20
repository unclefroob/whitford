# Whitford

Whitford is a fast, fixture-driven native mail shell for Wayland. This milestone proves the adaptive three-pane interaction, keyboard workflow, visual language, and Rust/GTK build. It does not connect to a mail server or persist changes.

## Requirements

- Rust 1.98 or newer
- GTK 4.22 development files
- libadwaita 1.9 development files
- A Wayland session (Hyprland is the primary verification environment)

On Arch Linux, install the native prerequisites with:

```sh
sudo pacman -S --needed base-devel rust gtk4 libadwaita pkgconf
```

On the validated development system, `pkg-config --modversion gtk4 libadwaita-1` reports GTK 4.22.5 and libadwaita 1.9.4.

## Run and verify

```sh
GDK_BACKEND=wayland cargo run
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

Manual smoke test at roughly 1584×982, 900×760, and 600×760. Confirm the folder rail changes to an overlay, the message reader changes to navigation with a working back route, resizing back restores the wide layout, all shortcuts work, search accepts normal typing, focus remains visible, and empty/search/deferred-action feedback is deliberate.

## Shortcuts

| Command | Shortcut |
|---|---|
| Compose | Ctrl+N |
| Focus search | Ctrl+F |
| Previous/next folder | Alt+Up / Alt+Down |
| Previous/next message | Ctrl+Up / Ctrl+Down |
| Archive selected | Delete |
| Reply | Ctrl+R |
| Clear search / back / close folders | Escape |

For fixture-state review, Ctrl+Shift+1 shows the online state, Ctrl+Shift+2 shows loading, and Ctrl+Shift+3 shows offline. These preview-only commands do not perform network work.

## Architecture

- `model`: immutable domain values and deterministic fixtures.
- `state`: pure filtering, selection, navigation, counts, and archive transitions.
- `ui`: modular GTK construction, actions, and rendering over owned state snapshots.

Real accounts, IMAP/SMTP/OAuth, MIME/HTML rendering, storage, compose/reply delivery, downloads, and notifications are intentionally deferred. Controls for those operations show an honest toast and do not pretend to succeed.

The Flatpak manifest is early metadata only. It has not been built locally because `flatpak-builder`, the GNOME SDK, and generated offline Cargo sources are not installed.

If Cargo reports a missing `gtk4` or `libadwaita-1` package, confirm `pkg-config --modversion gtk4 libadwaita-1` succeeds and that `pkgconf` plus the development packages are installed. If launch reports no display, run inside a graphical session with `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` set; use `GDK_BACKEND=wayland cargo run` to require the native Wayland backend.
