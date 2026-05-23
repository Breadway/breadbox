use std::{
    collections::HashMap,
    env,
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

const CACHE_TIMEOUT_SECS: u64 = 86400;

fn home_dir() -> PathBuf {
    PathBuf::from(env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
}

fn cache_path() -> PathBuf {
    let dir = env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".cache"));
    dir.join("breadbox.cache")
}

fn app_dirs() -> [PathBuf; 2] {
    [
        PathBuf::from("/usr/share/applications"),
        home_dir().join(".local/share/applications"),
    ]
}

fn mtime(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs())
        .unwrap_or(0)
}

fn cache_valid(cache: &Path) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let cm = mtime(cache);
    if now.saturating_sub(cm) >= CACHE_TIMEOUT_SECS {
        return false;
    }
    app_dirs().iter().all(|d| !d.is_dir() || mtime(d) <= cm)
}

fn strip_exec_codes(exec: &str) -> String {
    let mut out = String::with_capacity(exec.len());
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            match chars.peek().copied() {
                Some('%') => {
                    chars.next();
                    out.push('%');
                }
                Some(n) if n.is_ascii_alphabetic() => {
                    chars.next();
                }
                _ => out.push(c),
            }
        } else {
            out.push(c);
        }
    }
    out
}

struct App {
    name: String,
    exec: String,
    terminal: bool,
}

fn parse_desktop(path: &Path) -> Option<App> {
    let file = File::open(path).ok()?;
    let mut in_entry = false;
    let (mut name, mut exec, mut app_type) = (None::<String>, None::<String>, None::<String>);
    let (mut no_display, mut hidden, mut terminal) = (false, false, false);

    for line in BufReader::new(file).lines() {
        let Ok(raw) = line else { continue };
        let s = raw.trim();
        if s.starts_with('#') || s.is_empty() {
            continue;
        }
        if s.starts_with('[') {
            in_entry = s == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }

        if let Some(v) = s.strip_prefix("Name=") {
            name.get_or_insert_with(|| v.to_string());
        } else if let Some(v) = s.strip_prefix("Exec=") {
            exec.get_or_insert_with(|| v.to_string());
        } else if let Some(v) = s.strip_prefix("Type=") {
            app_type.get_or_insert_with(|| v.to_string());
        } else if let Some(v) = s.strip_prefix("NoDisplay=") {
            no_display = v == "true";
        } else if let Some(v) = s.strip_prefix("Hidden=") {
            hidden = v == "true";
        } else if let Some(v) = s.strip_prefix("Terminal=") {
            terminal = v == "true" || v == "1";
        }
    }

    if no_display || hidden {
        return None;
    }
    if app_type.as_deref().is_some_and(|t| t != "Application") {
        return None;
    }

    let name = name?.trim().to_string();
    let exec = strip_exec_codes(exec?.trim()).trim().to_string();
    if name.is_empty() || exec.is_empty() {
        return None;
    }

    Some(App { name, exec, terminal })
}

fn build_cache(cache: &Path) {
    let _ = fs::create_dir_all(cache.parent().unwrap_or(Path::new("/tmp")));
    let mut apps: HashMap<String, App> = HashMap::new();

    for dir in &app_dirs() {
        let Ok(entries) = fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            if let Some(app) = parse_desktop(&path) {
                apps.insert(id, app);
            }
        }
    }

    let mut lines: Vec<String> = apps
        .into_values()
        .map(|a| {
            let prefix = if a.terminal { "term" } else { "app" };
            format!("{}\t{}::{}", a.name, prefix, a.exec)
        })
        .collect();
    lines.sort_unstable();

    let tmp = cache.with_extension("tmp");
    if let Ok(mut f) = File::create(&tmp) {
        for line in &lines {
            let _ = writeln!(f, "{}", line);
        }
        let _ = fs::rename(&tmp, cache);
    }
}

fn pick_terminal() -> String {
    if let Ok(t) = env::var("TERMINAL") {
        if !t.is_empty() {
            return t;
        }
    }
    let path_var = env::var("PATH").unwrap_or_default();
    for t in ["foot", "kitty", "alacritty", "wezterm", "ghostty", "xterm"] {
        if path_var.split(':').any(|d| Path::new(d).join(t).exists()) {
            return t.to_string();
        }
    }
    "xterm".to_string()
}

fn main() {
    let cache = cache_path();

    if env::var("BREADBOX_REBUILD_ONLY").as_deref() == Ok("1") {
        build_cache(&cache);
        return;
    }

    // Toggle: second press closes an open wofi instance
    if Command::new("pgrep")
        .args(["-f", "wofi.*breadbox"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        let _ = Command::new("pkill")
            .args(["-f", "wofi.*breadbox"])
            .status();
        return;
    }

    // Stale-while-revalidate: never block on a rebuild if cache exists
    if !cache.exists() {
        build_cache(&cache);
    } else if !cache_valid(&cache) {
        if let Ok(exe) = env::current_exe() {
            let _ = Command::new(exe)
                .env("BREADBOX_REBUILD_ONLY", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
    }

    let content = fs::read_to_string(&cache).unwrap_or_default();

    let mut child = match Command::new("wofi")
        .args([
            "--dmenu",
            "--parse-search",
            "--matching",
            "fuzzy",
            "--insensitive",
            "--prompt",
            "breadbox",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return,
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = write!(stdin, "{}", content);
    }

    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(_) => return,
    };

    let choice = std::str::from_utf8(&out.stdout)
        .unwrap_or("")
        .trim()
        .to_string();
    if choice.is_empty() {
        return;
    }

    let action = choice.split('\t').nth(1).unwrap_or("");

    if let Some(cmd) = action.strip_prefix("app::") {
        let _ = Command::new("bash")
            .args(["-c", cmd])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    } else if let Some(cmd) = action.strip_prefix("term::") {
        let term = pick_terminal();
        let _ = Command::new(&term)
            .args(["-e", "bash", "-c", cmd])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}
