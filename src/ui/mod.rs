mod actions;
mod build;
mod email_view;
mod render;
mod time;
mod widgets;

use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};

use adw::prelude::*;

use crate::{
    oauth::AuthorizationUrl,
    state::{Action, AppState, Effect, MessageFilter},
    worker::{OperationId, WorkerCommand, WorkerEvent},
};

#[derive(Clone)]
pub struct Ui {
    pub window: adw::ApplicationWindow,
    pub(crate) state: Rc<RefCell<AppState>>,
    pub(crate) folders: gtk::ListBox,
    pub(crate) messages: gtk::ListBox,
    pub(crate) list_banner: gtk::Box,
    pub(crate) search: gtk::SearchEntry,
    pub(crate) reader: gtk::Box,
    pub(crate) reader_banner: gtk::Box,
    pub(crate) outer: adw::OverlaySplitView,
    pub(crate) inner: adw::NavigationSplitView,
    pub(crate) toast_overlay: adw::ToastOverlay,
    pub(crate) sync_title: gtk::Label,
    pub(crate) sync_detail: gtk::Label,
    pub(crate) cache_limit: gtk::DropDown,
    pub(crate) cache_usage: gtk::Label,
    pub(crate) last_list_revision: Rc<Cell<u64>>,
    pub(crate) last_reader_revision: Rc<Cell<u64>>,
    pub(crate) filter_buttons: Vec<(MessageFilter, gtk::Button)>,
    pub(crate) worker: tokio::sync::mpsc::UnboundedSender<WorkerCommand>,
    pub(crate) authorization: Rc<RefCell<Option<(OperationId, AuthorizationUrl)>>>,
}

#[derive(Clone)]
pub(crate) struct WeakUi {
    window: gtk::glib::WeakRef<adw::ApplicationWindow>,
    state: Weak<RefCell<AppState>>,
    folders: gtk::glib::WeakRef<gtk::ListBox>,
    messages: gtk::glib::WeakRef<gtk::ListBox>,
    list_banner: gtk::glib::WeakRef<gtk::Box>,
    search: gtk::glib::WeakRef<gtk::SearchEntry>,
    reader: gtk::glib::WeakRef<gtk::Box>,
    reader_banner: gtk::glib::WeakRef<gtk::Box>,
    outer: gtk::glib::WeakRef<adw::OverlaySplitView>,
    inner: gtk::glib::WeakRef<adw::NavigationSplitView>,
    toast_overlay: gtk::glib::WeakRef<adw::ToastOverlay>,
    sync_title: gtk::glib::WeakRef<gtk::Label>,
    sync_detail: gtk::glib::WeakRef<gtk::Label>,
    cache_limit: gtk::glib::WeakRef<gtk::DropDown>,
    cache_usage: gtk::glib::WeakRef<gtk::Label>,
    last_list_revision: Weak<Cell<u64>>,
    last_reader_revision: Weak<Cell<u64>>,
    filter_buttons: Vec<(MessageFilter, gtk::glib::WeakRef<gtk::Button>)>,
    worker: tokio::sync::mpsc::UnboundedSender<WorkerCommand>,
    authorization: Weak<RefCell<Option<(OperationId, AuthorizationUrl)>>>,
}

pub fn build(
    application: &adw::Application,
    worker: tokio::sync::mpsc::UnboundedSender<WorkerCommand>,
    mut events: tokio::sync::mpsc::UnboundedReceiver<WorkerEvent>,
) -> Ui {
    let state = Rc::new(RefCell::new(AppState::new()));
    let authorization = Rc::new(RefCell::new(None));
    let ui = build::build(application, state, worker, authorization);
    actions::install(&ui, application);
    build::connect_signals(&ui);
    ui.render();
    let weak = ui.downgrade();
    gtk::glib::MainContext::default().spawn_local(async move {
        while let Some(event) = events.recv().await {
            if let Some(ui) = weak.upgrade() {
                ui.handle_worker_event(event);
            } else {
                break;
            }
        }
        if let Some(ui) = weak.upgrade() {
            ui.dispatch(Action::WorkerUnavailable);
        }
    });
    ui.dispatch(Action::Startup);
    ui
}

impl Ui {
    pub(crate) fn downgrade(&self) -> WeakUi {
        WeakUi {
            window: self.window.downgrade(),
            state: Rc::downgrade(&self.state),
            folders: self.folders.downgrade(),
            messages: self.messages.downgrade(),
            list_banner: self.list_banner.downgrade(),
            search: self.search.downgrade(),
            reader: self.reader.downgrade(),
            reader_banner: self.reader_banner.downgrade(),
            outer: self.outer.downgrade(),
            inner: self.inner.downgrade(),
            toast_overlay: self.toast_overlay.downgrade(),
            sync_title: self.sync_title.downgrade(),
            sync_detail: self.sync_detail.downgrade(),
            cache_limit: self.cache_limit.downgrade(),
            cache_usage: self.cache_usage.downgrade(),
            last_list_revision: Rc::downgrade(&self.last_list_revision),
            last_reader_revision: Rc::downgrade(&self.last_reader_revision),
            filter_buttons: self
                .filter_buttons
                .iter()
                .map(|(filter, button)| (*filter, button.downgrade()))
                .collect(),
            worker: self.worker.clone(),
            authorization: Rc::downgrade(&self.authorization),
        }
    }
    pub(crate) fn dispatch(&self, action: Action) {
        let update = self.state.borrow_mut().dispatch(action);
        if let Some(message) = update.feedback {
            self.toast_overlay.add_toast(adw::Toast::new(message));
        }
        self.render();
        for effect in update.effects {
            self.execute(effect);
        }
    }

    pub(crate) fn render(&self) {
        let state = self.state.borrow();
        let include_visible_messages = self.last_list_revision.get() != state.list_revision();
        let snapshot = state.snapshot_for_render(include_visible_messages);
        drop(state);
        render::render(self, &snapshot);
    }

    pub(crate) fn toast(&self, message: &str) {
        self.toast_overlay.add_toast(adw::Toast::new(message));
    }
    fn handle_worker_event(&self, event: WorkerEvent) {
        let event = match event {
            WorkerEvent::AuthorizationRequired { id, url, deadline } => {
                let copy = AuthorizationUrl::new(url.expose().to_owned());
                *self.authorization.borrow_mut() = Some((id, url));
                WorkerEvent::AuthorizationRequired {
                    id,
                    url: copy,
                    deadline,
                }
            }
            other => other,
        };
        self.dispatch(Action::Worker(event));
    }
    fn execute(&self, effect: Effect) {
        match effect {
            Effect::SendWorker(command) => {
                if self.worker.send(command).is_err() {
                    self.toast("Mail worker is unavailable");
                    self.dispatch(Action::WorkerUnavailable);
                }
            }
            Effect::ClearAuthorization { id } => {
                if self
                    .authorization
                    .borrow()
                    .as_ref()
                    .is_some_and(|(current, _)| *current == id)
                {
                    self.authorization.borrow_mut().take();
                }
            }
            Effect::LaunchAuthorization { id } => {
                let Some(url) = self
                    .authorization
                    .borrow()
                    .as_ref()
                    .filter(|(current, _)| *current == id)
                    .map(|(_, url)| url.expose().to_owned())
                else {
                    return;
                };
                let weak = self.downgrade();
                gtk::glib::MainContext::default().spawn_local(async move {
                    if gtk::gio::AppInfo::launch_default_for_uri_future(
                        &url,
                        None::<&gtk::gio::AppLaunchContext>,
                    )
                    .await
                    .is_err()
                        && let Some(ui) = weak.upgrade()
                    {
                        ui.dispatch(Action::BrowserLaunchFailed(id));
                    }
                });
            }
            Effect::PresentDisconnectConfirmation => {
                let dialog = adw::AlertDialog::builder().heading("Disconnect Gmail?").body("This removes Whitford’s saved authorization from Secret Service and deletes its local mail cache. Revoke Google access separately in your Google Account.").build();
                dialog.add_response("cancel", "Cancel");
                dialog.add_response("disconnect", "Disconnect");
                dialog.set_response_appearance("disconnect", adw::ResponseAppearance::Destructive);
                let weak = self.downgrade();
                dialog.choose(
                    Some(&self.window),
                    None::<&gtk::gio::Cancellable>,
                    move |response| {
                        if response == "disconnect"
                            && let Some(ui) = weak.upgrade()
                        {
                            ui.dispatch(Action::ConfirmDisconnect);
                        }
                    },
                );
            }
            Effect::PresentClearCacheConfirmation => {
                let dialog = adw::AlertDialog::builder()
                    .heading("Clear downloaded messages?")
                    .body("This removes opened message bodies saved for offline reading. Message summaries, your Gmail authorization, and the Keep summaries setting remain.")
                    .build();
                dialog.add_response("cancel", "Cancel");
                dialog.add_response("clear", "Clear Cache");
                dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
                let weak = self.downgrade();
                dialog.choose(
                    Some(&self.window),
                    None::<&gtk::gio::Cancellable>,
                    move |response| {
                        if response == "clear"
                            && let Some(ui) = weak.upgrade()
                        {
                            ui.dispatch(Action::ConfirmClearCache);
                        }
                    },
                );
            }
        }
    }
}

impl WeakUi {
    pub(crate) fn upgrade(&self) -> Option<Ui> {
        Some(Ui {
            window: self.window.upgrade()?,
            state: self.state.upgrade()?,
            folders: self.folders.upgrade()?,
            messages: self.messages.upgrade()?,
            list_banner: self.list_banner.upgrade()?,
            search: self.search.upgrade()?,
            reader: self.reader.upgrade()?,
            reader_banner: self.reader_banner.upgrade()?,
            outer: self.outer.upgrade()?,
            inner: self.inner.upgrade()?,
            toast_overlay: self.toast_overlay.upgrade()?,
            sync_title: self.sync_title.upgrade()?,
            sync_detail: self.sync_detail.upgrade()?,
            cache_limit: self.cache_limit.upgrade()?,
            cache_usage: self.cache_usage.upgrade()?,
            last_list_revision: self.last_list_revision.upgrade()?,
            last_reader_revision: self.last_reader_revision.upgrade()?,
            filter_buttons: self
                .filter_buttons
                .iter()
                .map(|(filter, button)| Some((*filter, button.upgrade()?)))
                .collect::<Option<Vec<_>>>()?,
            worker: self.worker.clone(),
            authorization: self.authorization.upgrade()?,
        })
    }
}
