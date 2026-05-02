//! First-run welcome experience — single-window content swap.
//!
//! Shown when `~/.config/kryptos/config.toml` doesn't exist or
//! `[onboarding] completed` is `false`. Rather than open a second
//! window over the empty main shell, we *replace* the
//! `adw::ApplicationWindow`'s content with an `adw::NavigationView`
//! carrying four pages:
//!
//! 1. **Hello**           — hero title + body + "Continue" CTA.
//! 2. **Pick messengers** — Signal / Telegram action rows.
//! 3. **Link**            — Signal linker CTA + Telegram login CTA, scoped
//!    to whichever messengers the user enabled on page 2.
//! 4. **Done**            — "You're all set." + "Start chatting".
//!
//! On finish / skip we restore the saved real-shell widget and persist
//! `[onboarding].completed = true`. NavigationView gives us the smooth
//! native push/pop slide animation between pages for free.
//!
//! Design tenets, in order:
//!   * Generous whitespace (96px+ vertical breathing room).
//!   * 28px hero titles, monospace numerics, thin underlines for CTAs.
//!   * A "Skip onboarding" link in the bottom-left.
//!   * Toasts (via the parent's `ToastOverlay`) for transient status.
//!   * Carousel-style indicator dots so the user sees momentum across
//!     the four steps.
//!
//! The Signal linker still pops as its own modal child window (it owns
//! a long polling loop and a QR canvas; squeezing it into a Navigation
//! page would mean rewriting half of `onboarding/mod.rs`). The Telegram
//! login *does* run inline as its own dialog and finishes back into
//! this flow.

use std::cell::{Cell, RefCell};
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

/// Replace `window`'s content with the welcome navigation view. When the
/// user finishes or skips, the original content is restored and
/// `on_finish` fires once.
pub fn present(
    window: &adw::ApplicationWindow,
    config_path: PathBuf,
    on_finish: impl Fn() + 'static,
) {
    let on_finish = Rc::new(on_finish);
    let choices: Rc<Cell<Choices>> = Rc::new(Cell::new(Choices::default()));
    let finished: Rc<Cell<bool>> = Rc::new(Cell::new(false));

    // Stash the real shell so we can put it back when onboarding ends.
    let saved_content: Rc<RefCell<Option<gtk::Widget>>> = Rc::new(RefCell::new(window.content()));

    install_styles();

    let nav = adw::NavigationView::new();
    nav.add_css_class("kryptos-welcome");

    // Indicator dots — a tiny progress affordance pinned bottom-right.
    // We don't use `adw::CarouselIndicatorDots` because we're not on a
    // carousel anymore; render the four pips by hand.
    let progress = build_progress(0);
    let progress = Rc::new(RefCell::new(progress));

    // Each page is a NavigationPage with the same chrome:
    //   - header (flat, no title)
    //   - centred content
    //   - footer with skip + progress dots
    //
    // The skip handler is shared, so we build a closure once and clone
    // it into every page.
    let restore = make_restore_fn(
        window.clone(),
        saved_content.clone(),
        finished.clone(),
        config_path.clone(),
        on_finish.clone(),
    );

    let page1 = build_page_hello(&nav, &progress, &restore);
    let page2 = build_page_pick_messengers(&nav, &progress, &restore, choices.clone());
    let page3 = build_page_link(&nav, &progress, &restore, choices.clone(), window.clone());
    let page4 = build_page_done(&progress, &restore);

    nav.add(&page1);
    nav.add(&page2);
    nav.add(&page3);
    nav.add(&page4);

    // Bind nav.visible_page → progress dots. Pages are tagged "p0"..."p3".
    {
        let progress = progress.clone();
        nav.connect_visible_page_notify(move |nv| {
            if let Some(page) = nv.visible_page() {
                let tag = page.tag().map(|s| s.to_string()).unwrap_or_default();
                let idx = tag
                    .strip_prefix('p')
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(0);
                replace_progress(&progress, idx);
            }
        });
    }

    // Treat ESC at the top level as "skip onboarding".
    {
        let key = gtk::EventControllerKey::new();
        let restore_for_esc = restore.clone();
        key.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk::gdk::Key::Escape {
                restore_for_esc();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        nav.add_controller(key);
    }

    window.set_content(Some(&nav));
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

fn build_page_hello(
    nav: &adw::NavigationView,
    progress: &Rc<RefCell<gtk::Widget>>,
    restore: &Rc<dyn Fn()>,
) -> adw::NavigationPage {
    let column = column_with_breathing_room();

    let title = hero_title("Welcome to Kryptos");
    let body = hero_body("A clean, fast, vim-first messaging client for Linux.");
    let cta = primary_cta("Continue");

    {
        let nav = nav.clone();
        cta.connect_clicked(move |_| {
            nav.push_by_tag("p1");
        });
    }

    column.append(&title);
    column.append(&body);
    column.append(&spacer(8));
    column.append(&cta);

    wrap_page(
        "p0",
        "Welcome",
        column.upcast(),
        progress.borrow().clone(),
        restore.clone(),
    )
}

fn build_page_pick_messengers(
    nav: &adw::NavigationView,
    progress: &Rc<RefCell<gtk::Widget>>,
    restore: &Rc<dyn Fn()>,
    choices: Rc<Cell<Choices>>,
) -> adw::NavigationPage {
    let column = column_with_breathing_room();

    let title = hero_title("Pick your messengers");
    let body = hero_body("Choose what to set up now. You can always add more later.");

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
        .subtitle("MTProto chats via grammers — interactive login on the next screen")
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
    listbox.set_size_request(440, -1);
    listbox.set_halign(gtk::Align::Center);

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

    let cta = primary_cta("Continue");
    {
        let nav = nav.clone();
        cta.connect_clicked(move |_| {
            nav.push_by_tag("p2");
        });
    }

    column.append(&title);
    column.append(&body);
    column.append(&spacer(8));
    column.append(&listbox);
    column.append(&spacer(4));
    column.append(&cta);

    wrap_page(
        "p1",
        "Pick messengers",
        column.upcast(),
        progress.borrow().clone(),
        restore.clone(),
    )
}

fn build_page_link(
    nav: &adw::NavigationView,
    progress: &Rc<RefCell<gtk::Widget>>,
    restore: &Rc<dyn Fn()>,
    choices: Rc<Cell<Choices>>,
    window: adw::ApplicationWindow,
) -> adw::NavigationPage {
    let column = column_with_breathing_room();

    let title = hero_title("Link your accounts");
    let body = hero_body(
        "Each messenger has its own handshake. Run them now, or skip and \
         come back from settings.",
    );
    body.set_widget_name("kryptos-welcome-link-body");

    // Signal CTA — opens the existing linker as a transient child window.
    let signal_cta = primary_cta("Open Signal linker");
    {
        let window = window.clone();
        signal_cta.connect_clicked(move |btn| {
            super::open_linker(&window);
            btn.set_sensitive(false);
        });
    }

    // Telegram CTA — opens the new telegram_login flow.
    let telegram_cta = primary_cta("Set up Telegram");
    {
        let window = window.clone();
        telegram_cta.connect_clicked(move |btn| {
            super::telegram_login::present(&window, None);
            btn.set_sensitive(false);
        });
    }

    let next = secondary_cta("Continue");
    {
        let nav = nav.clone();
        next.connect_clicked(move |_| {
            nav.push_by_tag("p3");
        });
    }

    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .halign(gtk::Align::Center)
        .build();
    row.append(&signal_cta);
    row.append(&telegram_cta);
    row.append(&next);

    column.append(&title);
    column.append(&body);
    column.append(&spacer(8));
    column.append(&row);

    // Adapt visibility to the choices made on the previous page. We
    // recompute on every page-show so toggling back-and-forth doesn't
    // strand stale visibility state.
    let signal_cta_for_show = signal_cta.clone();
    let telegram_cta_for_show = telegram_cta.clone();
    let body_for_show = body.clone();
    nav.connect_visible_page_notify(move |nv| {
        if let Some(p) = nv.visible_page() {
            if p.tag().map(|t| t == "p2").unwrap_or(false) {
                let c = choices.get();
                signal_cta_for_show.set_visible(c.signal);
                telegram_cta_for_show.set_visible(c.telegram);
                if !c.signal && !c.telegram {
                    body_for_show.set_label(
                        "You skipped both messengers. You can set them up later \
                         from settings, or run :link / :telegram-login.",
                    );
                }
            }
        }
    });

    wrap_page(
        "p2",
        "Link",
        column.upcast(),
        progress.borrow().clone(),
        restore.clone(),
    )
}

fn build_page_done(
    progress: &Rc<RefCell<gtk::Widget>>,
    restore: &Rc<dyn Fn()>,
) -> adw::NavigationPage {
    let column = column_with_breathing_room();

    let title = hero_title("You're all set.");
    let body = hero_body(
        "Press : for commands, / to search, j/k to move between chats. \
         Welcome aboard.",
    );

    let cta = primary_cta("Start chatting");
    {
        let restore = restore.clone();
        cta.connect_clicked(move |_| {
            restore();
        });
    }

    column.append(&title);
    column.append(&body);
    column.append(&spacer(8));
    column.append(&cta);

    wrap_page(
        "p3",
        "Done",
        column.upcast(),
        progress.borrow().clone(),
        restore.clone(),
    )
}

// ---------------------------------------------------------------------------
// Page chrome
// ---------------------------------------------------------------------------

/// Wrap a content widget in a NavigationPage with a flat header, the
/// shared footer (skip link + progress dots), and Swiss vertical
/// breathing room.
fn wrap_page(
    tag: &str,
    title: &str,
    content: gtk::Widget,
    progress: gtk::Widget,
    restore: Rc<dyn Fn()>,
) -> adw::NavigationPage {
    let header = adw::HeaderBar::builder().show_title(false).build();
    header.add_css_class("flat");

    let skip = gtk::Button::with_label("Skip onboarding");
    skip.add_css_class("flat");
    skip.add_css_class("kryptos-welcome-skip");
    {
        let restore = restore.clone();
        skip.connect_clicked(move |_| restore());
    }

    let footer = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .build();
    footer.set_margin_start(24);
    footer.set_margin_end(24);
    footer.set_margin_top(12);
    footer.set_margin_bottom(20);
    footer.append(&skip);

    let dot_holder = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .halign(gtk::Align::End)
        .hexpand(true)
        .build();
    dot_holder.append(&progress);
    footer.append(&dot_holder);

    // Vertically centred body — `valign(Center)` + a tall scroller so the
    // page still works at any window size.
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .build();
    scroller.set_child(Some(&content));

    let body_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    body_box.append(&scroller);
    body_box.append(&footer);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&body_box));

    adw::NavigationPage::builder()
        .title(title)
        .tag(tag)
        .child(&toolbar)
        .build()
}

fn column_with_breathing_room() -> gtk::Box {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(20)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .build();
    // 96px of top/bottom breathing room. Side margins are tighter so
    // text wraps tastefully on narrow windows.
    column.set_margin_start(48);
    column.set_margin_end(48);
    column.set_margin_top(96);
    column.set_margin_bottom(96);
    column
}

fn hero_title(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("kryptos-welcome-title");
    l.set_halign(gtk::Align::Center);
    l.set_justify(gtk::Justification::Center);
    l
}

fn hero_body(text: &str) -> gtk::Label {
    let l = gtk::Label::builder()
        .label(text)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .justify(gtk::Justification::Center)
        .halign(gtk::Align::Center)
        .max_width_chars(56)
        .build();
    l.add_css_class("kryptos-welcome-body");
    l
}

fn primary_cta(text: &str) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    b.add_css_class("suggested-action");
    b.add_css_class("kryptos-welcome-cta");
    b.set_halign(gtk::Align::Center);
    b.set_size_request(160, 40);
    b
}

fn secondary_cta(text: &str) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    b.add_css_class("flat");
    b.add_css_class("kryptos-welcome-cta-secondary");
    b.set_halign(gtk::Align::Center);
    b
}

fn spacer(px: i32) -> gtk::Widget {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .height_request(px)
        .build()
        .upcast()
}

// ---------------------------------------------------------------------------
// Progress dots
// ---------------------------------------------------------------------------

const TOTAL_PAGES: usize = 4;

/// Render the dot row reflecting `active` (0-based). We rebuild on each
/// page change because GTK doesn't expose a clean "set active dot" API
/// on a hand-rolled row, and four widgets is cheap.
fn build_progress(active: usize) -> gtk::Widget {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .build();
    row.add_css_class("kryptos-welcome-dots");
    for i in 0..TOTAL_PAGES {
        let dot = gtk::Box::builder()
            .width_request(8)
            .height_request(8)
            .build();
        dot.add_css_class("kryptos-welcome-dot");
        if i == active {
            dot.add_css_class("active");
        }
        row.append(&dot);
    }
    row.upcast()
}

fn replace_progress(slot: &Rc<RefCell<gtk::Widget>>, active: usize) {
    let new_dots = build_progress(active);
    let old = slot.replace(new_dots.clone());
    // Replace the widget in any holder that's currently displaying it.
    if let Some(parent) = old.parent() {
        if let Some(parent_box) = parent.downcast_ref::<gtk::Box>() {
            parent_box.remove(&old);
            parent_box.append(&new_dots);
        }
    }
}

// ---------------------------------------------------------------------------
// Skip / restore plumbing
// ---------------------------------------------------------------------------

fn make_restore_fn(
    window: adw::ApplicationWindow,
    saved_content: Rc<RefCell<Option<gtk::Widget>>>,
    finished: Rc<Cell<bool>>,
    config_path: PathBuf,
    on_finish: Rc<dyn Fn()>,
) -> Rc<dyn Fn()> {
    Rc::new(move || {
        if finished.replace(true) {
            return;
        }
        persist_completed(&config_path);
        // Put the real shell back. If we somehow lost it, fall through —
        // the user will at least see an empty window rather than a stuck
        // welcome flow, and `on_finish` still fires.
        if let Some(prev) = saved_content.borrow_mut().take() {
            // If the welcome view has Insert-mode-y focus state hanging
            // around, reset window focus so the real shell isn't stuck on
            // a freed widget after swap.
            gtk::prelude::GtkWindowExt::set_focus(&window, gtk::Widget::NONE);
            window.set_content(Some(&prev));
        } else {
            warn!("welcome restore: saved content missing");
        }
        on_finish();
    })
}

fn persist_completed(path: &PathBuf) {
    let result = (|| -> crate::core::Result<()> {
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

// ---------------------------------------------------------------------------
// CSS
// ---------------------------------------------------------------------------

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
    font-feature-settings: "tnum";
}
.kryptos-welcome-body {
    font-size: 14px;
    line-height: 1.55;
    opacity: 0.78;
}
button.suggested-action.kryptos-welcome-cta {
    padding: 0 18px;
    border-radius: 4px;
    border: 1px solid alpha(currentColor, 0.20);
    font-weight: 700;
    letter-spacing: 0.04em;
}
button.kryptos-welcome-cta-secondary {
    padding: 0 14px;
    font-size: 13px;
    opacity: 0.78;
}
.kryptos-welcome-skip {
    font-size: 12px;
    opacity: 0.6;
}
.kryptos-welcome-dot {
    background-color: alpha(currentColor, 0.20);
    border-radius: 999px;
    min-width: 6px;
    min-height: 6px;
}
.kryptos-welcome-dot.active {
    background-color: alpha(currentColor, 0.85);
}
"#;
