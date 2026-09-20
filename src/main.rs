use adw::prelude::*;
use gtk::gdk;
use waymail::ui;

const APP_ID: &str = "dev.waymail.Waymail";

fn main() -> gtk::glib::ExitCode {
    let application = adw::Application::builder().application_id(APP_ID).build();

    application.connect_startup(|_| {
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        let provider = gtk::CssProvider::new();
        provider.load_from_string(include_str!("style.css"));
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });

    application.connect_activate(|application| {
        if let Some(window) = application.active_window() {
            window.present();
            return;
        }
        match ui::build(application) {
            Ok(ui) => ui.window.present(),
            Err(error) => eprintln!("Failed to construct {APP_ID}: {error:?}"),
        }
    });

    application.run()
}
