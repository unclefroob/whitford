mod actions;
mod build;
mod render;
mod widgets;

use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

use adw::prelude::*;

use crate::{
    model::{FixtureError, fixtures},
    state::{Action, AppState, MessageFilter},
};

#[derive(Clone)]
pub struct Ui {
    pub window: adw::ApplicationWindow,
    pub(crate) state: Rc<RefCell<AppState>>,
    pub(crate) folders: gtk::ListBox,
    pub(crate) messages: gtk::ListBox,
    pub(crate) search: gtk::SearchEntry,
    pub(crate) reader: gtk::Box,
    pub(crate) outer: adw::OverlaySplitView,
    pub(crate) inner: adw::NavigationSplitView,
    pub(crate) toast_overlay: adw::ToastOverlay,
    pub(crate) sync_title: gtk::Label,
    pub(crate) sync_detail: gtk::Label,
    pub(crate) filter_buttons: Vec<(MessageFilter, gtk::Button)>,
}

#[derive(Clone)]
pub(crate) struct WeakUi {
    window: gtk::glib::WeakRef<adw::ApplicationWindow>,
    state: Weak<RefCell<AppState>>,
    folders: gtk::glib::WeakRef<gtk::ListBox>,
    messages: gtk::glib::WeakRef<gtk::ListBox>,
    search: gtk::glib::WeakRef<gtk::SearchEntry>,
    reader: gtk::glib::WeakRef<gtk::Box>,
    outer: gtk::glib::WeakRef<adw::OverlaySplitView>,
    inner: gtk::glib::WeakRef<adw::NavigationSplitView>,
    toast_overlay: gtk::glib::WeakRef<adw::ToastOverlay>,
    sync_title: gtk::glib::WeakRef<gtk::Label>,
    sync_detail: gtk::glib::WeakRef<gtk::Label>,
    filter_buttons: Vec<(MessageFilter, gtk::glib::WeakRef<gtk::Button>)>,
}

pub fn build(application: &adw::Application) -> Result<Ui, FixtureError> {
    let state = Rc::new(RefCell::new(AppState::new(fixtures())?));
    let ui = build::build(application, state);
    actions::install(&ui, application);
    build::connect_signals(&ui);
    ui.render();
    Ok(ui)
}

impl Ui {
    pub(crate) fn downgrade(&self) -> WeakUi {
        WeakUi {
            window: self.window.downgrade(),
            state: Rc::downgrade(&self.state),
            folders: self.folders.downgrade(),
            messages: self.messages.downgrade(),
            search: self.search.downgrade(),
            reader: self.reader.downgrade(),
            outer: self.outer.downgrade(),
            inner: self.inner.downgrade(),
            toast_overlay: self.toast_overlay.downgrade(),
            sync_title: self.sync_title.downgrade(),
            sync_detail: self.sync_detail.downgrade(),
            filter_buttons: self
                .filter_buttons
                .iter()
                .map(|(filter, button)| (*filter, button.downgrade()))
                .collect(),
        }
    }
    pub(crate) fn dispatch(&self, action: Action) {
        let transition = self.state.borrow_mut().dispatch(action);
        let snapshot = self.state.borrow().snapshot();
        if let Some(message) = transition.feedback {
            self.toast_overlay.add_toast(adw::Toast::new(message));
        }
        render::render(self, &snapshot);
    }

    pub(crate) fn render(&self) {
        let snapshot = self.state.borrow().snapshot();
        render::render(self, &snapshot);
    }

    pub(crate) fn toast(&self, message: &str) {
        self.toast_overlay.add_toast(adw::Toast::new(message));
    }
}

impl WeakUi {
    pub(crate) fn upgrade(&self) -> Option<Ui> {
        Some(Ui {
            window: self.window.upgrade()?,
            state: self.state.upgrade()?,
            folders: self.folders.upgrade()?,
            messages: self.messages.upgrade()?,
            search: self.search.upgrade()?,
            reader: self.reader.upgrade()?,
            outer: self.outer.upgrade()?,
            inner: self.inner.upgrade()?,
            toast_overlay: self.toast_overlay.upgrade()?,
            sync_title: self.sync_title.upgrade()?,
            sync_detail: self.sync_detail.upgrade()?,
            filter_buttons: self
                .filter_buttons
                .iter()
                .map(|(filter, button)| Some((*filter, button.upgrade()?)))
                .collect::<Option<Vec<_>>>()?,
        })
    }
}
