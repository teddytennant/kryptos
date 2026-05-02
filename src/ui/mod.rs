use adw::prelude::*;
use gtk::glib;
use tracing::info;

const APP_ID: &str = "dev.kryptos.Kryptos";

/// Run the libadwaita application loop. Returns the glib exit code.
pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .build();

    app.connect_activate(build_window);
    app.run()
}

fn build_window(app: &adw::Application) {
    info!("activating main window");

    let label = gtk::Label::builder()
        .label("Kryptos\n\nphase 0 — foundation\n\n(GTK4 + libadwaita pipe verified)")
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .justify(gtk::Justification::Center)
        .build();

    let header = adw::HeaderBar::new();

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&label));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Kryptos")
        .default_width(1100)
        .default_height(720)
        .content(&toolbar)
        .build();

    window.present();
}
