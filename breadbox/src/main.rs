use bread_theme::{hex_to_rgba, ink_on, load_palette, Palette};
use std::{
    cell::RefCell,
    collections::HashMap,
    env,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    rc::Rc,
};

use breadbox_shared::{
    config_dir, load_all_desktop_entries, Config, DesktopEntry, IconCache, LaunchHistory,
};
use gtk4::{
    glib,
    pango::EllipsizeMode,
    prelude::*,
    Application, Box as GBox, CssProvider, EventControllerKey, Label,
    ListBox, Orientation, PolicyType, ScrolledWindow, SearchEntry, SelectionMode,
};

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

fn build_css(p: &Palette) -> String {
    let bg_panel = hex_to_rgba(&p.background, 0.60);
    // breadbox-specific rules only — fonts, palette, and generic widgets come
    // from the shared ecosystem stylesheet (applied first in connect_activate).
    // Colour is set on each surface (panel, search box, hovered/selected row) so
    // child labels inherit the legible ink for that background. `on_*` are
    // luminance-picked black/white — the pywal hues are untouched. Without this a
    // light `surface` slot makes the selected row's text vanish.
    format!(
        "window {{ background-color: transparent; }}\
         .launcher-bg {{ background-color: {bg_panel}; color: {on_bg}; border-radius: 8px;\
             box-shadow: 0 8px 32px rgba(0,0,0,0.6); }}\
         searchentry {{ background-color: {surface}; color: {on_surface}; caret-color: {accent};\
             border: none; outline: none; box-shadow: none;\
             padding: 12px 16px; border-radius: 6px 6px 0 0; }}\
         listbox {{ background-color: transparent; padding: 4px; }}\
         row {{ padding: 8px 12px; color: {on_bg}; background-color: transparent;\
             border-radius: 6px; }}\
         row:hover {{ background-color: {surface}; color: {on_surface}; }}\
         row:selected {{ background-color: {surface}; color: {on_surface}; }}\
         .app-name {{ font-size: 14px; }}\
         .app-muted {{ opacity: 0.6; font-size: 12px; }}\
         image {{ margin-right: 8px; }}",
        bg_panel   = bg_panel,
        surface    = p.color0,
        accent     = p.color4,
        on_bg      = ink_on(&p.background),
        on_surface = ink_on(&p.color0),
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

/// Same tiers as `fuzzy_score`, but `None` when nothing matches at all
/// (rather than falling through to a bare-subsequence score), and folds in
/// `exec` as a tier-4 (weakest) match too — used for filtering, not sorting.
/// Tier 4 is loose enough that e.g. querying "zen" matches "Avahi Zeroconf
/// Browser" (z…e…n as a subsequence) alongside the real "Zen Browser" hit;
/// the filter hides tier-4-only rows whenever a tier ≤2 (name-based) match
/// exists elsewhere in the list, so that kind of noise only shows up when
/// it's the best any entry can do.
fn match_tier(query: &str, entry: &DesktopEntry) -> Option<u32> {
    if query.is_empty() {
        return Some(0);
    }
    let q = query.to_lowercase();
    let name = entry.name.to_lowercase();
    let wm = entry.wm_class.as_deref().unwrap_or("").to_lowercase();
    if name == q || wm == q {
        return Some(0);
    }
    if name.starts_with(&q) {
        return Some(1);
    }
    if name.contains(&q) {
        return Some(2);
    }
    if wm.starts_with(&q) || wm.contains(&q) {
        return Some(3);
    }
    if fuzzy_matches(query, &entry.name)
        || entry.wm_class.as_deref().is_some_and(|w| fuzzy_matches(query, w))
        || fuzzy_matches(query, &entry.exec)
    {
        return Some(4);
    }
    None
}

// ---- UI ---------------------------------------------------------------------

fn get_row_entry(row: &gtk4::ListBoxRow) -> Option<DesktopEntry> {
    unsafe {
        row.data::<DesktopEntry>("entry")
            .map(|p| p.as_ref().clone())
    }
}

fn run_ui(entries: Vec<DesktopEntry>, history: LaunchHistory) {
    let app = Application::builder()
        .application_id("com.breadway.breadbox")
        .build();

    let history_rc = Rc::new(RefCell::new(history));
    let query_rc: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

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
        let window = bread_utils::gtk_popup::new_overlay_window(app, "breadbox");

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
        vbox.set_margin_top(120);
        vbox.set_size_request(600, -1);

        let search = SearchEntry::new();
        search.set_placeholder_text(Some("Search apps…"));
        vbox.append(&search);

        let scroll = ScrolledWindow::new();
        scroll.set_policy(PolicyType::Never, PolicyType::Automatic);
        scroll.set_max_content_height(480);
        scroll.set_propagate_natural_height(true);

        let list = ListBox::new();
        list.set_selection_mode(SelectionMode::Browse);

        for (idx, entry) in entries.iter().enumerate() {
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
        window.set_child(Some(&vbox));

        // Filter on keystroke
        let list_f = list.clone();
        let filter_query = Rc::clone(&query_rc);
        search.connect_changed(move |entry| {
            let text = entry.text();
            let query = text.as_str();
            *filter_query.borrow_mut() = query.to_string();

            // Two passes: first collect each row's match tier, then decide
            // visibility — a tier-4 (bare subsequence) row only gets hidden
            // once we know whether some *other* row has a real tier ≤2 hit.
            let mut rows = Vec::new();
            let mut i = 0i32;
            while let Some(row) = list_f.row_at_index(i) {
                let tier = get_row_entry(&row).and_then(|e| match_tier(query, &e));
                rows.push((row, tier));
                i += 1;
            }
            let has_direct_hit = rows.iter().any(|(_, t)| matches!(t, Some(0..=2)));
            for (row, tier) in &rows {
                let vis = match tier {
                    None => false,
                    Some(4) if has_direct_hit => false,
                    Some(_) => true,
                };
                row.set_visible(vis);
            }
            list_f.invalidate_sort();
            let first_vis = (0i32..).find_map(|j| {
                list_f.row_at_index(j).filter(|r| r.is_visible())
            });
            list_f.select_row(first_vis.as_ref());
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
                    bread_utils::gtk_popup::select_next_visible(&list_k);
                    glib::Propagation::Stop
                }
                Key::Up => {
                    bread_utils::gtk_popup::select_prev_visible(&list_k);
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
        bread_utils::gtk_popup::close_on_outside_click(&window, &vbox, move || close_outside());

        window.present();
        search.grab_focus();
    });

    app.run();
}

// ---- Main -------------------------------------------------------------------

fn main() {
    // Kept alive for the rest of `main` — dropping it releases the
    // single-instance lock and removes the pid file, which happens
    // naturally once `run_ui` returns (after the window closes).
    let _singleton_guard = match bread_utils::singleton::toggle_or_kill("breadbox") {
        Ok(bread_utils::singleton::Toggle::Started(guard)) => Some(guard),
        Ok(bread_utils::singleton::Toggle::KilledExisting) => return,
        Err(e) => {
            eprintln!("breadbox: single-instance lock unavailable ({e}); continuing without it");
            None
        }
    };

    let config = Config::load();
    let workspace = bread_utils::hypr::active_workspace_name().unwrap_or_default();
    let priority = config
        .context_for(&workspace)
        .map(|c| c.priority.clone())
        .unwrap_or_default();

    let history = LaunchHistory::load();
    let manifest = load_manifest();
    let entries = load_sorted_entries(&manifest, &priority, &history);

    run_ui(entries, history);
}
