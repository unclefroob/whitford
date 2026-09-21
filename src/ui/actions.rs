use adw::prelude::*;
use gtk::gio;

use super::Ui;
use crate::state::{Action, EscapeContext, EscapeOutcome, escape_outcome};

pub(super) fn install(ui: &Ui, application: &adw::Application) {
    add(ui, "connect", |ui| ui.dispatch(Action::Connect));
    add(ui, "refresh", |ui| ui.dispatch(Action::Refresh));
    add(ui, "cancel-authorization", |ui| {
        ui.dispatch(Action::CancelAuthorization)
    });
    add(ui, "reopen-authorization", |ui| {
        ui.dispatch(Action::ReopenAuthorization)
    });
    add(ui, "disconnect", |ui| {
        ui.dispatch(Action::RequestDisconnect)
    });
    add(ui, "retry", |ui| ui.dispatch(Action::Retry));
    add(ui, "retry-body", |ui| ui.dispatch(Action::RetryBody));
    add(ui, "retry-drafts", |ui| {
        ui.dispatch(Action::RetryDraftRestore)
    });
    add(ui, "clear-cache", |ui| {
        ui.dispatch(Action::RequestClearCache)
    });
    add(ui, "compose", |ui| ui.dispatch(Action::BeginNewMessage));
    add(ui, "reply", |ui| ui.dispatch(Action::BeginReply));
    add(ui, "reply-all", |ui| ui.dispatch(Action::BeginReplyAll));
    add(ui, "forward", |ui| ui.dispatch(Action::BeginForward));
    add(ui, "resume-draft", |ui| ui.dispatch(Action::ResumeDraft));
    add(ui, "send-message", Ui::send_message);
    add(ui, "cancel-compose", Ui::request_close_composer);
    add(ui, "focus-search", |ui| {
        ui.search.grab_focus();
    });
    add(ui, "search-gmail", |ui| {
        ui.dispatch(Action::SubmitServerSearch)
    });
    add(ui, "cancel-search", |ui| {
        ui.dispatch(Action::CancelServerSearch)
    });
    add(ui, "retry-search", |ui| {
        ui.dispatch(Action::RetryServerSearch)
    });
    add(ui, "folder-next", |ui| move_folder(ui, 1));
    add(ui, "folder-previous", |ui| move_folder(ui, -1));
    add(ui, "message-next", |ui| ui.dispatch(Action::SelectNext));
    add(ui, "message-previous", |ui| {
        ui.dispatch(Action::SelectPrevious)
    });
    add(ui, "archive", |ui| ui.dispatch(Action::Archive));
    add(ui, "mark-read", |ui| ui.dispatch(Action::ToggleRead));
    add(ui, "delete", |ui| ui.dispatch(Action::MoveToTrash));
    add(ui, "undo-message-operation", |ui| {
        ui.dispatch(Action::UndoMessageOperation)
    });
    add(ui, "star", |ui| ui.dispatch(Action::ToggleStar));
    let label = gio::SimpleAction::new("toggle-label", Some(&String::static_variant_type()));
    let weak_ui = ui.downgrade();
    label.connect_activate(move |_, parameter| {
        if let (Some(ui), Some(mailbox)) =
            (weak_ui.upgrade(), parameter.and_then(|value| value.str()))
        {
            ui.dispatch(Action::ToggleLabel(mailbox.to_owned()));
        }
    });
    ui.window.add_action(&label);
    add(ui, "toggle-folders", |ui| {
        ui.outer.set_show_sidebar(!ui.outer.shows_sidebar())
    });
    add(ui, "open-settings", |ui| {
        if ui.navigation.visible_page_tag().as_deref() != Some("settings") {
            ui.navigation.push_by_tag("settings");
        }
    });
    add(ui, "back", handle_back);

    for (action, accelerators) in [
        ("win.focus-search", &["<Primary>f"]),
        ("win.compose", &["<Primary>n"]),
        ("win.open-settings", &["<Primary>comma"]),
        ("win.folder-next", &["<Alt>Down"]),
        ("win.folder-previous", &["<Alt>Up"]),
        ("win.message-next", &["<Primary>Down"]),
        ("win.message-previous", &["<Primary>Up"]),
        ("win.back", &["Escape"]),
        ("win.archive", &["<Primary>e"]),
        ("win.mark-read", &["<Primary>u"]),
        ("win.delete", &["Delete"]),
        ("win.star", &["<Primary>period"]),
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
    let folders = snapshot
        .sidebar_folders
        .primary
        .iter()
        .chain(&snapshot.sidebar_folders.labels)
        .collect::<Vec<_>>();
    if folders.is_empty() {
        return;
    }
    let current = folders
        .iter()
        .position(|folder| folder.id == snapshot.selected_folder_id)
        .unwrap_or(0);
    let next = current.saturating_add_signed(step).min(folders.len() - 1);
    ui.dispatch(Action::SelectFolder(folders[next].id.clone()));
}

fn handle_back(ui: &Ui) {
    if ui.navigation.visible_page_tag().as_deref() == Some("settings") {
        ui.navigation.pop();
        return;
    }
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
