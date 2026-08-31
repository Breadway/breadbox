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

Under an `[launcher] mode = "embedded"` theme (spotlight), breadbar's
own bar-drawer capsule *is* the launcher UI, and breadbox redirects
instead of mapping its own overlay window — see the "Embedded launcher
theme" section below. Everything else on this page describes the
default (`overlay`-mode) behavior.

## Events published (`bread.box.*`)

| Event | Data | When |
|-------|------|------|
| `bread.box.launched` | `{ "id": "<desktop id or exec>", "name": "<display name>" }` | The user launched an app (Enter / keypad Enter on the selected row, or activating a row) **and** the spawn succeeded. Not emitted if `Command::spawn` fails (missing terminal, `exec` that cannot start). `id` is the desktop-file id (the `.desktop` filename, e.g. `firefox.desktop`), falling back to the stripped `Exec=` line when that id is empty. `name` is the desktop-entry display name. |
| `bread.box.open.done` | `{}` | `bread.command.box.open` was received (via `breadbox listen`) **and this binary was spawned** — non-embedded themes only. Not proof the overlay mapped, just that the process was started (same toggle as a keybind). Never emitted under an embedded theme — see below. |
| `bread.box.open.failed` | `{ "error": "<message>" }` | `bread.command.box.open` was received but this binary could not be started — non-embedded themes only. Unreachable under an embedded theme, since that branch returns before ever attempting to spawn. |
| `bread.box.open_requested` | `{}` | Emitted by the plain `breadbox` binary itself (not `breadbox listen`) instead of mapping its own overlay window, only when the active theme's `[launcher].mode` is `embedded`. breadbar's capsule (`launcher_command.rs`) subscribes to this and opens itself. Own-namespace event, not a command — see `dispatch_embedded_open`'s doc comment for why `bread.command.box.open` would be the wrong shape here. |
| `bread.box.open.redirected` | `{}` | Emitted by `breadbox listen`'s `handle_open`, only when `bread.command.box.open` is received while an embedded theme is active. Replaces `.done`/`.failed` in that case: this process never spawns anything (breadbar's capsule is the intended handler) and has no way to confirm breadbar actually received or handled the event — pub/sub here is one-way with no ack. This event means "redirect happened, outcome unknown", not "succeeded". |

Launch history is local to breadbox (`~/.cache/breadbox/history.json`);
the event bus is a notification that a launch happened, not a channel
for the exec line's arguments or the resulting process.

## Commands honored (`bread.command.box.*`)

These are only received while `breadbox listen` is running. Publishing a
command with no subscriber is a silent no-op — that is the documented
bread convention, not a breadbox bug.

| Verb | Data | Effect |
|------|------|--------|
| `open` | none | Under a non-embedded theme: same as running `breadbox` (toggle the launcher overlay via the existing singleton). Emits `bread.box.open.done` / `.failed`. Under an embedded theme: never spawns; emits `bread.box.open.redirected` instead — see "Embedded launcher theme" below. |

```lua
-- Non-embedded themes: `.done` is a real completion signal.
bread.spawn(function()
    bread.emit("bread.command.box.open")
    bread.wait("bread.box.open.done", { timeout = 5000 })
end)
```

Under an embedded theme this `.done` wait will time out — `.done` is
never emitted there. A workflow that needs to work under every theme
should wait on `bread.box.open.redirected` too (or treat a timeout as
"probably fine, embedded themes have no ack" rather than a failure).

### Not implemented: extra verbs

There is no `launch` / `close` / `query` command verb. Picking a desktop
id from the bus would be a new product surface. If/when that exists, add
the corresponding `bread.command.box.*` verb at the same time, not
stubbed as a no-op ahead of it.

## Embedded launcher theme (spotlight)

When the active shell theme's `[launcher].mode` is `embedded`, running
`breadbox` directly (e.g. from a keybind) does **not** map this binary's
overlay window — that would stack a second launcher on top of
breadbar's own capsule. Instead:

1. `breadbox`'s `main` checks `BreadClient::health()` — a real,
   bounded-timeout round trip to breadd, not just "did the socket
   exist".
2. If breadd **is** reachable, it emits `bread.box.open_requested` and
   returns. breadbar's capsule (subscribed only while its own active
   theme is also `embedded`) is expected to open itself in response.
   There is no ack for this — if breadbar isn't running, or is running
   under a different theme and therefore never subscribed, this is
   still a silent no-op on the bus with nothing further to fall back
   to. (`BreadClient` has no "is anyone subscribed" query.)
3. If breadd is **not** reachable, `breadbox` logs that to stderr and
   falls back to mapping its own overlay window anyway — a keybind
   press always opens *something*, rather than the historical fully
   silent no-op when the bus was down.

`breadbox listen`'s `handle_open` (the `bread.command.box.open`
handler) applies the same "never map a second launcher" rule but from
the command side: under an embedded theme it never spawns, logs to
stderr, and emits `bread.box.open.redirected` instead of `.done` — see
the events table above for why `.done` would be a false claim here.

## Fail-safe behavior

- If breadd isn't installed or isn't running, `emit` is a silent no-op
  (`BreadClient::emit` never blocks or errors the caller) and the
  command subscription simply never receives anything — launching,
  history, theming, and the singleton toggle are entirely unaffected.
  Under an embedded theme specifically, a direct `breadbox` invocation
  additionally checks reachability up front and falls back to mapping
  its own overlay window when breadd is down (see above) — so this
  invariant now holds there too, not just for every other theme.
- If breadd restarts, the command subscription reconnects automatically
  (`BreadClient::subscribe`'s background thread has its own backoff
  loop); no restart of `breadbox listen` is needed.
- If `breadbox listen` is not running, commands are a graceful no-op at
  the bus (no subscriber). The CLI still works, and one-shot invocations
  still emit `bread.box.launched` on their own short-lived connection.
- Closing the launcher without launching anything emits nothing.
- What is **not** covered: under an embedded theme, if breadd is
  reachable but breadbar itself isn't running (or isn't subscribed),
  neither the direct-keybind path nor the command path can detect that
  or fall back further — both are fire-and-forget with no ack. This is
  a known limitation of the current transport, not an oversight.
