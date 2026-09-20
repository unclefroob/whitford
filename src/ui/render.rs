use adw::prelude::*;

use super::{
    Ui,
    time::format_unix_local,
    widgets::{
        attachment_card, icon_button, message_row, notice_banner, onboarding_panel, status_panel,
        text_action,
    },
};
use crate::{
    config,
    state::{Action, MessageFilter, ReaderState, SessionState, ViewSnapshot, ViewStatus},
    worker::{BodyFailure, FailureKind, ServiceFailure, WorkerPhase},
};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn render(ui: &Ui, snapshot: &ViewSnapshot) {
    if ui.search.text().as_str() != snapshot.search_query {
        ui.search.set_text(&snapshot.search_query);
    }
    let cache_index = crate::cache::selected_index(snapshot.cache_limit);
    if ui.cache_limit.selected() != cache_index {
        ui.cache_limit.set_selected(cache_index);
    }
    render_folders(ui, snapshot);
    render_filters(ui, snapshot.message_filter);
    render_sync(ui, snapshot);
    render_cache_usage(ui, snapshot);
    render_banners(ui, snapshot);
    set_action_enabled(ui, "connect", snapshot.can_connect);
    set_action_enabled(ui, "refresh", snapshot.can_refresh);
    set_action_enabled(ui, "disconnect", snapshot.can_disconnect);
    set_action_enabled(ui, "reopen-authorization", snapshot.can_reopen);
    set_action_enabled(ui, "cancel-authorization", snapshot.can_cancel);
    set_action_enabled(ui, "retry", snapshot.can_retry);
    if ui.last_list_revision.get() != snapshot.list_revision {
        render_messages(ui, snapshot);
        ui.last_list_revision.set(snapshot.list_revision);
    }
    if ui.last_reader_revision.get() != snapshot.reader_revision {
        render_reader(ui, snapshot);
        ui.last_reader_revision.set(snapshot.reader_revision);
    }
}

fn render_cache_usage(ui: &Ui, snapshot: &ViewSnapshot) {
    let usage = snapshot.cache_usage;
    let text = if !usage.available {
        "Cache usage unavailable".into()
    } else if usage.body_count == 0 {
        "No downloaded messages".into()
    } else {
        format!(
            "{} downloaded · {}",
            usage.body_count,
            format_bytes(usage.body_bytes)
        )
    };
    ui.cache_usage.set_text(&text);
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn render_folders(ui: &Ui, snapshot: &ViewSnapshot) {
    clear_list(&ui.folders);
    for folder in &snapshot.folders {
        let row = gtk::Button::builder()
            .has_frame(false)
            .css_classes(["whitford-folder-row"])
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
                    .css_classes(["whitford-count"])
                    .build(),
            );
        }
        row.set_child(Some(&content));
        let count_kind = "unread";
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
    if !should_render_mail(snapshot.status, !snapshot.visible_messages.is_empty()) {
        ui.messages.append(&main_status_panel(snapshot));
        return;
    }
    let selected_id = snapshot
        .selected_message
        .as_ref()
        .map(|message| &message.id);
    for message in &snapshot.visible_messages {
        let row = message_row(message, selected_id == Some(&message.id));
        let id = message.id.clone();
        row.connect_clicked({
            let weak_ui = ui.downgrade();
            move |_| {
                if let Some(ui) = weak_ui.upgrade() {
                    ui.dispatch(Action::SelectMessage(id.clone()));
                    if ui.inner.is_collapsed() {
                        ui.inner.set_show_content(true);
                    }
                }
            }
        });
        ui.messages.append(&row);
    }
}

fn should_render_mail(status: ViewStatus, has_messages: bool) -> bool {
    status == ViewStatus::Ready
        || (matches!(status, ViewStatus::Offline | ViewStatus::Degraded) && has_messages)
}

fn render_reader(ui: &Ui, snapshot: &ViewSnapshot) {
    clear_box(&ui.reader);
    if matches!(&snapshot.reader, ReaderState::Closed) {
        ui.reader.append(&status_panel(ViewStatus::Ready));
        return;
    }
    if snapshot.selected_message.is_none() {
        ui.reader.append(&main_status_panel(snapshot));
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
            .label(&message.subject)
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

    let loaded_body = match &snapshot.reader {
        ReaderState::Loaded { id, body } if id == &message.id => Some(body.as_ref()),
        _ => None,
    };
    if loaded_body.is_some_and(|body| body.used_fallback) {
        ui.reader.append(&notice_banner(
            "Fallback content",
            "Whitford could not extract the preferred message body, so this reader shows a safe fallback.",
            None,
        ));
    }

    let sender_line = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(14)
        .build();
    sender_line.append(
        &gtk::Label::builder()
            .label(message.initials.as_deref().unwrap_or("?"))
            .width_chars(3)
            .height_request(50)
            .css_classes(["whitford-avatar", "large"])
            .build(),
    );
    let sender_copy = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .build();
    let sender_name = gtk::Label::builder()
        .label(&message.sender)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(36)
        .css_classes(["title-4"])
        .build();
    let sender_meta = gtk::Label::builder()
        .label(message.email.as_deref().unwrap_or("Address unavailable"))
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
            .label(format_unix_local(message.received_at_unix, now_unix()))
            .css_classes(["dim-label"])
            .build(),
    );
    sender_line.append(&icon_button(
        "mail-reply-sender-symbolic",
        "Reply",
        "win.reply",
    ));
    ui.reader.append(&sender_line);
    match &snapshot.reader {
        ReaderState::Loading { id, .. } if id == &message.id => {
            let loading = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(12)
                .halign(gtk::Align::Center)
                .margin_top(36)
                .build();
            loading.append(&gtk::Spinner::builder().spinning(true).build());
            loading.append(&gtk::Label::new(Some("Loading message…")));
            ui.reader.append(&loading);
            return;
        }
        ReaderState::Failed { id, failure } if id == &message.id => {
            let (title, detail, action) = match failure {
                BodyFailure::Offline => (
                    "Not available offline",
                    "This message has not been downloaded yet.",
                    Some(("Retry", "win.retry-body", "Retry loading this message")),
                ),
                BodyFailure::TimedOut => (
                    "Message loading timed out",
                    "Gmail took too long to return this message.",
                    Some(("Retry", "win.retry-body", "Retry loading this message")),
                ),
                BodyFailure::AuthorizationRequired => (
                    "Authorization required",
                    "Refresh Gmail authorization, then open this message again.",
                    Some(("Refresh", "win.refresh", "Refresh Gmail authorization")),
                ),
                BodyFailure::MailboxChanged => (
                    "Inbox changed",
                    "Refresh the inbox before opening this message.",
                    Some(("Refresh", "win.refresh", "Refresh Gmail inbox")),
                ),
                BodyFailure::Missing => (
                    "Message no longer available",
                    "This message may have been removed from Gmail.",
                    Some(("Refresh", "win.refresh", "Refresh Gmail inbox")),
                ),
                BodyFailure::Protocol => (
                    "Could not read this message",
                    "Gmail returned an unexpected response.",
                    Some(("Retry", "win.retry-body", "Retry loading this message")),
                ),
            };
            ui.reader.append(&notice_banner(title, detail, action));
            return;
        }
        ReaderState::Loaded { id, body } if id == &message.id => {
            if let Some(html) = &body.html {
                ui.reader.append(&super::email_view::message_body(html));
            } else {
                ui.reader.append(
                    &gtk::Label::builder()
                        .label(&body.text)
                        .xalign(0.0)
                        .yalign(0.0)
                        .wrap(true)
                        .wrap_mode(gtk::pango::WrapMode::WordChar)
                        .selectable(true)
                        .css_classes(["whitford-body"])
                        .build(),
                );
            }
            for attachment in &body.attachments {
                ui.reader.append(&attachment_card(attachment));
            }
        }
        _ => {
            ui.reader.append(&status_panel(ViewStatus::Ready));
            return;
        }
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
            button.add_css_class("whitford-filter-active");
        } else {
            button.remove_css_class("whitford-filter-active");
        }
        button.update_state(&[gtk::accessible::State::Selected(Some(selected))]);
    }
}

fn main_status_panel(snapshot: &ViewSnapshot) -> gtk::Box {
    match &snapshot.session {
        SessionState::Disconnected => {
            let (default_path, is_flatpak) = config::config_location_hint();
            onboarding_panel(default_path.as_deref(), is_flatpak)
        }
        SessionState::ConfigurationError { failure } => {
            let (title, detail) = failure_presentation(failure);
            recovery_panel(&title, &detail, Some(configuration_recovery_action()))
        }
        SessionState::AuthRequired { .. } if snapshot.sync_metadata.is_none() => recovery_panel(
            "Gmail authorization expired",
            "Reconnect Gmail and complete consent again. External Testing refresh tokens can expire after seven days.",
            Some(("Reconnect", "win.retry", "Reconnect Gmail")),
        ),
        SessionState::ServiceError { failure } if snapshot.sync_metadata.is_none() => {
            let (title, detail) = failure_presentation(failure);
            let action = if failure.retryable {
                Some(("Retry", "win.retry", "Retry Gmail operation"))
            } else if failure.kind == FailureKind::AuthorizationDenied {
                Some((
                    "Connect again",
                    "win.connect",
                    "Start Gmail connection again",
                ))
            } else {
                None
            };
            recovery_panel(&title, &detail, action)
        }
        _ => status_panel(snapshot.status),
    }
}

fn configuration_recovery_action() -> (&'static str, &'static str, &'static str) {
    (
        "Try again",
        "win.retry",
        "Retry Gmail startup after fixing google-oauth.json",
    )
}

fn recovery_panel(title: &str, detail: &str, action: Option<(&str, &str, &str)>) -> gtk::Box {
    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .build();
    panel.append(&notice_banner(title, detail, action));
    panel
}

fn render_banners(ui: &Ui, snapshot: &ViewSnapshot) {
    clear_box(&ui.list_banner);
    clear_box(&ui.reader_banner);
    let mut banners = Vec::new();
    match &snapshot.session {
        SessionState::Offline { failure } => {
            let (_, reason) = failure_presentation(failure);
            banners.push((
                "Offline — showing stale mail".to_owned(),
                format!("Last successful sync: {}. {reason}", last_sync(snapshot)),
                failure
                    .retryable
                    .then_some(("Retry", "win.retry", "Retry Gmail sync")),
            ));
        }
        SessionState::AuthRequired { cleanup_failed } if snapshot.sync_metadata.is_some() => {
            let cleanup = if *cleanup_failed {
                " The expired saved authorization could not be removed; revoke it in Google Account connections if reconnect does not resolve this."
            } else {
                ""
            };
            banners.push((
                "Authorization required — showing stale mail".to_owned(),
                format!(
                    "Last successful sync: {}. Reconnect Gmail to continue.{cleanup}",
                    last_sync(snapshot)
                ),
                Some((
                    "Reconnect",
                    "win.retry",
                    "Reconnect Gmail and refresh stale messages",
                )),
            ));
        }
        SessionState::ServiceError { failure }
            if failure.kind == FailureKind::DisconnectFailed
                && snapshot.sync_metadata.is_some() =>
        {
            banners.push((
                "Disconnect incomplete — showing retained mail".to_owned(),
                format!(
                    "Last successful sync: {}. Retry secure cleanup; revoke access in Google Account connections if it keeps failing.",
                    last_sync(snapshot)
                ),
                failure.retryable.then_some((
                    "Retry cleanup",
                    "win.retry",
                    "Retry removing saved Gmail authorization",
                )),
            ));
        }
        _ => {}
    }
    if let Some(detail) = partial_sync_detail(snapshot) {
        banners.push((
            "Some message details are unavailable".to_owned(),
            detail,
            None,
        ));
    }
    for (title, detail, action) in banners {
        ui.list_banner
            .append(&notice_banner(&title, &detail, action));
        ui.reader_banner
            .append(&notice_banner(&title, &detail, action));
    }
    ui.list_banner
        .set_visible(ui.list_banner.first_child().is_some());
    ui.reader_banner
        .set_visible(ui.reader_banner.first_child().is_some());
}

fn last_sync(snapshot: &ViewSnapshot) -> String {
    snapshot.sync_metadata.as_ref().map_or_else(
        || "not available".into(),
        |metadata| {
            system_time_unix(metadata.completed_at).map_or_else(
                || "not available".into(),
                |value| format_unix_local(Some(value), now_unix()),
            )
        },
    )
}

fn partial_sync_detail(snapshot: &ViewSnapshot) -> Option<String> {
    let metadata = snapshot.sync_metadata.as_ref()?;
    let mut details = Vec::new();
    if metadata.fallback_count > 0 {
        details.push(format!(
            "{} {} missing a usable sender or subject",
            metadata.fallback_count,
            if metadata.fallback_count == 1 {
                "message is"
            } else {
                "messages are"
            }
        ));
    }
    if metadata.skipped_count > 0 {
        details.push(format!(
            "{} {} not listed because Gmail did not return a usable header",
            metadata.skipped_count,
            if metadata.skipped_count == 1 {
                "message was"
            } else {
                "messages were"
            }
        ));
    }
    (!details.is_empty()).then(|| format!("{}.", details.join("; ")))
}

fn render_sync(ui: &Ui, snapshot: &ViewSnapshot) {
    let (title, detail, online): (String, String, bool) = match &snapshot.session {
        SessionState::Disconnected => (
            "Not connected".into(),
            "Developer preview · connect with your local OAuth client".into(),
            false,
        ),
        SessionState::Authorizing { .. } => (
            "Authorizing Gmail…".into(),
            "Complete sign-in in your browser, or cancel and retry.".into(),
            false,
        ),
        SessionState::Syncing { phase, .. } => {
            ("Syncing Gmail…".into(), worker_phase_copy(*phase).into(), false)
        }
        SessionState::Ready => ("●  Inbox up to date".into(), sync_detail(snapshot), true),
        SessionState::Offline { failure } => (
            "●  Inbox is stale".into(),
            format!("{} · {}", failure_presentation(failure).1, sync_detail(snapshot)),
            false,
        ),
        SessionState::AuthRequired { cleanup_failed } => {
            let detail = if *cleanup_failed {
                "Reconnect Gmail. Whitford could not remove the expired saved authorization; revoke it in Google Account connections if cleanup keeps failing."
            } else {
                "Reconnect Gmail to refresh this stale inbox."
            };
            ("Authorization required".into(), detail.into(), false)
        }
        SessionState::Disconnecting => (
            "Disconnecting…".into(),
            "Removing the saved authorization securely.".into(),
            false,
        ),
        SessionState::ConfigurationError { failure } => {
            let (failure_title, failure_detail) = failure_presentation(failure);
            (failure_title, failure_detail, false)
        }
        SessionState::ServiceError { failure } if failure.kind == crate::worker::FailureKind::DisconnectFailed => (
            "Saved authorization was not removed".into(),
            "Retry secure cleanup. Your mail remains loaded; revoke Whitford separately in Google Account connections if needed.".into(),
            false,
        ),
        SessionState::ServiceError { failure } => {
            let (failure_title, failure_detail) = failure_presentation(failure);
            (failure_title, failure_detail, false)
        }
    };
    ui.sync_title.set_text(&title);
    ui.sync_detail.set_text(&detail);
    if online {
        ui.sync_title.add_css_class("whitford-online");
    } else {
        ui.sync_title.remove_css_class("whitford-online");
    }
}

fn sync_detail(snapshot: &ViewSnapshot) -> String {
    let identity = snapshot
        .account
        .as_ref()
        .map(|account| account.email.as_str())
        .unwrap_or("No verified account");
    let detail = snapshot.sync_metadata.as_ref().map_or_else(
        || format!("Newest {} messages · read-only", snapshot.cache_limit),
        |meta| {
            let synced = system_time_unix(meta.completed_at).map_or_else(
                || "unknown time".into(),
                |value| format_unix_local(Some(value), now_unix()),
            );
            format!(
                "Synced {synced} · {} cached · newest {} refreshed",
                meta.loaded_count, meta.requested_limit
            )
        },
    );
    format!("{identity} · {detail}")
}

fn worker_phase_copy(phase: WorkerPhase) -> &'static str {
    match phase {
        WorkerPhase::LoadingConfiguration => "Checking google-oauth.json",
        WorkerPhase::WaitingForBrowser => "Waiting for browser authorization",
        WorkerPhase::ExchangingCode => "Completing secure authorization",
        WorkerPhase::OpeningKeyring => "Opening Secret Service",
        WorkerPhase::RefreshingToken => "Refreshing authorization",
        WorkerPhase::VerifyingIdentity => "Verifying Gmail identity",
        WorkerPhase::ConnectingImap => "Connecting securely to Gmail IMAP",
        WorkerPhase::FetchingInbox => "Loading the newest INBOX summaries",
        WorkerPhase::Disconnecting => "Removing saved authorization",
    }
}

fn failure_presentation(failure: &ServiceFailure) -> (String, String) {
    let path = failure
        .config_path
        .as_deref()
        .unwrap_or("the resolved Whitford configuration path");
    let (title, detail) = match failure.kind {
        FailureKind::ConfigurationDirectoryUnavailable => (
            "Configuration directory unavailable",
            "Set XDG_CONFIG_HOME to an absolute directory (or set HOME), then install google-oauth.json under whitford/ and connect again.",
        ),
        FailureKind::ConfigurationMissing => (
            "google-oauth.json is missing",
            "Download the Desktop app OAuth JSON from project whitford-email and install it with mode 600, then connect again.",
        ),
        FailureKind::ConfigurationUnreadable => (
            "google-oauth.json cannot be read",
            "Ensure it is a regular file owned by you and readable with mode 600, then connect again.",
        ),
        FailureKind::ConfigurationTooLarge => (
            "google-oauth.json is too large",
            "Replace it with the unmodified Desktop app JSON downloaded from Google Cloud (maximum 64 KiB).",
        ),
        FailureKind::ConfigurationInvalid => (
            "google-oauth.json is invalid",
            "Replace it with valid JSON for an OAuth Desktop app; web-client JSON and edited endpoint fields are not accepted.",
        ),
        FailureKind::ConfigurationWrongProject => (
            "OAuth client belongs to the wrong project",
            "Download a Desktop app credential from the exact Google Cloud project whitford-email and replace the file.",
        ),
        FailureKind::BrowserLaunchFailed => (
            "Browser did not open",
            "Retry or connect again to start a new Gmail authorization.",
        ),
        FailureKind::AuthorizationDenied => (
            "Gmail authorization was denied",
            "Connect again when ready and approve the requested Gmail IMAP and identity scopes.",
        ),
        FailureKind::AuthorizationTimedOut => (
            "Gmail authorization timed out",
            "Connect again and finish the browser consent before the callback expires.",
        ),
        FailureKind::AuthorizationInvalid => (
            "Gmail authorization response was invalid",
            "Connect again. If this repeats, close old consent tabs and use only the newly opened browser page.",
        ),
        FailureKind::Network => (
            "Network connection unavailable",
            "Check connectivity, then retry. Any loaded messages remain visible but stale.",
        ),
        FailureKind::ProviderUnavailable => (
            "Google authorization is unavailable",
            "Google returned a temporary service error. Wait briefly, then retry.",
        ),
        FailureKind::RateLimited => (
            "Google is temporarily rate limiting requests",
            "Wait before retrying so the limit can clear.",
        ),
        FailureKind::AuthorizationExpired => (
            "Gmail authorization expired",
            "Reconnect Gmail and complete consent again. External Testing refresh tokens can expire after seven days.",
        ),
        FailureKind::IdentityInvalid => (
            "Gmail identity could not be verified",
            "Reconnect and sign in with the Google account added as a consent-screen test user.",
        ),
        FailureKind::KeyringUnavailable => (
            "Secret Service is unavailable",
            "Unlock or start GNOME Keyring, KWallet, or KeePassXC, then retry. Whitford never stores the refresh token in plaintext.",
        ),
        FailureKind::CredentialSaveFailed => (
            "Authorization could not be saved",
            "Whitford removed any partial saved authorization. Unlock Secret Service and retry the connection; no plaintext fallback is used.",
        ),
        FailureKind::DisconnectFailed => (
            "Saved authorization was not removed",
            "Retry secure cleanup. Mail remains stale and visible; revoke Whitford in Google Account connections if cleanup keeps failing.",
        ),
        FailureKind::TlsFailed => (
            "Secure Gmail connection failed",
            "Check the system clock and network interception, then retry. Certificate checks are never disabled.",
        ),
        FailureKind::ImapAuthenticationFailed => (
            "Gmail rejected IMAP authorization",
            "Reconnect Gmail. Confirm Gmail API access and that the account is an allowed test user.",
        ),
        FailureKind::InboxUnavailable => (
            "Gmail INBOX is unavailable",
            "Confirm the account has Gmail enabled, then retry.",
        ),
        FailureKind::ImapProtocol => (
            "Gmail returned an unexpected IMAP response",
            "Retry once. If it repeats, keep the stale messages visible and report the issue without including message content.",
        ),
        FailureKind::SyncTimedOut => (
            "Gmail sync timed out",
            "Check connectivity and retry. The last successfully loaded messages remain stale and visible.",
        ),
        FailureKind::WorkerUnavailable => (
            "Mail service stopped",
            "Restart Whitford to restore the mail service.",
        ),
    };
    let detail = if failure.kind.is_configuration() && failure.config_path.is_some() {
        format!("{detail} Expected location: {path}")
    } else {
        detail.into()
    };
    (title.into(), detail)
}

fn now_unix() -> i64 {
    system_time_unix(SystemTime::now()).unwrap_or(0)
}

fn system_time_unix(value: SystemTime) -> Option<i64> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
}
fn set_action_enabled(ui: &Ui, name: &str, enabled: bool) {
    if let Some(action) = ui
        .window
        .lookup_action(name)
        .and_then(|action| action.downcast::<gtk::gio::SimpleAction>().ok())
    {
        action.set_enabled(enabled);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SyncMetadata;
    #[test]
    fn offline_snapshot_keeps_mail_visible() {
        assert!(should_render_mail(ViewStatus::Offline, true));
        assert!(!should_render_mail(ViewStatus::Offline, false));
        assert!(!should_render_mail(ViewStatus::Error, true));
        assert!(should_render_mail(ViewStatus::Degraded, true));
    }

    #[test]
    fn phase_and_failure_copy_is_human_readable_and_exhaustive() {
        let phases = [
            WorkerPhase::LoadingConfiguration,
            WorkerPhase::WaitingForBrowser,
            WorkerPhase::ExchangingCode,
            WorkerPhase::OpeningKeyring,
            WorkerPhase::RefreshingToken,
            WorkerPhase::VerifyingIdentity,
            WorkerPhase::ConnectingImap,
            WorkerPhase::FetchingInbox,
            WorkerPhase::Disconnecting,
        ];
        assert!(
            phases
                .into_iter()
                .all(|phase| !worker_phase_copy(phase).is_empty())
        );

        let failure = ServiceFailure {
            kind: FailureKind::ConfigurationMissing,
            retryable: true,
            preserve_mail: false,
            cleanup_failed: false,
            config_path: Some("/safe/google-oauth.json".into()),
        };
        let (title, detail) = failure_presentation(&failure);
        assert_eq!(title, "google-oauth.json is missing");
        assert!(detail.contains("/safe/google-oauth.json"));
        assert!(!detail.contains("ConfigurationMissing"));
    }

    #[test]
    fn recovery_copy_matches_available_actions() {
        let browser = failure_presentation(&ServiceFailure {
            kind: FailureKind::BrowserLaunchFailed,
            retryable: true,
            preserve_mail: false,
            cleanup_failed: false,
            config_path: None,
        });
        assert!(browser.1.contains("Retry or connect again"));
        assert!(!browser.1.contains("Reopen Browser"));

        let worker = failure_presentation(&ServiceFailure {
            kind: FailureKind::WorkerUnavailable,
            retryable: false,
            preserve_mail: true,
            cleanup_failed: false,
            config_path: None,
        });
        assert!(worker.1.contains("Restart Whitford"));
        assert!(!worker.1.contains("Retry"));
    }

    #[test]
    fn configuration_recovery_uses_retry_action() {
        let action = configuration_recovery_action();
        assert_eq!(action.0, "Try again");
        assert_eq!(action.1, "win.retry");
        assert!(!action.2.contains("connection"));
    }

    #[test]
    fn partial_sync_copy_hides_zero_counters() {
        let mut state = crate::state::AppState::new();
        let update = state.dispatch(Action::Connect);
        let id = match update.effects[0] {
            crate::state::Effect::SendWorker(crate::worker::WorkerCommand::Connect { id }) => id,
            _ => panic!(),
        };
        state.dispatch(Action::Worker(crate::worker::WorkerEvent::SyncComplete {
            id,
            account: crate::model::AccountIdentity {
                provider: crate::model::MailProvider::Gmail,
                email: "person@example.com".into(),
            },
            snapshot: crate::model::MailboxSnapshot {
                messages: vec![],
                metadata: SyncMetadata {
                    completed_at: UNIX_EPOCH,
                    requested_limit: 50,
                    loaded_count: 0,
                    fallback_count: 0,
                    skipped_count: 2,
                },
            },
        }));
        let detail = partial_sync_detail(&state.snapshot()).unwrap();
        assert!(detail.contains("2 messages were not listed"));
        assert!(!detail.contains("fallback"));
    }
}
