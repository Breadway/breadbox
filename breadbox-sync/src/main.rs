use std::{
    collections::HashMap,
    env,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use breadbox_shared::{home_dir, IconCache};

// ---- Icon theme lookup ------------------------------------------------------

fn current_icon_theme() -> String {
    let home = home_dir();
    for cfg in [
        home.join(".config/gtk-4.0/settings.ini"),
        home.join(".config/gtk-3.0/settings.ini"),
    ] {
        if let Ok(content) = fs::read_to_string(&cfg) {
            for line in content.lines() {
                if let Some(v) = line.strip_prefix("gtk-icon-theme-name=") {
                    let t = v.trim().trim_matches('"');
                    if !t.is_empty() {
                        return t.to_string();
                    }
                }
            }
        }
    }
    "hicolor".to_string()
}

fn icon_search_dirs() -> Vec<PathBuf> {
    let home = home_dir();
    let xdg_data_home = env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".local/share"));

    let mut dirs = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for d in [
        xdg_data_home.join("icons"),
        home.join(".local/share/icons"),
        PathBuf::from("/usr/share/icons"),
    ] {
        if seen.insert(d.clone()) {
            dirs.push(d);
        }
    }
    dirs
}

/// Search for `name` in system icon theme directories.
/// Prefers 64px > 48px > 128px > 32px > 256px PNG, then scalable SVG.
fn find_system_icon(name: &str, theme: &str) -> Option<PathBuf> {
    let sizes = ["64x64", "48x48", "128x128", "32x32", "256x256"];
    let dirs = icon_search_dirs();

    let themes: Vec<&str> = if theme != "hicolor" {
        vec![theme, "hicolor"]
    } else {
        vec!["hicolor"]
    };

    for dir in &dirs {
        for t in &themes {
            for size in &sizes {
                let p = dir.join(t).join(size).join("apps").join(format!("{}.png", name));
                if p.exists() {
                    return Some(p);
                }
                // Alternative path layout: <theme>/apps/<size>/
                let p2 = dir.join(t).join("apps").join(size).join(format!("{}.png", name));
                if p2.exists() {
                    return Some(p2);
                }
            }
            // SVG (scalable)
            for subdir in ["scalable/apps", "apps/scalable"] {
                let p = dir.join(t).join(subdir).join(format!("{}.svg", name));
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }

    // /usr/share/pixmaps
    for ext in ["png", "svg", "xpm"] {
        let p = PathBuf::from("/usr/share/pixmaps").join(format!("{}.{}", name, ext));
        if p.exists() {
            return Some(p);
        }
    }

    None
}

// ---- Helpers ----------------------------------------------------------------

/// Strip file extension from an icon field value, returning the canonical name.
fn canonical_icon_name(icon: &str) -> String {
    if icon.starts_with('/') {
        return Path::new(icon)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(icon)
            .to_string();
    }
    icon.strip_suffix(".png")
        .or_else(|| icon.strip_suffix(".svg"))
        .or_else(|| icon.strip_suffix(".xpm"))
        .unwrap_or(icon)
        .to_string()
}

/// A stem like `org.gnome.Gedit` or `com.github.App` — at least three segments,
/// all alphanumeric/hyphen/underscore.
fn looks_like_reverse_dns(stem: &str) -> bool {
    let parts: Vec<&str> = stem.split('.').collect();
    parts.len() >= 3
        && parts[0].len() >= 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        })
}

/// Try to GET `url` and write the body to `dest`. Returns true on success.
fn try_download(agent: &ureq::Agent, url: &str, dest: &Path) -> bool {
    let resp = match agent.get(url).call() {
        Ok(r) if r.status() == 200 => r,
        _ => return false,
    };
    let mut bytes = Vec::new();
    if resp.into_reader().take(2_097_152).read_to_end(&mut bytes).is_err() || bytes.is_empty() {
        return false;
    }
    // Validate the PNG signature so a 200 error page is never cached as an icon.
    const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if !bytes.starts_with(&PNG_MAGIC) {
        return false;
    }
    fs::write(dest, &bytes).is_ok()
}

/// Resolve an icon to a local path, downloading if necessary.
/// Returns None only if all strategies fail and no generic fallback is found.
fn resolve_icon(
    icon_field: &str,
    desktop_stem: &str,
    theme: &str,
    icon_cache: &IconCache,
    agent: &ureq::Agent,
) -> Option<PathBuf> {
    // Absolute path in Icon= field
    if icon_field.starts_with('/') {
        let p = PathBuf::from(icon_field);
        if p.exists() {
            return Some(p);
        }
    }

    let name = canonical_icon_name(icon_field);
    if name.is_empty() {
        return find_system_icon("application-x-executable", theme);
    }

    // 1. System icon theme
    if let Some(p) = find_system_icon(&name, theme) {
        return Some(p);
    }

    // Already cached from a previous run?
    let cached = icon_cache.path_for(&name);
    if cached.exists() {
        return Some(cached);
    }

    // 2. Flathub (appstream icon path, not the media CDN)
    if looks_like_reverse_dns(desktop_stem) {
        let url = format!(
            "https://dl.flathub.org/repo/appstream/x86_64/icons/128x128/{}.png",
            desktop_stem
        );
        let dest = icon_cache.path_for(desktop_stem);
        if try_download(agent, &url, &dest) {
            eprintln!("  [flathub] {}", desktop_stem);
            return Some(dest);
        }
    }

    // 3. Generic fallback
    find_system_icon("application-x-executable", theme)
}

// ---- Main -------------------------------------------------------------------

fn main() {
    if let Err(e) = run() {
        eprintln!("breadbox-sync: {}", e);
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let icon_cache = IconCache::new();
    icon_cache.ensure_dir()?;

    let theme = current_icon_theme();
    eprintln!("breadbox-sync: icon theme = {}", theme);

    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(10))
        .build();

    let mut manifest: HashMap<String, String> = HashMap::new();

    // Walk directories directly to get both the entry and its filename stem
    // (needed for Flathub reverse-DNS resolution).
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for dir in breadbox_shared::app_dirs() {
        let Ok(read_dir) = fs::read_dir(&dir) else { continue };
        for file_entry in read_dir.flatten() {
            let path = file_entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }

            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            // User-local overrides system; process in dir order (system first, local last).
            // Later entries for the same stem will overwrite earlier ones in the manifest.

            let app = match breadbox_shared::parse_desktop(&path) {
                Some(a) => a,
                None => continue,
            };

            if app.icon_name.is_empty() {
                continue;
            }

            // Deduplicate by the raw Icon= value, which is also the manifest key,
            // so every distinct icon_name gets its own entry.
            if !seen.insert(app.icon_name.clone()) {
                continue;
            }

            eprint!("resolving icon for {} ({}) ... ", app.name, app.icon_name);
            match resolve_icon(&app.icon_name, &stem, &theme, &icon_cache, &agent) {
                Some(p) => {
                    eprintln!("{}", p.display());
                    manifest.insert(app.icon_name.clone(), p.to_string_lossy().into_owned());
                }
                None => {
                    eprintln!("not found");
                }
            }
        }
    }

    let manifest_path = IconCache::manifest_path();
    let json = serde_json::to_string_pretty(&manifest)?;
    let tmp = manifest_path.with_extension("tmp");
    fs::write(&tmp, &json)?;
    fs::rename(&tmp, &manifest_path)?;

    eprintln!(
        "breadbox-sync: wrote manifest ({} entries) to {}",
        manifest.len(),
        manifest_path.display()
    );
    Ok(())
}
