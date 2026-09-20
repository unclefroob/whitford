use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use adw::prelude::*;
use gtk::glib::value::ToValue;

use super::Ui;
use crate::state::{Action, AppState, MessageFilter};

pub(super) fn build(
    application: &adw::Application,
    state: Rc<RefCell<AppState>>,
    worker: tokio::sync::mpsc::UnboundedSender<crate::worker::WorkerCommand>,
    authorization: Rc<
        RefCell<Option<(crate::worker::OperationId, crate::oauth::AuthorizationUrl)>>,
    >,
) -> Ui {
    let folders = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["whitford-folder-list"])
        .build();
    let messages = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["whitford-message-list"])
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
    let (folder_pane, sync_title, sync_detail, cache_limit, cache_usage) =
        build_folder_pane(&folders);
    let (message_page, list_header, filter_buttons, list_banner) =
        build_message_page(&messages, &search, &list_menu);
    let (reader_page, reader, reader_banner) = build_reader_page(&reader_menu);

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

    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&outer));
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
    window.connect_destroy(move |_| {
        let _ = shutdown.send(crate::worker::WorkerCommand::Shutdown);
    });

    Ui {
        window,
        state,
        folders,
        messages,
        list_banner,
        search,
        reader,
        reader_banner,
        outer,
        inner,
        toast_overlay,
        sync_title,
        sync_detail,
        cache_limit,
        cache_usage,
        composer_window: composer.0,
        composer_to: composer.1,
        composer_subject: composer.2,
        composer_body: composer.3,
        composer_send: composer.4,
        composer_cancel: composer.5,
        composer_refresh: composer.6,
        composer_progress: composer.7,
        composer_error: composer.8,
        composer_message_id: Rc::new(RefCell::new(None)),
        last_list_revision: Rc::new(Cell::new(u64::MAX)),
        last_reader_revision: Rc::new(Cell::new(u64::MAX)),
        filter_buttons,
        worker,
        authorization,
    }
}

#[allow(clippy::type_complexity)]
fn build_composer(
    window: &adw::ApplicationWindow,
) -> (
    adw::Window,
    gtk::Label,
    gtk::Label,
    gtk::TextView,
    gtk::Button,
    gtk::Button,
    gtk::Button,
    gtk::Spinner,
    gtk::Label,
) {
    let to = gtk::Label::builder()
        .xalign(0.0)
        .selectable(true)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .build();
    let subject = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["title-3"])
        .build();
    let body = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(12)
        .right_margin(12)
        .vexpand(true)
        .accepts_tab(false)
        .css_classes(["whitford-composer-body"])
        .build();
    body.update_property(&[gtk::accessible::Property::Label("Reply body")]);
    let scroll = gtk::ScrolledWindow::builder()
        .child(&body)
        .vexpand(true)
        .min_content_height(260)
        .build();
    let send = gtk::Button::builder()
        .label("Send")
        .css_classes(["suggested-action"])
        .build();
    let cancel = gtk::Button::with_label("Cancel");
    let refresh = gtk::Button::with_label("Refresh Gmail");
    let progress = gtk::Spinner::builder().visible(false).build();
    progress.update_property(&[gtk::accessible::Property::Label("Sending reply")]);
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
    actions.append(&refresh);
    actions.append(&cancel);
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
            .label("Reply")
            .xalign(0.0)
            .css_classes(["title-1"])
            .build(),
    );
    content.append(&to);
    content.append(&subject);
    content.append(&scroll);
    content.append(&error);
    content.append(&actions);
    let composer = adw::Window::builder()
        .title("Reply — Whitford")
        .default_width(560)
        .default_height(520)
        .modal(true)
        .transient_for(window)
        .content(&content)
        .build();
    (
        composer, to, subject, body, send, cancel, refresh, progress, error,
    )
}

pub(super) fn connect_signals(ui: &Ui) {
    ui.composer_send.connect_clicked({
        let weak_ui = ui.downgrade();
        move |_| {
            if let Some(ui) = weak_ui.upgrade() {
                ui.send_reply();
            }
        }
    });
    ui.composer_cancel.connect_clicked({
        let weak_ui = ui.downgrade();
        move |_| {
            if let Some(ui) = weak_ui.upgrade() {
                ui.request_close_composer();
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
                ui.send_reply();
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
                ui.composer_window.present();
                ui.request_close_composer();
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
    for (filter, button) in &ui.filter_buttons {
        let filter = *filter;
        let weak_ui = ui.downgrade();
        button.connect_clicked(move |_| {
            if let Some(ui) = weak_ui.upgrade() {
                ui.dispatch(Action::SetFilter(filter));
            }
        });
    }
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
}

fn build_folder_pane(
    folders: &gtk::ListBox,
) -> (gtk::Box, gtk::Label, gtk::Label, gtk::DropDown, gtk::Label) {
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
        .label("Gmail developer preview")
        .xalign(0.0)
        .css_classes(["title-3"])
        .build();
    let address = gtk::Label::builder()
        .label("Lightweight Gmail")
        .xalign(0.0)
        .css_classes(["dim-label"])
        .build();
    account.append(&name);
    account.append(&address);

    let connect = gtk::Button::builder()
        .label("Connect Gmail")
        .icon_name("network-server-symbolic")
        .action_name("win.connect")
        .tooltip_text("Authorize a Gmail account")
        .margin_start(16)
        .margin_end(16)
        .margin_bottom(18)
        .css_classes(["suggested-action", "whitford-compose"])
        .build();
    let account_actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .margin_start(16)
        .margin_end(16)
        .margin_bottom(18)
        .build();
    account_actions.append(
        &gtk::Button::builder()
            .label("Refresh")
            .action_name("win.refresh")
            .hexpand(true)
            .build(),
    );
    account_actions.append(
        &gtk::Button::builder()
            .label("Disconnect")
            .action_name("win.disconnect")
            .hexpand(true)
            .build(),
    );
    let recovery_actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .margin_start(16)
        .margin_end(16)
        .margin_bottom(18)
        .build();
    recovery_actions.append(
        &gtk::Button::builder()
            .label("Retry")
            .action_name("win.retry")
            .hexpand(true)
            .build(),
    );
    recovery_actions.append(
        &gtk::Button::builder()
            .label("Reopen Browser")
            .action_name("win.reopen-authorization")
            .hexpand(true)
            .build(),
    );
    recovery_actions.append(
        &gtk::Button::builder()
            .label("Cancel")
            .action_name("win.cancel-authorization")
            .hexpand(true)
            .build(),
    );
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .child(folders)
        .build();
    let cache_limit = gtk::DropDown::from_strings(&[
        "50 messages",
        "100 messages",
        "250 messages",
        "500 messages",
    ]);
    cache_limit.update_property(&[gtk::accessible::Property::Label(
        "Number of messages to keep locally",
    )]);
    let cache_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_start(24)
        .margin_end(24)
        .margin_top(12)
        .build();
    cache_row.append(
        &gtk::Label::builder()
            .label("Keep summaries")
            .xalign(0.0)
            .hexpand(true)
            .build(),
    );
    cache_row.append(&cache_limit);
    let cache_usage = gtk::Label::builder()
        .label("No downloaded messages")
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["dim-label", "caption"])
        .build();
    let clear_cache = gtk::Button::builder()
        .label("Clear Cache")
        .has_frame(false)
        .halign(gtk::Align::Start)
        .action_name("win.clear-cache")
        .build();
    let cache_actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_start(24)
        .margin_end(24)
        .margin_top(4)
        .build();
    cache_actions.append(&cache_usage);
    cache_actions.append(&clear_cache);
    let sync = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_start(24)
        .margin_end(24)
        .margin_top(18)
        .margin_bottom(22)
        .build();
    let sync_title = gtk::Label::builder()
        .label("Not connected")
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .css_classes(["whitford-online"])
        .build();
    let sync_detail = gtk::Label::builder()
        .label("Place google-oauth.json in the Whitford XDG config directory.")
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .css_classes(["dim-label", "caption"])
        .build();
    sync.append(&sync_title);
    sync.append(&sync_detail);
    pane.append(&account);
    pane.append(&connect);
    pane.append(&account_actions);
    pane.append(&recovery_actions);
    pane.append(&scroll);
    pane.append(&cache_row);
    pane.append(&cache_actions);
    pane.append(&sync);
    (pane, sync_title, sync_detail, cache_limit, cache_usage)
}

fn build_message_page(
    messages: &gtk::ListBox,
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
    search_box.append(search);
    let filters = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(22)
        .margin_start(18)
        .margin_end(18)
        .margin_top(8)
        .margin_bottom(6)
        .build();
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

fn build_reader_page(menu: &gtk::Button) -> (adw::NavigationPage, gtk::Box, gtk::Box) {
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_show_title(false);
    header.pack_start(menu);
    for (icon, tooltip, action) in [
        ("mail-mark-read-symbolic", "Mark read", "win.mark-read"),
        ("user-trash-symbolic", "Delete", "win.delete"),
        ("mail-archive-symbolic", "Archive (Delete)", "win.archive"),
        ("tag-symbolic", "Add label", "win.label"),
    ] {
        header.pack_start(&icon_button(icon, tooltip, action));
    }
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
