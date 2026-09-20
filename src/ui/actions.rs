use adw::prelude::*;
use gtk::gio;

use super::Ui;
use crate::state::{Action, EscapeContext, EscapeOutcome, Surface, escape_outcome};

pub(super) fn install(ui: &Ui, application: &adw::Application) {
    add(ui, "compose", |ui| {
        ui.toast("Compose is available in a later milestone")
    });
    add(ui, "focus-search", |ui| {
        ui.search.grab_focus();
    });
    add(ui, "folder-next", |ui| move_folder(ui, 1));
    add(ui, "folder-previous", |ui| move_folder(ui, -1));
    add(ui, "message-next", |ui| ui.dispatch(Action::SelectNext));
    add(ui, "message-previous", |ui| {
        ui.dispatch(Action::SelectPrevious)
    });
    add(ui, "archive", |ui| ui.dispatch(Action::ArchiveSelected));
    add(ui, "reply", |ui| {
        ui.toast("Reply is available in a later milestone")
    });
    add(ui, "reply-all", |ui| {
        ui.toast("Reply all is available in a later milestone")
    });
    add(ui, "forward", |ui| {
        ui.toast("Forward is available in a later milestone")
    });
    add(ui, "mark-read", |ui| {
        ui.toast("Read status is fixture-only in this milestone")
    });
    add(ui, "delete", |ui| {
        ui.toast("Delete is available in a later milestone")
    });
    add(ui, "label", |ui| {
        ui.toast("Labels are available in a later milestone")
    });
    add(ui, "star", |ui| {
        ui.toast("Starring is available in a later milestone")
    });
    add(ui, "download", |ui| {
        ui.toast("Attachment download is available in a later milestone")
    });
    add(ui, "toggle-folders", |ui| {
        ui.outer.set_show_sidebar(!ui.outer.shows_sidebar())
    });
    add(ui, "back", handle_back);
    add(ui, "demo-online", |ui| {
        ui.dispatch(Action::SetSurface(Surface::Online))
    });
    add(ui, "demo-loading", |ui| {
        ui.dispatch(Action::SetSurface(Surface::Loading))
    });
    add(ui, "demo-offline", |ui| {
        ui.dispatch(Action::SetSurface(Surface::Offline))
    });

    for (action, accelerators) in [
        ("win.compose", &["<Primary>n"][..]),
        ("win.focus-search", &["<Primary>f"]),
        ("win.folder-next", &["<Alt>Down"]),
        ("win.folder-previous", &["<Alt>Up"]),
        ("win.message-next", &["<Primary>Down"]),
        ("win.message-previous", &["<Primary>Up"]),
        ("win.archive", &["Delete"]),
        ("win.reply", &["<Primary>r"]),
        ("win.back", &["Escape"]),
        ("win.demo-online", &["<Primary><Shift>1"]),
        ("win.demo-loading", &["<Primary><Shift>2"]),
        ("win.demo-offline", &["<Primary><Shift>3"]),
    ] {
        application.set_accels_for_action(action, accelerators);
    }
}

fn add(ui: &Ui, name: &str, handler: impl Fn(&Ui) + 'static) {
    let action = gio::SimpleAction::new(name, None);
    let weak_ui = ui.downgrade();
    action.connect_activate(move |_, _| {
        if let Some(ui) = weak_ui.upgrade() {
            handler(&ui);
        }
    });
    ui.window.add_action(&action);
}

fn move_folder(ui: &Ui, step: isize) {
    let snapshot = ui.state.borrow().snapshot();
    if snapshot.folders.is_empty() {
        return;
    }
    let current = snapshot
        .folders
        .iter()
        .position(|folder| folder.id == snapshot.selected_folder_id)
        .unwrap_or(0);
    let next = current
        .saturating_add_signed(step)
        .min(snapshot.folders.len() - 1);
    ui.dispatch(Action::SelectFolder(snapshot.folders[next].id));
}

fn handle_back(ui: &Ui) {
    let outcome = escape_outcome(EscapeContext {
        search_active: !ui.search.text().is_empty(),
        reader_visible: ui.inner.is_collapsed() && ui.inner.shows_content(),
        folders_visible: ui.outer.is_collapsed() && ui.outer.shows_sidebar(),
    });
    match outcome {
        EscapeOutcome::ClearSearch => ui.search.set_text(""),
        EscapeOutcome::ShowMessageList => ui.inner.set_show_content(false),
        EscapeOutcome::HideFolders => ui.outer.set_show_sidebar(false),
        EscapeOutcome::None => {}
    }
}
