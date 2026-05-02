//! GTK4 + libadwaita view layer for Kryptos.
//!
//! Module layout:
//!
//! - [`window`]      — widget tree construction.
//! - [`statusline`]  — mode line + command/search bar widgets.
//! - [`input`]       — gdk → [`crate::vim::Key`] translation.
//! - [`dispatcher`]  — apply [`crate::vim::Action`]s to the widget tree.
//! - [`settings`]    — `adw::PreferencesWindow` over `~/.config/kryptos/config.toml`.
//! - [`onboarding`]  — first-run device-link flow (QR + signal-cli polling).

mod commands;
mod composer;
mod dispatcher;
mod input;
pub mod onboarding;
pub mod settings;
mod statusline;
mod window;

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use tracing::{error, info, warn};

use crate::config::{loader, Config};
use crate::theme::ThemeManager;
use crate::vim::{Engine, KeySym, KeymapSet, Mode, Outcome};

use dispatcher::Dispatcher;
use window::WindowParts;

const APP_ID: &str = "dev.kryptos.Kryptos";

/// Run the libadwaita application loop. Returns the glib exit code.
pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();

    app.connect_activate(activate);
    app.run()
}

fn activate(app: &adw::Application) {
    info!("activating main window");

    let config_path = match loader::default_path() {
        Ok(p) => p,
        Err(e) => {
            error!(error = %e, "could not resolve config path");
            return;
        }
    };
    let cfg = match loader::load_or_default(&config_path) {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "config load failed; falling back to defaults");
            Config::default()
        }
    };

    if cfg.backends.telegram.enabled {
        warn!(
            "Telegram backend is enabled in config but requires manual login — \
             use `:telegram-login` (TODO)"
        );
    }

    let parts = window::build(app, &cfg);

    // Install theme stack against the default display and apply the
    // configured theme. Failures to apply log + fall through with the
    // built-in provider unloaded (libadwaita defaults).
    let theme = match gtk::gdk::Display::default() {
        Some(display) => {
            let mut tm = ThemeManager::install_for_display(&display);
            if let Err(e) = tm.apply(&cfg.general.theme) {
                warn!(error = %e, "initial theme apply failed");
            }
            tm
        }
        None => {
            error!("no default display; theme manager disabled");
            // We still need *something*; install_for_display panics
            // without a display anyway, so fall back to a placeholder
            // by creating one against any available display we can find.
            // In practice activate() always runs with a display available.
            return;
        }
    };
    let theme = Rc::new(RefCell::new(theme));

    let engine = match KeymapSet::from_config(&cfg) {
        Ok(set) => Engine::new(set),
        Err(e) => {
            error!(error = %e, "keymap build failed; using empty keymaps");
            Engine::new(KeymapSet::default())
        }
    };
    let engine = Rc::new(RefCell::new(engine));
    let dispatcher = Dispatcher::new(&parts, theme.clone(), config_path.clone());

    // Composer-local Enter: when the user hits Enter in the composer
    // itself (Normal or Insert), the composer's own controller fires
    // first and ends up here. We log + sync the window-level engine
    // back to Normal so the modeline reflects reality.
    {
        let engine = engine.clone();
        let mode_line = parts.mode_line.clone();
        parts.composer.set_on_send(move |text| {
            info!(message = %text, "send message (composer Enter)");
            engine.borrow_mut().set_mode(Mode::Normal);
            mode_line.set_mode(Mode::Normal);
        });
    }

    wire_command_bar(&parts, &dispatcher, engine.clone());
    wire_keys(&parts, &dispatcher, engine.clone());

    parts.mode_line.set_mode(engine.borrow().mode());
    parts.window.present();

    maybe_open_first_run_linker(&parts.window);
}

/// Off-thread first-run check. If signal-cli is reachable and reports
/// zero accounts, we surface the linker non-modally on top of the
/// freshly-presented main window. Any error path (no bus, no daemon)
/// falls through to the main UI so the user is never trapped.
fn maybe_open_first_run_linker(window: &adw::ApplicationWindow) {
    use std::sync::mpsc;
    use std::time::Duration;

    use crate::dbus::SignalClient;

    let (tx, rx) = mpsc::channel::<bool>();
    std::thread::Builder::new()
        .name("kryptos-first-run".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()
                    .map(|rt| {
                        rt.block_on(async {
                            match SignalClient::connect().await {
                                Ok(c) => {
                                    let _ = crate::dbus::ensure_running(c.connection()).await;
                                    onboarding::first_run_check_async(&c).await
                                }
                                Err(_) => false,
                            }
                        })
                    })
                    .unwrap_or(false)
            });
            let _ = tx.send(result.unwrap_or(false));
        })
        .expect("spawn first-run probe thread");

    let win = window.clone();
    glib::source::timeout_add_local(Duration::from_millis(150), move || match rx.try_recv() {
        Ok(true) => {
            onboarding::open_linker(&win);
            glib::ControlFlow::Break
        }
        Ok(false) => glib::ControlFlow::Break,
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
    });
}

fn wire_command_bar(parts: &WindowParts, dispatcher: &Dispatcher, engine: Rc<RefCell<Engine>>) {
    // Activate (Enter) — commit and return to Normal.
    let entry = parts.command_bar.entry().clone();
    let bar = parts.command_bar.clone();
    let mode_line = parts.mode_line.clone();
    let dispatcher_act = dispatcher.clone();
    let engine_act = engine.clone();
    entry.connect_activate(move |entry| {
        let text = entry.text().to_string();
        let mode = engine_act.borrow().mode();
        match mode {
            Mode::Command => dispatcher_act.run_command(&text),
            Mode::Search => dispatcher_act.run_search(&text),
            _ => {}
        }
        bar.hide();
        engine_act.borrow_mut().set_mode(Mode::Normal);
        mode_line.set_mode(Mode::Normal);
    });

    // Esc inside the entry — cancel and return to Normal. In Search
    // mode we also drop the active filter so the sidebar is whole again.
    let esc = gtk::EventControllerKey::new();
    let bar_esc = parts.command_bar.clone();
    let mode_line_esc = parts.mode_line.clone();
    let engine_esc = engine.clone();
    let dispatcher_esc = dispatcher.clone();
    esc.connect_key_pressed(move |_, keyval, _, _| {
        if keyval == gtk::gdk::Key::Escape {
            let mode_before = engine_esc.borrow().mode();
            if mode_before == Mode::Search {
                dispatcher_esc.clear_search();
            }
            bar_esc.hide();
            engine_esc.borrow_mut().set_mode(Mode::Normal);
            mode_line_esc.set_mode(Mode::Normal);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    parts.command_bar.entry().add_controller(esc);
}

fn wire_keys(parts: &WindowParts, dispatcher: &Dispatcher, engine: Rc<RefCell<Engine>>) {
    let controller = gtk::EventControllerKey::new();

    let mode_line = parts.mode_line.clone();
    let command_bar = parts.command_bar.clone();
    let dispatcher = dispatcher.clone();

    controller.connect_key_pressed(move |_ctrl, keyval, _keycode, state| {
        // While the command bar is up, leave its entry alone — its own
        // controllers handle Enter / Esc.
        if command_bar.is_visible() {
            return glib::Propagation::Proceed;
        }

        let Some(key) = input::translate_gdk(keyval, state) else {
            return glib::Propagation::Proceed;
        };

        let current_mode = engine.borrow().mode();

        // In Insert, only forward Esc and Ctrl-modified keys to the
        // engine. Plain printable keys flow to the focused composer.
        if current_mode == Mode::Insert
            && !key.mods.ctrl
            && !matches!(&key.sym, KeySym::Named(n) if n == "Esc")
        {
            return glib::Propagation::Proceed;
        }

        let outcome = engine.borrow_mut().feed(key);
        match outcome {
            Outcome::Action(action) => {
                let mode_before = engine.borrow().mode();
                if let Some(new_mode) = dispatcher.dispatch(&action, mode_before) {
                    engine.borrow_mut().set_mode(new_mode);
                }
                mode_line.set_mode(engine.borrow().mode());
                glib::Propagation::Stop
            }
            Outcome::Pending => glib::Propagation::Stop,
            Outcome::Cancelled => {
                if engine.borrow().mode() == Mode::Insert {
                    glib::Propagation::Proceed
                } else {
                    glib::Propagation::Stop
                }
            }
        }
    });

    parts.window.add_controller(controller);
}
