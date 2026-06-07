use std::{
    collections::HashMap,
    env,
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

// ---- XDG path helpers -------------------------------------------------------

pub fn home_dir() -> PathBuf {
    PathBuf::from(env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
}

pub fn cache_dir() -> PathBuf {
    env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".cache"))
        .join("breadbox")
}

pub fn config_dir() -> PathBuf {
    env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".config"))
        .join("breadbox")
}

pub fn app_dirs() -> Vec<PathBuf> {
    let home = home_dir();
    let mut dirs = vec![PathBuf::from("/usr/share/applications")];

    let xdg_data_dirs = env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for d in xdg_data_dirs.split(':') {
        let p = PathBuf::from(d).join("applications");
        if p != dirs[0] {
            dirs.push(p);
        }
    }

    dirs.push(
        env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".local/share"))
            .join("applications"),
    );
    dirs
}

// ---- Desktop entry ----------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DesktopEntry {
    pub name: String,
    pub exec: String,
    pub icon_name: String,
    pub icon_path: Option<PathBuf>, // resolved by caller from manifest
    pub categories: Vec<String>,
    pub wm_class: Option<String>,
    pub terminal: bool,
}

pub fn strip_exec_codes(exec: &str) -> String {
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

/// Returns `None` for entries that should not be shown (hidden, NoDisplay, non-Application type).
pub fn parse_desktop(path: &Path) -> Option<DesktopEntry> {
    let file = File::open(path).ok()?;
    let mut in_entry = false;
    let mut name: Option<String> = None;
    let mut exec: Option<String> = None;
    let mut icon: Option<String> = None;
    let mut categories: Option<String> = None;
    let mut wm_class: Option<String> = None;
    let mut app_type: Option<String> = None;
    let mut no_display = false;
    let mut hidden = false;
    let mut terminal = false;

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
        } else if let Some(v) = s.strip_prefix("Icon=") {
            icon.get_or_insert_with(|| v.to_string());
        } else if let Some(v) = s.strip_prefix("Categories=") {
            categories.get_or_insert_with(|| v.to_string());
        } else if let Some(v) = s.strip_prefix("StartupWMClass=") {
            wm_class.get_or_insert_with(|| v.to_string());
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
    if app_type.as_deref() != Some("Application") {
        return None;
    }

    let name = name?.trim().to_string();
    let exec = strip_exec_codes(exec?.trim()).trim().to_string();
    if name.is_empty() || exec.is_empty() {
        return None;
    }

    let icon_name = icon.unwrap_or_default().trim().to_string();
    let cats = categories
        .unwrap_or_default()
        .split(';')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    Some(DesktopEntry {
        name,
        exec,
        icon_name,
        icon_path: None,
        categories: cats,
        wm_class: wm_class.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        terminal,
    })
}

/// Walk all configured application directories and return deduplicated entries.
/// Entries from later directories (user-local) override those from earlier ones.
pub fn load_all_desktop_entries() -> Vec<DesktopEntry> {
    let mut seen: std::collections::HashMap<String, DesktopEntry> = std::collections::HashMap::new();
    for dir in app_dirs() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let key = entry.file_name().to_string_lossy().into_owned();
            if let Some(app) = parse_desktop(&path) {
                seen.insert(key, app);
            }
        }
    }
    seen.into_values().collect()
}

// ---- Icon cache -------------------------------------------------------------

pub struct IconCache {
    pub dir: PathBuf,
}

impl IconCache {
    pub fn new() -> Self {
        IconCache { dir: cache_dir().join("icons") }
    }

    pub fn path_for(&self, icon_name: &str) -> PathBuf {
        self.dir.join(format!("{}.png", icon_name))
    }

    pub fn manifest_path() -> PathBuf {
        cache_dir().join("manifest.json")
    }

    pub fn ensure_dir(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)
    }
}

impl Default for IconCache {
    fn default() -> Self {
        Self::new()
    }
}

// ---- Launch history ---------------------------------------------------------

pub struct LaunchHistory {
    counts: HashMap<String, u32>,
    path: PathBuf,
}

impl LaunchHistory {
    pub fn load() -> Self {
        let path = cache_dir().join("history.json");
        let counts = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        LaunchHistory { counts, path }
    }

    pub fn count(&self, name: &str) -> u32 {
        self.counts.get(name).copied().unwrap_or(0)
    }

    pub fn increment(&mut self, name: &str) {
        *self.counts.entry(name.to_string()).or_insert(0) += 1;
    }

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_string(&self.counts) {
            let _ = fs::write(&self.path, json);
        }
    }
}

// ---- Config -----------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default, rename = "context")]
    pub contexts: Vec<Context>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    pub name: String,
    #[serde(default)]
    pub priority: Vec<String>,
}

impl Config {
    pub fn load() -> Self {
        let path = config_dir().join("config.toml");
        let content = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                eprintln!("breadbox: could not read {}: {}", path.display(), e);
                return Self::default();
            }
        };
        match toml::from_str(&content) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("breadbox: parse error in {}: {}", path.display(), e);
                Self::default()
            }
        }
    }

    /// Find the context matching `workspace`, falling back to "default", then
    /// returning None if neither exists.
    pub fn context_for(&self, workspace: &str) -> Option<&Context> {
        self.contexts
            .iter()
            .find(|c| c.name == workspace)
            .or_else(|| self.contexts.iter().find(|c| c.name == "default"))
    }
}
