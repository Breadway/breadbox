//! Long-running command subscription for `bread.command.box.*`.
//!
//! `breadbox` is still a one-shot toggle overlay by default. `breadbox listen`
//! is the optional persistent process that can honor bus commands. See
//! `EVENTS.md`.

use bread_utils::bread_client::{BreadClient, BreadEvent};

/// Sibling-app id in `bread_shared::apps::KNOWN_APPS`.
const APP_ID: &str = "box";

/// Subscribe to `bread.command.box.**` and block until the process is killed.
///
/// breadd being absent is not an error: [`BreadClient::subscribe`] reconnects
/// with backoff, and `on_event` simply isn't called until the daemon is up.
pub fn run() {
    let client = BreadClient::connect(APP_ID);
    if client.health().is_none() {
        eprintln!("breadbox: breadd unreachable; command subscription will connect when it comes back");
    }

    let _commands = client.subscribe("bread.command.box.**", |event| {
        handle_command(&event);
    });

    eprintln!("breadbox: listening for bread.command.box.**");
    loop {
        std::thread::park();
    }
}

/// Reacts to `bread.command.box.*` verbs. Only `open` is honored today —
/// other verbs are ignored, not stubbed as no-ops that pretend to succeed.
fn handle_command(event: &BreadEvent) {
    let Some(verb) = command_verb(&event.event) else {
        return;
    };
    match verb {
        "open" => handle_open(),
        other => {
            eprintln!("breadbox: ignoring unrecognized bread.command.box.{other}");
        }
    }
}

fn handle_open() {
    let client = BreadClient::connect(APP_ID);

    // Under an embedded-launcher theme (spotlight), breadbar's own capsule
    // subscribes to both this command and `bread.box.open_requested` (see
    // `main.rs`'s `dispatch_embedded_open` doc comment) — it, not this
    // process, is the thing that should react. Spawning `breadbox` here
    // would just re-run that same binary's own embedded-mode redirect
    // (`main`'s `dispatch_embedded_open`), emitting
    // `bread.box.open_requested` back onto the bus; breadbar would open the
    // capsule again and this still-running subscription would keep spawning
    // on every command it sees. So this branch never spawns — that part is
    // correct and stays.
    //
    // It used to also emit `bread.box.open.done` here unconditionally, which
    // was a false-positive success report: `.done`'s documented meaning
    // (EVENTS.md) is "breadbox was spawned", which is literally untrue in
    // this branch, and there is no ack from breadbar on this one-way
    // pub/sub bus — `BreadClient` has no way to ask "did anything actually
    // pick this up" (same limitation `main.rs`'s `dispatch_embedded_open`
    // documents). Claiming `.done` reported a completion this process
    // cannot observe — the same shape of bug as bug #6 (a namespace
    // violation making a false claim), just an over-eager "done" instead of
    // an under-eager warning. Since confirming the handoff isn't possible
    // with the current transport, this does the honest thing instead: log
    // locally (so the failure mode is diagnosable from `breadbox listen`'s
    // own output) and emit a distinct, explicitly-unconfirmed event so a bus
    // observer can tell "redirected, outcome unknown" apart from "breadbox
    // spawned" rather than being told a specific untrue thing.
    if crate::theme::shell_theme().launcher().mode == bread_theme::shell::LauncherMode::Embedded {
        eprintln!(
            "breadbox: bread.command.box.open received under an embedded launcher \
             theme; breadbar's capsule is the intended handler and this process \
             cannot confirm it received the event"
        );
        client.emit("bread.box.open.redirected", serde_json::json!({}));
        return;
    }

    // Same as running `breadbox` from a keybind: toggle the overlay via the
    // existing singleton. Spawn success is the command confirmation — we do
    // not wait for the GTK window to map.
    let result = spawn_self();
    match result {
        Ok(_) => client.emit("bread.box.open.done", serde_json::json!({})),
        Err(e) => {
            eprintln!("breadbox: bread.command.box.open failed: {e}");
            client.emit(
                "bread.box.open.failed",
                serde_json::json!({ "error": e.to_string() }),
            );
        }
    }
}

fn spawn_self() -> std::io::Result<std::process::Child> {
    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("breadbox"));
    std::process::Command::new(exe).spawn()
}

fn command_verb(event_name: &str) -> Option<&str> {
    event_name.strip_prefix("bread.command.box.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_verb_strips_box_prefix() {
        assert_eq!(command_verb("bread.command.box.open"), Some("open"));
        assert_eq!(command_verb("bread.command.box.launch"), Some("launch"));
        assert_eq!(command_verb("bread.command.clip.clear"), None);
        assert_eq!(command_verb("bread.box.launched"), None);
    }
}
