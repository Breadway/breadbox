use bread_launcher::gtk::{row_entry, ResultsList};
use bread_theme::{hex_to_rgba, ink_on, load_palette, Palette};
use std::{
    cell::RefCell,
    collections::HashMap,
    env, fs,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    rc::Rc,
};

/// This app's id in bread's sibling-app namespace registry
/// (`bread_shared::apps::KNOWN_APPS`) — events publish as `bread.box.*`.
const APP_ID: &str = "box";

/// Emitted instead of mapping breadbox's own window when the active shell
/// theme's launcher is embedded (theme 04's capsule). Must stay inside
/// `bread.box.*` — see [`dispatch_embedded_open`] for why.
pub const EMBEDDED_OPEN_EVENT: &str = "bread.box.open_requested";

/// Published via `bread_launcher::do_launch` after a successful launch —
/// see `EVENTS.md`.
const LAUNCHED_EVENT: &str = "bread.box.launched";

use breadbox_shared::{config_dir, Config, DesktopEntry, LaunchHistory};
use gtk4::{
    glib, prelude::*, Application, Box as GBox, CssProvider,
    EventControllerKey, Label, Orientation, SearchEntry,
};

mod listen;
mod screenshot;
mod theme;

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
    let path = breadbox_shared::icon_manifest_path();
    let content = fs::read_to_string(&path).unwrap_or_default();
    serde_json::from_str::<HashMap<String, String>>(&content)
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k, PathBuf::from(v)))
        .collect()
}

// ---- Theming ----------------------------------------------------------------

fn build_css(
    p: &Palette,
    launcher: &bread_theme::shell::Launcher,
    tokens: &bread_theme::shell::Tokens,
) -> String {
    // Panel opacity comes from the theme, not a hardcoded constant. 0.60 made
    // the launcher wash out over a bright wallpaper and its text hard to read —
    // a bar can be that translucent because it covers a thin strip, a full
    // panel cannot. The approved reference uses 0.95/0.93 per theme.
    let bg_panel = hex_to_rgba(&p.background, launcher.panel_alpha as f32);
    let radius = format!("{}px", launcher.radius);
    let on_bg = ink_on(&p.background);
    // breadbox-specific rules only — the generic widget baseline (buttons,
    // switches, plain `entry`/`spinbutton`) comes from the shared ecosystem
    // stylesheet, applied first (lower priority) in connect_activate via
    // `apply_shared`. Colour is set on each surface (panel, search box,
    // hovered/selected row) so child labels inherit the legible ink for that
    // background. `on_*` are luminance-picked black/white — the pywal hues
    // are untouched. Without this a light `surface` slot makes the selected
    // row's text vanish.
    //
    // Selectors matter here: GTK4's `GtkSearchEntry` CSS node is `entry`
    // carrying a `.search` style class (confirmed against
    // `/usr/share/gir-1.0/Gtk-4.0.gir`'s `SearchEntry` "## CSS Nodes" doc,
    // `entry.search ╰── text`) — NOT a node literally named `searchentry`,
    // and `GtkListBox`'s node is `list`, NOT `listbox`. Both bogus selectors
    // shipped here previously and silently matched nothing, which is why
    // this rule block's `border`/`outline` suppression never actually beat
    // the shared stylesheet's `entry:focus-within { border-color: @accent }`
    // rule (`bread_theme::stylesheet`, still layered in underneath at
    // APPLICATION priority) — the accent-coloured focus "outline" the user
    // was seeing was that shared rule showing through unopposed. `entry`
    // alone (no `.search`) would work too since this window has only one
    // entry, but `.search` documents which GtkSearchEntry state this targets
    // and is what actually renders.
    //
    // Per-theme geometry now comes entirely from `[launcher]`/`[tokens]` —
    // liquid-motion and glass-workbench set every one of these to different
    // values (see their `assets/shell/*/theme.toml`), which is what makes
    // the two themes read as different instruments rather than one
    // launcher recoloured.
    format!(
        "window {{ background-color: transparent; }}\
         .launcher-bg {{ background-color: {bg_panel}; color: {on_bg}; border-radius: {radius};\
             /* NO drop shadow. In a browser `backdrop-filter` blurs only the\
                element's own box, so the demo's shadow is free. Hyprland\
                blurs any surface pixel above `ignore_alpha` (0.2), and a\
                32px shadow at 0.6 alpha is far above it — so the compositor\
                blurred the shadow as well, painting a rectangular blurred\
                halo around the rounded panel. That halo is the \"blur\
                backdrop whose radius doesn't match the content\" problem.\
                The blur behind a 0.95-opaque panel already gives plenty of\
                separation; the shadow only fought it. */\
             font-family: \"{font_family}\", {font_fallback}; }}\
         entry.search {{ background-color: transparent; color: {on_bg}; caret-color: {accent};\
             border: none; outline: none; box-shadow: none; border-radius: 0;\
             border-bottom: 1px solid {hairline};\
             padding: {search_pv}px {search_ph}px; font-size: {search_fs}px; }}\
         entry.search:focus, entry.search:focus-within {{\
             outline: none; box-shadow: none; border-color: transparent;\
             border-bottom: 1px solid {hairline}; }}\
         list {{ background-color: transparent; padding: 4px 0; }}\
         row {{ padding: {row_pv}px {row_ph}px; margin: 0 {row_inset}px; color: {on_bg};\
             background-color: transparent; border-radius: {row_radius}px;\
             font-size: {row_fs}px; }}\
         row:hover, row:selected {{ background-color: {selection_bg}; color: {on_bg}; }}\
         .app-muted {{ opacity: 0.4; font-size: 11px; }}\
         /* No background tile: these are real app icons, not the demo's\
            placeholder .ico boxes. A tint behind a real icon reads as a\
            failed/unloaded image. */\
         image {{ margin-right: 8px; border-radius: {icon_radius}px; }}\
         /* \"Recent\"/\"Apps\" headers (`[launcher].sections`, liquid-motion\
            only — glass-workbench's flat list never builds these rows). */\
         .section-header-label {{ font-size: 10px; letter-spacing: 0.14em;\
             text-transform: uppercase; font-weight: 600; opacity: 0.45; }}\
         .bread-drawer-section-header {{ padding: 11px 16px 5px; }}\
         .launcher-footer {{ padding: 8px 16px 12px; font-size: 10px; opacity: 0.4;\
             {footer_case} }}",
        bg_panel      = bg_panel,
        accent        = hex_to_rgba(&p.color4, 0.9),
        on_bg         = on_bg,
        hairline      = hex_to_rgba(on_bg, 0.08),
        selection_bg  = hex_to_rgba(&p.color4, launcher.selection_alpha as f32),
        radius        = radius,
        font_family   = tokens.font_family(),
        font_fallback = tokens.font_fallback(),
        row_fs        = tokens.font_size_base(),
        row_pv        = launcher.row_padding_v,
        row_ph        = launcher.row_padding_h,
        row_inset     = launcher.row_inset,
        row_radius    = launcher.row_radius,
        icon_radius   = launcher.icon_radius,
        search_pv     = launcher.search_padding_v,
        search_ph     = launcher.search_padding_h,
        search_fs     = launcher.search_font_size,
        // Section headers and an uppercase, letter-spaced footer travel
        // together in both demos (liquid-motion has both; glass-workbench's
        // flat list has neither) — reusing `sections` here instead of a
        // dedicated schema key for this one footer detail.
        footer_case   = if launcher.sections {
            "text-transform: uppercase; letter-spacing: 0.12em;"
        } else {
            ""
        },
    )
}

/// `[launcher].footer`'s noun: `"count_apps"` → "applications", anything
/// else (`"count_results"` included — every other current/future value
/// falls back to this rather than a hard error, matching the "never fails
/// to start over a theme string" rule the rest of this schema follows) →
/// "results". `n`'s plural/singular form is picked here rather than baked
/// into `[launcher].footer` itself, since neither built-in theme's value is
/// singular-aware.
fn footer_text(footer_kind: &str, n: usize) -> String {
    let noun = if footer_kind == "count_apps" {
        "application"
    } else {
        "result"
    };
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// Counts the rows currently shown as real (non-header) app matches — the
/// same "visible AND carries a `DesktopEntry`" test `ResultsList` itself
/// uses for keyboard selection (`select_next`/`select_prev` in
/// `bread_launcher::gtk`), reimplemented here read-only since `ResultsList`
/// doesn't expose a count of its own.
fn visible_app_count(list: &gtk4::ListBox) -> usize {
    let mut i = 0i32;
    let mut n = 0usize;
    while let Some(row) = list.row_at_index(i) {
        if row.is_visible() && bread_launcher::gtk::row_entry(&row).is_some() {
            n += 1;
        }
        i += 1;
    }
    n
}

// ---- UI ---------------------------------------------------------------------

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
    let is_screenshot_run = screenshot_req.is_some();

    app.connect_activate(move |app| {
        // Shell theme, loaded once per process and cached (see src/theme.rs)
        // — the source of the launcher's geometry and style values below,
        // per THEME_SYSTEM_PLAN.md §4/§11 Phase 4.
        let shell_theme = theme::shell_theme();
        let launcher = shell_theme.launcher().clone();

        // Shared ecosystem base (fonts, palette, generic widgets) as a
        // display-level fallback; the real per-window sheet is bound below.
        bread_theme::gtk::apply_shared();
        {
            let launcher = launcher.clone();
            let tokens = shell_theme.tokens().clone();
            bread_theme::gtk::apply_app_css(move || build_css(&load_palette(), &launcher, &tokens));
        }

        // User CSS override
        {
            let user_css_path = config_dir().join("style.css");
            let user_cell: RefCell<Option<CssProvider>> = RefCell::new(None);
            bread_theme::gtk::apply_user_css(&user_css_path, &user_cell);
        }

        // Full-screen transparent overlay; panel widget is positioned inside it.
        let window = bread_utils::gtk_popup::new_overlay_window(app, "breadbox");
        // Bind the *app* sheet — not just the shared one — as the widget-tree
        // provider. `bind_window_auto` alone re-broadcasts the shared
        // component sheet (which includes `window { background-color: @bg }`)
        // at USER-10, outranking our APPLICATION-priority
        // `window { background-color: transparent }` regardless of specificity
        // — so the "transparent overlay" paints solid @bg and the launcher
        // covers the screen in an opaque black rectangle (worse under a VM's
        // software renderer, where there's no GL to mask it). `_with_app_css`
        // rides our sheet at USER-9, back on top.
        {
            let launcher = launcher.clone();
            let tokens = shell_theme.tokens().clone();
            bread_theme::gtk::bind_window_auto_with_app_css(&window, move |p| {
                build_css(p, &launcher, &tokens)
            });
        }

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
        vbox.set_margin_top(theme::top_margin_px(&launcher.top));
        // `set_size_request` only pins a MINIMUM, not a maximum — the same
        // class of bug that made breadbar's capsule stretch to its widest
        // row (`surface.rs`/`main.rs`'s `set_size_request` pin comment).
        // This is safe here because `results.scroller` below is the
        // `bread_launcher::gtk::ResultsList` widget, which already calls
        // `scroller.set_propagate_natural_width(false)` specifically so its
        // `ListBox`'s widest row (icon + full app name + wm-class) can never
        // push a host wider than what the host requests — see that widget's
        // own comment in `bread-launcher/src/gtk.rs`. `search` (a plain
        // `SearchEntry`) has no unbounded natural width either. So nothing
        // under `vbox` can grow past `launcher.width`, and this minimum-only
        // request is effectively exact.
        vbox.set_size_request(launcher.width, -1);

        let search = SearchEntry::new();
        // "Search", not the app's own name — matches both design demos'
        // `<input placeholder="Search">` (bos-ui-demos/proposed/
        // {liquid-motion,glass-workbench}.html); no user-visible copy here
        // should read like an internal identifier.
        search.set_placeholder_text(Some("Search"));
        vbox.append(&search);

        // Row building, fuzzy filtering, match/history sorting, and
        // keyboard-style selection movement all live in bread-launcher's
        // `ResultsList` — the widget breadbar's embedded capsule also uses
        // (THEME_SYSTEM_PLAN.md §7). `[launcher].sections`: liquid-motion
        // groups the idle view into "Recent"/"Apps" headers, glass-workbench
        // stays the flat, ungrouped list — no longer hardcoded to `false`.
        let results = ResultsList::new(
            &entries,
            launcher.icon_px,
            Rc::clone(&history_rc),
            launcher.sections,
        );

        vbox.append(&results.scroller);

        // Footer: "N applications" (liquid-motion) / "N results"
        // (glass-workbench) — `[launcher].footer` selects the noun (see
        // `footer_text`), updated alongside the query on every keystroke.
        let footer = Label::new(None);
        footer.add_css_class("launcher-footer");
        footer.set_xalign(0.0);
        footer.set_label(&footer_text(&launcher.footer, visible_app_count(&results.list)));
        vbox.append(&footer);

        window.set_child(Some(&vbox));

        // Filter on keystroke
        let results_f = results.clone();
        let footer_f = footer.clone();
        let footer_kind = launcher.footer.clone();
        search.connect_changed(move |entry| {
            results_f.set_query(entry.text().as_str());
            footer_f.set_label(&footer_text(&footer_kind, visible_app_count(&results_f.list)));
        });

        // Keyboard handling — capture phase on window
        let key_ctrl = EventControllerKey::new();
        key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let close_k = Rc::clone(&close_all);
        let results_k = results.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, _| {
            use gtk4::gdk::Key;
            match key {
                Key::Escape => {
                    close_k();
                    glib::Propagation::Stop
                }
                Key::Return | Key::KP_Enter => {
                    if let Some(entry) = results_k.selected_entry() {
                        results_k.record_launch(&entry);
                        bread_launcher::do_launch(&entry, APP_ID, LAUNCHED_EVENT);
                        close_k();
                    }
                    glib::Propagation::Stop
                }
                Key::Down => {
                    results_k.select_next();
                    glib::Propagation::Stop
                }
                Key::Up => {
                    results_k.select_prev();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        window.add_controller(key_ctrl);

        // Row click launches
        let close_a = Rc::clone(&close_all);
        let results_a = results.clone();
        results.list.connect_row_activated(move |_, row| {
            if let Some(entry) = row_entry(row) {
                results_a.record_launch(&entry);
                bread_launcher::do_launch(&entry, APP_ID, LAUNCHED_EVENT);
                close_a();
            }
        });

        // Click outside launcher panel → close
        {
            let close_outside = Rc::clone(&close_all);
            bread_utils::gtk_popup::close_on_outside_click(&window, &vbox, move || close_outside());
        }

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
    // Surface bread-theme / bread-utils `tracing::warn!` (dropped silently
    // before). `RUST_LOG` overrides; default warn+.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    if std::env::args().nth(1).as_deref() == Some("listen") {
        listen::run();
        return;
    }

    use clap::Parser;
    let cli = screenshot::Cli::parse();
    let screenshot_req = cli.screenshot_request();

    // Under an `[launcher] mode = "embedded"` theme (spotlight,
    // THEME_SYSTEM_PLAN.md §7 phase 6c), breadbar's own bar-drawer capsule
    // IS the launcher — mapping this binary's overlay window on top of it
    // would stack a second launcher over the capsule, exactly the bug a
    // keybind that still directly execs `breadbox` (see the CLAUDE.md task
    // notes: `bash -c breadbox` in ~/.config/hypr/binds.json) would hit
    // every time. `--screenshot` runs are exempt — those exist to capture
    // THIS binary's own overlay for its own screenshot views regardless of
    // whatever theme happens to be active on the machine running them.
    if screenshot_req.is_none()
        && crate::theme::shell_theme().launcher().mode
            == bread_theme::shell::LauncherMode::Embedded
    {
        // `dispatch_embedded_open` only returns `true` once it has confirmed
        // breadd itself is reachable (`BreadClient::health`) and emitted the
        // redirect — see its doc comment for why that is the strongest
        // guarantee this transport can give (there is no ack from breadbar).
        // When breadd is unreachable, fall through to mapping breadbox's own
        // overlay below instead of returning: a keybind press must always
        // open *something*, even under an embedded theme, rather than
        // silently doing nothing because the bus happens to be down.
        if dispatch_embedded_open() {
            return;
        }
    }

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
                eprintln!(
                    "breadbox: single-instance lock unavailable ({e}); continuing without it"
                );
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

    let history = breadbox_shared::launch_history();
    let manifest = load_manifest();
    let entries = bread_launcher::load_sorted_entries(&manifest, &priority, &history);

    run_ui(entries, history, screenshot_req);
}

/// Redirects an embedded-theme launch to the bus instead of mapping a
/// window (see the `main` call site). breadbar's capsule (spotlight)
/// subscribes to this and focuses/opens itself on receipt — see
/// `breadbar/src/launcher_command.rs`.
///
/// Emits `bread.box.open_requested`, NOT `bread.command.box.open`. Two
/// reasons, one mechanical and one semantic:
///
/// `BreadClient::emit` enforces that an app may only publish within its own
/// `bread.<app_id>.*` namespace (`bread_utils::bread_client`'s
/// `validate_app_namespace`). breadbox's app id is `box`, so a
/// `bread.command.*` event is refused outright and the redirect silently did
/// nothing but print a warning.
///
/// The guard is right, and the original name had the direction backwards.
/// `bread.command.<app>.*` is a command addressed TO an app by an outside
/// trigger — it is what breadbox's own `listen` subscribes to. An app
/// emitting a command at itself would mean breadbox both sends and receives
/// the same verb, which is also how the respawn loop `listen.rs` guards
/// against arises. What actually happened here is an event: breadbox was
/// asked to open and is reporting that, so it belongs in breadbox's own
/// namespace as a past-tense fact.
///
/// Fire-and-forget, same as every other `BreadClient::emit` here — breadd
/// being unreachable must never turn a keybind press into an error dialog or
/// a hung process. But "nobody was listening" used to also mean "the
/// keybind silently does nothing at all", which is the worst failure mode
/// available for a launcher: no window, no error, nothing on stderr. This
/// now checks reachability first (`BreadClient::health`, a real round trip
/// with a bounded timeout) so the `main` call site can fall back to mapping
/// breadbox's own overlay window when breadd itself is down — see its call
/// site.
///
/// That fallback only covers "breadd is unreachable", not "breadd is up but
/// nobody is subscribed" (breadbar isn't running, or is running under a
/// non-Embedded theme and therefore never subscribed —
/// `launcher_command.rs::spawn`). `BreadClient`'s API has no way to ask "is
/// anything actually subscribed to this event" — `emit` has no ack and
/// `health` only reports breadd's own liveness — so that narrower gap can't
/// be closed without a bus-level ack protocol, which is out of scope here.
/// Returns `true` iff breadd was reachable and the redirect was sent.
fn dispatch_embedded_open() -> bool {
    let client = bread_utils::bread_client::BreadClient::connect(APP_ID);
    if client.health().is_none() {
        eprintln!(
            "breadbox: embedded launcher theme active but breadd is unreachable \
             (health check failed); falling back to breadbox's own overlay window \
             instead of a silent no-op"
        );
        return false;
    }
    client.emit(EMBEDDED_OPEN_EVENT, serde_json::json!({}));
    eprintln!(
        "breadbox: embedded launcher theme active; redirected the open request to \
         '{EMBEDDED_OPEN_EVENT}' for breadbar's capsule to handle (breadd is \
         reachable, but this is fire-and-forget with no ack — if breadbar isn't \
         running or isn't subscribed, this is still a silent no-op on the bus)"
    );
    true
}
