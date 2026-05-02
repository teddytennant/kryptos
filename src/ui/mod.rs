//! GTK4 + libadwaita view layer for Kryptos.
//!
//! Module layout:
//!
//! - [`window`]      — widget tree construction.
//! - [`statusline`]  — mode line + command/search bar widgets.
//! - [`input`]       — gdk → [`crate::vim::Key`] translation.
//! - [`dispatcher`]  — apply [`crate::vim::Action`]s to the widget tree.
//! - [`settings`]    — `adw::PreferencesWindow` over `~/.config/kryptos/config.toml`.

mod dispatcher;
mod input;
pub mod settings;
mod statusline;
mod window;

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use tracing::{error, info};

use crate::config::{loader, Config};
use crate::vim::{Engine, KeymapSet, KeySym, Mode, Outcome};

use dispatcher::Dispatcher;
use window::WindowParts;

const APP_ID: &str = "dev.kryptos.Kryptos";

/// Run the libadwaita application loop. Returns the glib exit code.
pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .build();

    app.connect_activate(activate);
    app.run()
}

fn activate(app: &adw::Application) {
    info!("activating main window");

    let cfg = match loader::default_path().and_then(|p| loader::load_or_default(&p)) {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "config load failed; falling back to defaults");
            Config::default()
        }
    };

    let parts = window::build(app, &cfg);

    let engine = match KeymapSet::from_config(&cfg) {
        Ok(set) => Engine::new(set),
        Err(e) => {
            error!(error = %e, "keymap build failed; using empty keymaps");
            Engine::new(KeymapSet::default())
        }
    };
    let engine = Rc::new(RefCell::new(engine));
    let dispatcher = Dispatcher::from_parts(&parts);

    wire_command_bar(&parts, &dispatcher, engine.clone());
    wire_keys(&parts, &dispatcher, engine.clone());

    parts.mode_line.set_mode(engine.borrow().mode());
    parts.window.present();
}

fn wire_command_bar(
    parts: &WindowParts,
    dispatcher: &Dispatcher,
    engine: Rc<RefCell<Engine>>,
) {
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

    // Esc inside the entry — cancel and return to Normal.
    let esc = gtk::EventControllerKey::new();
    let bar_esc = parts.command_bar.clone();
    let mode_line_esc = parts.mode_line.clone();
    let engine_esc = engine.clone();
    esc.connect_key_pressed(move |_, keyval, _, _| {
        if keyval == gtk::gdk::Key::Escape {
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
