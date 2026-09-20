use std::{cell::RefCell, rc::Rc};

use adw::prelude::*;
use gtk::glib::value::ToValue;

use super::Ui;
use crate::state::{Action, AppState, MessageFilter};

pub(super) fn build(application: &adw::Application, state: Rc<RefCell<AppState>>) -> Ui {
    let folders = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["waymail-folder-list"])
        .build();
    let messages = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["waymail-message-list"])
        .build();
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search mail")
        .hexpand(true)
        .css_classes(["waymail-search"])
        .build();
    search.update_property(&[
        gtk::accessible::Property::Label("Search mail"),
        gtk::accessible::Property::KeyShortcuts("Control+F"),
    ]);

    let list_menu = sidebar_button();
    let reader_menu = sidebar_button();
    let (folder_pane, sync_title, sync_detail) = build_folder_pane(&folders);
    let (message_page, list_header, filter_buttons) =
        build_message_page(&messages, &search, &list_menu);
    let (reader_page, reader) = build_reader_page(&reader_menu);

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
        .title("Waymail")
        .default_width(1440)
        .default_height(900)
        .content(&toast_overlay)
        .css_classes(["waymail-window"])
        .build();
    window.set_size_request(600, 560);
    add_breakpoints(
        &window,
        &outer,
        &inner,
        &[list_menu, reader_menu],
        &list_header,
    );
    let state_owner = state.clone();
    window.connect_destroy(move |_| drop(state_owner.borrow()));

    Ui {
        window,
        state,
        folders,
        messages,
        search,
        reader,
        outer,
        inner,
        toast_overlay,
        sync_title,
        sync_detail,
        filter_buttons,
    }
}

pub(super) fn connect_signals(ui: &Ui) {
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
}

fn build_folder_pane(folders: &gtk::ListBox) -> (gtk::Box, gtk::Label, gtk::Label) {
    let pane = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["waymail-folder-pane"])
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
        .label("Personal")
        .xalign(0.0)
        .css_classes(["title-3"])
        .build();
    let address = gtk::Label::builder()
        .label("alex@northfield.dev")
        .xalign(0.0)
        .css_classes(["dim-label"])
        .build();
    account.append(&name);
    account.append(&address);

    let compose = gtk::Button::builder()
        .label("Compose")
        .icon_name("document-edit-symbolic")
        .action_name("win.compose")
        .tooltip_text("Compose a message (Ctrl+N)")
        .margin_start(16)
        .margin_end(16)
        .margin_bottom(18)
        .css_classes(["suggested-action", "waymail-compose"])
        .build();
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .child(folders)
        .build();
    let sync = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_start(24)
        .margin_end(24)
        .margin_top(18)
        .margin_bottom(22)
        .build();
    let sync_title = gtk::Label::builder()
        .label("●  All caught up")
        .xalign(0.0)
        .css_classes(["waymail-online"])
        .build();
    let sync_detail = gtk::Label::builder()
        .label("Last sync: 2 minutes ago")
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .build();
    sync.append(&sync_title);
    sync.append(&sync_detail);
    pane.append(&account);
    pane.append(&compose);
    pane.append(&scroll);
    pane.append(&sync);
    (pane, sync_title, sync_detail)
}

fn build_message_page(
    messages: &gtk::ListBox,
    search: &gtk::SearchEntry,
    menu: &gtk::Button,
) -> (
    adw::NavigationPage,
    adw::HeaderBar,
    Vec<(MessageFilter, gtk::Button)>,
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
            .css_classes(["waymail-filter"])
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
    )
}

fn build_reader_page(menu: &gtk::Button) -> (adw::NavigationPage, gtk::Box) {
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
        .css_classes(["waymail-reader"])
        .build();
    toolbar.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .vexpand(true)
            .child(&reader)
            .build(),
    ));
    (
        adw::NavigationPage::with_tag(&toolbar, "Message", "reader"),
        reader,
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
