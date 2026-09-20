use adw::prelude::*;

use crate::{model::Message, state::ViewStatus};

pub(super) fn message_row(message: &Message, selected: bool) -> gtk::Button {
    let row = gtk::Button::builder()
        .has_frame(false)
        .css_classes(["waymail-message-row"])
        .build();
    if selected {
        row.add_css_class("selected");
    }
    if message.unread {
        row.add_css_class("unread");
    }
    let line = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .margin_start(14)
        .margin_end(14)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    let avatar = gtk::Label::builder()
        .label(message.initials.unwrap_or("?"))
        .width_chars(3)
        .height_request(44)
        .valign(gtk::Align::Start)
        .css_classes(["waymail-avatar"])
        .build();
    let copy = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .build();
    let top = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    let sender = ellipsized(message.sender, true);
    sender.set_hexpand(true);
    top.append(&sender);
    top.append(
        &gtk::Label::builder()
            .label(message.timestamp.unwrap_or("—"))
            .css_classes(["dim-label", "caption"])
            .build(),
    );
    copy.append(&top);
    copy.append(&ellipsized(message.subject, message.unread));
    copy.append(&ellipsized(
        message.preview.unwrap_or("No preview available"),
        false,
    ));
    line.append(&avatar);
    line.append(&copy);
    if message.starred {
        line.append(&gtk::Image::from_icon_name("starred-symbolic"));
    }
    row.set_child(Some(&line));
    let mut traits = Vec::new();
    if selected {
        traits.push("selected");
    }
    if message.unread {
        traits.push("unread");
    }
    if message.starred {
        traits.push("starred");
    }
    if !message.attachments.is_empty() {
        traits.push("has attachments");
    }
    let traits = if traits.is_empty() {
        String::new()
    } else {
        format!(", {}", traits.join(", "))
    };
    row.update_property(&[gtk::accessible::Property::Label(&format!(
        "{}: {}{}",
        message.sender, message.subject, traits
    ))]);
    row.update_state(&[gtk::accessible::State::Selected(Some(selected))]);
    row
}

pub(super) fn status_panel(status: ViewStatus) -> gtk::Box {
    let (icon, title, detail) = match status {
        ViewStatus::Loading => (
            "content-loading-symbolic",
            "Loading mail",
            "Preparing your local mailbox…",
        ),
        ViewStatus::Offline => (
            "network-offline-symbolic",
            "Offline preview",
            "Local fixture mail is paused in this status view.",
        ),
        ViewStatus::EmptyFolder => (
            "mail-read-symbolic",
            "Nothing here",
            "This folder is empty.",
        ),
        ViewStatus::NoSearchResults => (
            "system-search-symbolic",
            "No matches",
            "Try a different sender, subject, or phrase.",
        ),
        ViewStatus::Ready => (
            "mail-unread-symbolic",
            "Select a message",
            "Choose a message to read it.",
        ),
    };
    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .margin_top(48)
        .margin_bottom(48)
        .css_classes(["waymail-status"])
        .build();
    panel.append(&gtk::Image::builder().icon_name(icon).pixel_size(40).build());
    panel.append(
        &gtk::Label::builder()
            .label(title)
            .css_classes(["title-3"])
            .build(),
    );
    panel.append(
        &gtk::Label::builder()
            .label(detail)
            .wrap(true)
            .justify(gtk::Justification::Center)
            .css_classes(["dim-label"])
            .build(),
    );
    panel
}

pub(super) fn attachment_card(name: &str, details: Option<&str>) -> gtk::Box {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(14)
        .margin_top(8)
        .css_classes(["waymail-attachment"])
        .build();
    card.append(&gtk::Image::from_icon_name("x-office-document-symbolic"));
    let copy = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .build();
    copy.append(
        &gtk::Label::builder()
            .label(name)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(48)
            .css_classes(["heading"])
            .build(),
    );
    copy.append(
        &gtk::Label::builder()
            .label(details.unwrap_or("Attachment details unavailable"))
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(48)
            .css_classes(["dim-label"])
            .build(),
    );
    card.append(&copy);
    card.append(&icon_button(
        "folder-download-symbolic",
        "Download attachment",
        "win.download",
    ));
    card
}

pub(super) fn text_action(icon: &str, label: &str, action: &str) -> gtk::Button {
    gtk::Button::builder()
        .label(label)
        .icon_name(icon)
        .action_name(action)
        .css_classes(["waymail-reader-action"])
        .build()
}

pub(super) fn icon_button(icon: &str, tooltip: &str, action: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .action_name(action)
        .css_classes(["flat", "circular"])
        .build();
    button.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    button
}

fn ellipsized(text: &str, strong: bool) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(38)
        .build();
    if strong {
        label.add_css_class("heading");
    } else {
        label.add_css_class("dim-label");
    }
    label
}
