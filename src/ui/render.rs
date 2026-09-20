use adw::prelude::*;

use super::{
    Ui,
    widgets::{attachment_card, icon_button, message_row, status_panel, text_action},
};
use crate::{
    model::FolderId,
    state::{Action, MessageFilter, Surface, ViewSnapshot, ViewStatus},
};

pub(super) fn render(ui: &Ui, snapshot: &ViewSnapshot) {
    if ui.search.text().as_str() != snapshot.search_query {
        ui.search.set_text(&snapshot.search_query);
    }
    render_folders(ui, snapshot);
    render_filters(ui, snapshot.message_filter);
    render_sync(ui, snapshot.surface);
    render_messages(ui, snapshot);
    render_reader(ui, snapshot);
}

fn render_folders(ui: &Ui, snapshot: &ViewSnapshot) {
    clear_list(&ui.folders);
    for folder in &snapshot.folders {
        let row = gtk::Button::builder()
            .has_frame(false)
            .css_classes(["waymail-folder-row"])
            .build();
        if folder.id == snapshot.selected_folder_id {
            row.add_css_class("selected");
        }
        let selected = folder.id == snapshot.selected_folder_id;
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .margin_start(12)
            .margin_end(12)
            .margin_top(10)
            .margin_bottom(10)
            .build();
        content.append(&gtk::Image::from_icon_name(folder.icon));
        content.append(
            &gtk::Label::builder()
                .label(folder.name)
                .xalign(0.0)
                .hexpand(true)
                .build(),
        );
        let count = snapshot
            .folder_counts
            .iter()
            .find(|(id, _)| *id == folder.id)
            .map_or(0, |(_, count)| *count);
        if count > 0 {
            content.append(
                &gtk::Label::builder()
                    .label(count.to_string())
                    .css_classes(["waymail-count"])
                    .build(),
            );
        }
        row.set_child(Some(&content));
        let count_kind = if folder.id == FolderId::INBOX {
            "unread"
        } else if folder.id == FolderId::STARRED {
            "starred"
        } else {
            "messages"
        };
        let selected_text = if selected { ", selected" } else { "" };
        row.update_property(&[gtk::accessible::Property::Label(&format!(
            "{}, {count} {count_kind}{selected_text}",
            folder.name
        ))]);
        row.update_state(&[gtk::accessible::State::Selected(Some(selected))]);
        let id = folder.id;
        row.connect_clicked({
            let weak_ui = ui.downgrade();
            move |_| {
                if let Some(ui) = weak_ui.upgrade() {
                    ui.dispatch(Action::SelectFolder(id));
                    if ui.outer.is_collapsed() {
                        ui.outer.set_show_sidebar(false);
                    }
                }
            }
        });
        ui.folders.append(&row);
    }
}

fn render_messages(ui: &Ui, snapshot: &ViewSnapshot) {
    clear_list(&ui.messages);
    if snapshot.status != ViewStatus::Ready {
        ui.messages.append(&status_panel(snapshot.status));
        return;
    }
    let selected_id = snapshot.selected_message.as_ref().map(|message| message.id);
    for message in &snapshot.visible_messages {
        let row = message_row(message, selected_id == Some(message.id));
        let id = message.id;
        row.connect_clicked({
            let weak_ui = ui.downgrade();
            move |_| {
                if let Some(ui) = weak_ui.upgrade() {
                    ui.dispatch(Action::SelectMessage(id));
                    if ui.inner.is_collapsed() {
                        ui.inner.set_show_content(true);
                    }
                }
            }
        });
        ui.messages.append(&row);
    }
}

fn render_reader(ui: &Ui, snapshot: &ViewSnapshot) {
    clear_box(&ui.reader);
    if matches!(snapshot.status, ViewStatus::Loading | ViewStatus::Offline) {
        ui.reader.append(&status_panel(snapshot.status));
        return;
    }
    let Some(message) = &snapshot.selected_message else {
        ui.reader.append(&status_panel(snapshot.status));
        return;
    };
    let title_line = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .build();
    title_line.append(
        &gtk::Label::builder()
            .label(message.subject)
            .xalign(0.0)
            .hexpand(true)
            .wrap(true)
            .css_classes(["title-1"])
            .build(),
    );
    title_line.append(&icon_button(
        "non-starred-symbolic",
        "Star message",
        "win.star",
    ));
    ui.reader.append(&title_line);

    let sender_line = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(14)
        .build();
    sender_line.append(
        &gtk::Label::builder()
            .label(message.initials.unwrap_or("?"))
            .width_chars(3)
            .height_request(50)
            .css_classes(["waymail-avatar", "large"])
            .build(),
    );
    let sender_copy = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .build();
    let sender_name = gtk::Label::builder()
        .label(message.sender)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(36)
        .css_classes(["title-4"])
        .build();
    let sender_meta = gtk::Label::builder()
        .label(format!("to me  •  {}", message.email))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(42)
        .css_classes(["dim-label"])
        .build();
    sender_copy.append(&sender_name);
    sender_copy.append(&sender_meta);
    sender_line.append(&sender_copy);
    sender_line.append(
        &gtk::Label::builder()
            .label(message.timestamp.unwrap_or("Time unavailable"))
            .css_classes(["dim-label"])
            .build(),
    );
    sender_line.append(&icon_button(
        "mail-reply-sender-symbolic",
        "Reply",
        "win.reply",
    ));
    ui.reader.append(&sender_line);
    ui.reader.append(
        &gtk::Label::builder()
            .label(message.body)
            .xalign(0.0)
            .yalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .css_classes(["waymail-body"])
            .build(),
    );
    for attachment in &message.attachments {
        ui.reader
            .append(&attachment_card(attachment.name, attachment.details));
    }
    let replies = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .margin_top(12)
        .build();
    replies.append(&text_action(
        "mail-reply-sender-symbolic",
        "Reply",
        "win.reply",
    ));
    replies.append(&text_action(
        "mail-reply-all-symbolic",
        "Reply all",
        "win.reply-all",
    ));
    replies.append(&text_action(
        "mail-forward-symbolic",
        "Forward",
        "win.forward",
    ));
    ui.reader.append(&replies);
}

fn render_filters(ui: &Ui, active: MessageFilter) {
    for (filter, button) in &ui.filter_buttons {
        let selected = *filter == active;
        if selected {
            button.add_css_class("waymail-filter-active");
        } else {
            button.remove_css_class("waymail-filter-active");
        }
        button.update_state(&[gtk::accessible::State::Selected(Some(selected))]);
    }
}

fn render_sync(ui: &Ui, surface: Surface) {
    let (title, detail, online) = match surface {
        Surface::Online => ("●  All caught up", "Last sync: 2 minutes ago", true),
        Surface::Loading => ("◌  Loading local mail", "Preparing fixture mailbox…", false),
        Surface::Offline => (
            "●  Offline preview",
            "Local fixture mail is paused in this view.",
            false,
        ),
    };
    ui.sync_title.set_text(title);
    ui.sync_detail.set_text(detail);
    if online {
        ui.sync_title.add_css_class("waymail-online");
    } else {
        ui.sync_title.remove_css_class("waymail-online");
    }
}

fn clear_list(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}
