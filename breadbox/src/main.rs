use bread_theme::{hex_to_rgba, ink_on, load_palette, Palette};
use bread_utils::bread_client::BreadClient;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    env,
    fs,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    rc::Rc,
    time::Duration,
};

/// This app's id in bread's sibling-app namespace registry
/// (`bread_shared::apps::KNOWN_APPS`) — events publish as `bread.box.*`.
const APP_ID: &str = "box";

use breadbox_shared::{
    config_dir, load_all_desktop_entries, Config, DesktopEntry, IconCache, LaunchHistory,
};
use gtk4::{
    glib,
    pango::EllipsizeMode,
    prelude::*,
    Application, ApplicationWindow, Box as GBox, CssProvider, Entry, EventControllerKey, Label,
    ListBox, Orientation, PolicyType, ScrolledWindow, SelectionMode,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

mod listen;
mod screenshot;

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
    history: &LaunchHistory,
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
            (None, None) => {
                // Most-launched first, then alphabetical
                history.count(&b.name).cmp(&history.count(&a.name))
                    .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            }
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

const STAGGER_ROWS: usize = 12;

fn build_css(p: &Palette) -> String {
    let bg_panel = hex_to_rgba(&p.background, 0.68);
    // breadbox-specific rules only — fonts, palette, and generic widgets come
    // from the shared ecosystem stylesheet (applied first in connect_activate).
    // Colour is set on each surface (panel, search, hovered/selected row) so
    // child labels inherit the legible ink for that background. `on_*` are
    // luminance-picked black/white — the pywal hues are untouched.
    //
    // GTK4 ListBox's node is `list`, not `listbox`. These `list row:selected`
    // rules beat the shared sheet's solid accent fill + on-accent ink so the
    // glass card keeps a tinted selection and a left inset hairline.
    let stagger = (0..STAGGER_ROWS)
        .map(|i| {
            format!(
                ".launcher-bg.just-opened list row.stagger-{i} {{ animation-delay: {}ms; }}",
                i * 28
            )
        })
        .collect::<Vec<_>>()
        .join("");
    format!(
        "\
window {{ background-color: rgba(0, 0, 0, 0.28); animation: scrim-in 0.28s ease both; }}\
@keyframes scrim-in {{\
  from {{ background-color: rgba(0, 0, 0, 0); }}\
  to {{ background-color: rgba(0, 0, 0, 0.28); }}\
}}\
.launcher-bg {{\
  background-color: {bg_panel}; color: {on_bg}; border-radius: 20px;\
  border: 1px solid alpha({on_bg}, 0.14);\
  box-shadow: 0 24px 64px rgba(0, 0, 0, 0.50);\
  animation: card-in 0.42s cubic-bezier(0.22, 1, 0.36, 1) both;\
}}\
@keyframes card-in {{\
  from {{ opacity: 0; margin-top: 112px; }}\
  to {{ opacity: 1; margin-top: 88px; }}\
}}\
.launcher-bg entry {{\
  background-color: transparent; color: {on_bg}; caret-color: {accent};\
  border: none; outline: none; box-shadow: none;\
  padding: 20px 22px 14px; border-radius: 20px 20px 0 0;\
  font-size: 17px; min-height: 28px;\
}}\
.launcher-bg entry:focus, .launcher-bg entry:focus-within {{\
  border: none; outline: none; box-shadow: none; background-color: transparent;\
}}\
entry > text {{ background: transparent; }}\
entry image {{ opacity: 0; min-width: 0; margin: 0; padding: 0; }}\
.launcher-caret {{\
  min-height: 2px; max-height: 2px; margin: 0 20px; border-radius: 2px;\
  background-color: {accent};\
  background-image: linear-gradient(90deg, {accent}, {accent2});\
}}\
.launcher-bg.just-opened .launcher-caret {{\
  animation: caret-draw 0.45s cubic-bezier(0.22, 1, 0.36, 1) both;\
}}\
@keyframes caret-draw {{\
  from {{ margin-right: 600px; opacity: 0.25; }}\
  to {{ margin-right: 20px; opacity: 1; }}\
}}\
scrolledwindow {{ background: transparent; }}\
list {{ background-color: transparent; padding: 8px 8px 4px; }}\
list row {{\
  padding: 6px 8px; color: {on_bg}; background-color: transparent;\
  border-radius: 14px; margin: 1px 10px; outline: none;\
}}\
list row:hover {{ background-color: alpha({on_bg}, 0.07); color: {on_bg}; }}\
row:selected, list row:selected, list row:selected:focus,\
list row:selected:hover, list row:selected:focus:hover {{\
  background-color: alpha({accent}, 0.22); color: {on_bg};\
  outline: none; box-shadow: none;\
}}\
list row:selected label, list row:selected .app-name, list row:selected .app-muted {{\
  color: {on_bg};\
}}\
.app-row {{ min-height: 48px; }}\
.app-icon-well {{\
  min-width: 38px; min-height: 38px; margin-right: 12px;\
  border-radius: 999px; background-color: alpha({on_bg}, 0.08);\
}}\
.app-icon {{ color: {on_bg}; opacity: 0.88; }}\
.app-name {{ font-size: 14px; font-weight: bold; }}\
.app-muted {{ opacity: 0.48; font-size: 11px; }}\
.launcher-footer {{\
  padding: 8px 18px 12px; font-size: 11px; opacity: 0.40;\
  letter-spacing: 0.08em; text-transform: uppercase;\
}}\
.launcher-bg.just-opened list row {{\
  animation: row-in 0.32s cubic-bezier(0.22, 1, 0.36, 1) both;\
}}\
@keyframes row-in {{\
  from {{ opacity: 0; }}\
  to {{ opacity: 1; }}\
}}\
{stagger}\
list.reflow row {{ animation: row-fade 0.16s ease both; }}\
@keyframes row-fade {{\
  from {{ opacity: 0.40; }}\
  to {{ opacity: 1; }}\
}}\
.no-motion, .no-motion * {{ animation: none; transition: none; }}",
        bg_panel = bg_panel,
        accent = p.color4,
        accent2 = p.color5,
        on_bg = ink_on(&p.background),
        stagger = stagger,
    )
}

fn category_label(entry: &DesktopEntry) -> &'static str {
    let has = |needle: &str| {
        entry
            .categories
            .iter()
            .any(|c| c.eq_ignore_ascii_case(needle) || c.to_ascii_lowercase().contains(needle))
    };
    if entry.terminal || has("terminalemulator") {
        "Terminal"
    } else if has("webbrowser") {
        "Browser"
    } else if has("game") {
        "Games"
    } else if has("instantmessaging") || has("chat") || has("ircclient") {
        "Chat"
    } else if has("settings") || has("desktopsettings") || has("system") {
        "System"
    } else if has("ide") || has("development") {
        "IDE"
    } else if has("office") || has("wordprocessor") || has("texteditor") || has("notes") {
        "Notes"
    } else if has("audio") || has("player") || has("audiovideo") {
        "Music"
    } else if has("graphics") || has("photography") || has("camera") {
        "Capture"
    } else if has("filemanager") {
        "Files"
    } else {
        "App"
    }
}

fn symbolic_icon(entry: &DesktopEntry) -> &'static str {
    match category_label(entry) {
        "Browser" => "web-browser-symbolic",
        "Terminal" => "utilities-terminal-symbolic",
        "Notes" => "accessories-text-editor-symbolic",
        "System" => "emblem-system-symbolic",
        "Chat" => "user-available-symbolic",
        "Games" => "applications-games-symbolic",
        "Music" => "audio-x-generic-symbolic",
        "Capture" => "camera-photo-symbolic",
        "IDE" => "applications-engineering-symbolic",
        "Files" => "folder-symbolic",
        _ => "application-x-executable-symbolic",
    }
}

// ---- Icon loading -----------------------------------------------------------

fn make_icon(entry: &DesktopEntry) -> gtk4::Image {
    let img = gtk4::Image::from_icon_name(symbolic_icon(entry));
    img.set_pixel_size(18);
    img.add_css_class("app-icon");
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
    let spawned = if entry.terminal {
        let term = pick_terminal();
        Command::new(&term)
            .args(["-e", "bash", "-c", cmd])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    } else {
        Command::new("bash")
            .args(["-c", cmd])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };
    if spawned.is_ok() {
        emit_launched(entry);
    }
}

/// Publishes `bread.box.launched` after a successful spawn. Fire-and-forget
/// and non-fatal (`BreadClient::emit` never blocks or errors this caller) —
/// breadd being absent must never affect launching itself.
fn emit_launched(entry: &DesktopEntry) {
    let id = if entry.id.is_empty() {
        entry.exec.as_str()
    } else {
        entry.id.as_str()
    };
    BreadClient::connect(APP_ID).emit(
        "bread.box.launched",
        serde_json::json!({ "id": id, "name": entry.name }),
    );
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

fn fuzzy_score(query: &str, entry: &DesktopEntry) -> u32 {
    let q = query.to_lowercase();
    let name = entry.name.to_lowercase();
    let wm = entry.wm_class.as_deref().unwrap_or("").to_lowercase();
    if name == q || wm == q { return 0; }
    if name.starts_with(&q) { return 1; }
    if name.contains(&q) { return 2; }
    if wm.starts_with(&q) || wm.contains(&q) { return 3; }
    4 // subsequence match
}

// ---- UI ---------------------------------------------------------------------

fn get_row_entry(row: &gtk4::ListBoxRow) -> Option<DesktopEntry> {
    unsafe {
        row.data::<DesktopEntry>("entry")
            .map(|p| p.as_ref().clone())
    }
}

fn visible_row_count(list: &ListBox) -> u32 {
    let mut n = 0;
    let mut i = 0;
    while let Some(row) = list.row_at_index(i) {
        if row.is_visible() {
            n += 1;
        }
        i += 1;
    }
    n
}

fn set_footer_count(footer: &Label, n: u32) {
    match n {
        0 => footer.set_text("no match"),
        1 => footer.set_text("1 app"),
        n => footer.set_text(&format!("{n} apps")),
    }
}

fn run_ui(
    entries: Vec<DesktopEntry>,
    history: LaunchHistory,
    screenshot_req: Option<screenshot::ScreenshotRequest>,
) {
    let mut builder = Application::builder().application_id("com.breadway.breadbox");
    if screenshot_req.is_some() {
        // GApplication is single-instance by default; this machine typically
        // already has a real breadbox instance, so without this a screenshot
        // run would just message the *existing* instance instead of
        // starting a fresh one that ever sees `screenshot_req`.
        builder = builder.flags(gtk4::gio::ApplicationFlags::NON_UNIQUE);
    }
    let app = builder.build();

    let history_rc = Rc::new(RefCell::new(history));
    let query_rc: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let is_screenshot_run = screenshot_req.is_some();

    app.connect_activate(move |app| {
        // Shared ecosystem base (fonts, palette, generic widgets) first, then
        // breadbox-specific CSS layered on top — both hot-reload on
        // `bread-theme reload` (the closure re-reads the pywal palette).
        bread_theme::gtk::apply_shared();
        bread_theme::gtk::apply_app_css(|| build_css(&load_palette()));

        // User CSS override
        {
            let user_css_path = config_dir().join("style.css");
            let user_cell: RefCell<Option<CssProvider>> = RefCell::new(None);
            bread_theme::gtk::apply_user_css(&user_css_path, &user_cell);
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
        bread_theme::gtk::bind_window_auto(&window);

        let close_all: Rc<dyn Fn()> = Rc::new({
            let w = window.clone();
            move || {
                w.close();
            }
        });

        let vbox = GBox::new(Orientation::Vertical, 0);
        vbox.add_css_class("launcher-bg");
        vbox.set_halign(gtk4::Align::Center);
        vbox.set_valign(gtk4::Align::Start);
        vbox.set_margin_top(88);
        vbox.set_size_request(600, -1);
        if is_screenshot_run {
            window.add_css_class("no-motion");
            vbox.add_css_class("no-motion");
        } else {
            vbox.add_css_class("just-opened");
        }

        let search = Entry::new();
        search.set_placeholder_text(Some("Search"));
        search.set_has_frame(false);
        vbox.append(&search);

        let caret = GBox::new(Orientation::Horizontal, 0);
        caret.add_css_class("launcher-caret");
        caret.set_hexpand(true);
        vbox.append(&caret);

        let scroll = ScrolledWindow::new();
        scroll.set_policy(PolicyType::Never, PolicyType::Automatic);
        scroll.set_max_content_height(480);
        scroll.set_propagate_natural_height(true);

        let list = ListBox::new();
        list.set_selection_mode(SelectionMode::Browse);

        for (idx, entry) in entries.iter().enumerate() {
            let row = gtk4::ListBoxRow::new();
            if idx < STAGGER_ROWS {
                row.add_css_class(&format!("stagger-{idx}"));
            }
            let hbox = GBox::new(Orientation::Horizontal, 0);
            hbox.add_css_class("app-row");
            hbox.set_valign(gtk4::Align::Center);

            let well = GBox::new(Orientation::Horizontal, 0);
            well.add_css_class("app-icon-well");
            well.set_size_request(38, 38);
            well.set_halign(gtk4::Align::Center);
            well.set_valign(gtk4::Align::Center);
            well.set_hexpand(false);
            let icon = make_icon(entry);
            icon.set_halign(gtk4::Align::Center);
            icon.set_valign(gtk4::Align::Center);
            icon.set_hexpand(true);
            well.append(&icon);
            hbox.append(&well);

            let text = GBox::new(Orientation::Vertical, 1);
            text.add_css_class("app-text");
            text.set_hexpand(true);
            text.set_valign(gtk4::Align::Center);

            let name_lbl = Label::new(Some(&entry.name));
            name_lbl.add_css_class("app-name");
            name_lbl.set_xalign(0.0);
            name_lbl.set_ellipsize(EllipsizeMode::End);
            text.append(&name_lbl);

            let sub_lbl = Label::new(Some(category_label(entry)));
            sub_lbl.add_css_class("app-muted");
            sub_lbl.set_xalign(0.0);
            sub_lbl.set_ellipsize(EllipsizeMode::End);
            text.append(&sub_lbl);

            hbox.append(&text);
            row.set_child(Some(&hbox));
            unsafe { row.set_data("entry", entry.clone()) };
            unsafe { row.set_data("initial_order", idx as u32) };
            list.append(&row);
        }

        // Sort by match quality + launch count when a query is active;
        // fall back to insertion order (priority + launch frequency) when empty.
        let sort_query = Rc::clone(&query_rc);
        let sort_history = Rc::clone(&history_rc);
        list.set_sort_func(move |row_a, row_b| {
            let query = sort_query.borrow();
            if query.is_empty() {
                let oa = unsafe { row_a.data::<u32>("initial_order").map_or(u32::MAX, |p| *p.as_ref()) };
                let ob = unsafe { row_b.data::<u32>("initial_order").map_or(u32::MAX, |p| *p.as_ref()) };
                return oa.cmp(&ob).into();
            }
            let (Some(ea), Some(eb)) = (get_row_entry(row_a), get_row_entry(row_b)) else {
                return std::cmp::Ordering::Equal.into();
            };
            let sa = fuzzy_score(&query, &ea);
            let sb = fuzzy_score(&query, &eb);
            let history = sort_history.borrow();
            let ca = history.count(&ea.name);
            let cb = history.count(&eb.name);
            sa.cmp(&sb)
                .then(cb.cmp(&ca))
                .then(ea.name.to_lowercase().cmp(&eb.name.to_lowercase()))
                .into()
        });

        if let Some(first) = list.row_at_index(0) {
            list.select_row(Some(&first));
        }

        scroll.set_child(Some(&list));
        vbox.append(&scroll);

        let footer = Label::new(None);
        footer.add_css_class("launcher-footer");
        footer.set_xalign(0.0);
        set_footer_count(&footer, visible_row_count(&list));
        vbox.append(&footer);

        window.set_child(Some(&vbox));

        if !is_screenshot_run {
            let vbox_open = vbox.clone();
            glib::timeout_add_local_once(Duration::from_millis(520), move || {
                vbox_open.remove_css_class("just-opened");
            });
        }

        // Filter on keystroke. ListBox keeps row identity across sort, so the
        // reorder is already a cheap FLIP analog; a short CSS fade is the extra.
        let list_f = list.clone();
        let footer_f = footer.clone();
        let vbox_f = vbox.clone();
        let filter_query = Rc::clone(&query_rc);
        let reflow_gen = Rc::new(Cell::new(0u32));
        search.connect_changed(move |entry| {
            let text = entry.text();
            let query = text.as_str();
            *filter_query.borrow_mut() = query.to_string();
            let mut i = 0i32;
            while let Some(row) = list_f.row_at_index(i) {
                let vis = get_row_entry(&row)
                    .map(|e| {
                        fuzzy_matches(query, &e.name)
                            || fuzzy_matches(query, category_label(&e))
                            || e.wm_class
                                .as_deref()
                                .is_some_and(|w| fuzzy_matches(query, w))
                            || fuzzy_matches(query, &e.exec)
                    })
                    .unwrap_or(false);
                row.set_visible(vis);
                i += 1;
            }
            list_f.invalidate_sort();
            let first_vis = (0i32..).find_map(|j| {
                list_f.row_at_index(j).filter(|r| r.is_visible())
            });
            list_f.select_row(first_vis.as_ref());
            set_footer_count(&footer_f, visible_row_count(&list_f));
            if !vbox_f.has_css_class("just-opened") {
                list_f.remove_css_class("reflow");
                list_f.add_css_class("reflow");
                let gen = reflow_gen.get().wrapping_add(1);
                reflow_gen.set(gen);
                let list_fade = list_f.clone();
                let reflow_gen = Rc::clone(&reflow_gen);
                glib::timeout_add_local_once(Duration::from_millis(180), move || {
                    if reflow_gen.get() == gen {
                        list_fade.remove_css_class("reflow");
                    }
                });
            }
        });

        // Keyboard handling — capture phase on window
        let key_ctrl = EventControllerKey::new();
        key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let close_k = Rc::clone(&close_all);
        let list_k = list.clone();
        let history_k = Rc::clone(&history_rc);
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
                            history_k.borrow_mut().increment(&entry.name);
                            history_k.borrow().save();
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
        let history_a = Rc::clone(&history_rc);
        list.connect_row_activated(move |_, row| {
            if let Some(entry) = get_row_entry(row) {
                history_a.borrow_mut().increment(&entry.name);
                history_a.borrow().save();
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

        if let Some(req) = screenshot_req.clone() {
            screenshot::dispatch(&window, req);
        }

        window.present();
        search.grab_focus();
    });

    if is_screenshot_run {
        // GLib's own option parser otherwise rejects --screenshot/--output
        // before clap ever sees them (`Cli::parse()` already ran in `main`,
        // over the real argv).
        app.run_with_args(&[] as &[&str]);
    } else {
        app.run();
    }
}

// ---- Main -------------------------------------------------------------------

fn main() {
    if std::env::args().nth(1).as_deref() == Some("listen") {
        listen::run();
        return;
    }

    use clap::Parser;
    let cli = screenshot::Cli::parse();
    let screenshot_req = cli.screenshot_request();

    // `toggle_or_kill` kills whatever's holding the single-instance lock —
    // a real, already-running breadbox included. A screenshot run must
    // never touch it: it's a separate, disposable instance by design (same
    // reasoning as breadbar's `allow_multiple_instances`), not a toggle of
    // the operator's real launcher.
    //
    // Kept alive for the rest of `main` — dropping it releases the
    // single-instance lock and removes the pid file, which happens
    // naturally once `run_ui` returns (after the window closes).
    let _singleton_guard = if screenshot_req.is_some() {
        None
    } else {
        match bread_utils::singleton::toggle_or_kill("breadbox") {
            Ok(bread_utils::singleton::Toggle::Started(guard)) => Some(guard),
            Ok(bread_utils::singleton::Toggle::KilledExisting) => return,
            Err(e) => {
                eprintln!("breadbox: single-instance lock unavailable ({e}); continuing without it");
                None
            }
        }
    };

    let config = Config::load();
    let workspace = get_active_workspace().unwrap_or_default();
    let priority = config
        .context_for(&workspace)
        .map(|c| c.priority.clone())
        .unwrap_or_default();

    let history = LaunchHistory::load();
    let manifest = load_manifest();
    let entries = load_sorted_entries(&manifest, &priority, &history);

    run_ui(entries, history, screenshot_req);
}
