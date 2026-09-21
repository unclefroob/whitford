mod actions;
mod build;
mod composer_editor;
mod email_view;
mod render;
mod time;
mod widgets;

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
    time::Duration,
};

use adw::prelude::*;

use crate::{
    model::{FolderId, MessageSummary},
    oauth::AuthorizationUrl,
    state::{Action, AppState, ComposerState, Effect, MessageFilter},
    worker::{OperationId, WorkerCommand, WorkerEvent},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MessageListItem {
    pub(crate) message: MessageSummary,
    pub(crate) selected: bool,
    /// Present only for the incremental multi-account projection.  Keeping the
    /// scoped identity beside the legacy summary avoids treating a Gmail ID as
    /// globally unique when the row is activated.
    pub(crate) account_message_id: Option<crate::model::AccountMessageId>,
    pub(crate) account_label: Option<String>,
}

thread_local! {
    // ToastOverlay owns presentation, while this reference lets a later state snapshot
    // dismiss the exact operation's toast when Gmail rejects or consumes it.
    static UNDO_TOAST: RefCell<Option<(u64, adw::Toast)>> = const { RefCell::new(None) };
}

/// The GLib handle is deliberately kept outside `AppState`: state only describes
/// scheduling intent, while this owns the main-loop resource that can be removed
/// as soon as a window goes away.
pub(super) struct BackgroundSyncTimer {
    pub(super) source: gtk::glib::SourceId,
    pub(super) generation: u64,
}

#[derive(Clone)]
pub struct Ui {
    pub window: adw::ApplicationWindow,
    pub(crate) state: Rc<RefCell<AppState>>,
    pub(crate) folders: gtk::ListBox,
    pub(crate) messages: gtk::ListView,
    pub(crate) message_model: gtk::gio::ListStore,
    pub(crate) message_status: gtk::Box,
    pub(crate) list_banner: gtk::Box,
    pub(crate) search: gtk::SearchEntry,
    pub(crate) reader: gtk::Box,
    pub(crate) reader_banner: gtk::Box,
    pub(crate) label_menu: gtk::gio::Menu,
    pub(crate) outer: adw::OverlaySplitView,
    pub(crate) inner: adw::NavigationSplitView,
    pub(crate) navigation: adw::NavigationView,
    pub(crate) toast_overlay: adw::ToastOverlay,
    pub(crate) sync_title: gtk::Label,
    pub(crate) sync_detail: adw::ActionRow,
    pub(crate) cache_limit: gtk::DropDown,
    pub(crate) cache_usage: gtk::Label,
    pub(crate) appearance: gtk::DropDown,
    pub(crate) accounts_rows: gtk::Box,
    pub(crate) sync_refresh: adw::ActionRow,
    pub(crate) sync_retry: adw::ActionRow,
    pub(crate) sync_reopen: adw::ActionRow,
    pub(crate) sync_cancel: adw::ActionRow,
    pub(crate) composer_window: adw::Window,
    pub(crate) composer_to: gtk::Entry,
    pub(crate) composer_cc: gtk::Entry,
    pub(crate) composer_bcc: gtk::Entry,
    pub(crate) composer_cc_bcc: gtk::Box,
    pub(crate) composer_subject: gtk::Entry,
    pub(crate) composer_editor: composer_editor::ComposerEditor,
    pub(crate) composer_attachments: gtk::Box,
    pub(crate) composer_attachment_total: gtk::Label,
    pub(crate) composer_attach: gtk::Button,
    pub(crate) composer_inline: gtk::Button,
    pub(crate) composer_expand: gtk::Button,
    pub(crate) composer_signature: gtk::Button,
    pub(crate) composer_send: gtk::Button,
    pub(crate) composer_hide: gtk::Button,
    pub(crate) composer_discard: gtk::Button,
    pub(crate) composer_refresh: gtk::Button,
    pub(crate) composer_progress: gtk::Spinner,
    pub(crate) composer_draft_status: gtk::Label,
    pub(crate) composer_error: gtk::Label,
    pub(crate) composer_message_id: Rc<RefCell<Option<String>>>,
    pub(crate) composer_inline_ids: Rc<RefCell<Vec<String>>>,
    pub(crate) composer_attachment_fingerprint: Rc<Cell<u64>>,
    pub(crate) last_list_revision: Rc<Cell<u64>>,
    pub(crate) last_reader_revision: Rc<Cell<u64>>,
    pub(crate) attachment_progress:
        Rc<RefCell<HashMap<crate::worker::AttachmentJobId, gtk::ProgressBar>>>,
    pub(crate) filter_buttons: Vec<(MessageFilter, gtk::Button)>,
    pub(crate) worker: tokio::sync::mpsc::UnboundedSender<WorkerCommand>,
    pub(crate) authorization: Rc<RefCell<Option<(OperationId, AuthorizationUrl)>>>,
    pub(crate) background_sync_timer: Rc<RefCell<Option<BackgroundSyncTimer>>>,
    pub(crate) application: gtk::glib::WeakRef<adw::Application>,
}

#[derive(Clone)]
pub(crate) struct WeakUi {
    window: gtk::glib::WeakRef<adw::ApplicationWindow>,
    state: Weak<RefCell<AppState>>,
    folders: gtk::glib::WeakRef<gtk::ListBox>,
    messages: gtk::glib::WeakRef<gtk::ListView>,
    message_model: gtk::gio::ListStore,
    message_status: gtk::glib::WeakRef<gtk::Box>,
    list_banner: gtk::glib::WeakRef<gtk::Box>,
    search: gtk::glib::WeakRef<gtk::SearchEntry>,
    reader: gtk::glib::WeakRef<gtk::Box>,
    reader_banner: gtk::glib::WeakRef<gtk::Box>,
    label_menu: gtk::gio::Menu,
    outer: gtk::glib::WeakRef<adw::OverlaySplitView>,
    inner: gtk::glib::WeakRef<adw::NavigationSplitView>,
    navigation: gtk::glib::WeakRef<adw::NavigationView>,
    toast_overlay: gtk::glib::WeakRef<adw::ToastOverlay>,
    sync_title: gtk::glib::WeakRef<gtk::Label>,
    sync_detail: gtk::glib::WeakRef<adw::ActionRow>,
    cache_limit: gtk::glib::WeakRef<gtk::DropDown>,
    cache_usage: gtk::glib::WeakRef<gtk::Label>,
    appearance: gtk::glib::WeakRef<gtk::DropDown>,
    accounts_rows: gtk::glib::WeakRef<gtk::Box>,
    sync_refresh: gtk::glib::WeakRef<adw::ActionRow>,
    sync_retry: gtk::glib::WeakRef<adw::ActionRow>,
    sync_reopen: gtk::glib::WeakRef<adw::ActionRow>,
    sync_cancel: gtk::glib::WeakRef<adw::ActionRow>,
    composer_window: gtk::glib::WeakRef<adw::Window>,
    composer_to: gtk::glib::WeakRef<gtk::Entry>,
    composer_cc: gtk::glib::WeakRef<gtk::Entry>,
    composer_bcc: gtk::glib::WeakRef<gtk::Entry>,
    composer_cc_bcc: gtk::glib::WeakRef<gtk::Box>,
    composer_subject: gtk::glib::WeakRef<gtk::Entry>,
    composer_editor: composer_editor::ComposerEditor,
    composer_attachments: gtk::glib::WeakRef<gtk::Box>,
    composer_attachment_total: gtk::glib::WeakRef<gtk::Label>,
    composer_attach: gtk::glib::WeakRef<gtk::Button>,
    composer_inline: gtk::glib::WeakRef<gtk::Button>,
    composer_expand: gtk::glib::WeakRef<gtk::Button>,
    composer_signature: gtk::glib::WeakRef<gtk::Button>,
    composer_send: gtk::glib::WeakRef<gtk::Button>,
    composer_hide: gtk::glib::WeakRef<gtk::Button>,
    composer_discard: gtk::glib::WeakRef<gtk::Button>,
    composer_refresh: gtk::glib::WeakRef<gtk::Button>,
    composer_progress: gtk::glib::WeakRef<gtk::Spinner>,
    composer_draft_status: gtk::glib::WeakRef<gtk::Label>,
    composer_error: gtk::glib::WeakRef<gtk::Label>,
    composer_message_id: Weak<RefCell<Option<String>>>,
    composer_inline_ids: Weak<RefCell<Vec<String>>>,
    composer_attachment_fingerprint: Weak<Cell<u64>>,
    last_list_revision: Weak<Cell<u64>>,
    last_reader_revision: Weak<Cell<u64>>,
    attachment_progress: Weak<RefCell<HashMap<crate::worker::AttachmentJobId, gtk::ProgressBar>>>,
    filter_buttons: Vec<(MessageFilter, gtk::glib::WeakRef<gtk::Button>)>,
    worker: tokio::sync::mpsc::UnboundedSender<WorkerCommand>,
    authorization: Weak<RefCell<Option<(OperationId, AuthorizationUrl)>>>,
    background_sync_timer: Weak<RefCell<Option<BackgroundSyncTimer>>>,
    application: gtk::glib::WeakRef<adw::Application>,
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
            message_model: self.message_model.clone(),
            message_status: self.message_status.downgrade(),
            list_banner: self.list_banner.downgrade(),
            search: self.search.downgrade(),
            reader: self.reader.downgrade(),
            reader_banner: self.reader_banner.downgrade(),
            label_menu: self.label_menu.clone(),
            outer: self.outer.downgrade(),
            inner: self.inner.downgrade(),
            navigation: self.navigation.downgrade(),
            toast_overlay: self.toast_overlay.downgrade(),
            sync_title: self.sync_title.downgrade(),
            sync_detail: self.sync_detail.downgrade(),
            cache_limit: self.cache_limit.downgrade(),
            cache_usage: self.cache_usage.downgrade(),
            appearance: self.appearance.downgrade(),
            accounts_rows: self.accounts_rows.downgrade(),
            sync_refresh: self.sync_refresh.downgrade(),
            sync_retry: self.sync_retry.downgrade(),
            sync_reopen: self.sync_reopen.downgrade(),
            sync_cancel: self.sync_cancel.downgrade(),
            composer_window: self.composer_window.downgrade(),
            composer_to: self.composer_to.downgrade(),
            composer_cc: self.composer_cc.downgrade(),
            composer_bcc: self.composer_bcc.downgrade(),
            composer_cc_bcc: self.composer_cc_bcc.downgrade(),
            composer_subject: self.composer_subject.downgrade(),
            composer_editor: self.composer_editor.clone(),
            composer_attachments: self.composer_attachments.downgrade(),
            composer_attachment_total: self.composer_attachment_total.downgrade(),
            composer_attach: self.composer_attach.downgrade(),
            composer_inline: self.composer_inline.downgrade(),
            composer_expand: self.composer_expand.downgrade(),
            composer_signature: self.composer_signature.downgrade(),
            composer_send: self.composer_send.downgrade(),
            composer_hide: self.composer_hide.downgrade(),
            composer_discard: self.composer_discard.downgrade(),
            composer_refresh: self.composer_refresh.downgrade(),
            composer_progress: self.composer_progress.downgrade(),
            composer_draft_status: self.composer_draft_status.downgrade(),
            composer_error: self.composer_error.downgrade(),
            composer_message_id: Rc::downgrade(&self.composer_message_id),
            composer_inline_ids: Rc::downgrade(&self.composer_inline_ids),
            composer_attachment_fingerprint: Rc::downgrade(&self.composer_attachment_fingerprint),
            last_list_revision: Rc::downgrade(&self.last_list_revision),
            last_reader_revision: Rc::downgrade(&self.last_reader_revision),
            attachment_progress: Rc::downgrade(&self.attachment_progress),
            filter_buttons: self
                .filter_buttons
                .iter()
                .map(|(filter, button)| (*filter, button.downgrade()))
                .collect(),
            worker: self.worker.clone(),
            authorization: Rc::downgrade(&self.authorization),
            background_sync_timer: Rc::downgrade(&self.background_sync_timer),
            application: self.application.clone(),
        }
    }
    /// Apply an application action from the GTK main context.
    ///
    /// This is public for the binary's application-level notification action;
    /// worker and cache code continue to communicate through state effects.
    pub fn dispatch(&self, action: Action) {
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
        self.sync_undo_message_toast(snapshot.undo_message_operation.as_ref());
        render::render(self, &snapshot);
    }

    pub(crate) fn toast(&self, message: &str) {
        self.toast_overlay.add_toast(adw::Toast::new(message));
    }

    fn sync_undo_message_toast(&self, operation: Option<&crate::state::UndoMessageOperationView>) {
        UNDO_TOAST.with(|current| {
            let mut current = current.borrow_mut();
            if current
                .as_ref()
                .is_some_and(|(id, _)| operation.is_some_and(|operation| operation.id == *id))
            {
                return;
            }
            if let Some((_, toast)) = current.take() {
                toast.dismiss();
            }
            let Some(operation) = operation else {
                return;
            };
            let toast = adw::Toast::new(&format!("Message changed: {}", operation.title));
            toast.set_button_label(Some("Undo"));
            toast.set_action_name(Some("win.undo-message-operation"));
            // The button's enabled state comes from the operation-specific window
            // action. It stays unavailable until Gmail confirms the forward action.
            toast.set_timeout(8);
            self.toast_overlay.add_toast(toast.clone());
            *current = Some((operation.id, toast));
        });
    }
    pub(crate) fn send_message(&self) {
        self.flush_recipients();
        let weak = self.downgrade();
        self.composer_editor.snapshot(move |html, text| {
            if let Some(ui) = weak.upgrade() {
                ui.dispatch(Action::UpdateHtml { html, text });
                ui.dispatch(Action::SendMessage);
            }
        });
    }
    pub(crate) fn request_close_composer(&self) {
        let composer = self.state.borrow().snapshot().composer;
        match &composer {
            ComposerState::Closed => self.composer_window.set_visible(false),
            ComposerState::Sending { .. } => self.toast("Wait for the message to finish sending"),
            ComposerState::Editing { .. } | ComposerState::Failed { .. } => {
                self.flush_recipients();
                let weak = self.downgrade();
                self.composer_editor.snapshot(move |html, text| {
                    if let Some(ui) = weak.upgrade() {
                        ui.dispatch(Action::UpdateHtml { html, text });
                        ui.dispatch(Action::HideComposer);
                    }
                });
            }
        }
    }

    pub(crate) fn save_composer_then_close_app(&self) {
        let composer = self.state.borrow().snapshot().composer;
        if matches!(composer, ComposerState::Sending { .. }) {
            self.toast("Wait for the message to finish sending");
            return;
        }
        if matches!(composer, ComposerState::Closed) {
            self.window.close();
            return;
        }
        self.flush_recipients();
        let weak = self.downgrade();
        self.composer_editor.snapshot(move |html, text| {
            if let Some(ui) = weak.upgrade() {
                ui.dispatch(Action::UpdateHtml { html, text });
                ui.dispatch(Action::HideComposerAndCloseApp);
            }
        });
    }

    pub(crate) fn flush_recipients(&self) {
        self.dispatch(Action::UpdateRecipients {
            to: parse_recipients(&self.composer_to.text()),
            cc: parse_recipients(&self.composer_cc.text()),
            bcc: parse_recipients(&self.composer_bcc.text()),
        });
        self.dispatch(Action::UpdateSubject(
            self.composer_subject.text().to_string(),
        ));
    }
    fn handle_worker_event(&self, event: WorkerEvent) {
        if let WorkerEvent::AttachmentProgress {
            job_id,
            transferred,
            total,
            ..
        } = &event
            && let Some(progress) = self.attachment_progress.borrow().get(job_id)
        {
            progress.set_fraction(if *total == 0 {
                0.0
            } else {
                (*transferred as f64 / *total as f64).clamp(0.0, 1.0)
            });
            progress.set_text(Some(&format!(
                "{} of {}",
                format_bytes(*transferred),
                format_bytes(*total)
            )));
        }
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
            Effect::ScheduleBackgroundSync {
                after,
                schedule_generation,
            } => self.schedule_background_sync(after, schedule_generation),
            Effect::CancelBackgroundSyncTimer => self.cancel_background_sync_timer(),
            Effect::NotifyNewUnread { count } => self.notify_new_unread(count),
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
            Effect::LaunchAttachment(path) => {
                let uri = gtk::gio::File::for_path(path).uri();
                let weak = self.downgrade();
                gtk::glib::MainContext::default().spawn_local(async move {
                    if gtk::gio::AppInfo::launch_default_for_uri_future(
                        &uri,
                        None::<&gtk::gio::AppLaunchContext>,
                    )
                    .await
                    .is_err()
                        && let Some(ui) = weak.upgrade()
                    {
                        ui.toast("No application could open this attachment");
                    }
                });
            }
            Effect::PresentDisconnectConfirmation => {
                let dialog = adw::AlertDialog::builder().heading("Disconnect Gmail?").body("This removes this account’s local drafts, staged attachments, signature settings, downloaded mail cache, and saved authorization from Secret Service. Revoke Google access separately in your Google Account.").build();
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
                    .heading("Clear downloaded mail?")
                    .body("This removes opened message bodies and attachment files saved locally. Message summaries, your Gmail authorization, and the Keep summaries setting remain.")
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
            Effect::PresentUncertainResendConfirmation => {
                let dialog = adw::AlertDialog::builder()
                    .heading("Send this message again?")
                    .body("Gmail may already have accepted the previous attempt. Check Sent first; sending again can create a duplicate.")
                    .build();
                dialog.add_response("cancel", "Cancel");
                dialog.add_response("sent", "Open Sent");
                dialog.add_response("resend", "Send Again");
                dialog.set_response_appearance("resend", adw::ResponseAppearance::Destructive);
                let weak = self.downgrade();
                dialog.choose(
                    Some(&self.composer_window),
                    None::<&gtk::gio::Cancellable>,
                    move |response| {
                        if let Some(ui) = weak.upgrade() {
                            if response == "resend" {
                                ui.dispatch(Action::ConfirmResend);
                            } else if response == "sent" {
                                ui.dispatch(Action::SelectFolder(FolderId::Sent));
                                ui.request_close_composer();
                            }
                        }
                    },
                );
            }
            Effect::CloseApplicationWindow => self.window.close(),
        }
    }

    fn schedule_background_sync(&self, after: Duration, schedule_generation: u64) {
        self.cancel_background_sync_timer();

        let weak_ui = self.downgrade();
        let weak_timer = Rc::downgrade(&self.background_sync_timer);
        let source = gtk::glib::timeout_add_local_once(after, move || {
            let Some(timer) = weak_timer.upgrade() else {
                return;
            };
            // A replacement timer may have been armed after this callback entered
            // the main context. Only its matching generation can dispatch a tick.
            let is_current = timer
                .borrow()
                .as_ref()
                .is_some_and(|current| current.generation == schedule_generation);
            if !is_current {
                return;
            }
            timer.borrow_mut().take();
            if let Some(ui) = weak_ui.upgrade() {
                ui.dispatch(Action::BackgroundSyncTimer {
                    schedule_generation,
                });
            }
        });
        *self.background_sync_timer.borrow_mut() = Some(BackgroundSyncTimer {
            source,
            generation: schedule_generation,
        });
    }

    fn cancel_background_sync_timer(&self) {
        if let Some(timer) = self.background_sync_timer.borrow_mut().take() {
            timer.source.remove();
        }
    }

    fn notify_new_unread(&self, count: usize) {
        if count == 0 {
            return;
        }
        let Some(application) = self.application.upgrade() else {
            tracing::debug!(
                count,
                "skipped new-unread notification after application shutdown"
            );
            return;
        };
        let notification = gtk::gio::Notification::new("New unread mail");
        notification.set_body(Some(&new_unread_notification_body(count)));
        notification.set_default_action("app.open-inbox");
        // gio deliberately does not report desktop delivery failures. Sending is
        // best-effort and, importantly, does not feed back into sync state.
        application.send_notification(Some("inbox-new-unread"), &notification);
    }
}

impl WeakUi {
    pub(crate) fn upgrade(&self) -> Option<Ui> {
        Some(Ui {
            window: self.window.upgrade()?,
            state: self.state.upgrade()?,
            folders: self.folders.upgrade()?,
            messages: self.messages.upgrade()?,
            message_model: self.message_model.clone(),
            message_status: self.message_status.upgrade()?,
            list_banner: self.list_banner.upgrade()?,
            search: self.search.upgrade()?,
            reader: self.reader.upgrade()?,
            reader_banner: self.reader_banner.upgrade()?,
            label_menu: self.label_menu.clone(),
            outer: self.outer.upgrade()?,
            inner: self.inner.upgrade()?,
            navigation: self.navigation.upgrade()?,
            toast_overlay: self.toast_overlay.upgrade()?,
            sync_title: self.sync_title.upgrade()?,
            sync_detail: self.sync_detail.upgrade()?,
            cache_limit: self.cache_limit.upgrade()?,
            cache_usage: self.cache_usage.upgrade()?,
            appearance: self.appearance.upgrade()?,
            accounts_rows: self.accounts_rows.upgrade()?,
            sync_refresh: self.sync_refresh.upgrade()?,
            sync_retry: self.sync_retry.upgrade()?,
            sync_reopen: self.sync_reopen.upgrade()?,
            sync_cancel: self.sync_cancel.upgrade()?,
            composer_window: self.composer_window.upgrade()?,
            composer_to: self.composer_to.upgrade()?,
            composer_cc: self.composer_cc.upgrade()?,
            composer_bcc: self.composer_bcc.upgrade()?,
            composer_cc_bcc: self.composer_cc_bcc.upgrade()?,
            composer_subject: self.composer_subject.upgrade()?,
            composer_editor: self.composer_editor.clone(),
            composer_attachments: self.composer_attachments.upgrade()?,
            composer_attachment_total: self.composer_attachment_total.upgrade()?,
            composer_attach: self.composer_attach.upgrade()?,
            composer_inline: self.composer_inline.upgrade()?,
            composer_expand: self.composer_expand.upgrade()?,
            composer_signature: self.composer_signature.upgrade()?,
            composer_send: self.composer_send.upgrade()?,
            composer_hide: self.composer_hide.upgrade()?,
            composer_discard: self.composer_discard.upgrade()?,
            composer_refresh: self.composer_refresh.upgrade()?,
            composer_progress: self.composer_progress.upgrade()?,
            composer_draft_status: self.composer_draft_status.upgrade()?,
            composer_error: self.composer_error.upgrade()?,
            composer_message_id: self.composer_message_id.upgrade()?,
            composer_inline_ids: self.composer_inline_ids.upgrade()?,
            composer_attachment_fingerprint: self.composer_attachment_fingerprint.upgrade()?,
            last_list_revision: self.last_list_revision.upgrade()?,
            last_reader_revision: self.last_reader_revision.upgrade()?,
            attachment_progress: self.attachment_progress.upgrade()?,
            filter_buttons: self
                .filter_buttons
                .iter()
                .map(|(filter, button)| Some((*filter, button.upgrade()?)))
                .collect::<Option<Vec<_>>>()?,
            worker: self.worker.clone(),
            authorization: self.authorization.upgrade()?,
            background_sync_timer: self.background_sync_timer.upgrade()?,
            application: self.application.clone(),
        })
    }
}

fn parse_recipients(value: &str) -> Vec<crate::composer::Recipient> {
    value
        .split([',', ';'])
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            let (name, email) = part
                .rsplit_once('<')
                .and_then(|(name, email)| email.strip_suffix('>').map(|email| (name, email)))
                .map_or((None, part), |(name, email)| {
                    let name = name.trim().trim_matches('"');
                    ((!name.is_empty()).then(|| name.to_owned()), email.trim())
                });
            Some(crate::composer::Recipient {
                name,
                email: email.to_owned(),
            })
        })
        .collect()
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

fn new_unread_notification_body(count: usize) -> String {
    format!(
        "{count} new unread message{}",
        if count == 1 { "" } else { "s" }
    )
}

#[cfg(test)]
mod tests {
    use super::new_unread_notification_body;

    #[test]
    fn unread_notification_body_is_count_only() {
        assert_eq!(new_unread_notification_body(1), "1 new unread message");
        assert_eq!(new_unread_notification_body(3), "3 new unread messages");
    }
}
