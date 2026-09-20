use adw::prelude::*;
use gtk::gdk;
use std::{cell::RefCell, rc::Rc};
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};
use whitford::{ui, worker::WorkerHandle};

const APP_ID: &str = "dev.whitford.Whitford";

fn main() -> gtk::glib::ExitCode {
    let level = match std::env::var("WHITFORD_LOG").as_deref() {
        Ok("error") => tracing::Level::ERROR,
        Ok("warn") => tracing::Level::WARN,
        Ok("debug") => tracing::Level::DEBUG,
        Ok("trace") => tracing::Level::TRACE,
        _ => tracing::Level::INFO,
    };
    let application_logs = tracing_subscriber::fmt::layer()
        .with_target(false)
        .without_time()
        .with_filter(tracing_subscriber::filter::filter_fn(move |metadata| {
            metadata.target().starts_with("whitford") && *metadata.level() <= level
        }));
    let _ = tracing_subscriber::registry()
        .with(application_logs)
        .try_init();
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

    let (worker, events) = WorkerHandle::start();
    let worker_sender = worker.sender.clone();
    let pending_events = Rc::new(RefCell::new(Some(events)));
    let active_ui = Rc::new(RefCell::new(None::<ui::Ui>));
    application.connect_activate({
        let active_ui = active_ui.clone();
        move |application| {
            if let Some(window) = application.active_window() {
                window.present();
                return;
            }
            let Some(events) = pending_events.borrow_mut().take() else {
                return;
            };
            let ui = ui::build(application, worker_sender.clone(), events);
            let weak_active_ui = Rc::downgrade(&active_ui);
            ui.window.connect_destroy(move |_| {
                if let Some(active_ui) = weak_active_ui.upgrade() {
                    active_ui.borrow_mut().take();
                }
            });
            ui.window.present();
            active_ui.replace(Some(ui));
        }
    });

    let exit_code = application.run();
    if worker.shutdown_and_join().is_err() {
        tracing::error!("mail worker thread panicked during shutdown");
    }
    exit_code
}
