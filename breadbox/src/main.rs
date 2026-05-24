use std::{
    collections::HashMap,
    env,
    fs,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    rc::Rc,
};

use breadbox_shared::{
    config_dir, home_dir, load_all_desktop_entries, Config, DesktopEntry, IconCache,
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

// ---- Hyprland IPC -----------------------------------------------------------

fn get_active_workspace() -> Option<String> {
    let sig = env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let rt = env::var("XDG_RUNTIME_DIR").ok()?;
    let socket_path = format!("{}/hypr/{}/.socket.sock", rt, sig);

    let mut stream = UnixStream::connect(&socket_path).ok()?;
    stream.write_all(b"j/activeworkspace").ok()?;
    stream.shutdown(std::net::Shutdown::Write).ok()?;

    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;

    let v: serde_json::Value = serde_json::from_str(&response).ok()?;
    v["name"].as_str().map(|s| s.to_string())
}

// ---- Manifest ---------------------------------------------------------------

fn load_manifest() -> HashMap<String, PathBuf> {
    let path = IconCache::manifest_path();
    let content = fs::read_to_string(&path).unwrap_or_default();
    serde_json::from_str::<HashMap<String, String>>(&content)
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k, PathBuf::from(v)))
        .collect()
}

// ---- Entry loading and sorting ----------------------------------------------

fn load_sorted_entries(
    manifest: &HashMap<String, PathBuf>,
    priority: &[String],
) -> Vec<DesktopEntry> {
    let mut entries = load_all_desktop_entries();

    // Populate icon_path from manifest
    for entry in &mut entries {
        if let Some(path) = manifest.get(&entry.icon_name) {
            if path.exists() {
                entry.icon_path = Some(path.clone());
            }
        }
    }

    let priority_lower: Vec<String> = priority.iter().map(|s| s.to_lowercase()).collect();

    entries.sort_by(|a, b| {
        let ai = priority_rank(a, &priority_lower);
        let bi = priority_rank(b, &priority_lower);
        match (ai, bi) {
            (Some(i), Some(j)) => i.cmp(&j),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });

    entries
}

fn priority_rank(entry: &DesktopEntry, priority_lower: &[String]) -> Option<usize> {
    let name_l = entry.name.to_lowercase();
    let wm_l = entry.wm_class.as_deref().unwrap_or("").to_lowercase();
    priority_lower
        .iter()
        .position(|p| matches_term(&name_l, p) || matches_term(&wm_l, p))
}

/// Whole-word / exact match of `term` within `field` (both lowercase). Avoids
/// "code" matching "vscodium" while still matching "Code", "code-oss", and
/// "Visual Studio Code".
fn matches_term(field: &str, term: &str) -> bool {
    if term.is_empty() || field.is_empty() {
        return false;
    }
    if field == term {
        return true;
    }
    let bytes = field.as_bytes();
    let tlen = term.len();
    let mut start = 0;
    while let Some(pos) = field[start..].find(term) {
        let i = start + pos;
        let before_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        let after = i + tlen;
        let after_ok = after >= bytes.len() || !bytes[after].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        start = i + 1;
        if start >= field.len() {
            break;
        }
    }
    false
}

// ---- Theming ----------------------------------------------------------------

#[derive(Debug)]
struct Palette {
    bg: String,
    surface: String,
    fg: String,
    accent: String,
}

impl Palette {
    fn catppuccin_mocha() -> Self {
        Palette {
            bg: "#1e1e2e".into(),
            surface: "#181825".into(),
            fg: "#cdd6f4".into(),
            accent: "#89b4fa".into(),
        }
    }

    fn from_wal() -> Option<Self> {
        let path = env::var("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_dir().join(".cache"))
            .join("wal/colors.json");
        let content = fs::read_to_string(&path).ok()?;
        let v: serde_json::Value = serde_json::from_str(&content).ok()?;

        let spec = &v["special"];
        let cols = &v["colors"];

        let bg = spec["background"].as_str()?.to_string();
        let surface = cols["color0"].as_str().unwrap_or(&bg).to_string();
        let fg = cols["color15"].as_str().unwrap_or("#cdd6f4").to_string();
        let accent = cols["color1"].as_str().unwrap_or("#89b4fa").to_string();

        Some(Palette { bg, surface, fg, accent })
    }
}

fn hex_to_rgba(hex: &str, alpha: f32) -> String {
    let h = hex.trim_start_matches('#');
    let r = u8::from_str_radix(h.get(0..2).unwrap_or("00"), 16).unwrap_or(0);
    let g = u8::from_str_radix(h.get(2..4).unwrap_or("00"), 16).unwrap_or(0);
    let b = u8::from_str_radix(h.get(4..6).unwrap_or("00"), 16).unwrap_or(0);
    format!("rgba({r}, {g}, {b}, {alpha})")
}

fn build_css(p: &Palette) -> String {
    let bg_panel = hex_to_rgba(&p.bg, 0.60);
    format!(
        "* {{ font-family: 'JetBrainsMono Nerd Font Mono', monospace; font-size: 14px; }}\
         window {{ background-color: transparent; }}\
         .launcher-bg {{ background-color: {bg_panel}; border-radius: 8px;\
             box-shadow: 0 8px 32px rgba(0,0,0,0.6); }}\
         searchentry {{ background-color: {surface}; color: {fg}; caret-color: {accent};\
             border: none; outline: none; box-shadow: none;\
             padding: 12px 16px; border-radius: 4px 4px 0 0; }}\
         listbox {{ background-color: transparent; padding: 4px; }}\
         row {{ padding: 5px 10px; color: {fg}; background-color: transparent;\
             border-radius: 4px; }}\
         row:hover {{ background-color: {surface}; }}\
         row:selected {{ background-color: {surface}; }}\
         .app-name {{ font-size: 14px; }}\
         .app-muted {{ color: {fg}; opacity: 0.6; font-size: 12px; }}\
         image {{ margin-right: 8px; }}",
        bg_panel = bg_panel,
        surface = p.surface,
        fg = p.fg,
        accent = p.accent,
    )
}

// ---- Icon loading -----------------------------------------------------------

fn make_icon(icon_name: &str, icon_path: Option<&Path>) -> gtk4::Image {
    // Try loading from resolved cached path via gio::File
    if let Some(path) = icon_path {
        let gio_file = gtk4::gio::File::for_path(path);
        if let Ok(texture) = gtk4::gdk::Texture::from_file(&gio_file) {
            let img = gtk4::Image::new();
            img.set_paintable(Some(&texture));
            img.set_pixel_size(32);
            return img;
        }
    }
    // Fall back to GTK icon theme lookup by name
    let name = if icon_name.is_empty() {
        "application-x-executable"
    } else {
        icon_name
    };
    let img = gtk4::Image::from_icon_name(name);
    img.set_pixel_size(32);
    img
}

// ---- Launch -----------------------------------------------------------------

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

fn do_launch(entry: &DesktopEntry) {
    let cmd = entry.exec.trim();
    if entry.terminal {
        let term = pick_terminal();
        let _ = Command::new(&term)
            .args(["-e", "bash", "-c", cmd])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    } else {
        let _ = Command::new("bash")
            .args(["-c", cmd])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

// ---- Fuzzy matching ---------------------------------------------------------

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

// ---- PID file toggle --------------------------------------------------------

fn pid_file() -> PathBuf {
    env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("breadbox.pid")
}

fn is_breadbox_pid(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{}/comm", pid))
        .map(|s| s.trim() == "breadbox")
        .unwrap_or(false)
}

// Returns false if an existing instance was killed (caller should exit).
fn toggle_or_continue() -> bool {
    let pf = pid_file();
    if let Ok(content) = fs::read_to_string(&pf) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            if is_breadbox_pid(pid) {
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

// ---- UI ---------------------------------------------------------------------

fn get_row_entry(row: &gtk4::ListBoxRow) -> Option<DesktopEntry> {
    unsafe {
        row.data::<DesktopEntry>("entry")
            .map(|p| p.as_ref().clone())
    }
}

fn run_ui(entries: Vec<DesktopEntry>, css: String) {
    let app = Application::builder()
        .application_id("com.breadway.breadbox")
        .build();

    app.connect_activate(move |app| {
        // Base CSS
        let provider = CssProvider::new();
        provider.load_from_string(&css);
        gtk4::style_context_add_provider_for_display(
            &Display::default().expect("no display"),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        // User CSS override
        let user_css_path = config_dir().join("style.css");
        if user_css_path.exists() {
            let user_provider = CssProvider::new();
            user_provider.load_from_path(&user_css_path);
            gtk4::style_context_add_provider_for_display(
                &Display::default().expect("no display"),
                &user_provider,
                gtk4::STYLE_PROVIDER_PRIORITY_USER,
            );
        }

        // Full-screen transparent window; clicks outside the launcher panel close it.
        let window = ApplicationWindow::builder().application(app).build();
        window.init_layer_shell();
        window.set_namespace(Some("breadbox"));
        window.set_layer(Layer::Overlay);
        window.set_keyboard_mode(KeyboardMode::Exclusive);
        for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_anchor(edge, true);
        }
        window.set_exclusive_zone(0);

        let close_all: Rc<dyn Fn()> = Rc::new({
            let w = window.clone();
            move || {
                cleanup_pid();
                w.close();
            }
        });

        let vbox = GBox::new(Orientation::Vertical, 0);
        vbox.add_css_class("launcher-bg");
        vbox.set_halign(gtk4::Align::Center);
        vbox.set_valign(gtk4::Align::Start);
        vbox.set_margin_top(120);
        vbox.set_size_request(600, -1);

        let search = SearchEntry::new();
        search.set_placeholder_text(Some("breadbox"));
        vbox.append(&search);

        let scroll = ScrolledWindow::new();
        scroll.set_policy(PolicyType::Never, PolicyType::Automatic);
        scroll.set_max_content_height(480);
        scroll.set_propagate_natural_height(true);

        let list = ListBox::new();
        list.set_selection_mode(SelectionMode::Browse);

        for entry in &entries {
            let row = gtk4::ListBoxRow::new();
            let hbox = GBox::new(Orientation::Horizontal, 0);
            hbox.set_margin_start(6);
            hbox.set_margin_end(6);
            hbox.set_valign(gtk4::Align::Center);

            let icon = make_icon(&entry.icon_name, entry.icon_path.as_deref());
            hbox.append(&icon);

            let name_lbl = Label::new(Some(&entry.name));
            name_lbl.add_css_class("app-name");
            name_lbl.set_xalign(0.0);
            name_lbl.set_hexpand(true);
            name_lbl.set_ellipsize(EllipsizeMode::End);
            hbox.append(&name_lbl);

            if let Some(ref wm) = entry.wm_class {
                let wm_lbl = Label::new(Some(wm));
                wm_lbl.add_css_class("app-muted");
                wm_lbl.set_xalign(1.0);
                hbox.append(&wm_lbl);
            }

            row.set_child(Some(&hbox));
            unsafe { row.set_data("entry", entry.clone()) };
            list.append(&row);
        }

        if let Some(first) = list.row_at_index(0) {
            list.select_row(Some(&first));
        }

        scroll.set_child(Some(&list));
        vbox.append(&scroll);
        window.set_child(Some(&vbox));

        // Filter on keystroke
        let list_f = list.clone();
        search.connect_changed(move |entry| {
            let text = entry.text();
            let query = text.as_str();
            let mut first_vis: Option<gtk4::ListBoxRow> = None;
            let mut i = 0i32;
            while let Some(row) = list_f.row_at_index(i) {
                let vis = get_row_entry(&row)
                    .map(|e| {
                        fuzzy_matches(query, &e.name)
                            || e.wm_class
                                .as_deref()
                                .is_some_and(|w| fuzzy_matches(query, w))
                            || fuzzy_matches(query, &e.exec)
                    })
                    .unwrap_or(false);
                row.set_visible(vis);
                if vis && first_vis.is_none() {
                    first_vis = Some(row);
                }
                i += 1;
            }
            list_f.select_row(first_vis.as_ref());
        });

        // Keyboard handling — capture phase on window
        let key_ctrl = EventControllerKey::new();
        key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let close_k = Rc::clone(&close_all);
        let list_k = list.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, _| {
            use gtk4::gdk::Key;
            match key {
                Key::Escape => {
                    close_k();
                    glib::Propagation::Stop
                }
                Key::Return | Key::KP_Enter => {
                    if let Some(row) = list_k.selected_row() {
                        if let Some(entry) = get_row_entry(&row) {
                            do_launch(&entry);
                            close_k();
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

        // Row click launches
        let close_a = Rc::clone(&close_all);
        list.connect_row_activated(move |_, row| {
            if let Some(entry) = get_row_entry(row) {
                do_launch(&entry);
                close_a();
            }
        });

        // Click outside launcher panel → close
        let close_outside = Rc::clone(&close_all);
        let vbox_ref = vbox.clone();
        let win_ref = window.clone();
        let outside_click = gtk4::GestureClick::new();
        outside_click.connect_pressed(move |_, _, x, y| {
            if let Some(b) = vbox_ref.compute_bounds(&win_ref) {
                if x < b.x() as f64
                    || x > (b.x() + b.width()) as f64
                    || y < b.y() as f64
                    || y > (b.y() + b.height()) as f64
                {
                    close_outside();
                }
            }
        });
        window.add_controller(outside_click);

        window.connect_destroy(|_| cleanup_pid());
        window.present();
        search.grab_focus();
    });

    app.run();
}

// ---- Main -------------------------------------------------------------------

fn main() {
    if !toggle_or_continue() {
        return;
    }

    let config = Config::load();
    let workspace = get_active_workspace().unwrap_or_default();
    let priority = config
        .context_for(&workspace)
        .map(|c| c.priority.clone())
        .unwrap_or_default();

    let manifest = load_manifest();
    let entries = load_sorted_entries(&manifest, &priority);

    let palette = Palette::from_wal().unwrap_or_else(Palette::catppuccin_mocha);
    let css = build_css(&palette);

    run_ui(entries, css);
}
