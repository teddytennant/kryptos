//! First-run welcome experience.
//!
//! Shown when `~/.config/kryptos/config.toml` doesn't exist or
//! `[onboarding] completed` is `false`. The flow is a four-page
//! `adw::Carousel` (carousel chosen over `NavigationView` so the user
//! can see momentum dots and we don't have to hand-code a back stack):
//!
//! 1. **Hello** — hero title + body + "Continue" CTA.
//! 2. **Pick your messengers** — Signal / Telegram action rows with
//!    "Set up later" toggles. Selections are stored in the shared
//!    `Choices` cell so page 3 knows what to inline.
//! 3. **Link Signal** — inlines the existing linker UI when Signal is
//!    selected, otherwise advances on its own.
//! 4. **Done** — "You're all set." + "Start chatting" button.
//!
//! Completion (or "Skip onboarding") writes `[onboarding] completed = true`
//! into the config so the welcome window is one-shot.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use tracing::{error, warn};

use crate::config::loader;

#[derive(Clone, Copy, Default)]
struct Choices {
    signal: bool,
    telegram: bool,
}

/// Build and present the welcome window over `parent`.
///
/// `on_finish` fires once — when the user finishes the flow or clicks
/// "Skip onboarding" — and is the caller's signal that they should make
/// the main shell interactive (or kick off post-link refresh).
pub fn present(
    parent: &impl IsA<gtk::Window>,
    config_path: PathBuf,
    on_finish: impl Fn() + 'static,
) {
    let on_finish = Rc::new(on_finish);
    let choices: Rc<Cell<Choices>> = Rc::new(Cell::new(Choices::default()));
    let finished: Rc<Cell<bool>> = Rc::new(Cell::new(false));

    let win = adw::Window::builder()
        .transient_for(parent.as_ref())
        .modal(true)
        .default_width(560)
        .default_height(640)
        .title("Welcome to Kryptos")
        .build();
    win.add_css_class("kryptos-welcome");

    let header = adw::HeaderBar::builder().show_title(false).build();
    header.add_css_class("flat");

    let carousel = adw::Carousel::builder()
        .interactive(false) // pages advance via buttons, not swipe
        .vexpand(true)
        .hexpand(true)
        .build();

    // Indicator dots so the user can see they're on page X of 4.
    let dots = adw::CarouselIndicatorDots::builder()
        .carousel(&carousel)
        .build();
    dots.add_css_class("kryptos-welcome-dots");

    let skip_link = gtk::Button::with_label("Skip onboarding");
    skip_link.add_css_class("flat");
    skip_link.add_css_class("kryptos-welcome-skip");

    let footer = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .build();
    footer.set_margin_start(24);
    footer.set_margin_end(24);
    footer.set_margin_top(12);
    footer.set_margin_bottom(20);
    footer.append(&skip_link);

    let dot_holder = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .halign(gtk::Align::End)
        .hexpand(true)
        .build();
    dot_holder.append(&dots);
    footer.append(&dot_holder);

    // Pages.
    let page1 = page_hello(&carousel);
    let page2 = page_pick_messengers(&carousel, choices.clone());
    let page3 = page_link_signal(&carousel, choices.clone(), parent.as_ref());
    let page4 = page_done(&carousel, &win, &finished, on_finish.clone(), &config_path);

    carousel.append(&page1);
    carousel.append(&page2);
    carousel.append(&page3);
    carousel.append(&page4);

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    body.append(&carousel);
    body.append(&footer);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&body));
    win.set_content(Some(&toolbar));

    install_styles();

    // Skip path: write completed=true and close.
    {
        let win = win.clone();
        let path = config_path.clone();
        let finished = finished.clone();
        let on_finish = on_finish.clone();
        skip_link.connect_clicked(move |_| {
            finished.set(true);
            persist_completed(&path);
            on_finish();
            win.close();
        });
    }

    // If the user closes the window via the title-bar close button
    // without clicking through, treat it as "skip" so we still mark
    // onboarding done and unlock the main shell.
    {
        let on_finish = on_finish.clone();
        let path = config_path.clone();
        let finished = finished.clone();
        win.connect_close_request(move |_| {
            if !finished.get() {
                finished.set(true);
                persist_completed(&path);
                on_finish();
            }
            glib::Propagation::Proceed
        });
    }

    win.present();
}

fn page_hello(carousel: &adw::Carousel) -> gtk::Widget {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(16)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .build();
    column.set_margin_start(48);
    column.set_margin_end(48);

    let title = gtk::Label::new(Some("Welcome to Kryptos"));
    title.add_css_class("kryptos-welcome-title");
    title.set_halign(gtk::Align::Center);

    let body = gtk::Label::builder()
        .label("A clean, fast, vim-first messaging client for Linux.")
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .justify(gtk::Justification::Center)
        .halign(gtk::Align::Center)
        .build();
    body.add_css_class("kryptos-welcome-body");

    let cta = gtk::Button::with_label("Continue");
    cta.add_css_class("suggested-action");
    cta.add_css_class("kryptos-welcome-cta");
    cta.set_halign(gtk::Align::Center);
    cta.set_size_request(140, 40);

    {
        let carousel = carousel.clone();
        cta.connect_clicked(move |_| {
            advance_to(&carousel, 1);
        });
    }

    column.append(&title);
    column.append(&body);
    column.append(&cta);
    column.upcast()
}

fn page_pick_messengers(carousel: &adw::Carousel, choices: Rc<Cell<Choices>>) -> gtk::Widget {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(16)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .build();
    column.set_margin_start(48);
    column.set_margin_end(48);

    let title = gtk::Label::new(Some("Pick your messengers"));
    title.add_css_class("kryptos-welcome-title");
    title.set_halign(gtk::Align::Center);

    let body = gtk::Label::builder()
        .label("Choose what to set up now. You can always add more later.")
        .wrap(true)
        .justify(gtk::Justification::Center)
        .halign(gtk::Align::Center)
        .build();
    body.add_css_class("kryptos-welcome-body");

    let signal_row = adw::ActionRow::builder()
        .title("Signal")
        .subtitle("Encrypted messaging via signal-cli")
        .build();
    let signal_switch = gtk::Switch::new();
    signal_switch.set_valign(gtk::Align::Center);
    signal_row.add_suffix(&signal_switch);
    signal_row.set_activatable_widget(Some(&signal_switch));

    let telegram_row = adw::ActionRow::builder()
        .title("Telegram")
        .subtitle("MTProto chats — login UI lands later")
        .build();
    let telegram_switch = gtk::Switch::new();
    telegram_switch.set_valign(gtk::Align::Center);
    telegram_row.add_suffix(&telegram_switch);
    telegram_row.set_activatable_widget(Some(&telegram_switch));

    let listbox = gtk::ListBox::new();
    listbox.add_css_class("boxed-list");
    listbox.set_selection_mode(gtk::SelectionMode::None);
    listbox.append(&signal_row);
    listbox.append(&telegram_row);
    listbox.set_size_request(420, -1);

    {
        let choices = choices.clone();
        signal_switch.connect_active_notify(move |sw| {
            let mut c = choices.get();
            c.signal = sw.is_active();
            choices.set(c);
        });
    }
    {
        let choices = choices.clone();
        telegram_switch.connect_active_notify(move |sw| {
            let mut c = choices.get();
            c.telegram = sw.is_active();
            choices.set(c);
        });
    }

    let cta = gtk::Button::with_label("Continue");
    cta.add_css_class("suggested-action");
    cta.add_css_class("kryptos-welcome-cta");
    cta.set_halign(gtk::Align::Center);
    cta.set_size_request(140, 40);

    {
        let carousel = carousel.clone();
        cta.connect_clicked(move |_| {
            advance_to(&carousel, 2);
        });
    }

    column.append(&title);
    column.append(&body);
    column.append(&listbox);
    column.append(&cta);
    column.upcast()
}

fn page_link_signal(
    carousel: &adw::Carousel,
    choices: Rc<Cell<Choices>>,
    _parent: &gtk::Window,
) -> gtk::Widget {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .build();
    column.set_margin_start(36);
    column.set_margin_end(36);
    column.set_margin_top(12);
    column.set_margin_bottom(12);

    let title = gtk::Label::new(Some("Link Signal"));
    title.add_css_class("kryptos-welcome-title");
    title.set_halign(gtk::Align::Center);

    let body = gtk::Label::builder()
        .label(
            "Open the linker, generate a code, and scan it from \
             Signal → Settings → Linked devices.",
        )
        .wrap(true)
        .justify(gtk::Justification::Center)
        .halign(gtk::Align::Center)
        .build();
    body.add_css_class("kryptos-welcome-body");

    let open_linker = gtk::Button::with_label("Open linker");
    open_linker.add_css_class("suggested-action");
    open_linker.add_css_class("kryptos-welcome-cta");
    open_linker.set_halign(gtk::Align::Center);
    open_linker.set_size_request(160, 40);

    let next = gtk::Button::with_label("Continue");
    next.add_css_class("flat");
    next.set_halign(gtk::Align::Center);

    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::Center)
        .build();
    row.append(&open_linker);
    row.append(&next);

    {
        let carousel = carousel.clone();
        next.connect_clicked(move |_| {
            advance_to(&carousel, 3);
        });
    }
    {
        let column_clone = column.clone();
        open_linker.connect_clicked(move |btn| {
            let win = btn.root().and_then(|r| r.downcast::<gtk::Window>().ok());
            if let Some(win) = win {
                super::open_linker(&win);
            } else {
                warn!("welcome: linker open without a parent window");
            }
            // Cosmetic: dim the button so the user knows the linker is up.
            btn.set_sensitive(false);
            // Drop the column reference so the closure doesn't leak it.
            let _ = column_clone;
        });
    }

    // If the user didn't pick Signal on page 2, swap the linker CTA for
    // an explanatory note and auto-route to "Continue" focus.
    {
        let choices = choices.clone();
        let row_for_show = row.clone();
        let body_for_show = body.clone();
        carousel.connect_position_notify(move |c| {
            // We're page index 2.
            if (c.position() - 2.0).abs() < 0.01 {
                let c = choices.get();
                if !c.signal {
                    body_for_show.set_label(
                        "You can set up Signal later from the sidebar — \
                         skipping for now.",
                    );
                    row_for_show.set_visible(true);
                }
            }
        });
    }

    column.append(&title);
    column.append(&body);
    column.append(&row);
    column.upcast()
}

fn page_done(
    carousel: &adw::Carousel,
    win: &adw::Window,
    finished: &Rc<Cell<bool>>,
    on_finish: Rc<dyn Fn()>,
    config_path: &PathBuf,
) -> gtk::Widget {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(16)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .build();
    column.set_margin_start(48);
    column.set_margin_end(48);

    let title = gtk::Label::new(Some("You're all set."));
    title.add_css_class("kryptos-welcome-title");
    title.set_halign(gtk::Align::Center);

    let body = gtk::Label::builder()
        .label("Press : for commands, / to search, j/k to move between chats.")
        .wrap(true)
        .justify(gtk::Justification::Center)
        .halign(gtk::Align::Center)
        .build();
    body.add_css_class("kryptos-welcome-body");

    let cta = gtk::Button::with_label("Start chatting");
    cta.add_css_class("suggested-action");
    cta.add_css_class("kryptos-welcome-cta");
    cta.set_halign(gtk::Align::Center);
    cta.set_size_request(160, 40);

    {
        let win = win.clone();
        let path = config_path.clone();
        let finished = finished.clone();
        let on_finish = on_finish.clone();
        cta.connect_clicked(move |_| {
            finished.set(true);
            persist_completed(&path);
            on_finish();
            win.close();
        });
    }

    let _ = carousel; // page index 3; nothing to advance to
    column.append(&title);
    column.append(&body);
    column.append(&cta);
    column.upcast()
}

fn advance_to(carousel: &adw::Carousel, index: u32) {
    let n = carousel.n_pages();
    if index >= n {
        return;
    }
    let page = carousel.nth_page(index);
    carousel.scroll_to(&page, true);
}

fn persist_completed(path: &PathBuf) {
    let result = (|| -> crate::core::Result<()> {
        // Load (or default) so we round-trip every other key cleanly.
        let mut cfg = loader::load_or_default(path)?;
        cfg.onboarding.completed = true;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let serialised = toml::to_string_pretty(&cfg)?;
        std::fs::write(path, serialised)?;
        Ok(())
    })();
    if let Err(e) = result {
        error!(error = %e, "welcome: persist completed=true failed");
    }
}

fn install_styles() {
    use std::sync::OnceLock;
    static INSTALLED: OnceLock<()> = OnceLock::new();
    if INSTALLED.set(()).is_err() {
        return;
    }

    let provider = gtk::CssProvider::new();
    provider.load_from_string(WELCOME_STYLES);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    }
}

const WELCOME_STYLES: &str = r#"
.kryptos-welcome-title {
    font-size: 28px;
    font-weight: 600;
    letter-spacing: -0.01em;
}
.kryptos-welcome-body {
    font-size: 14px;
    line-height: 1.5;
    opacity: 0.78;
}
button.suggested-action.kryptos-welcome-cta {
    padding: 0 18px;
    border-radius: 4px;
    border: 1px solid alpha(currentColor, 0.20);
    font-weight: 700;
    letter-spacing: 0.04em;
}
.kryptos-welcome-skip {
    font-size: 12px;
    opacity: 0.6;
}
.kryptos-welcome-dots {
    margin: 0 12px;
}
"#;
