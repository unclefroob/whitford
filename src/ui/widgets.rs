use adw::prelude::*;

use super::time::format_unix_local;
use crate::{
    model::{Attachment, Message},
    state::ViewStatus,
};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn message_row(message: &Message, selected: bool) -> gtk::Button {
    let row = gtk::Button::builder()
        .has_frame(false)
        .css_classes(["whitford-message-row"])
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
        .label(message.initials.as_deref().unwrap_or("?"))
        .width_chars(3)
        .height_request(44)
        .valign(gtk::Align::Start)
        .css_classes(["whitford-avatar"])
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
    let sender = ellipsized(&message.sender, true);
    sender.set_hexpand(true);
    top.append(&sender);
    top.append(
        &gtk::Label::builder()
            .label(message.received_at_unix.map_or_else(
                || "Time unavailable".into(),
                |value| format_unix_local(Some(value), now_unix()),
            ))
            .css_classes(["dim-label", "caption"])
            .build(),
    );
    copy.append(&top);
    copy.append(&ellipsized(&message.subject, message.unread));
    copy.append(&ellipsized(
        message.preview.as_deref().unwrap_or("No preview available"),
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

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}

pub(super) fn status_panel(status: ViewStatus) -> gtk::Box {
    let (icon, title, detail) = match status {
        ViewStatus::Disconnected => (
            "network-offline-symbolic",
            "Connect Gmail",
            "Developer preview: add your OAuth client, then connect. Gmail grants its broad IMAP scope; Whitford remains read-only.",
        ),
        ViewStatus::Loading => (
            "content-loading-symbolic",
            "Loading mail",
            "Securely preparing your Gmail inbox…",
        ),
        ViewStatus::Offline => (
            "network-offline-symbolic",
            "Offline — showing stale mail",
            "Retry to refresh the loaded messages.",
        ),
        ViewStatus::Degraded => (
            "dialog-warning-symbolic",
            "Showing stale mail",
            "Use the recovery action above the message list or reader.",
        ),
        ViewStatus::EmptyInbox => (
            "mail-read-symbolic",
            "Nothing here",
            "No messages were returned in the newest 50.",
        ),
        ViewStatus::NoSearchResults => (
            "system-search-symbolic",
            "No matches",
            "No match in the loaded messages.",
        ),
        ViewStatus::Ready => (
            "mail-unread-symbolic",
            "Select a message",
            "Choose a message to read it.",
        ),
        ViewStatus::Error => (
            "dialog-warning-symbolic",
            "Gmail is unavailable",
            "Review the status, then retry.",
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
        .css_classes(["whitford-status"])
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

pub(super) fn onboarding_panel(path: Option<&str>, is_flatpak: bool) -> gtk::Box {
    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .halign(gtk::Align::Fill)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .margin_start(24)
        .margin_end(24)
        .margin_top(32)
        .margin_bottom(32)
        .css_classes(["whitford-status"])
        .build();
    panel.update_property(&[gtk::accessible::Property::Label(
        "Gmail developer preview setup",
    )]);
    panel.append(
        &gtk::Label::builder()
            .label("Connect the Gmail developer preview")
            .xalign(0.0)
            .wrap(true)
            .css_classes(["title-2"])
            .build(),
    );
    let environment = if is_flatpak {
        "Flatpak configuration"
    } else {
        "Native configuration"
    };
    let resolved = path.unwrap_or("Set an absolute XDG_CONFIG_HOME or HOME first");
    for copy in [
        "In Google Cloud project whitford-email, enable Gmail API, configure the consent audience, and add this Gmail account as a test user.",
        "Download a Desktop app credential, name it google-oauth.json, and install it at:",
        resolved,
        "Google requires https://mail.google.com/ for IMAP, plus openid and email. That scope is broad, but this preview only reads the newest 50 INBOX messages.",
    ] {
        panel.append(
            &gtk::Label::builder()
                .label(copy)
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .selectable(copy == resolved)
                .css_classes(["dim-label"])
                .build(),
        );
    }
    panel.append(
        &gtk::Label::builder()
            .label(environment)
            .xalign(0.0)
            .wrap(true)
            .css_classes(["caption", "dim-label"])
            .build(),
    );
    let connect = gtk::Button::builder()
        .label("Connect Gmail")
        .action_name("win.connect")
        .halign(gtk::Align::Start)
        .css_classes(["suggested-action"])
        .build();
    connect.update_property(&[gtk::accessible::Property::Label(
        "Connect Gmail after installing google-oauth.json",
    )]);
    panel.append(&connect);
    panel
}

pub(super) fn notice_banner(
    title: &str,
    detail: &str,
    action: Option<(&str, &str, &str)>,
) -> gtk::Box {
    let banner = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(8)
        .css_classes(["card"])
        .build();
    banner.update_property(&[gtk::accessible::Property::Label(&format!(
        "{title}. {detail}"
    ))]);
    let copy = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .build();
    copy.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["heading"])
            .build(),
    );
    copy.append(
        &gtk::Label::builder()
            .label(detail)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["dim-label", "caption"])
            .build(),
    );
    banner.append(&copy);
    if let Some((label, action_name, accessible_label)) = action {
        let button = gtk::Button::builder()
            .label(label)
            .action_name(action_name)
            .valign(gtk::Align::Center)
            .build();
        button.update_property(&[gtk::accessible::Property::Label(accessible_label)]);
        banner.append(&button);
    }
    banner
}

pub(super) fn attachment_card(attachment: &Attachment) -> gtk::Box {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(14)
        .margin_top(8)
        .css_classes(["whitford-attachment"])
        .build();
    card.append(&gtk::Image::from_icon_name("x-office-document-symbolic"));
    let copy = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .build();
    copy.append(
        &gtk::Label::builder()
            .label(&attachment.name)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(48)
            .css_classes(["heading"])
            .build(),
    );
    copy.append(
        &gtk::Label::builder()
            .label(
                attachment
                    .media_type
                    .as_deref()
                    .unwrap_or("Attachment details unavailable"),
            )
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(48)
            .css_classes(["dim-label"])
            .build(),
    );
    card.append(&copy);
    card
}

pub(super) fn text_action(icon: &str, label: &str, action: &str) -> gtk::Button {
    gtk::Button::builder()
        .label(label)
        .icon_name(icon)
        .action_name(action)
        .css_classes(["whitford-reader-action"])
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
