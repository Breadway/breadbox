# breadbox — bread event integration

breadbox is a standalone app launcher: it works exactly the same with or
without `breadd` running. When breadd *is* present, the GTK launcher
publishes a single event into the shared bread automation fabric after a
successful launch. See the parent `bread` repo's `Documentation.md` —
specifically its "Namespaces" and "Integrating a bread\* app" sections —
for the general convention this follows.

App id: **`box`**. Transport: `bread-utils`'s `bread_client` module
(feature `bread-client`) — `breadbox` links it directly. One-shot
launcher invocations each `emit` on their own fire-and-forget
connection. Command verbs are only received while `breadbox listen` is
running — that process holds the `bread.command.box.**` subscription
open.

## Events published (`bread.box.*`)

| Event | Data | When |
|-------|------|------|
| `bread.box.launched` | `{ "id": "<desktop id or exec>", "name": "<display name>" }` | The user launched an app (Enter / keypad Enter on the selected row, or activating a row) **and** the spawn succeeded. Not emitted if `Command::spawn` fails (missing terminal, `exec` that cannot start). `id` is the desktop-file id (the `.desktop` filename, e.g. `firefox.desktop`), falling back to the stripped `Exec=` line when that id is empty. `name` is the desktop-entry display name. |
| `bread.box.open.done` | `{}` | `bread.command.box.open` was received and `breadbox` was spawned. This is the command confirmation, not proof the overlay mapped — the spawned process is the same toggle as a keybind. |
| `bread.box.open.failed` | `{ "error": "<message>" }` | `bread.command.box.open` was received but this binary could not be started. |

Launch history is local to breadbox (`~/.cache/breadbox/history.json`);
the event bus is a notification that a launch happened, not a channel
for the exec line's arguments or the resulting process.

## Commands honored (`bread.command.box.*`)

These are only received while `breadbox listen` is running. Publishing a
command with no subscriber is a silent no-op — that is the documented
bread convention, not a breadbox bug.

| Verb | Data | Effect |
|------|------|--------|
| `open` | none | Same as running `breadbox` (toggle the launcher overlay via the existing singleton). Emits `bread.box.open.done` / `.failed`. |

```lua
bread.spawn(function()
    bread.emit("bread.command.box.open")
    bread.wait("bread.box.open.done", { timeout = 5000 })
end)
```

### Not implemented: extra verbs

There is no `launch` / `close` / `query` command verb. Picking a desktop
id from the bus would be a new product surface. If/when that exists, add
the corresponding `bread.command.box.*` verb at the same time, not
stubbed as a no-op ahead of it.

## Fail-safe behavior

- If breadd isn't installed or isn't running, `emit` is a silent no-op
  (`BreadClient::emit` never blocks or errors the caller) and the
  command subscription simply never receives anything — launching,
  history, theming, and the singleton toggle are entirely unaffected.
- If breadd restarts, the command subscription reconnects automatically
  (`BreadClient::subscribe`'s background thread has its own backoff
  loop); no restart of `breadbox listen` is needed.
- If `breadbox listen` is not running, commands are a graceful no-op at
  the bus (no subscriber). The CLI still works, and one-shot invocations
  still emit `bread.box.launched` on their own short-lived connection.
- Closing the launcher without launching anything emits nothing.
