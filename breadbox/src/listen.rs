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
    // Same as running `breadbox` from a keybind: toggle the overlay via the
    // existing singleton. Spawn success is the command confirmation — we do
    // not wait for the GTK window to map.
    let result = spawn_self();
    let client = BreadClient::connect(APP_ID);
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
