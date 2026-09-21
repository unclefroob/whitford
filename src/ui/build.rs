use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use adw::prelude::*;
use gtk::glib::value::ToValue;

use super::{MessageListItem, Ui, widgets::message_row};
use crate::{
    cache::AppearancePreference,
    state::{Action, AppState, MessageFilter},
};

pub(super) fn build(
    application: &adw::Application,
    state: Rc<RefCell<AppState>>,
    worker: tokio::sync::mpsc::UnboundedSender<crate::worker::WorkerCommand>,
    authorization: Rc<
        RefCell<Option<(crate::worker::OperationId, crate::oauth::AuthorizationUrl)>>,
    >,
) -> Ui {
    let background_sync_timer = Rc::new(RefCell::new(None::<super::BackgroundSyncTimer>));
    let folders = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["whitford-folder-list"])
        .build();
    let message_model = gtk::gio::ListStore::new::<gtk::glib::BoxedAnyObject>();
    let message_selection = gtk::NoSelection::new(Some(message_model.clone()));
    let message_factory = gtk::SignalListItemFactory::new();
    message_factory.connect_bind(|_, object| {
        let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(object) = list_item.item().and_downcast::<gtk::glib::BoxedAnyObject>() else {
            return;
        };
        let item = object.borrow::<MessageListItem>();
        list_item.set_child(Some(&message_row(&item.message, item.selected)));
    });
    message_factory.connect_unbind(|_, object| {
        if let Some(list_item) = object.downcast_ref::<gtk::ListItem>() {
            list_item.set_child(None::<&gtk::Widget>);
        }
    });
    let messages = gtk::ListView::builder()
        .model(&message_selection)
        .factory(&message_factory)
        .single_click_activate(true)
        .css_classes(["whitford-message-list"])
        .build();
    let message_status = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .vexpand(true)
        .visible(false)
        .build();
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search sender or subject")
        .hexpand(true)
        .css_classes(["whitford-search"])
        .build();
    search.update_property(&[
        gtk::accessible::Property::Label("Search sender or subject"),
        gtk::accessible::Property::KeyShortcuts("Control+F"),
    ]);

    let list_menu = sidebar_button();
    let reader_menu = sidebar_button();
    let folder_pane = build_folder_pane(&folders);
    let settings = build_settings_page();
    let (message_page, list_header, filter_buttons, list_banner) =
        build_message_page(&messages, &message_status, &search, &list_menu);
    let (reader_page, reader, reader_banner, label_menu) = build_reader_page(&reader_menu);

    let inner = adw::NavigationSplitView::builder()
        .sidebar(&message_page)
        .content(&reader_page)
        .sidebar_width_fraction(0.37)
        .min_sidebar_width(320.0)
        .max_sidebar_width(520.0)
        .sidebar_width_unit(adw::LengthUnit::Sp)
        .build();
    let outer = adw::OverlaySplitView::builder()
        .sidebar(&folder_pane)
        .content(&inner)
        .sidebar_width_fraction(0.18)
        .min_sidebar_width(220.0)
        .max_sidebar_width(280.0)
        .sidebar_width_unit(adw::LengthUnit::Sp)
        .enable_show_gesture(true)
        .enable_hide_gesture(true)
        .build();

    let mail_page = adw::NavigationPage::builder()
        .child(&outer)
        .tag("mail")
        .title("Whitford")
        .can_pop(false)
        .build();
    let navigation = adw::NavigationView::new();
    navigation.add(&mail_page);
    navigation.add(&settings.page);
    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&navigation));
    let window = adw::ApplicationWindow::builder()
        .application(application)
        .title("Whitford")
        .default_width(1440)
        .default_height(900)
        .content(&toast_overlay)
        .css_classes(["whitford-window"])
        .build();
    let composer = build_composer(&window);
    window.set_size_request(600, 560);
    add_breakpoints(
        &window,
        &outer,
        &inner,
        &[list_menu, reader_menu],
        &list_header,
    );
    let shutdown = worker.clone();
    let timer_on_destroy = background_sync_timer.clone();
    window.connect_destroy(move |_| {
        if let Some(timer) = timer_on_destroy.borrow_mut().take() {
            timer.source.remove();
        }
        let _ = shutdown.send(crate::worker::WorkerCommand::Shutdown);
    });

    Ui {
        window,
        state,
        folders,
        messages,
        message_model,
        message_status,
        list_banner,
        search,
        reader,
        reader_banner,
        label_menu,
        outer,
        inner,
        navigation,
        toast_overlay,
        sync_title: settings.sync_title,
        sync_detail: settings.sync_detail,
        cache_limit: settings.cache_limit,
        cache_usage: settings.cache_usage,
        appearance: settings.appearance,
        account_email: settings.account_email,
        sync_refresh: settings.sync_refresh,
        sync_retry: settings.sync_retry,
        sync_reopen: settings.sync_reopen,
        sync_cancel: settings.sync_cancel,
        account_connect: settings.account_connect,
        account_disconnect: settings.account_disconnect,
        composer_window: composer.window,
        composer_to: composer.to,
        composer_cc: composer.cc,
        composer_bcc: composer.bcc,
        composer_cc_bcc: composer.cc_bcc,
        composer_subject: composer.subject,
        composer_editor: composer.editor,
        composer_attachments: composer.attachments,
        composer_attachment_total: composer.attachment_total,
        composer_attach: composer.attach,
        composer_inline: composer.inline,
        composer_expand: composer.expand,
        composer_signature: composer.signature,
        composer_send: composer.send,
        composer_hide: composer.hide,
        composer_discard: composer.discard,
        composer_refresh: composer.refresh,
        composer_progress: composer.progress,
        composer_draft_status: composer.draft_status,
        composer_error: composer.error,
        composer_message_id: Rc::new(RefCell::new(None)),
        composer_inline_ids: Rc::new(RefCell::new(Vec::new())),
        composer_attachment_fingerprint: Rc::new(Cell::new(u64::MAX)),
        last_list_revision: Rc::new(Cell::new(u64::MAX)),
        last_reader_revision: Rc::new(Cell::new(u64::MAX)),
        attachment_progress: Rc::new(RefCell::new(std::collections::HashMap::new())),
        filter_buttons,
        worker,
        authorization,
        background_sync_timer,
        application: application.downgrade(),
    }
}

struct ComposerWidgets {
    window: adw::Window,
    to: gtk::Entry,
    cc: gtk::Entry,
    bcc: gtk::Entry,
    cc_bcc: gtk::Box,
    subject: gtk::Entry,
    editor: super::composer_editor::ComposerEditor,
    attachments: gtk::Box,
    attachment_total: gtk::Label,
    attach: gtk::Button,
    inline: gtk::Button,
    expand: gtk::Button,
    signature: gtk::Button,
    send: gtk::Button,
    hide: gtk::Button,
    discard: gtk::Button,
    refresh: gtk::Button,
    progress: gtk::Spinner,
    draft_status: gtk::Label,
    error: gtk::Label,
}

fn build_composer(window: &adw::ApplicationWindow) -> ComposerWidgets {
    let to = recipient_entry("To", "Recipients, separated by commas");
    let cc = recipient_entry("Cc", "Carbon copy recipients");
    let bcc = recipient_entry("Bcc", "Blind carbon copy recipients");
    let subject = gtk::Entry::builder().placeholder_text("Subject").build();
    subject.update_property(&[gtk::accessible::Property::Label("Subject")]);
    let editor = super::composer_editor::ComposerEditor::new();
    let toolbar = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(4)
        .css_classes(["whitford-composer-toolbar"])
        .build();
    for (icon, label, command) in [
        ("edit-undo-symbolic", "Undo", "undo"),
        ("edit-redo-symbolic", "Redo", "redo"),
        ("format-text-bold-symbolic", "Bold", "bold"),
        ("format-text-italic-symbolic", "Italic", "italic"),
        ("format-text-underline-symbolic", "Underline", "underline"),
        (
            "view-list-bullet-symbolic",
            "Bulleted list",
            "insertUnorderedList",
        ),
        (
            "view-list-ordered-symbolic",
            "Numbered list",
            "insertOrderedList",
        ),
        ("format-indent-more-symbolic", "Quote", "formatBlock"),
        (
            "edit-clear-all-symbolic",
            "Remove formatting",
            "removeFormat",
        ),
    ] {
        let button = gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(label)
            .build();
        button.update_property(&[gtk::accessible::Property::Label(label)]);
        let editor_copy = editor.clone();
        button.connect_clicked(move |_| {
            editor_copy.command(command, (command == "formatBlock").then_some("blockquote"))
        });
        toolbar.append(&button);
    }
    let link = gtk::Button::builder()
        .icon_name("insert-link-symbolic")
        .tooltip_text("Insert link")
        .build();
    link.update_property(&[gtk::accessible::Property::Label("Insert link")]);
    let editor_copy = editor.clone();
    link.connect_clicked(move |_| editor_copy.command("promptLink", None));
    toolbar.append(&link);
    for (text, label, command) in [
        ("Quote", "Show or hide quoted message", "toggleQuote"),
        ("Remove quote", "Remove quoted message", "removeQuote"),
    ] {
        let button = gtk::Button::with_label(text);
        button.set_tooltip_text(Some(label));
        button.update_property(&[gtk::accessible::Property::Label(label)]);
        let editor_copy = editor.clone();
        button.connect_clicked(move |_| editor_copy.command(command, None));
        toolbar.append(&button);
    }
    let signature = gtk::Button::with_label("Signature");
    signature.set_tooltip_text(Some("Configure account signature"));
    signature.update_property(&[gtk::accessible::Property::Label(
        "Configure account signature",
    )]);
    toolbar.append(&signature);
    let attach = gtk::Button::builder()
        .icon_name("mail-attachment-symbolic")
        .tooltip_text("Attach files")
        .build();
    attach.update_property(&[gtk::accessible::Property::Label("Attach files")]);
    toolbar.append(&attach);
    let inline = gtk::Button::builder()
        .icon_name("insert-image-symbolic")
        .tooltip_text("Insert inline image")
        .build();
    inline.update_property(&[gtk::accessible::Property::Label("Insert inline image")]);
    toolbar.append(&inline);
    let toolbar_scroll = gtk::ScrolledWindow::builder()
        .child(&toolbar)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .build();
    let attachments = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    let attachment_total = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["caption", "dim-label"])
        .build();
    let scroll = gtk::ScrolledWindow::builder()
        .child(&editor.view)
        .vexpand(true)
        .min_content_height(260)
        .build();
    let send = gtk::Button::builder()
        .label("Send")
        .css_classes(["suggested-action"])
        .build();
    let hide = gtk::Button::with_label("Save & Close");
    let discard = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Discard draft")
        .build();
    discard.update_property(&[gtk::accessible::Property::Label("Discard draft")]);
    let expand = gtk::Button::builder()
        .icon_name("view-fullscreen-symbolic")
        .tooltip_text("Expand composer")
        .build();
    expand.update_property(&[gtk::accessible::Property::Label("Expand composer")]);
    let refresh = gtk::Button::with_label("Refresh Gmail");
    let progress = gtk::Spinner::builder().visible(false).build();
    progress.update_property(&[gtk::accessible::Property::Label("Sending message")]);
    let draft_status = gtk::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["caption", "dim-label"])
        .build();
    let error = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .focusable(true)
        .css_classes(["error"])
        .build();
    let actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .build();
    actions.append(&progress);
    actions.append(&draft_status);
    actions.append(&discard);
    actions.append(&expand);
    actions.append(&refresh);
    actions.append(&hide);
    actions.append(&send);
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    content.append(
        &gtk::Label::builder()
            .label("Compose message")
            .xalign(0.0)
            .css_classes(["title-1"])
            .build(),
    );
    let recipients = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .build();
    to.set_hexpand(true);
    recipients.append(&to);
    let disclose = gtk::Button::with_label("Cc/Bcc");
    disclose.set_tooltip_text(Some("Show or hide Cc and Bcc"));
    recipients.append(&disclose);
    let cc_bcc = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .visible(false)
        .build();
    cc_bcc.append(&cc);
    cc_bcc.append(&bcc);
    let cc_bcc_copy = cc_bcc.clone();
    disclose.connect_clicked(move |_| cc_bcc_copy.set_visible(!cc_bcc_copy.is_visible()));
    content.append(&recipients);
    content.append(&cc_bcc);
    content.append(&subject);
    content.append(&toolbar_scroll);
    content.append(&scroll);
    content.append(&attachments);
    content.append(&attachment_total);
    content.append(&error);
    content.append(&actions);
    let content_scroll = gtk::ScrolledWindow::builder()
        .child(&content)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_width(true)
        .build();
    let composer = adw::Window::builder()
        .title("Compose — Whitford")
        .default_width(560)
        .default_height(520)
        .modal(false)
        .transient_for(window)
        .content(&content_scroll)
        .build();
    ComposerWidgets {
        window: composer,
        to,
        cc,
        bcc,
        cc_bcc,
        subject,
        editor,
        attachments,
        attachment_total,
        attach,
        inline,
        expand,
        signature,
        send,
        hide,
        discard,
        refresh,
        progress,
        draft_status,
        error,
    }
}

fn recipient_entry(placeholder: &str, label: &str) -> gtk::Entry {
    let entry = gtk::Entry::builder().placeholder_text(placeholder).build();
    entry.update_property(&[gtk::accessible::Property::Label(label)]);
    entry
}

pub(super) fn connect_signals(ui: &Ui) {
    for entry in [&ui.composer_to, &ui.composer_cc, &ui.composer_bcc] {
        entry.connect_changed({
            let weak_ui = ui.downgrade();
            move |_| {
                if let Some(ui) = weak_ui.upgrade() {
                    ui.flush_recipients();
                }
            }
        });
    }
    ui.composer_subject.connect_changed({
        let weak_ui = ui.downgrade();
        move |entry| {
            if let Some(ui) = weak_ui.upgrade() {
                ui.dispatch(Action::UpdateSubject(entry.text().to_string()));
            }
        }
    });
    ui.composer_send.connect_clicked({
        let weak_ui = ui.downgrade();
        move |_| {
            if let Some(ui) = weak_ui.upgrade() {
                ui.send_message();
            }
        }
    });
    ui.composer_hide.connect_clicked({
        let weak_ui = ui.downgrade();
        move |_| {
            if let Some(ui) = weak_ui.upgrade() {
                ui.request_close_composer();
            }
        }
    });
    ui.composer_discard.connect_clicked({
        let weak_ui = ui.downgrade();
        move |_| {
            let Some(ui) = weak_ui.upgrade() else { return };
            let dialog = adw::AlertDialog::builder()
                .heading("Discard this draft?")
                .body("The message and staged attachments will be removed from this device.")
                .build();
            dialog.add_response("keep", "Keep Draft");
            dialog.add_response("discard", "Discard");
            dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
            let weak = ui.downgrade();
            dialog.choose(
                Some(&ui.composer_window),
                None::<&gtk::gio::Cancellable>,
                move |response| {
                    if response == "discard"
                        && let Some(ui) = weak.upgrade()
                    {
                        ui.dispatch(Action::DiscardDraft);
                    }
                },
            );
        }
    });
    ui.composer_expand.connect_clicked({
        let window = ui.composer_window.clone();
        move |_| {
            if window.width() < 800 {
                window.set_default_size(900, 760)
            } else {
                window.set_default_size(620, 600)
            }
        }
    });
    ui.composer_signature.connect_clicked({
        let weak_ui = ui.downgrade();
        move |_| {
            let Some(ui) = weak_ui.upgrade() else { return };
            let current = ui.state.borrow().snapshot().signature;
            let enabled = gtk::CheckButton::with_label("Add signature to new replies and forwards");
            enabled.set_active(current.enabled);
            let value = gtk::TextView::builder()
                .wrap_mode(gtk::WrapMode::WordChar)
                .height_request(120)
                .build();
            value
                .buffer()
                .set_text(&crate::composer::html_to_plain(&current.html));
            value.update_property(&[gtk::accessible::Property::Label("Signature text")]);
            let fields = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(12)
                .build();
            fields.append(&enabled);
            fields.append(
                &gtk::ScrolledWindow::builder()
                    .child(&value)
                    .min_content_height(120)
                    .build(),
            );
            let dialog = adw::AlertDialog::builder()
                .heading("Account signature")
                .body("Stored privately on this device and added to new drafts.")
                .extra_child(&fields)
                .build();
            dialog.add_response("cancel", "Cancel");
            dialog.add_response("save", "Save");
            dialog.set_default_response(Some("save"));
            let weak = ui.downgrade();
            dialog.choose(
                Some(&ui.composer_window),
                None::<&gtk::gio::Cancellable>,
                move |response| {
                    if response == "save"
                        && let Some(ui) = weak.upgrade()
                    {
                        ui.dispatch(Action::UpdateSignature {
                            html: crate::composer::plain_to_html(&value.buffer().text(
                                &value.buffer().start_iter(),
                                &value.buffer().end_iter(),
                                false,
                            )),
                            enabled: enabled.is_active(),
                        });
                    }
                },
            );
        }
    });
    ui.composer_attach.connect_clicked({
        let weak_ui = ui.downgrade();
        move |_| {
            let Some(ui) = weak_ui.upgrade() else { return };
            let dialog = gtk::FileDialog::builder().title("Attach files").build();
            let weak = ui.downgrade();
            dialog.open_multiple(
                Some(&ui.composer_window),
                None::<&gtk::gio::Cancellable>,
                move |result| {
                    let Ok(files) = result else { return };
                    let Some(ui) = weak.upgrade() else { return };
                    for index in 0..files.n_items() {
                        let Some(file) = files.item(index).and_downcast::<gtk::gio::File>() else {
                            continue;
                        };
                        let Some(path) = file.path() else { continue };
                        let display_name = path
                            .file_name()
                            .map(|v| v.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "attachment".into());
                        let media_type = media_type_for_path(&path).to_owned();
                        ui.dispatch(Action::StageAttachment {
                            source: path,
                            display_name,
                            media_type,
                        });
                    }
                },
            );
        }
    });
    ui.composer_inline.connect_clicked({
        let weak_ui = ui.downgrade();
        move |_| {
            let Some(ui) = weak_ui.upgrade() else { return };
            let dialog = gtk::FileDialog::builder()
                .title("Insert inline image")
                .build();
            let weak = ui.downgrade();
            dialog.open(
                Some(&ui.composer_window),
                None::<&gtk::gio::Cancellable>,
                move |result| {
                    let Ok(file) = result else { return };
                    let Some(path) = file.path() else { return };
                    let Some(ui) = weak.upgrade() else { return };
                    let display_name = path
                        .file_name()
                        .map(|v| v.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "image".into());
                    let media_type = media_type_for_path(&path).to_owned();
                    if !media_type.starts_with("image/") {
                        ui.toast("Choose a PNG, JPEG, GIF, or WebP image");
                        return;
                    }
                    ui.dispatch(Action::StageInlineImage {
                        source: path,
                        display_name,
                        media_type,
                    });
                },
            );
        }
    });
    let file_drop = gtk::DropTarget::new(gtk::gio::File::static_type(), gtk::gdk::DragAction::COPY);
    file_drop.connect_drop({
        let weak_ui = ui.downgrade();
        move |_, value, _, _| {
            let Ok(file) = value.get::<gtk::gio::File>() else {
                return false;
            };
            let Some(path) = file.path() else {
                return false;
            };
            let Some(ui) = weak_ui.upgrade() else {
                return false;
            };
            let display_name = path
                .file_name()
                .map(|v| v.to_string_lossy().into_owned())
                .unwrap_or_else(|| "attachment".into());
            let media_type = media_type_for_path(&path).to_owned();
            ui.dispatch(Action::StageAttachment {
                source: path,
                display_name,
                media_type,
            });
            true
        }
    });
    ui.composer_editor.view.add_controller(file_drop);
    ui.composer_editor
        .manager
        .connect_script_message_received(Some("changed"), {
            let weak_ui = ui.downgrade();
            move |_, value| {
                let Some(ui) = weak_ui.upgrade() else { return };
                let raw = value.to_str();
                if raw.len() > crate::composer::MAX_HTML_BYTES.saturating_mul(4) {
                    ui.toast("Message body is too large");
                    return;
                }
                if let Ok((html, text)) = serde_json::from_str::<(String, String)>(&raw) {
                    if html.len() > crate::composer::MAX_HTML_BYTES
                        || text.len() > crate::composer::MAX_HTML_BYTES
                    {
                        ui.toast("Message body is too large");
                        return;
                    }
                    ui.dispatch(Action::UpdateHtml { html, text });
                }
            }
        });
    ui.composer_refresh.connect_clicked({
        let weak_ui = ui.downgrade();
        move |_| {
            if let Some(ui) = weak_ui.upgrade() {
                ui.dispatch(Action::Refresh);
            }
        }
    });
    let composer_keys = gtk::EventControllerKey::new();
    composer_keys.connect_key_pressed({
        let weak_ui = ui.downgrade();
        move |_, key, _, modifiers| {
            let Some(ui) = weak_ui.upgrade() else {
                return gtk::glib::Propagation::Proceed;
            };
            if key == gtk::gdk::Key::Escape {
                ui.request_close_composer();
                gtk::glib::Propagation::Stop
            } else if key == gtk::gdk::Key::Return
                && modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK)
            {
                ui.send_message();
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        }
    });
    composer_keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    ui.composer_window.add_controller(composer_keys);
    ui.window.connect_close_request({
        let weak_ui = ui.downgrade();
        move |_| {
            let Some(ui) = weak_ui.upgrade() else {
                return gtk::glib::Propagation::Proceed;
            };
            if matches!(
                ui.state.borrow().snapshot().composer,
                crate::state::ComposerState::Closed
            ) {
                gtk::glib::Propagation::Proceed
            } else {
                ui.save_composer_then_close_app();
                gtk::glib::Propagation::Stop
            }
        }
    });
    ui.composer_window.connect_close_request({
        let weak_ui = ui.downgrade();
        move |_| {
            if let Some(ui) = weak_ui.upgrade() {
                ui.request_close_composer();
            }
            gtk::glib::Propagation::Stop
        }
    });
    ui.search.connect_search_changed({
        let weak_ui = ui.downgrade();
        move |entry| {
            if let Some(ui) = weak_ui.upgrade() {
                ui.dispatch(Action::SetSearch(entry.text().to_string()));
            }
        }
    });
    ui.search.connect_activate({
        let weak_ui = ui.downgrade();
        move |_| {
            if let Some(ui) = weak_ui.upgrade() {
                ui.dispatch(Action::SubmitServerSearch);
            }
        }
    });
    for (filter, button) in &ui.filter_buttons {
        let filter = *filter;
        let weak_ui = ui.downgrade();
        button.connect_clicked(move |_| {
            if let Some(ui) = weak_ui.upgrade() {
                ui.dispatch(Action::SetFilter(filter));
            }
        });
    }
    ui.messages.connect_activate({
        let weak_ui = ui.downgrade();
        move |_, position| {
            let Some(ui) = weak_ui.upgrade() else {
                return;
            };
            let Some(object) = ui
                .message_model
                .item(position)
                .and_downcast::<gtk::glib::BoxedAnyObject>()
            else {
                return;
            };
            let id = object.borrow::<MessageListItem>().message.id.clone();
            ui.dispatch(Action::SelectMessage(id));
            if ui.inner.is_collapsed() {
                ui.inner.set_show_content(true);
            }
        }
    });
    ui.cache_limit.connect_selected_notify({
        let weak_ui = ui.downgrade();
        move |dropdown| {
            if let Some(ui) = weak_ui.upgrade()
                && let Some(limit) = crate::cache::limit_at(dropdown.selected())
                && ui.state.borrow().snapshot().cache_limit != limit
            {
                ui.dispatch(Action::SetCacheLimit(limit));
            }
        }
    });
    ui.appearance.connect_selected_notify({
        let weak_ui = ui.downgrade();
        move |dropdown| {
            let preference = match dropdown.selected() {
                0 => AppearancePreference::System,
                1 => AppearancePreference::Light,
                2 => AppearancePreference::Dark,
                _ => return,
            };
            if let Some(ui) = weak_ui.upgrade()
                && ui.state.borrow().snapshot().appearance != preference
            {
                ui.dispatch(Action::SetAppearance(preference));
            }
        }
    });
}

fn media_type_for_path(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("pdf") => "application/pdf",
        Some("txt") => "text/plain",
        Some("html" | "htm") => "text/html",
        _ => "application/octet-stream",
    }
}

fn build_folder_pane(folders: &gtk::ListBox) -> gtk::Box {
    let pane = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["whitford-folder-pane"])
        .build();
    let account = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .margin_top(24)
        .margin_start(24)
        .margin_end(24)
        .margin_bottom(18)
        .build();
    let name = gtk::Label::builder()
        .label("Whitford")
        .xalign(0.0)
        .css_classes(["title-3"])
        .build();
    let address = gtk::Label::builder()
        .label("Gmail, made quiet")
        .xalign(0.0)
        .css_classes(["dim-label"])
        .build();
    account.append(&name);
    account.append(&address);

    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .child(folders)
        .build();
    pane.append(&account);
    pane.append(
        &gtk::Button::builder()
            .label("Compose")
            .icon_name("mail-message-new-symbolic")
            .action_name("win.compose")
            .tooltip_text("Write a new message (Ctrl+N)")
            .margin_start(16)
            .margin_end(16)
            .margin_bottom(12)
            .css_classes(["suggested-action", "whitford-compose"])
            .build(),
    );
    pane.append(&scroll);
    pane.append(
        &gtk::Button::builder()
            .label("Settings")
            .icon_name("emblem-system-symbolic")
            .action_name("win.open-settings")
            .tooltip_text("Open settings (Ctrl+,)")
            .margin_start(12)
            .margin_end(12)
            .margin_top(10)
            .margin_bottom(14)
            .build(),
    );
    pane
}

struct SettingsWidgets {
    page: adw::NavigationPage,
    sync_title: gtk::Label,
    sync_detail: adw::ActionRow,
    cache_limit: gtk::DropDown,
    cache_usage: gtk::Label,
    appearance: gtk::DropDown,
    account_email: gtk::Label,
    sync_refresh: adw::ActionRow,
    sync_retry: adw::ActionRow,
    sync_reopen: adw::ActionRow,
    sync_cancel: adw::ActionRow,
    account_connect: adw::ActionRow,
    account_disconnect: adw::ActionRow,
}

fn build_settings_page() -> SettingsWidgets {
    let preferences = adw::PreferencesPage::new();
    preferences.set_title("Settings");

    let appearance_group = adw::PreferencesGroup::builder()
        .title("Appearance")
        .description("Choose how Whitford follows your desktop color scheme.")
        .build();
    let appearance = gtk::DropDown::from_strings(&["System default", "Light", "Dark"]);
    appearance.update_property(&[gtk::accessible::Property::Label("Color scheme")]);
    let appearance_row = adw::ActionRow::builder()
        .title("Color scheme")
        .subtitle("System default follows the desktop automatically")
        .activatable_widget(&appearance)
        .build();
    appearance_row.add_suffix(&appearance);
    appearance_group.add(&appearance_row);
    preferences.add(&appearance_group);

    let sync_group = adw::PreferencesGroup::builder()
        .title("Synchronization")
        .build();
    let sync_title = gtk::Label::builder()
        .xalign(1.0)
        .css_classes(["whitford-online"])
        .build();
    let sync_row = adw::ActionRow::builder().title("Gmail status").build();
    sync_row.add_suffix(&sync_title);
    sync_group.add(&sync_row);
    let sync_detail = adw::ActionRow::builder()
        .title("Sync details")
        .subtitle_lines(3)
        .build();
    sync_group.add(&sync_detail);
    let sync_refresh = setting_action_row("Refresh now", "win.refresh");
    let sync_retry = setting_action_row("Retry", "win.retry");
    let sync_reopen = setting_action_row("Reopen browser", "win.reopen-authorization");
    let sync_cancel = setting_action_row("Cancel sign-in", "win.cancel-authorization");
    for row in [&sync_refresh, &sync_retry, &sync_reopen, &sync_cancel] {
        sync_group.add(row);
    }
    preferences.add(&sync_group);

    let storage_group = adw::PreferencesGroup::builder().title("Storage").build();
    let cache_limit = gtk::DropDown::from_strings(&[
        "50 messages",
        "100 messages",
        "250 messages",
        "500 messages",
    ]);
    cache_limit.update_property(&[gtk::accessible::Property::Label(
        "Number of messages to keep locally",
    )]);
    let limit_row = adw::ActionRow::builder()
        .title("Keep summaries")
        .activatable_widget(&cache_limit)
        .build();
    limit_row.add_suffix(&cache_limit);
    storage_group.add(&limit_row);
    let cache_usage = gtk::Label::builder()
        .xalign(1.0)
        .css_classes(["dim-label"])
        .build();
    let usage_row = adw::ActionRow::builder().title("Local cache").build();
    usage_row.add_suffix(&cache_usage);
    storage_group.add(&usage_row);
    storage_group.add(
        &adw::ActionRow::builder()
            .title("Clear cache")
            .subtitle("Remove downloaded mail from this device")
            .activatable(true)
            .action_name("win.clear-cache")
            .build(),
    );
    preferences.add(&storage_group);

    let account_group = adw::PreferencesGroup::builder().title("Account").build();
    let account_email = gtk::Label::builder()
        .label("Not connected")
        .xalign(1.0)
        .css_classes(["dim-label"])
        .build();
    let account_row = adw::ActionRow::builder().title("Gmail account").build();
    account_row.add_suffix(&account_email);
    account_group.add(&account_row);
    let account_connect = setting_action_row("Connect Gmail", "win.connect");
    let account_disconnect = setting_action_row("Disconnect Gmail", "win.disconnect");
    account_disconnect.set_subtitle("Remove local authorization and mail data");
    account_group.add(&account_connect);
    account_group.add(&account_disconnect);
    preferences.add(&account_group);

    let privacy_group = adw::PreferencesGroup::builder()
        .title("About &amp; Privacy")
        .description("Whitford stores its local cache, drafts, and authorization data privately on this device. Gmail access can be revoked from your Google Account.")
        .build();
    privacy_group.add(
        &adw::ActionRow::builder()
            .title("Whitford")
            .subtitle("A lightweight native Gmail client for Wayland")
            .build(),
    );
    preferences.add(&privacy_group);

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_show_title(true);
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&preferences));

    SettingsWidgets {
        page: adw::NavigationPage::builder()
            .child(&toolbar)
            .tag("settings")
            .title("Settings")
            .build(),
        sync_title,
        sync_detail,
        cache_limit,
        cache_usage,
        appearance,
        account_email,
        sync_refresh,
        sync_retry,
        sync_reopen,
        sync_cancel,
        account_connect,
        account_disconnect,
    }
}

fn setting_action_row(title: &str, action_name: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(title)
        .activatable(true)
        .action_name(action_name)
        .build()
}

fn build_message_page(
    messages: &gtk::ListView,
    message_status: &gtk::Box,
    search: &gtk::SearchEntry,
    menu: &gtk::Button,
) -> (
    adw::NavigationPage,
    adw::HeaderBar,
    Vec<(MessageFilter, gtk::Button)>,
    gtk::Box,
) {
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_show_title(false);
    header.set_visible(false);
    header.pack_start(menu);
    toolbar.add_top_bar(&header);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    let search_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    let search_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .build();
    search.set_hexpand(true);
    search_row.append(search);
    search_row.append(
        &gtk::Button::builder()
            .label("Search Gmail")
            .action_name("win.search-gmail")
            .tooltip_text("Search all Gmail mail (Enter)")
            .build(),
    );
    search_box.append(&search_row);
    let filters = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(22)
        .margin_start(18)
        .margin_end(18)
        .margin_top(8)
        .margin_bottom(6)
        .build();
    filters.set_accessible_role(gtk::AccessibleRole::RadioGroup);
    let filter_buttons = [
        (MessageFilter::All, "All"),
        (MessageFilter::Unread, "Unread"),
        (MessageFilter::Attachments, "Attachments"),
    ]
    .into_iter()
    .map(|(filter, text)| {
        let button = gtk::Button::builder()
            .label(text)
            .has_frame(false)
            .css_classes(["whitford-filter"])
            .build();
        button.set_accessible_role(gtk::AccessibleRole::Radio);
        button.update_property(&[gtk::accessible::Property::Label(&format!(
            "Show {text} messages"
        ))]);
        filters.append(&button);
        (filter, button)
    })
    .collect::<Vec<_>>();
    search_box.append(&filters);
    content.append(&search_box);
    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let list_banner = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .visible(false)
        .build();
    content.append(&list_banner);
    content.append(message_status);
    content.append(
        &gtk::ScrolledWindow::builder()
            .vexpand(true)
            .child(messages)
            .build(),
    );
    toolbar.set_content(Some(&content));
    (
        adw::NavigationPage::with_tag(&toolbar, "Messages", "messages"),
        header,
        filter_buttons,
        list_banner,
    )
}

fn build_reader_page(
    menu: &gtk::Button,
) -> (adw::NavigationPage, gtk::Box, gtk::Box, gtk::gio::Menu) {
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_show_title(false);
    header.pack_start(menu);
    for (icon, tooltip, action) in [
        ("mail-mark-read-symbolic", "Mark read", "win.mark-read"),
        ("user-trash-symbolic", "Delete", "win.delete"),
        ("mail-archive-symbolic", "Archive (Delete)", "win.archive"),
    ] {
        header.pack_start(&icon_button(icon, tooltip, action));
    }
    let label_menu = gtk::gio::Menu::new();
    let label_button = gtk::MenuButton::builder()
        .icon_name("tag-symbolic")
        .tooltip_text("Apply or remove label")
        .menu_model(&label_menu)
        .build();
    header.pack_start(&label_button);
    header.pack_end(&icon_button(
        "go-next-symbolic",
        "Next message",
        "win.message-next",
    ));
    header.pack_end(&icon_button(
        "go-previous-symbolic",
        "Previous message",
        "win.message-previous",
    ));
    toolbar.add_top_bar(&header);
    let reader = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(20)
        .margin_start(32)
        .margin_end(32)
        .margin_top(24)
        .margin_bottom(32)
        .css_classes(["whitford-reader"])
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    let reader_banner = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .visible(false)
        .build();
    content.append(&reader_banner);
    content.append(
        &gtk::ScrolledWindow::builder()
            .vexpand(true)
            .child(&reader)
            .build(),
    );
    toolbar.set_content(Some(&content));
    (
        adw::NavigationPage::with_tag(&toolbar, "Message", "reader"),
        reader,
        reader_banner,
        label_menu,
    )
}

fn sidebar_button() -> gtk::Button {
    let button = icon_button(
        "sidebar-show-symbolic",
        "Show folders",
        "win.toggle-folders",
    );
    button.set_visible(false);
    button
}

fn icon_button(icon: &str, tooltip: &str, action: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .action_name(action)
        .css_classes(["flat", "circular"])
        .build();
    button.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    button
}

fn add_breakpoints(
    window: &adw::ApplicationWindow,
    outer: &adw::OverlaySplitView,
    inner: &adw::NavigationSplitView,
    menu_buttons: &[gtk::Button],
    list_header: &adw::HeaderBar,
) {
    let medium = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        1000.0,
        adw::LengthUnit::Sp,
    ));
    let true_value = true.to_value();
    medium.add_setter(outer, "collapsed", Some(&true_value));
    for button in menu_buttons {
        medium.add_setter(button, "visible", Some(&true_value));
    }
    medium.add_setter(list_header, "visible", Some(&true_value));
    window.add_breakpoint(medium);

    let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        650.0,
        adw::LengthUnit::Sp,
    ));
    narrow.add_setter(inner, "collapsed", Some(&true_value));
    window.add_breakpoint(narrow);
}
