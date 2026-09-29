//! Private, bounded diagnostics for intermittent launcher input problems.

use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const MAX_LOG_BYTES: u64 = 1024 * 1024;
const LOG_FILE: &str = "breadbox.log";

type SharedLog = Arc<Mutex<RotatingLog>>;

struct RotatingLog {
    path: PathBuf,
    file: File,
    size: u64,
}

struct DiagnosticWriter {
    log: SharedLog,
    buffer: Vec<u8>,
}

impl Write for DiagnosticWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.buffer.len().saturating_add(buf.len()) as u64 > MAX_LOG_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "diagnostic event exceeds maximum log size",
            ));
        }
        self.buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.commit()
    }
}

impl DiagnosticWriter {
    fn commit(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let mut log = self
            .log
            .lock()
            .map_err(|_| io::Error::other("diagnostic log lock poisoned"))?;
        log.rotate_if_needed(self.buffer.len() as u64)?;
        log.file.write_all(&self.buffer)?;
        log.size += self.buffer.len() as u64;
        self.buffer.clear();
        Ok(())
    }
}

impl Drop for DiagnosticWriter {
    fn drop(&mut self) {
        let _ = self.commit();
    }
}

impl RotatingLog {
    fn open(path: PathBuf) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
            #[cfg(unix)]
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }

        let size = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        let mut log = Self {
            file: open_private_append(&path)?,
            path,
            size,
        };
        if log.size >= MAX_LOG_BYTES {
            log.rotate()?;
        }
        Ok(log)
    }

    fn rotate_if_needed(&mut self, incoming: u64) -> io::Result<()> {
        if self.size.saturating_add(incoming) > MAX_LOG_BYTES {
            self.rotate()?;
        }
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        let previous = rotated_path(&self.path);
        match fs::remove_file(&previous) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        match fs::rename(&self.path, &previous) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        self.file = open_private_append(&self.path)?;
        self.size = 0;
        Ok(())
    }
}

#[cfg(unix)]
fn open_private_append(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

#[cfg(not(unix))]
fn open_private_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn rotated_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".1");
    PathBuf::from(name)
}

fn state_dir(xdg_state_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    if let Some(path) = xdg_state_home
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Some(path.join("breadbox"));
    }

    home.filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .map(|path| path.join(".local").join("state").join("breadbox"))
}

fn log_path() -> Option<PathBuf> {
    state_dir(
        env::var("XDG_STATE_HOME").ok().as_deref(),
        env::var("HOME").ok().as_deref(),
    )
    .map(|path| path.join(LOG_FILE))
}

/// Installs persistent diagnostics and returns the private log path when the
/// state directory could be opened. If it cannot, logs continue to stderr.
pub fn init() -> Option<PathBuf> {
    let Some(path) = log_path() else {
        install_stderr_subscriber();
        return None;
    };
    let sink = match RotatingLog::open(path.clone()) {
        Ok(log) => Arc::new(Mutex::new(log)),
        Err(error) => {
            eprintln!(
                "breadbox: could not open diagnostics log {}: {error}",
                path.display()
            );
            install_stderr_subscriber();
            return None;
        }
    };

    let filter = diagnostic_filter();
    let writer = sink.clone();
    if tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_target(true)
        .with_writer(move || DiagnosticWriter {
            log: writer.clone(),
            buffer: Vec::new(),
        })
        .try_init()
        .is_err()
    {
        return None;
    }

    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        // Avoid copying panic payloads or source locations into the shareable
        // report because they may contain user data.
        tracing::error!(target: "breadbox::diagnostics", event = "panic_observed");
        previous_hook(panic);
    }));

    Some(path)
}

fn install_stderr_subscriber() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(diagnostic_filter())
        .with_ansi(false)
        .with_target(true)
        .try_init();
}

/// Do not use `RUST_LOG` for either destination. It may enable unrelated
/// targets whose event fields can contain user input.
fn diagnostic_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::new("off,breadbox::diagnostics=info")
}

/// Emits only an allowlist of session context. In particular, it never reads
/// configuration contents, display names, typed input, or query contents.
pub fn log_startup_context() {
    tracing::info!(
        target: "breadbox::diagnostics",
        event = "startup",
        desktop = sanitized_label("XDG_CURRENT_DESKTOP"),
        session_type = session_type(),
        wayland_display_present = env::var_os("WAYLAND_DISPLAY").is_some(),
        gtk_version = format!("{}.{}.{}", gtk4::major_version(), gtk4::minor_version(), gtk4::micro_version()),
        keyboard_mode = "exclusive",
    );
}

fn sanitized_label(name: &str) -> String {
    env::var(name)
        .ok()
        .as_deref()
        .map(safe_desktop_names)
        .unwrap_or_else(|| "unknown".to_owned())
}

fn safe_desktop_names(value: &str) -> String {
    const KNOWN: &[&str] = &[
        "cosmic", "gnome", "hyprland", "i3", "kde", "sway", "xfce", "wayfire",
    ];
    let names: Vec<_> = value
        .split(':')
        .flat_map(|part| part.split(','))
        .filter_map(|name| {
            KNOWN
                .iter()
                .find(|known| known.eq_ignore_ascii_case(name.trim()))
                .copied()
        })
        .collect();
    if names.is_empty() {
        "unknown".to_owned()
    } else {
        names.join(",")
    }
}

fn session_type() -> &'static str {
    match env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("wayland") => "wayland",
        Ok("x11") => "x11",
        _ => "unknown",
    }
}

/// Prints a copyable report containing only the diagnostic target's events.
pub fn print_bug_report() {
    let path = log_path();
    println!("# breadbox diagnostics");
    println!("desktop: {}", sanitized_label("XDG_CURRENT_DESKTOP"));
    println!("session type: {}", session_type());
    println!(
        "Wayland display configured: {}",
        env::var_os("WAYLAND_DISPLAY").is_some()
    );
    println!(
        "GTK version: {}.{}.{}",
        gtk4::major_version(),
        gtk4::minor_version(),
        gtk4::micro_version()
    );

    let Some(path) = path else {
        println!("log: unavailable, HOME and XDG_STATE_HOME are unset");
        return;
    };
    let display_path = if env::var("XDG_STATE_HOME")
        .ok()
        .map(PathBuf::from)
        .is_some_and(|path| path.is_absolute())
    {
        "$XDG_STATE_HOME/breadbox/breadbox.log"
    } else {
        "$HOME/.local/state/breadbox/breadbox.log"
    };
    println!("log path: {display_path}");
    println!("\n## Recent breadbox diagnostic events");

    let mut log_contents = Vec::new();
    for candidate in [rotated_path(&path), path.clone()] {
        if let Ok(mut file) = File::open(candidate) {
            let mut content = String::new();
            if file.read_to_string(&mut content).is_ok() {
                log_contents.push(content);
            }
        }
    }
    let events = recent_diagnostic_events(log_contents.iter().map(String::as_str));
    if events.is_empty() {
        println!("No diagnostic events recorded yet.");
    } else {
        for line in &events {
            println!("{line}");
        }
    }
    println!("\nReview this report before sharing it. Typed text is not recorded.");
}

fn recent_diagnostic_events<'a>(files: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let events: Vec<_> = files
        .into_iter()
        .flat_map(str::lines)
        .filter(|line| line.contains("breadbox::diagnostics"))
        .map(str::to_owned)
        .collect();
    events
        .into_iter()
        .rev()
        .take(200)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_absolute_xdg_state_home() {
        assert_eq!(
            state_dir(Some("/state"), Some("/home/user")),
            Some(PathBuf::from("/state/breadbox"))
        );
    }

    #[test]
    fn falls_back_to_home_state_directory() {
        assert_eq!(
            state_dir(None, Some("/home/user")),
            Some(PathBuf::from("/home/user/.local/state/breadbox"))
        );
    }

    #[test]
    fn ignores_relative_xdg_state_home() {
        assert_eq!(
            state_dir(Some("relative"), Some("/home/user")),
            Some(PathBuf::from("/home/user/.local/state/breadbox"))
        );
    }

    #[test]
    fn returns_none_without_a_home_directory() {
        assert_eq!(state_dir(None, None), None);
    }

    #[test]
    fn desktop_label_is_restricted_to_safe_characters() {
        assert_eq!(safe_desktop_names("Hyprland:GNOME"), "hyprland,gnome");
        assert_eq!(safe_desktop_names("TOKEN=secret\n"), "unknown");
    }

    #[test]
    fn rotated_file_uses_a_single_backup_name() {
        assert_eq!(
            rotated_path(Path::new("/state/breadbox/breadbox.log")),
            PathBuf::from("/state/breadbox/breadbox.log.1")
        );
    }

    #[test]
    fn bug_report_only_includes_diagnostic_target_lines() {
        let events = recent_diagnostic_events([
            "INFO breadbox::diagnostics event=key_pressed keycode=28",
            "INFO bread_launcher event=query query=private text",
            "INFO breadbox::diagnostics event=main_loop_heartbeat tick=1",
        ]);
        assert_eq!(events.len(), 2);
        assert!(!events.iter().any(|line| line.contains("private text")));
    }

    #[test]
    fn subscriber_filters_other_targets_with_broad_rust_log() {
        const CHILD_MARKER: &str = "BREADBOX_DIAGNOSTIC_FILTER_TEST_CHILD";
        if env::var_os(CHILD_MARKER).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg("diagnostics::tests::subscriber_filters_other_targets_with_broad_rust_log")
                .arg("--nocapture")
                .env("RUST_LOG", "trace")
                .env(CHILD_MARKER, "1")
                .status()
                .expect("run isolated filter test process");
            assert!(status.success());
            return;
        }

        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after UNIX epoch")
            .as_nanos();
        let directory =
            env::temp_dir().join(format!("breadbox-filter-{}-{nonce}", std::process::id()));
        let path = directory.join(LOG_FILE);
        let log = Arc::new(Mutex::new(RotatingLog::open(path.clone()).unwrap()));
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(diagnostic_filter())
            .with_ansi(false)
            .with_target(true)
            .with_writer(move || DiagnosticWriter {
                log: writer.clone(),
                buffer: Vec::new(),
            })
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "breadbox::diagnostics", event = "allowed_event");
            tracing::info!(target: "bread_launcher", query = "private query text");
        });

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("allowed_event"));
        assert!(!contents.contains("private query text"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rotation_keeps_one_bounded_backup() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after UNIX epoch")
            .as_nanos();
        let directory = env::temp_dir().join(format!(
            "breadbox-diagnostics-{}-{nonce}",
            std::process::id()
        ));
        let path = directory.join(LOG_FILE);
        let mut log = RotatingLog::open(path.clone()).expect("open temporary diagnostic log");
        let old_contents = vec![b'x'; (MAX_LOG_BYTES - 1) as usize];
        log.file
            .write_all(&old_contents)
            .expect("write temporary log contents");
        log.size = old_contents.len() as u64;

        log.rotate_if_needed(2)
            .expect("rotate before exceeding size limit");
        log.file.write_all(b"ok").expect("write new log contents");
        log.size = 2;
        drop(log);

        assert_eq!(
            fs::metadata(rotated_path(&path)).unwrap().len(),
            MAX_LOG_BYTES - 1
        );
        assert_eq!(fs::metadata(&path).unwrap().len(), 2);
        fs::remove_dir_all(directory).expect("remove temporary diagnostic log");
    }
}
