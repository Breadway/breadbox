# breadbox

A GTK4 app launcher for Hyprland / Wayland on Arch Linux.

```
breadbox-shared   shared types (DesktopEntry, IconCache, Config)
breadbox-sync     standalone icon resolution + caching binary
breadbox          GTK4 layer-shell launcher
```

## Features

- Layer-shell window, centered 600 px wide, keyboard-exclusive
- Reads the active Hyprland workspace and sorts apps by context priority
- Launch history: non-priority apps sort by most-launched first, then alphabetically
- Fuzzy filtering as you type; Enter or click to launch, Escape or click outside to close
- App icons loaded from the resolved icon cache (see `breadbox-sync`)
- pywal palette auto-detected from `~/.cache/wal/colors.json`, falls back to Catppuccin Mocha
- User CSS override at `~/.config/breadbox/style.css`
- Toggle/dismiss: running a second instance kills the first

## Build dependencies

```
gtk4                (pacman -S gtk4)
gtk4-layer-shell    (pacman -S gtk4-layer-shell)
librsvg             (pacman -S librsvg)   # for SVG icon support
rust (stable)       (rustup toolchain install stable)
```

## Build

```bash
# debug
cargo build

# release (recommended — put both binaries on $PATH)
cargo build --release
# binaries are at target/release/breadbox and target/release/breadbox-sync
```

Install to `~/.cargo/bin` (or anywhere on your PATH):

```bash
cargo install --path breadbox
cargo install --path breadbox-sync
```

## Configuration

Copy and edit the example config:

```bash
mkdir -p ~/.config/breadbox
cp config.example.toml ~/.config/breadbox/config.toml
```

The `[[context]]` blocks map Hyprland workspace names to app priority lists.
Workspace name `"default"` is the catch-all fallback.

```toml
[[context]]
name = "default"
priority = ["firefox", "code", "obsidian", "kitty"]

[[context]]
name = "2"
priority = ["slack", "discord"]
```

### CSS theming

breadbox applies pywal colors automatically when `~/.cache/wal/colors.json` is
present. To override or extend the theme:

```bash
~/.config/breadbox/style.css
```

This file is loaded at the highest CSS priority level, so any rule here wins.

## Icon sync

`breadbox-sync` resolves icons for all installed apps and writes them to
`~/.cache/breadbox/`. Run it once before first launch:

```bash
breadbox-sync
```

Icon resolution order:
1. System icon theme (`~/.local/share/icons`, `/usr/share/icons`, `/usr/share/pixmaps`) — 64 px > 48 px > 128 px > 32 px > 256 px PNG, then SVG
2. Flathub appstream CDN — for reverse-DNS app IDs (e.g. `org.gnome.Gedit`)
3. `application-x-executable` fallback from system theme

### Systemd service (run on login)

```bash
cp packaging/breadbox-sync.service ~/.config/systemd/user/
systemctl --user enable --now breadbox-sync.service
```

The service runs `breadbox-sync` once at login (after network is up) and logs
to journald. Re-run manually after installing new apps:

```bash
systemctl --user start breadbox-sync.service
# or just:
breadbox-sync
```

## Hyprland keybind

Add to `~/.config/hypr/hyprland.conf`:

```
bind = $mainMod, SPACE, exec, breadbox
```

Pressing the keybind again while the launcher is open dismisses it.

## Licence

MIT — see [LICENSE](LICENSE).
