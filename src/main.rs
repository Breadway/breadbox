use std::{
    collections::HashMap,
    env,
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use gtk4::{
    gdk::Display,
    glib,
    pango::EllipsizeMode,
    prelude::*,
    Application, ApplicationWindow, Box as GBox, CssProvider, EventControllerKey, Label,
    ListBox, Orientation, PolicyType, ScrolledWindow, SearchEntry, SelectionMode,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

const CACHE_TIMEOUT_SECS: u64 = 86400;

const CSS: &str = "
window, .background {
    background-color: #1e1e2e;
}
searchentry {
    background-color: #313244;
    color: #cdd6f4;
    caret-color: #cba6f7;
    border: none;
    outline: none;
    box-shadow: none;
    padding: 12px 16px;
    font-size: 15px;
}
listbox {
    background-color: transparent;
    padding: 4px;
}
row {
    padding: 6px 12px;
    color: #cdd6f4;
    background-color: transparent;
    border-radius: 4px;
}
row:selected {
    background-color: #45475a;
}
.action {
    color: #6c7086;
    font-size: 12px;
}
";

// ---- cache helpers --------------------------------------------------------

fn home_dir() -> PathBuf {
    PathBuf::from(env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
}

fn cache_path() -> PathBuf {
    env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".cache"))
        .join("breadbox.cache")
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
    now.saturating_sub(cm) < CACHE_TIMEOUT_SECS
        && app_dirs().iter().all(|d| !d.is_dir() || mtime(d) <= cm)
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

struct DesktopApp {
    name: String,
    exec: String,
    terminal: bool,
}

fn parse_desktop(path: &Path) -> Option<DesktopApp> {
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

    Some(DesktopApp { name, exec, terminal })
}

fn build_cache(cache: &Path) {
    let _ = fs::create_dir_all(cache.parent().unwrap_or(Path::new("/tmp")));
    let mut apps: HashMap<String, DesktopApp> = HashMap::new();

    for dir in &app_dirs() {
        let Ok(entries) = fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            if let Some(app) = parse_desktop(&path) {
                apps.insert(entry.file_name().to_string_lossy().into_owned(), app);
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

fn load_entries(cache: &Path) -> Vec<(String, String)> {
    fs::read_to_string(cache)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(2, '\t');
            let name = parts.next()?.to_string();
            let action = parts.next()?.to_string();
            (!name.is_empty() && !action.is_empty()).then_some((name, action))
        })
        .collect()
}

// ---- launch ---------------------------------------------------------------

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

fn do_launch(action: &str) {
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

// ---- fuzzy matching -------------------------------------------------------

fn fuzzy_matches(pattern: &str, text: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }
    let mut chars = text.chars();
    for pc in pattern.chars() {
        let pl = pc.to_lowercase().next().unwrap_or(pc);
        if !chars
            .by_ref()
            .any(|tc| tc.to_lowercase().next().unwrap_or(tc) == pl)
        {
            return false;
        }
    }
    true
}

// ---- toggle via pid file --------------------------------------------------

fn pid_file() -> PathBuf {
    env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("breadbox.pid")
}

// Returns false if an existing instance was killed (caller should exit).
fn toggle_or_continue() -> bool {
    let pf = pid_file();
    if let Ok(content) = fs::read_to_string(&pf) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            if Path::new(&format!("/proc/{}", pid)).exists() {
                let _ = Command::new("kill").arg(pid.to_string()).status();
                return false;
            }
        }
    }
    let _ = fs::write(&pf, std::process::id().to_string());
    true
}

fn cleanup_pid() {
    let _ = fs::remove_file(pid_file());
}

// ---- UI -------------------------------------------------------------------

fn get_row_data(row: &gtk4::ListBoxRow, key: &str) -> String {
    unsafe {
        row.data::<String>(key)
            .map(|p| p.as_ref().clone())
            .unwrap_or_default()
    }
}

fn run_ui(entries: Vec<(String, String)>) {
    let app = Application::builder()
        .application_id("com.breadway.breadbox")
        .build();

    app.connect_activate(move |app| {
        let provider = CssProvider::new();
        provider.load_from_data(CSS);
        gtk4::style_context_add_provider_for_display(
            &Display::default().expect("no display"),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let window = ApplicationWindow::builder()
            .application(app)
            .default_width(700)
            .build();

        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_keyboard_mode(KeyboardMode::OnDemand);
        window.set_anchor(Edge::Top, true);
        window.set_exclusive_zone(-1);

        let vbox = GBox::new(Orientation::Vertical, 0);

        let search = SearchEntry::new();
        search.set_placeholder_text(Some("breadbox"));
        vbox.append(&search);

        let scroll = ScrolledWindow::new();
        scroll.set_policy(PolicyType::Never, PolicyType::Automatic);
        scroll.set_max_content_height(400);
        scroll.set_propagate_natural_height(true);

        let list = ListBox::new();
        list.set_selection_mode(SelectionMode::Browse);

        for (name, action) in &entries {
            let row = gtk4::ListBoxRow::new();
            let hbox = GBox::new(Orientation::Horizontal, 8);
            hbox.set_margin_start(4);
            hbox.set_margin_end(4);

            let name_lbl = Label::new(Some(name));
            name_lbl.set_xalign(0.0);
            name_lbl.set_hexpand(true);
            hbox.append(&name_lbl);

            let action_lbl = Label::new(Some(action));
            action_lbl.add_css_class("action");
            action_lbl.set_xalign(1.0);
            action_lbl.set_ellipsize(EllipsizeMode::End);
            action_lbl.set_max_width_chars(50);
            hbox.append(&action_lbl);

            row.set_child(Some(&hbox));
            unsafe {
                row.set_data("name", name.clone());
                row.set_data("action", action.clone());
            }
            list.append(&row);
        }

        if let Some(first) = list.row_at_index(0) {
            list.select_row(Some(&first));
        }

        scroll.set_child(Some(&list));
        vbox.append(&scroll);
        window.set_child(Some(&vbox));

        // Filter rows on every keystroke
        let list_f = list.clone();
        search.connect_changed(move |entry| {
            let text = entry.text();
            let query = text.as_str();
            let mut first_vis: Option<gtk4::ListBoxRow> = None;
            let mut i = 0i32;
            while let Some(row) = list_f.row_at_index(i) {
                let name = get_row_data(&row, "name");
                let vis = fuzzy_matches(query, &name);
                row.set_visible(vis);
                if vis && first_vis.is_none() {
                    first_vis = Some(row);
                }
                i += 1;
            }
            list_f.select_row(first_vis.as_ref());
        });

        // Keyboard: Esc, Enter, arrows — capture phase on window so we
        // intercept before SearchEntry's own handlers consume them
        let key_ctrl = EventControllerKey::new();
        key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let window_k = window.clone();
        let list_k = list.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, _| {
            use gtk4::gdk::Key;
            match key {
                Key::Escape => {
                    cleanup_pid();
                    window_k.close();
                    glib::Propagation::Stop
                }
                Key::Return | Key::KP_Enter => {
                    if let Some(row) = list_k.selected_row() {
                        let action = get_row_data(&row, "action");
                        if !action.is_empty() {
                            do_launch(&action);
                            cleanup_pid();
                            window_k.close();
                        }
                    }
                    glib::Propagation::Stop
                }
                Key::Down => {
                    let cur = list_k.selected_row().map(|r| r.index()).unwrap_or(-1);
                    let mut i = cur + 1;
                    loop {
                        match list_k.row_at_index(i) {
                            Some(r) if r.is_visible() => {
                                list_k.select_row(Some(&r));
                                break;
                            }
                            Some(_) => i += 1,
                            None => break,
                        }
                    }
                    glib::Propagation::Stop
                }
                Key::Up => {
                    let cur = list_k.selected_row().map(|r| r.index()).unwrap_or(0);
                    let mut i = cur - 1;
                    loop {
                        if i < 0 {
                            break;
                        }
                        match list_k.row_at_index(i) {
                            Some(r) if r.is_visible() => {
                                list_k.select_row(Some(&r));
                                break;
                            }
                            Some(_) => i -= 1,
                            None => break,
                        }
                    }
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        window.add_controller(key_ctrl);

        // Click to launch
        let window_a = window.clone();
        list.connect_row_activated(move |_, row| {
            let action = get_row_data(row, "action");
            if !action.is_empty() {
                do_launch(&action);
                cleanup_pid();
                window_a.close();
            }
        });

        // Close when focus leaves the window (click outside, alt-tab, etc.)
        let window_foc = window.clone();
        let focus_ctrl = gtk4::EventControllerFocus::new();
        focus_ctrl.connect_leave(move |_| {
            cleanup_pid();
            window_foc.close();
        });
        window.add_controller(focus_ctrl);

        // Cleanup pid when window is destroyed for any reason
        window.connect_destroy(|_| cleanup_pid());

        window.present();
        search.grab_focus();
    });

    app.run();
}

// ---- main -----------------------------------------------------------------

fn main() {
    let cache = cache_path();

    if env::var("BREADBOX_REBUILD_ONLY").as_deref() == Ok("1") {
        build_cache(&cache);
        return;
    }

    if !toggle_or_continue() {
        return;
    }

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

    let entries = load_entries(&cache);
    run_ui(entries);
}
