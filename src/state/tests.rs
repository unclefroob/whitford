use super::*;
use crate::model::{FixtureSet, FolderId, MessageId, fixtures};

#[test]
fn initial_state_selects_first_inbox_message() {
    let state = AppState::new(fixtures()).unwrap();
    assert_eq!(state.selected_folder_id(), FolderId::INBOX);
    assert_eq!(state.selected_message_id(), Some(MessageId("mara")));
    assert_eq!(state.status(), ViewStatus::Ready);
}

#[test]
fn search_is_trimmed_case_insensitive_and_repairs_selection() {
    let mut state = AppState::new(fixtures()).unwrap();
    state.dispatch(Action::SetSearch("  ROADMAP ".into()));

    assert_eq!(state.visible_message_ids(), vec![MessageId("daniel")]);
    assert_eq!(state.selected_message_id(), Some(MessageId("daniel")));
    assert_eq!(state.status(), ViewStatus::Ready);

    state.dispatch(Action::SetSearch("not present".into()));
    assert_eq!(state.selected_message_id(), None);
    assert_eq!(state.status(), ViewStatus::NoSearchResults);

    state.dispatch(Action::SetSearch(" ".into()));
    assert_eq!(state.visible_message_ids().len(), 8);

    state.dispatch(Action::SetSearch("MÜLLER".into()));
    assert_eq!(state.visible_message_ids(), vec![MessageId("lena")]);
}

#[test]
fn filters_are_interactive_and_repair_selection() {
    let mut state = AppState::new(fixtures()).unwrap();
    state.dispatch(Action::SelectMessage(MessageId("daniel")));
    state.dispatch(Action::SetFilter(MessageFilter::Unread));
    assert_eq!(
        state.visible_message_ids(),
        vec![MessageId("mara"), MessageId("priya")]
    );
    assert_eq!(state.selected_message_id(), Some(MessageId("mara")));

    state.dispatch(Action::SetFilter(MessageFilter::Attachments));
    assert_eq!(
        state.visible_message_ids(),
        vec![MessageId("mara"), MessageId("noah")]
    );
    assert_eq!(state.selected_message_id(), Some(MessageId("mara")));

    state.dispatch(Action::SelectFolder(FolderId::DRAFTS));
    state.dispatch(Action::SetFilter(MessageFilter::Attachments));
    assert_eq!(state.selected_message_id(), None);
    assert_eq!(state.status(), ViewStatus::NoSearchResults);
}

#[test]
fn empty_folder_and_no_results_are_distinct() {
    let mut state = AppState::new(fixtures()).unwrap();
    state.dispatch(Action::SelectFolder(FolderId::TRASH));
    assert_eq!(state.status(), ViewStatus::EmptyFolder);

    state.dispatch(Action::SelectFolder(FolderId::INBOX));
    state.dispatch(Action::SetSearch("xyzzy".into()));
    assert_eq!(state.status(), ViewStatus::NoSearchResults);
}

#[test]
fn loading_and_offline_have_explicit_statuses() {
    let mut state = AppState::new(fixtures()).unwrap();
    state.dispatch(Action::SetSurface(Surface::Loading));
    assert_eq!(state.status(), ViewStatus::Loading);
    state.dispatch(Action::SetSurface(Surface::Offline));
    assert_eq!(state.status(), ViewStatus::Offline);
}

#[test]
fn navigation_is_bounded_and_uses_visible_order() {
    let mut state = AppState::new(fixtures()).unwrap();
    state.dispatch(Action::SelectPrevious);
    assert_eq!(state.selected_message_id(), Some(MessageId("mara")));

    state.dispatch(Action::SelectNext);
    assert_eq!(state.selected_message_id(), Some(MessageId("daniel")));

    for _ in 0..20 {
        state.dispatch(Action::SelectNext);
    }
    assert_eq!(state.selected_message_id(), Some(MessageId("lena")));
}

#[test]
fn archive_moves_message_updates_counts_and_selects_neighbor() {
    let mut state = AppState::new(fixtures()).unwrap();
    let inbox_before = state.folder_count(FolderId::INBOX);
    let archive_before = state.folder_count(FolderId::ARCHIVE);

    state.dispatch(Action::SelectMessage(MessageId("mara")));
    let transition = state.dispatch(Action::ArchiveSelected);

    assert!(transition.list_changed && transition.folders_changed);
    assert_eq!(state.folder_count(FolderId::INBOX), inbox_before - 1);
    assert_eq!(state.folder_count(FolderId::ARCHIVE), archive_before + 1);
    assert_eq!(state.selected_message_id(), Some(MessageId("daniel")));
}

#[test]
fn folder_badges_use_folder_specific_semantics() {
    let state = AppState::new(fixtures()).unwrap();
    assert_eq!(state.folder_count(FolderId::INBOX), 2);
    assert_eq!(state.folder_count(FolderId::STARRED), 2);
    assert_eq!(state.folder_count(FolderId::DRAFTS), 1);
    assert_eq!(state.folder_count(FolderId::SENT), 1);
    assert_eq!(state.folder_count(FolderId::ARCHIVE), 1);
    assert_eq!(state.folder_count(FolderId::TRASH), 0);
    assert_eq!(state.folder_count(FolderId("unknown")), 0);
}

#[test]
fn single_visible_message_navigation_and_archive_are_safe() {
    let mut state = AppState::new(fixtures()).unwrap();
    state.dispatch(Action::SetSearch("Q2 roadmap".into()));
    state.dispatch(Action::SelectNext);
    state.dispatch(Action::SelectPrevious);
    assert_eq!(state.selected_message_id(), Some(MessageId("daniel")));
    state.dispatch(Action::ArchiveSelected);
    assert_eq!(state.selected_message_id(), None);
    assert_eq!(state.status(), ViewStatus::NoSearchResults);
}

#[test]
fn empty_fixture_constructs_without_panicking() {
    let state = AppState::new(FixtureSet {
        folders: vec![],
        messages: vec![],
    })
    .unwrap();
    assert_eq!(state.selected_message_id(), None);
    assert_eq!(state.folder_count(FolderId::INBOX), 0);
    assert_eq!(state.status(), ViewStatus::EmptyFolder);
}

#[test]
fn invalid_targets_and_missing_selection_are_safe() {
    let mut state = AppState::new(fixtures()).unwrap();
    let selected = state.selected_message_id();
    let transition = state.dispatch(Action::SelectFolder(FolderId("unknown")));
    assert_eq!(state.selected_message_id(), selected);
    assert_eq!(transition.feedback, Some("Folder is unavailable"));

    let transition = state.dispatch(Action::SelectMessage(MessageId("unknown")));
    assert_eq!(transition.feedback, Some("Message is unavailable"));
    assert_eq!(state.selected_message_id(), selected);

    state.dispatch(Action::SelectFolder(FolderId::TRASH));
    let transition = state.dispatch(Action::ArchiveSelected);
    assert_eq!(transition.feedback, Some("No message selected"));
}

#[test]
fn escape_precedence_is_pure_and_non_destructive() {
    assert_eq!(
        escape_outcome(EscapeContext {
            search_active: true,
            reader_visible: true,
            folders_visible: true,
        }),
        EscapeOutcome::ClearSearch
    );
    assert_eq!(
        escape_outcome(EscapeContext {
            search_active: false,
            reader_visible: true,
            folders_visible: true,
        }),
        EscapeOutcome::ShowMessageList
    );
    assert_eq!(
        escape_outcome(EscapeContext {
            search_active: false,
            reader_visible: false,
            folders_visible: true,
        }),
        EscapeOutcome::HideFolders
    );
}
