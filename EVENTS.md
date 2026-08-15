# breadbox — bread event integration

breadbox is a standalone app launcher: it works exactly the same with or
without `breadd` running. When breadd *is* present, the GTK launcher
publishes a single event into the shared bread automation fabric after a
successful launch. See the parent `bread` repo's `Documentation.md` —
specifically its "Namespaces" and "Integrating a bread\* app" sections —
for the general convention this follows.

App id: **`box`**. Transport: `bread-utils`'s `bread_client` module
(feature `bread-client`) — `breadbox` links it directly. breadbox is a
short-lived process (it exits when the launcher closes), so each `emit`
is its own short-lived connection. There is no long-running daemon and
therefore no command subscription.

## Events published (`bread.box.*`)

| Event | Data | When |
|-------|------|------|
| `bread.box.launched` | `{ "id": "<desktop id or exec>", "name": "<display name>" }` | The user launched an app (Enter / keypad Enter on the selected row, or activating a row) **and** the spawn succeeded. Not emitted if `Command::spawn` fails (missing terminal, `exec` that cannot start). `id` is the desktop-file id (the `.desktop` filename, e.g. `firefox.desktop`), falling back to the stripped `Exec=` line when that id is empty. `name` is the desktop-entry display name. |

Launch history is local to breadbox (`~/.cache/breadbox/history.json`);
the event bus is a notification that a launch happened, not a channel
for the exec line's arguments or the resulting process.

## Commands honored (`bread.command.box.*`)

None. breadbox is not a daemon — it is not running (and not subscribed)
except while the launcher overlay is open. There is no existing
"launch this desktop id from the bus" product surface, and inventing
`bread.command.box.launch` (or similar) without that surface would be a
stub. If/when breadbox grows a long-running piece that can honor a
verb, the corresponding `bread.command.box.*` command should be added
at the same time, not stubbed out ahead of it.

## Fail-safe behavior

- If breadd isn't installed or isn't running, `emit` is a silent no-op
  (`BreadClient::emit` never blocks or errors the caller) — launching,
  history, theming, and the singleton toggle are entirely unaffected.
- Closing the launcher without launching anything emits nothing.
