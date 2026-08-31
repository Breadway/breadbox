//! **Compatibility shim.** This crate used to own desktop-entry parsing,
//! icon caching, and launch history; that substance now lives in
//! `bread-launcher` (`bread-ecosystem`, feature/launcher-core) so breadbar's
//! future embedded capsule can share it with breadbox's overlay window —
//! one launcher implementation, two hosts (see
//! `bos-ui-demos/THEME_SYSTEM_PLAN.md` §3, §7).
//!
//! Everything below is either a direct re-export of `bread_launcher`, or a
//! thin wrapper that just bakes in `"breadbox"` as the app name
//! `bread_launcher`'s path/cache functions now take explicitly (so more
//! than one host can use that crate without colliding on
//! `~/.cache/<app>`). This exists purely so `breadbox` and `breadbox-sync`
//! keep compiling with minimal churn — plan to remove it once both callers
//! depend on `bread-launcher` directly instead.
//!
//! `Config`/`Context` are NOT part of that move: they're breadbox's own
//! per-workspace launch-priority config format, not launcher substance, so
//! they stay here.

use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};

pub use bread_launcher::{
    app_dirs, home_dir, load_all_desktop_entries, parse_desktop, strip_exec_codes, DesktopEntry,
    IconCache, LaunchHistory,
};

/// The launcher's shared identity (`bread_launcher::LAUNCHER_APP`, currently
/// `"breadbox"`) — NOT a breadbox-specific literal. breadbar's embedded
/// capsule (theme 04/spotlight) reads/writes this exact same on-disk cache
/// and history, so the two surfaces stay one launcher with one ranking
/// instead of forking into two (see `LAUNCHER_APP`'s own doc comment).
const APP: &str = bread_launcher::LAUNCHER_APP;

pub fn cache_dir() -> PathBuf {
    bread_launcher::cache_dir(APP)
}

pub fn config_dir() -> PathBuf {
    bread_launcher::config_dir(APP)
}

pub fn icon_cache() -> IconCache {
    IconCache::new(APP)
}

pub fn icon_manifest_path() -> PathBuf {
    IconCache::manifest_path(APP)
}

pub fn launch_history() -> LaunchHistory {
    LaunchHistory::load(APP)
}

// ---- Config (breadbox-specific, not launcher substance) --------------------

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
