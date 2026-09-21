use super::*;
use crate::model::{MailProvider, SyncMetadata, fixture_messages};
use std::sync::Arc;

fn account() -> AccountIdentity {
    AccountIdentity {
        provider: MailProvider::Gmail,
        email: "person@example.com".into(),
    }
}

fn multi_account_id(value: u8) -> crate::model::AccountId {
    crate::model::AccountId::new(format!("acct-{value:032x}")).expect("valid account fixture")
}

fn multi_account(email: &str) -> AccountIdentity {
    AccountIdentity {
        provider: MailProvider::Gmail,
        email: email.into(),
    }
}

fn inbox_snapshot(messages: Vec<MessageSummary>) -> MailboxSnapshot {
    MailboxSnapshot {
        messages,
        folder_catalog: crate::model::FolderCatalog::inbox_only(),
        metadata: SyncMetadata {
            completed_at: SystemTime::UNIX_EPOCH,
            requested_limit: 50,
            loaded_count: 0,
            fallback_count: 0,
            skipped_count: 0,
        },
    }
}

fn navigable_inbox_snapshot(messages: Vec<MessageSummary>) -> MailboxSnapshot {
    let mut snapshot = inbox_snapshot(messages);
    snapshot.folder_catalog = browsing_catalog();
    snapshot
}

#[test]
fn unified_inbox_is_newest_first_and_keeps_equal_gmail_ids_account_scoped() {
    let first = multi_account_id(1);
    let second = multi_account_id(2);
    let mut first_messages = fixture_messages();
    for message in &mut first_messages {
        message.in_inbox = true;
        message.received_at_unix = None;
    }
    first_messages[0].id = MessageId::gmail(7);
    first_messages[0].received_at_unix = Some(100);
    first_messages[1].id = MessageId::gmail(8);
    first_messages[1].received_at_unix = None;

    let mut second_messages = fixture_messages();
    for message in &mut second_messages {
        message.in_inbox = true;
        message.received_at_unix = None;
    }
    second_messages[0].id = MessageId::gmail(7);
    second_messages[0].received_at_unix = Some(100);
    second_messages[1].id = MessageId::gmail(9);
    second_messages[1].received_at_unix = Some(101);
    second_messages[2].received_at_unix = None;

    let mut state = AppState::new();
    state.dispatch(Action::UpsertAccountMailbox {
        account_id: second.clone(),
        identity: multi_account("work@example.com"),
        session: SessionState::Ready,
        mailbox: Some(inbox_snapshot(second_messages)),
    });
    state.dispatch(Action::UpsertAccountMailbox {
        account_id: first.clone(),
        identity: multi_account("personal@example.com"),
        session: SessionState::Ready,
        mailbox: Some(inbox_snapshot(first_messages)),
    });

    let snapshot = state.snapshot();
    assert_eq!(snapshot.mailbox_view, MailboxView::UnifiedInbox);
    assert_eq!(snapshot.accounts.len(), 2);
    let ids = snapshot
        .account_visible_messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(ids[0].account_id, second);
    assert_eq!(ids[0].message_id, MessageId::gmail(9));
    // Timestamp ties use opaque account ID, then Gmail message ID. The two
    // equal Gmail IDs are therefore present as separate rows.
    assert_eq!(ids[1].account_id, first);
    assert_eq!(ids[1].message_id, MessageId::gmail(7));
    assert_eq!(ids[2].account_id, second);
    assert_eq!(ids[2].message_id, MessageId::gmail(7));
    assert!(
        snapshot
            .account_visible_messages
            .last()
            .is_some_and(|message| message.message.received_at_unix.is_none())
    );
}

#[test]
fn unified_reader_load_is_account_scoped_and_ignores_a_same_id_other_account() {
    let first = multi_account_id(41);
    let second = multi_account_id(42);
    let mut state = AppState::new();
    for (id, email) in [
        (&first, "personal@example.com"),
        (&second, "work@example.com"),
    ] {
        let mut messages = fixture_messages();
        messages[0].id = MessageId::gmail(77);
        state.dispatch(Action::UpsertAccountMailbox {
            account_id: id.clone(),
            identity: multi_account(email),
            session: SessionState::Ready,
            mailbox: Some(inbox_snapshot(messages)),
        });
    }
    // A retained legacy projection must never win over a unified selection.
    // This mirrors switching from the original account's cached inbox to a
    // second account in the unified inbox.
    let stale = fixture_messages().remove(0);
    state.mailbox = Some(inbox_snapshot(vec![stale.clone()]));
    state.selected_message_id = Some(stale.id.clone());
    state.reader = ReaderState::Loaded {
        id: stale.id,
        body: reply_body(),
    };
    let selected = crate::model::AccountMessageId {
        account_id: first.clone(),
        message_id: MessageId::gmail(77),
    };
    let update = state.dispatch(Action::SelectAccountMessage(selected.clone()));
    let selected_snapshot = state.snapshot();
    assert!(selected_snapshot.selected_message.is_none());
    assert_eq!(
        selected_snapshot.selected_account_message,
        Some(selected.clone())
    );
    // A later list normalization (for example after searching or filtering)
    // must not restore the stale compatibility selection.
    state.normalize();
    let normalized = state.snapshot();
    assert!(normalized.selected_message.is_none());
    assert_eq!(normalized.selected_account_message, Some(selected.clone()));
    let (request_id, generation) = match update.effects.as_slice() {
        [
            Effect::SendWorker(WorkerCommand::FetchAccountBody {
                request_id,
                generation,
                message,
                account_email,
                ..
            }),
        ] if message == &selected && account_email == "personal@example.com" => {
            (*request_id, *generation)
        }
        other => panic!("expected scoped body request, got {other:?}"),
    };
    let wrong = crate::model::AccountMessageId {
        account_id: second,
        message_id: MessageId::gmail(77),
    };
    state.dispatch(Action::Worker(WorkerEvent::AccountBodyLoaded {
        request_id,
        generation,
        message: wrong,
        body: reply_body(),
    }));
    assert!(
        matches!(state.snapshot().reader, ReaderState::AccountLoading { id, .. } if id == selected)
    );
    state.dispatch(Action::Worker(WorkerEvent::AccountBodyLoaded {
        request_id,
        generation,
        message: selected.clone(),
        body: reply_body(),
    }));
    assert!(
        matches!(state.snapshot().reader, ReaderState::AccountLoaded { id, .. } if id == selected)
    );
}

#[test]
fn unified_mutation_keeps_the_account_id_when_gmail_ids_collide() {
    let first = multi_account_id(43);
    let second = multi_account_id(44);
    let mut state = AppState::new();
    for (id, email) in [
        (&first, "personal@example.com"),
        (&second, "work@example.com"),
    ] {
        let mut messages = fixture_messages();
        messages[0].id = MessageId::gmail(88);
        state.dispatch(Action::UpsertAccountMailbox {
            account_id: id.clone(),
            identity: multi_account(email),
            session: SessionState::Ready,
            mailbox: Some(inbox_snapshot(messages)),
        });
    }
    let selected = crate::model::AccountMessageId {
        account_id: first.clone(),
        message_id: MessageId::gmail(88),
    };
    state.dispatch(Action::SelectAccountMessage(selected.clone()));
    let update = state.dispatch(Action::ToggleStar);
    let (request_id, generation) = match update.effects.as_slice() {
        [
            Effect::SendWorker(WorkerCommand::MutateAccountMessage {
                request_id,
                generation,
                message,
                account_email,
                mutation: MessageMutation::SetStarred(true),
                ..
            }),
        ] if message == &selected && account_email == "personal@example.com" => {
            (*request_id, *generation)
        }
        other => panic!("expected scoped mutation, got {other:?}"),
    };
    state.dispatch(Action::Worker(WorkerEvent::AccountMutationConfirmed {
        request_id,
        generation,
        message: selected.clone(),
        mutation: MessageMutation::SetStarred(true),
    }));
    let snapshot = state.snapshot();
    assert!(
        snapshot
            .account_visible_messages
            .iter()
            .find(|row| row.id == selected)
            .unwrap()
            .message
            .starred
    );
    assert!(
        !snapshot
            .account_visible_messages
            .iter()
            .find(|row| row.id.account_id == second && row.id.message_id == MessageId::gmail(88))
            .unwrap()
            .message
            .starred
    );
}

#[test]
fn account_view_selection_and_removal_are_scoped_without_disturbing_other_accounts() {
    let first = multi_account_id(11);
    let second = multi_account_id(12);
    let mut state = AppState::new();
    state.dispatch(Action::UpsertAccountMailbox {
        account_id: first.clone(),
        identity: multi_account("personal@example.com"),
        session: SessionState::Ready,
        mailbox: Some(navigable_inbox_snapshot(fixture_messages())),
    });
    state.dispatch(Action::UpsertAccountMailbox {
        account_id: second.clone(),
        identity: multi_account("work@example.com"),
        session: SessionState::Ready,
        mailbox: Some(inbox_snapshot(fixture_messages())),
    });

    let projects = FolderId::Label("Projects/Rust".into());
    state.dispatch(Action::UpsertAccountFolder {
        folder: crate::model::AccountFolderId {
            account_id: first.clone(),
            folder_id: projects.clone(),
        },
        snapshot: folder_snapshot(projects.clone(), "Projects/Rust", "Project mail"),
    });
    state.dispatch(Action::SelectMailboxView(MailboxView::AccountFolder(
        crate::model::AccountFolderId {
            account_id: first.clone(),
            folder_id: projects,
        },
    )));
    let project_view = state.snapshot();
    assert_eq!(project_view.account_visible_messages.len(), 1);
    assert_eq!(
        project_view.account_visible_messages[0].message.subject,
        "Project mail"
    );

    state.dispatch(Action::SelectMailboxView(MailboxView::AccountFolder(
        crate::model::AccountFolderId {
            account_id: first.clone(),
            folder_id: FolderId::Inbox,
        },
    )));
    let selected = crate::model::AccountMessageId {
        account_id: first.clone(),
        message_id: MessageId::gmail(1),
    };
    state.dispatch(Action::SelectAccountMessage(selected.clone()));
    assert_eq!(state.snapshot().selected_account_message, Some(selected));

    state.dispatch(Action::RemoveAccountMailbox { account_id: first });
    let snapshot = state.snapshot();
    assert_eq!(snapshot.mailbox_view, MailboxView::UnifiedInbox);
    assert_eq!(snapshot.selected_account_message, None);
    assert_eq!(snapshot.accounts.len(), 1);
    assert!(
        snapshot
            .account_visible_messages
            .iter()
            .all(|message| message.id.account_id == second)
    );
}

#[test]
fn account_reconnect_and_durable_removal_are_scoped_to_the_selected_account() {
    let first = multi_account_id(13);
    let second = multi_account_id(14);
    let mut state = AppState::new();
    for (id, email) in [
        (first.clone(), "personal@example.com"),
        (second.clone(), "work@example.com"),
    ] {
        state.dispatch(Action::UpsertAccountMailbox {
            account_id: id,
            identity: multi_account(email),
            session: SessionState::Ready,
            mailbox: Some(inbox_snapshot(fixture_messages())),
        });
    }

    let update = state.dispatch(Action::ReconnectAccount {
        account_id: first.clone(),
    });
    let reconnect_id = match update.effects.as_slice() {
        [Effect::SendWorker(WorkerCommand::ReconnectAccount { id, account_id })]
            if account_id == &first =>
        {
            *id
        }
        other => panic!("unexpected effects: {other:?}"),
    };
    state.dispatch(Action::Worker(WorkerEvent::AccountSynced {
        id: reconnect_id,
        account_id: first.clone(),
        account: multi_account("personal@example.com"),
        snapshot: inbox_snapshot(Vec::new()),
    }));
    let snapshot = state.snapshot();
    assert_eq!(snapshot.accounts.len(), 2);
    assert!(matches!(snapshot.accounts[0].session, SessionState::Ready));

    let update = state.dispatch(Action::RemoveAccount {
        account_id: first.clone(),
    });
    let remove_id = match update.effects.as_slice() {
        [Effect::SendWorker(WorkerCommand::RemoveAccount { id, account_id })]
            if account_id == &first =>
        {
            *id
        }
        other => panic!("unexpected effects: {other:?}"),
    };
    state.dispatch(Action::Worker(WorkerEvent::AccountRemoved {
        id: remove_id,
        account_id: first,
    }));
    let snapshot = state.snapshot();
    assert_eq!(snapshot.accounts.len(), 1);
    assert_eq!(snapshot.accounts[0].id, second);
}

#[test]
fn account_projection_rejects_a_duplicate_gmail_identity() {
    let mut state = AppState::new();
    state.dispatch(Action::UpsertAccountMailbox {
        account_id: multi_account_id(21),
        identity: multi_account("same@example.com"),
        session: SessionState::Ready,
        mailbox: None,
    });
    let update = state.dispatch(Action::UpsertAccountMailbox {
        account_id: multi_account_id(22),
        identity: multi_account("SAME@example.com"),
        session: SessionState::Ready,
        mailbox: None,
    });
    assert_eq!(
        update.feedback,
        Some("That Gmail account is already connected")
    );
    assert_eq!(state.snapshot().accounts.len(), 1);
}

#[test]
fn compose_from_only_accepts_connected_primary_identities() {
    let first = multi_account_id(91);
    let second = multi_account_id(92);
    let mut state = AppState::new();
    for (id, email) in [
        (&first, "personal@example.com"),
        (&second, "work@example.com"),
    ] {
        state.dispatch(Action::UpsertAccountMailbox {
            account_id: id.clone(),
            identity: multi_account(email),
            session: SessionState::Ready,
            mailbox: None,
        });
    }
    state.draft_catalog_state = DraftCatalogState::Ready;
    state.composer = ComposerState::Editing {
        draft: compatibility_draft(
            crate::composer::new_message("draft-from".into(), "personal@example.com", "").unwrap(),
        ),
    };

    state.dispatch(Action::SelectComposeFrom(second.clone()));
    assert!(matches!(
        state.snapshot().composer,
        ComposerState::Editing { draft } if draft.compose.account_email == "work@example.com"
    ));

    state.dispatch(Action::SelectComposeFrom(multi_account_id(93)));
    assert!(matches!(
        state.snapshot().composer,
        ComposerState::Editing { draft } if draft.compose.account_email == "work@example.com"
    ));
}

#[test]
fn verified_send_as_rows_are_request_bound_and_not_picker_identities() {
    let account_id = multi_account_id(94);
    let mut state = AppState::new();
    let update = state.dispatch(Action::UpsertAccountMailbox {
        account_id: account_id.clone(),
        identity: multi_account("person@example.com"),
        session: SessionState::Ready,
        mailbox: None,
    });
    let (request_id, generation) = match update.effects.as_slice() {
        [
            Effect::SendWorker(WorkerCommand::FetchAccountSendAs {
                request_id,
                generation,
                account_id: received,
                account_email,
            }),
        ] if received == &account_id && account_email == "person@example.com" => {
            (*request_id, *generation)
        }
        other => panic!("expected send-as fetch, got {other:?}"),
    };
    state.dispatch(Action::Worker(WorkerEvent::AccountSendAsLoaded {
        request_id,
        generation,
        account_id: account_id.clone(),
        aliases: vec![
            crate::model::GmailSendAsIdentity {
                email: "alias@example.com".into(),
                display_name: Some("Alias".into()),
                is_default: false,
            },
            crate::model::GmailSendAsIdentity {
                email: "ALIAS@example.com".into(),
                display_name: Some("Duplicate".into()),
                is_default: true,
            },
        ],
    }));
    let snapshot = state.snapshot();
    assert_eq!(snapshot.verified_send_as.len(), 1);
    assert_eq!(snapshot.verified_send_as[0].aliases.len(), 1);
    assert_eq!(
        snapshot.verified_send_as[0].aliases[0].email,
        "alias@example.com"
    );
    assert_eq!(snapshot.compose_identities.len(), 1);
    assert_eq!(snapshot.compose_identities[0].email, "person@example.com");
}

#[test]
fn background_tick_is_separate_from_foreground_and_duplicate_events_notify_once() {
    let mut state = AppState::new();
    ready(&mut state);
    let scheduled = state.arm_background_sync(std::time::Duration::from_secs(60));
    let generation = match scheduled {
        Effect::ScheduleBackgroundSync {
            schedule_generation,
            ..
        } => schedule_generation,
        _ => unreachable!(),
    };
    let update = state.dispatch(Action::BackgroundSyncTimer {
        schedule_generation: generation,
    });
    let request_id = match &update.effects[0] {
        Effect::SendWorker(WorkerCommand::BackgroundSync { request_id, .. }) => *request_id,
        other => panic!("unexpected effect: {other:?}"),
    };
    let mut snapshot = state.mailbox.clone().unwrap();
    let mut message = fixture_messages().remove(0);
    message.id = MessageId::gmail(999);
    message.unread = true;
    snapshot.messages.push(message);
    let update = state.dispatch(Action::Worker(WorkerEvent::BackgroundSyncComplete {
        request_id,
        account_email: account().email,
        snapshot,
        new_unread_ids: vec![MessageId::gmail(999)],
    }));
    assert!(
        update
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::NotifyNewUnread { count: 1 }))
    );
    // The same worker event is stale after its request is cleared and cannot
    // produce a second desktop notification.
    assert!(
        state
            .dispatch(Action::Worker(WorkerEvent::BackgroundSyncComplete {
                request_id,
                account_email: account().email,
                snapshot: state.mailbox.clone().unwrap(),
                new_unread_ids: vec![MessageId::gmail(999)],
            }))
            .effects
            .is_empty()
    );
}

fn begin_background_sync(state: &mut AppState) -> BackgroundSyncRequestId {
    let generation = match state.arm_background_sync(std::time::Duration::from_secs(60)) {
        Effect::ScheduleBackgroundSync {
            schedule_generation,
            ..
        } => schedule_generation,
        _ => unreachable!(),
    };
    match &state
        .dispatch(Action::BackgroundSyncTimer {
            schedule_generation: generation,
        })
        .effects[0]
    {
        Effect::SendWorker(WorkerCommand::BackgroundSync { request_id, .. }) => *request_id,
        other => panic!("unexpected effect: {other:?}"),
    }
}

#[test]
fn background_notification_dedup_is_scoped_to_the_account() {
    let mut state = AppState::new();
    let first_id = ready(&mut state);
    let request_id = begin_background_sync(&mut state);
    let initial = state.mailbox.clone().unwrap();
    assert!(
        state
            .dispatch(Action::Worker(WorkerEvent::BackgroundSyncComplete {
                request_id,
                account_email: account().email,
                snapshot: initial,
                new_unread_ids: vec![MessageId::gmail(999)],
            }))
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::NotifyNewUnread { count: 1 }))
    );

    let other = AccountIdentity {
        provider: MailProvider::Gmail,
        email: "other@example.com".into(),
    };
    let connect = state.dispatch(Action::Connect);
    let connect_id = match connect.effects[0] {
        Effect::SendWorker(WorkerCommand::Connect { id }) => id,
        _ => panic!(),
    };
    assert_ne!(connect_id, first_id);
    state.dispatch(Action::Worker(WorkerEvent::IdentityVerified {
        id: connect_id,
        account: other.clone(),
    }));
    state.dispatch(Action::Worker(WorkerEvent::SyncComplete {
        id: connect_id,
        account: other.clone(),
        snapshot: MailboxSnapshot {
            messages: fixture_messages(),
            folder_catalog: crate::model::FolderCatalog::inbox_only(),
            metadata: SyncMetadata {
                completed_at: SystemTime::UNIX_EPOCH,
                requested_limit: 50,
                loaded_count: 3,
                fallback_count: 0,
                skipped_count: 0,
            },
        },
    }));
    let request_id = begin_background_sync(&mut state);
    assert!(
        state
            .dispatch(Action::Worker(WorkerEvent::BackgroundSyncComplete {
                request_id,
                account_email: other.email,
                snapshot: state.mailbox.clone().unwrap(),
                new_unread_ids: vec![MessageId::gmail(999)],
            }))
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::NotifyNewUnread { count: 1 }))
    );
}

#[test]
fn background_completion_does_not_publish_over_active_search_or_reader() {
    let mut searching = AppState::new();
    ready(&mut searching);
    searching.mailbox = Some(folder_snapshot(FolderId::Inbox, "INBOX", "Inbox"));
    let request_id = begin_background_sync(&mut searching);
    searching.dispatch(Action::SetSearch("anything".into()));
    searching.dispatch(Action::SubmitServerSearch);
    let before = searching.mailbox.clone().unwrap();
    searching.dispatch(Action::Worker(WorkerEvent::BackgroundSyncComplete {
        request_id,
        account_email: account().email,
        snapshot: MailboxSnapshot {
            messages: Vec::new(),
            ..before.clone()
        },
        new_unread_ids: Vec::new(),
    }));
    assert!(matches!(
        searching.server_search,
        ServerSearchState::Loading { .. }
    ));
    assert_eq!(searching.mailbox, Some(before));

    let mut reading = AppState::new();
    ready(&mut reading);
    let request_id = begin_background_sync(&mut reading);
    reading.dispatch(Action::SelectMessage(MessageId::gmail(1)));
    let before = reading.mailbox.clone().unwrap();
    reading.dispatch(Action::Worker(WorkerEvent::BackgroundSyncComplete {
        request_id,
        account_email: account().email,
        snapshot: MailboxSnapshot {
            messages: Vec::new(),
            ..before.clone()
        },
        new_unread_ids: Vec::new(),
    }));
    assert!(matches!(reading.reader, ReaderState::Loading { .. }));
    assert_eq!(reading.mailbox, Some(before));
}

#[test]
fn background_timer_and_terminal_failure_ignore_stale_or_late_work() {
    let mut state = AppState::new();
    ready(&mut state);
    let stale_generation = match state.arm_background_sync(std::time::Duration::from_secs(60)) {
        Effect::ScheduleBackgroundSync {
            schedule_generation,
            ..
        } => schedule_generation,
        _ => unreachable!(),
    };
    let _current = state.arm_background_sync(std::time::Duration::from_secs(60));
    assert!(
        state
            .dispatch(Action::BackgroundSyncTimer {
                schedule_generation: stale_generation,
            })
            .effects
            .is_empty()
    );

    let request_id = begin_background_sync(&mut state);
    let failure = ServiceFailure {
        kind: FailureKind::AuthorizationExpired,
        retryable: false,
        preserve_mail: true,
        cleanup_failed: false,
        config_path: None,
    };
    assert!(
        state
            .dispatch(Action::Worker(WorkerEvent::BackgroundSyncFailed {
                request_id,
                failure,
            }))
            .effects
            .is_empty()
    );
    assert_eq!(
        state.snapshot().background_sync_status,
        BackgroundSyncStatus::Paused
    );

    let mut late = AppState::new();
    ready(&mut late);
    let late_request = begin_background_sync(&mut late);
    let disconnect = late.dispatch(Action::ConfirmDisconnect);
    assert!(
        disconnect
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::SendWorker(WorkerCommand::Disconnect { .. })))
    );
    assert!(
        late.dispatch(Action::Worker(WorkerEvent::BackgroundSyncComplete {
            request_id: late_request,
            account_email: account().email,
            snapshot: MailboxSnapshot {
                messages: fixture_messages(),
                folder_catalog: crate::model::FolderCatalog::inbox_only(),
                metadata: SyncMetadata {
                    completed_at: SystemTime::UNIX_EPOCH,
                    requested_limit: 50,
                    loaded_count: 3,
                    fallback_count: 0,
                    skipped_count: 0,
                },
            },
            new_unread_ids: vec![MessageId::gmail(1000)],
        }))
        .effects
        .is_empty()
    );
}
fn ready(state: &mut AppState) -> OperationId {
    let update = state.dispatch(Action::Connect);
    let id = match &update.effects[0] {
        Effect::SendWorker(WorkerCommand::Connect { id }) => *id,
        _ => panic!("wrong effect"),
    };
    state.dispatch(Action::Worker(WorkerEvent::SyncComplete {
        id,
        account: account(),
        snapshot: MailboxSnapshot {
            messages: fixture_messages(),
            folder_catalog: crate::model::FolderCatalog::inbox_only(),
            metadata: SyncMetadata {
                completed_at: SystemTime::UNIX_EPOCH,
                requested_limit: 50,
                loaded_count: 3,
                fallback_count: 0,
                skipped_count: 0,
            },
        },
    }));
    id
}

fn browsing_catalog() -> crate::model::FolderCatalog {
    use crate::model::{FolderDescriptor, FolderKind};

    crate::model::FolderCatalog::bounded(vec![
        FolderDescriptor {
            id: FolderId::Inbox,
            mailbox: "INBOX".into(),
            display_name: "Inbox".into(),
            kind: FolderKind::Inbox,
        },
        FolderDescriptor {
            id: FolderId::Sent,
            mailbox: "[Gmail]/Sent Mail".into(),
            display_name: "Sent".into(),
            kind: FolderKind::Sent,
        },
        FolderDescriptor {
            id: FolderId::AllMail,
            mailbox: "[Gmail]/All Mail".into(),
            display_name: "All Mail".into(),
            kind: FolderKind::AllMail,
        },
        FolderDescriptor {
            id: FolderId::Trash,
            mailbox: "[Gmail]/Trash".into(),
            display_name: "Trash".into(),
            kind: FolderKind::Trash,
        },
        FolderDescriptor {
            id: FolderId::Starred,
            mailbox: "[Gmail]/Starred".into(),
            display_name: "Starred".into(),
            kind: FolderKind::Starred,
        },
        FolderDescriptor {
            id: FolderId::Label("Projects/Rust".into()),
            mailbox: "Projects/Rust".into(),
            display_name: "Projects/Rust".into(),
            kind: FolderKind::Label,
        },
    ])
}

fn folder_snapshot(folder_id: FolderId, mailbox_name: &str, subject: &str) -> MailboxSnapshot {
    let mut message = fixture_messages().remove(0);
    message.folder_id = folder_id.clone();
    message.locator.folder_id = folder_id;
    message.locator.mailbox = mailbox_name.into();
    message.locator.uid = 91;
    message.subject = subject.into();
    MailboxSnapshot {
        messages: vec![message],
        folder_catalog: browsing_catalog(),
        metadata: SyncMetadata {
            completed_at: SystemTime::UNIX_EPOCH,
            requested_limit: 50,
            loaded_count: 1,
            fallback_count: 0,
            skipped_count: 0,
        },
    }
}

fn body() -> Arc<crate::model::MessageBody> {
    Arc::new(crate::model::MessageBody {
        text: "Complete body".into(),
        html: None,
        attachments: Vec::new(),
        reply_context: crate::model::ReplyContext::default(),
        used_fallback: false,
    })
}

fn reply_body() -> Arc<crate::model::MessageBody> {
    let mut value = (*body()).clone();
    value.reply_context.from.push(crate::model::ReplyAddress {
        name: Some("Sender".into()),
        email: "sender@example.com".into(),
    });
    Arc::new(value)
}

fn load_replyable(state: &mut AppState) {
    ready(state);
    finish_draft_restore(state, Vec::new());
    let id = MessageId::gmail(1);
    let update = state.dispatch(Action::SelectMessage(id.clone()));
    let (request_id, generation) = match update.effects[0] {
        Effect::SendWorker(WorkerCommand::FetchBody {
            request_id,
            generation,
            ..
        }) => (request_id, generation),
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::BodyLoaded {
        request_id,
        generation,
        message_id: id,
        body: reply_body(),
        usage: Default::default(),
        saved: true,
    }));
}

fn finish_draft_restore(state: &mut AppState, drafts: Vec<crate::composer::ComposeDraft>) {
    let operation_id = state.pending_draft_load.expect("draft restore operation");
    let signature_operation_id = state
        .pending_signature_load
        .expect("signature restore operation");
    let generation = state.draft_generation;
    state.dispatch(Action::Worker(WorkerEvent::DraftsLoaded {
        operation_id,
        generation,
        account_email: "person@example.com".into(),
        drafts,
    }));
    state.dispatch(Action::Worker(WorkerEvent::SignatureLoaded {
        operation_id: signature_operation_id,
        generation,
        account_email: "person@example.com".into(),
        preference: crate::drafts::SignaturePreference::default(),
    }));
}

#[test]
fn composing_waits_for_draft_restore_then_runs_the_queued_intent() {
    let mut state = AppState::new();
    ready(&mut state);
    let id = MessageId::gmail(1);
    let update = state.dispatch(Action::SelectMessage(id.clone()));
    let (request_id, generation) = match update.effects[0] {
        Effect::SendWorker(WorkerCommand::FetchBody {
            request_id,
            generation,
            ..
        }) => (request_id, generation),
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::BodyLoaded {
        request_id,
        generation,
        message_id: id,
        body: reply_body(),
        usage: Default::default(),
        saved: true,
    }));

    let queued = state.dispatch(Action::BeginReply);
    assert_eq!(
        queued.feedback,
        Some("Restoring local drafts before composing")
    );
    assert!(queued.effects.is_empty());
    assert!(matches!(state.snapshot().composer, ComposerState::Closed));

    finish_draft_restore(&mut state, Vec::new());
    assert!(matches!(
        state.snapshot().composer,
        ComposerState::Editing { .. }
    ));
}

#[test]
fn draft_restore_failure_blocks_writes_and_can_be_retried() {
    let mut state = AppState::new();
    ready(&mut state);
    let operation_id = state.pending_draft_load.expect("draft restore operation");
    let generation = state.draft_generation;
    let failed = state.dispatch(Action::Worker(WorkerEvent::DraftOperationFailed {
        operation_id,
        generation,
        account_email: "person@example.com".into(),
    }));
    assert_eq!(failed.feedback, Some("Could not restore local drafts"));

    let retry = state.dispatch(Action::RetryDraftRestore);
    assert!(retry.effects.iter().any(|effect| matches!(
        effect,
        Effect::SendWorker(WorkerCommand::LoadDrafts { generation: current, .. })
            if *current == generation.wrapping_add(1)
    )));
    let stale = crate::composer::new_forward(
        "stale-draft".into(),
        "person@example.com",
        MessageId::gmail(1),
        "Stale",
        &Default::default(),
        "<p>stale</p>",
        "",
    )
    .unwrap();
    state.dispatch(Action::Worker(WorkerEvent::DraftsLoaded {
        operation_id,
        generation,
        account_email: "person@example.com".into(),
        drafts: vec![stale],
    }));
    assert!(state.saved_drafts.is_empty());
}

#[test]
fn restored_drafts_resume_only_the_exact_message_and_compose_kind() {
    let mut state = AppState::new();
    ready(&mut state);
    let id = MessageId::gmail(1);
    let update = state.dispatch(Action::SelectMessage(id.clone()));
    let (request_id, generation) = match update.effects[0] {
        Effect::SendWorker(WorkerCommand::FetchBody {
            request_id,
            generation,
            ..
        }) => (request_id, generation),
        _ => panic!(),
    };
    let loaded_body = reply_body();
    state.dispatch(Action::Worker(WorkerEvent::BodyLoaded {
        request_id,
        generation,
        message_id: id.clone(),
        body: loaded_body.clone(),
        usage: Default::default(),
        saved: true,
    }));
    let reply = crate::composer::new_reply(
        "reply-one".into(),
        "person@example.com",
        id.clone(),
        "Subject",
        &loaded_body.reply_context,
        "<p>Original</p>",
        "",
    )
    .unwrap();
    let forward = crate::composer::new_forward(
        "forward-one".into(),
        "person@example.com",
        id,
        "Subject",
        &loaded_body.reply_context,
        "<p>Original</p>",
        "",
    )
    .unwrap();
    finish_draft_restore(&mut state, vec![reply, forward]);

    let resumed = state.dispatch(Action::BeginReply);
    assert_eq!(
        resumed.feedback,
        Some("Resumed the matching draft saved on this device")
    );
    assert!(matches!(state.snapshot().composer,
        ComposerState::Editing { draft } if draft.compose.id == "reply-one"));
}

#[test]
fn startup_is_disconnected_then_restore_is_an_effect() {
    let mut state = AppState::new();
    assert_eq!(state.snapshot().status, ViewStatus::Disconnected);
    let update = state.dispatch(Action::Startup);
    assert!(matches!(
        update.effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::Restore { .. })]
    ));
}

#[test]
fn startup_restores_every_registered_account_without_waiting_for_live_auth() {
    let first = multi_account_id(31);
    let second = multi_account_id(32);
    let mut state = AppState::new();
    let startup = state.dispatch(Action::Startup);
    let id = match startup.effects.as_slice() {
        [Effect::SendWorker(WorkerCommand::Restore { id })] => *id,
        other => panic!("unexpected startup effects: {other:?}"),
    };

    state.dispatch(Action::Worker(WorkerEvent::AccountMailboxesRestored {
        id,
        accounts: vec![
            crate::worker::RestoredAccountMailbox {
                account_id: first.clone(),
                account: multi_account("personal@example.com"),
                snapshot: Some(inbox_snapshot(fixture_messages())),
            },
            crate::worker::RestoredAccountMailbox {
                account_id: second.clone(),
                account: multi_account("work@example.com"),
                snapshot: None,
            },
        ],
    }));
    // The legacy restore may still have no token. That must only disconnect
    // its singleton UI, not discard the restored multi-account projection.
    state.dispatch(Action::Worker(WorkerEvent::NoStoredAccount { id }));

    let snapshot = state.snapshot();
    assert_eq!(snapshot.accounts.len(), 2);
    assert_eq!(snapshot.accounts[0].id, first);
    assert_eq!(snapshot.accounts[1].id, second);
    assert!(!snapshot.account_visible_messages.is_empty());
}

fn mutation_effect(update: Update) -> (MutationRequestId, u64, MessageId, MessageMutation) {
    match update.effects.into_iter().next().expect("mutation effect") {
        Effect::SendWorker(WorkerCommand::MutateMessageInCatalog {
            request_id,
            generation,
            message_id,
            mutation,
            ..
        }) => (request_id, generation, message_id, mutation),
        Effect::SendWorker(WorkerCommand::MutateMessage {
            request_id,
            generation,
            message_id,
            mutation,
            ..
        }) => (request_id, generation, message_id, mutation),
        other => panic!("unexpected effect: {other:?}"),
    }
}

fn folder_effect(update: Update) -> (FolderRequestId, u64, FolderId) {
    match update.effects.into_iter().next().expect("folder effect") {
        Effect::SendWorker(WorkerCommand::FetchFolder {
            request_id,
            generation,
            folder,
            ..
        }) => (request_id, generation, folder.id),
        other => panic!("unexpected effect: {other:?}"),
    }
}

fn received_attachment(path: Vec<u32>, name: &str) -> crate::model::Attachment {
    crate::model::Attachment {
        name: name.into(),
        media_type: Some("application/pdf".into()),
        octets: Some(12),
        part: crate::model::MimePartDescriptor {
            path,
            encoding: crate::model::TransferEncoding::Base64,
            encoded_octets: 12,
        },
    }
}

fn loaded_with_attachments(state: &mut AppState, attachments: Vec<crate::model::Attachment>) {
    ready(state);
    state.selected_message_id = Some(MessageId::gmail(1));
    let mut loaded = (*body()).clone();
    loaded.attachments = attachments;
    state.reader = ReaderState::Loaded {
        id: MessageId::gmail(1),
        body: Arc::new(loaded),
    };
}

#[test]
fn attachment_jobs_are_bounded_progress_without_rebuilding_reader_and_complete_durably() {
    let first = received_attachment(vec![2], "first.pdf");
    let second = received_attachment(vec![3], "second.pdf");
    let third = received_attachment(vec![4], "third.pdf");
    let mut state = AppState::new();
    loaded_with_attachments(
        &mut state,
        vec![first.clone(), second.clone(), third.clone()],
    );

    let open = state.dispatch(Action::OpenAttachment(first));
    let (job_id, generation) = match open.effects.as_slice() {
        [
            Effect::SendWorker(WorkerCommand::DownloadAttachment {
                job_id,
                generation,
                destination: AttachmentDestination::Open,
                ..
            }),
        ] => (*job_id, *generation),
        other => panic!("unexpected effects: {other:?}"),
    };
    let revision = state.reader_revision;
    state.dispatch(Action::Worker(WorkerEvent::AttachmentProgress {
        job_id,
        generation,
        transferred: 6,
        total: 12,
    }));
    assert_eq!(state.reader_revision, revision);
    assert_eq!(state.snapshot().attachment_downloads[0].transferred, 6);

    let save = state.dispatch(Action::SaveAttachment {
        attachment: second,
        destination: "/tmp/second.pdf".into(),
    });
    assert!(matches!(
        save.effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::DownloadAttachment {
            destination: AttachmentDestination::SaveAs(_),
            ..
        })]
    ));
    let bounded = state.dispatch(Action::OpenAttachment(third));
    assert_eq!(
        bounded.feedback,
        Some("Wait for an attachment download to finish")
    );
    assert!(bounded.effects.is_empty());

    let complete = state.dispatch(Action::Worker(WorkerEvent::AttachmentCompleted {
        job_id,
        generation,
        path: "/tmp/private-cache-file.pdf".into(),
        open: true,
    }));
    assert!(matches!(
        complete.effects.as_slice(),
        [Effect::LaunchAttachment(path)] if path.ends_with("private-cache-file.pdf")
    ));
    assert_eq!(state.snapshot().attachment_downloads.len(), 1);
}

#[test]
fn cancelling_attachment_removes_ui_state_and_aborts_the_exact_job() {
    let attachment = received_attachment(vec![2], "report.pdf");
    let mut state = AppState::new();
    loaded_with_attachments(&mut state, vec![attachment.clone()]);
    let start = state.dispatch(Action::OpenAttachment(attachment));
    let job_id = match start.effects.as_slice() {
        [Effect::SendWorker(WorkerCommand::DownloadAttachment { job_id, .. })] => *job_id,
        _ => panic!(),
    };
    let cancel = state.dispatch(Action::CancelAttachment(job_id));
    assert!(matches!(
        cancel.effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::CancelAttachment { job_id: current, .. })]
            if *current == job_id
    ));
    assert!(state.snapshot().attachment_downloads.is_empty());
}

#[test]
fn folder_navigation_shows_cache_then_replaces_it_with_fresh_mail() {
    let mut state = AppState::new();
    ready(&mut state);
    state.mailbox.as_mut().unwrap().folder_catalog = browsing_catalog();

    let (request_id, generation, folder_id) =
        folder_effect(state.dispatch(Action::SelectFolder(FolderId::Sent)));
    assert_eq!(folder_id, FolderId::Sent);
    let loading = state.snapshot();
    assert_eq!(loading.selected_folder_id, FolderId::Sent);
    assert_eq!(loading.status, ViewStatus::Loading);
    assert!(loading.visible_messages.is_empty());
    assert_eq!(loading.folders.len(), 6);
    assert_eq!(loading.folder_counts, vec![(FolderId::Sent, 0)]);

    state.dispatch(Action::Worker(WorkerEvent::FolderCacheLoaded {
        request_id,
        generation,
        folder_id: FolderId::Sent,
        snapshot: folder_snapshot(FolderId::Sent, "[Gmail]/Sent Mail", "Cached subject"),
    }));
    let cached = state.snapshot();
    assert_eq!(cached.status, ViewStatus::Ready);
    assert_eq!(cached.visible_messages[0].subject, "Cached subject");
    assert_eq!(cached.folder_counts, vec![(FolderId::Sent, 1)]);
    assert!(cached.can_mutate, "a cached folder row remains actionable");

    let body_request = state.dispatch(Action::SelectMessage(MessageId::gmail(1)));
    assert!(matches!(
        body_request.effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::FetchBody { locator, .. })]
            if locator.folder_id == FolderId::Sent
                && locator.mailbox == "[Gmail]/Sent Mail"
                && locator.uid == 91
    ));

    state.dispatch(Action::Worker(WorkerEvent::FolderLoaded {
        request_id,
        generation,
        folder_id: FolderId::Sent,
        snapshot: folder_snapshot(FolderId::Sent, "[Gmail]/Sent Mail", "Fresh subject"),
    }));
    let fresh = state.snapshot();
    assert_eq!(fresh.status, ViewStatus::Ready);
    assert_eq!(fresh.visible_messages[0].subject, "Fresh subject");
    assert_eq!(fresh.visible_messages[0].id, MessageId::gmail(1));
}

#[test]
fn rapid_folder_switch_ignores_the_superseded_request() {
    let mut state = AppState::new();
    ready(&mut state);
    state.mailbox.as_mut().unwrap().folder_catalog = browsing_catalog();

    let (sent_request, sent_generation, _) =
        folder_effect(state.dispatch(Action::SelectFolder(FolderId::Sent)));
    let label_id = FolderId::Label("Projects/Rust".into());
    let (label_request, label_generation, _) =
        folder_effect(state.dispatch(Action::SelectFolder(label_id.clone())));
    assert_ne!(sent_request, label_request);
    assert_ne!(sent_generation, label_generation);

    state.dispatch(Action::Worker(WorkerEvent::FolderLoaded {
        request_id: sent_request,
        generation: sent_generation,
        folder_id: FolderId::Sent,
        snapshot: folder_snapshot(FolderId::Sent, "[Gmail]/Sent Mail", "Stale Sent"),
    }));
    let still_loading = state.snapshot();
    assert_eq!(still_loading.selected_folder_id, label_id);
    assert_eq!(still_loading.status, ViewStatus::Loading);
    assert!(still_loading.visible_messages.is_empty());

    state.dispatch(Action::Worker(WorkerEvent::FolderLoaded {
        request_id: label_request,
        generation: label_generation,
        folder_id: label_id.clone(),
        snapshot: folder_snapshot(label_id.clone(), "Projects/Rust", "Rust label"),
    }));
    let loaded = state.snapshot();
    assert_eq!(loaded.selected_folder_id, label_id.clone());
    assert_eq!(loaded.visible_messages[0].subject, "Rust label");
    assert_eq!(loaded.folder_counts, vec![(label_id, 1)]);
}

#[test]
fn optimistic_dimensions_settle_without_clobbering_each_other() {
    let mut state = AppState::new();
    ready(&mut state);
    state.selected_message_id = Some(MessageId::gmail(1));
    let (star_id, generation, id, _) = mutation_effect(state.dispatch(Action::ToggleStar));
    let (read_id, _, _, _) = mutation_effect(state.dispatch(Action::ToggleRead));
    let message = state.selected_message().unwrap();
    assert!(message.starred);
    assert!(!message.unread);

    state.dispatch(Action::Worker(WorkerEvent::MutationFailed {
        request_id: star_id,
        generation,
        message_id: id.clone(),
        uncertain: false,
    }));
    let message = state.selected_message().unwrap();
    assert!(!message.starred);
    assert!(!message.unread, "read dimension must remain optimistic");
    state.dispatch(Action::Worker(WorkerEvent::MutationConfirmed {
        request_id: read_id,
        generation,
        message_id: id,
    }));
    assert!(!state.selected_message().unwrap().unread);
}

#[test]
fn uncertain_mutation_is_never_rolled_back_or_replayed() {
    let mut state = AppState::new();
    ready(&mut state);
    state.selected_message_id = Some(MessageId::gmail(1));
    let (request_id, generation, message_id, _) =
        mutation_effect(state.dispatch(Action::ToggleStar));
    let settled = state.dispatch(Action::Worker(WorkerEvent::MutationFailed {
        request_id,
        generation,
        message_id,
        uncertain: true,
    }));
    assert!(state.selected_message().unwrap().starred);
    assert!(settled.effects.is_empty());
    assert!(settled.feedback.unwrap().contains("could not be verified"));
}

#[test]
fn archive_rolls_back_at_the_original_position_on_definite_failure() {
    let mut state = AppState::new();
    ready(&mut state);
    state.selected_message_id = Some(MessageId::gmail(2));
    let (request_id, generation, message_id, _) = mutation_effect(state.dispatch(Action::Archive));
    assert!(!state.visible_message_ids().contains(&message_id));
    state.dispatch(Action::Worker(WorkerEvent::MutationFailed {
        request_id,
        generation,
        message_id: message_id.clone(),
        uncertain: false,
    }));
    assert_eq!(state.mailbox.as_ref().unwrap().messages[1].id, message_id);
}

#[test]
fn archive_reconciliation_restores_authoritative_inbox_membership() {
    let mut state = AppState::new();
    ready(&mut state);
    state.selected_message_id = Some(MessageId::gmail(2));
    let (request_id, generation, message_id, _) = mutation_effect(state.dispatch(Action::Archive));
    state.dispatch(Action::Worker(WorkerEvent::MutationReconciled {
        request_id,
        generation,
        message_id: message_id.clone(),
        state: Some(ReconciledMessageState {
            unread: true,
            starred: true,
            in_inbox: true,
            in_trash: false,
            labels: vec!["Work".into()],
        }),
    }));
    let restored = state
        .mailbox
        .as_ref()
        .unwrap()
        .messages
        .iter()
        .find(|message| message.id == message_id)
        .unwrap();
    assert!(restored.unread && restored.starred);
    assert_eq!(restored.labels, ["Work"]);
}

#[test]
fn archive_reconciliation_does_not_restore_an_authoritatively_archived_inbox_row() {
    let mut state = AppState::new();
    ready(&mut state);
    state.selected_message_id = Some(MessageId::gmail(2));
    let (request_id, generation, message_id, _) = mutation_effect(state.dispatch(Action::Archive));
    state.dispatch(Action::Worker(WorkerEvent::MutationReconciled {
        request_id,
        generation,
        message_id: message_id.clone(),
        state: Some(ReconciledMessageState {
            unread: false,
            starred: false,
            in_inbox: false,
            in_trash: false,
            labels: Vec::new(),
        }),
    }));
    assert!(!state.visible_message_ids().contains(&message_id));
}

#[test]
fn confirmed_archive_can_be_undone_with_the_catalog_aware_restore_command() {
    let mut state = ready_for_search();
    state.selected_message_id = Some(MessageId::gmail(2));
    let (forward_request, generation, message_id, _) =
        mutation_effect(state.dispatch(Action::Archive));
    assert!(!state.visible_message_ids().contains(&message_id));
    let available = state.dispatch(Action::Worker(WorkerEvent::MutationConfirmed {
        request_id: forward_request,
        generation,
        message_id: message_id.clone(),
    }));
    assert_eq!(
        available.feedback,
        Some("Message changed — Undo is available")
    );

    let undo = state.dispatch(Action::UndoMessageOperation);
    let (undo_request, undo_generation) = match undo.effects.as_slice() {
        [
            Effect::SendWorker(WorkerCommand::MutateMessageInCatalog {
                request_id,
                generation,
                message_id: id,
                mutation: MessageMutation::RestoreArchive { inbox_mailbox },
                catalog,
                ..
            }),
        ] => {
            assert_eq!(id, &message_id);
            assert_eq!(inbox_mailbox, "INBOX");
            assert!(catalog.find(&FolderId::AllMail).is_some());
            (*request_id, *generation)
        }
        effect => panic!("expected catalog-aware archive restore, got {effect:?}"),
    };
    assert!(state.visible_message_ids().contains(&message_id));
    let completed = state.dispatch(Action::Worker(WorkerEvent::MutationConfirmed {
        request_id: undo_request,
        generation: undo_generation,
        message_id,
    }));
    assert_eq!(completed.feedback, Some("Message action undone"));
}

#[test]
fn undo_requested_before_archive_confirmation_waits_then_dispatches_inverse() {
    let mut state = ready_for_search();
    state.selected_message_id = Some(MessageId::gmail(2));
    let (forward_request, generation, message_id, _) =
        mutation_effect(state.dispatch(Action::Archive));
    let waiting = state.dispatch(Action::UndoMessageOperation);
    assert!(waiting.effects.is_empty());
    assert!(state.visible_message_ids().contains(&message_id));

    let dispatched = state.dispatch(Action::Worker(WorkerEvent::MutationConfirmed {
        request_id: forward_request,
        generation,
        message_id: message_id.clone(),
    }));
    assert!(matches!(
        dispatched.effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::MutateMessageInCatalog {
            mutation: MessageMutation::RestoreArchive { .. },
            ..
        })]
    ));
    assert!(state.visible_message_ids().contains(&message_id));
}

#[test]
fn uncertain_forward_after_preconfirmation_undo_keeps_the_forward_presentation() {
    let mut state = ready_for_search();
    state.selected_message_id = Some(MessageId::gmail(2));
    let (request_id, generation, message_id, _) = mutation_effect(state.dispatch(Action::Archive));
    state.dispatch(Action::UndoMessageOperation);
    assert!(state.visible_message_ids().contains(&message_id));
    state.dispatch(Action::Worker(WorkerEvent::MutationFailed {
        request_id,
        generation,
        message_id: message_id.clone(),
        uncertain: true,
    }));
    assert!(
        !state.visible_message_ids().contains(&message_id),
        "an uncertain forward must not leave the optimistic Undo row visible"
    );
}

#[test]
fn confirmed_trash_can_be_undone_with_the_original_labels() {
    let mut state = ready_for_search();
    let message_id = MessageId::gmail(2);
    state
        .mailbox
        .as_mut()
        .unwrap()
        .messages
        .iter_mut()
        .find(|message| message.id == message_id)
        .unwrap()
        .labels = vec!["Projects/Rust".into()];
    state.selected_message_id = Some(message_id.clone());
    let (forward_request, generation, _, _) = mutation_effect(state.dispatch(Action::MoveToTrash));
    state.dispatch(Action::Worker(WorkerEvent::MutationConfirmed {
        request_id: forward_request,
        generation,
        message_id: message_id.clone(),
    }));
    let undo = state.dispatch(Action::UndoMessageOperation);
    assert!(matches!(
        undo.effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::MutateMessageInCatalog {
            mutation: MessageMutation::RestoreFromTrash {
                inbox_mailbox,
                trash_mailbox,
                restore_inbox,
                labels,
            }, ..
        })] if inbox_mailbox == "INBOX"
            && trash_mailbox == "[Gmail]/Trash"
            && *restore_inbox
            && labels == &vec!["Projects/Rust".to_owned()]
    ));
    assert!(state.visible_message_ids().contains(&message_id));
}

#[test]
fn trash_undo_keeps_a_non_inbox_message_out_of_inbox() {
    let mut state = ready_for_search();
    let message_id = MessageId::gmail(2);
    let message = state
        .mailbox
        .as_mut()
        .unwrap()
        .messages
        .iter_mut()
        .find(|message| message.id == message_id)
        .unwrap();
    message.folder_id = FolderId::AllMail;
    message.locator.folder_id = FolderId::AllMail;
    message.locator.mailbox = "[Gmail]/All Mail".into();
    message.in_inbox = false;
    state.selected_message_id = Some(message_id.clone());
    let (forward_request, generation, _, _) = mutation_effect(state.dispatch(Action::MoveToTrash));
    state.dispatch(Action::Worker(WorkerEvent::MutationConfirmed {
        request_id: forward_request,
        generation,
        message_id,
    }));
    let undo = state.dispatch(Action::UndoMessageOperation);
    assert!(matches!(
        undo.effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::MutateMessageInCatalog {
            mutation: MessageMutation::RestoreFromTrash {
                restore_inbox: false,
                ..
            },
            ..
        })]
    ));
}

#[test]
fn failed_undo_reinstates_the_confirmed_archive_presentation() {
    let mut state = ready_for_search();
    state.selected_message_id = Some(MessageId::gmail(2));
    let (forward_request, generation, message_id, _) =
        mutation_effect(state.dispatch(Action::Archive));
    state.dispatch(Action::Worker(WorkerEvent::MutationConfirmed {
        request_id: forward_request,
        generation,
        message_id: message_id.clone(),
    }));
    let undo = state.dispatch(Action::UndoMessageOperation);
    let (undo_request, undo_generation) = match undo.effects.as_slice() {
        [
            Effect::SendWorker(WorkerCommand::MutateMessageInCatalog {
                request_id,
                generation,
                ..
            }),
        ] => (*request_id, *generation),
        effect => panic!("expected undo request, got {effect:?}"),
    };
    assert!(state.visible_message_ids().contains(&message_id));
    let failed = state.dispatch(Action::Worker(WorkerEvent::MutationFailed {
        request_id: undo_request,
        generation: undo_generation,
        message_id: message_id.clone(),
        uncertain: false,
    }));
    assert_eq!(
        failed.feedback,
        Some("Gmail could not undo the message action")
    );
    assert!(!state.visible_message_ids().contains(&message_id));
}

fn load_folder_for_test(state: &mut AppState, id: FolderId, mailbox: &str, subject: &str) {
    let (request_id, generation, folder_id) =
        folder_effect(state.dispatch(Action::SelectFolder(id)));
    state.dispatch(Action::Worker(WorkerEvent::FolderLoaded {
        request_id,
        generation,
        folder_id: folder_id.clone(),
        snapshot: folder_snapshot(folder_id, mailbox, subject),
    }));
}

#[test]
fn trash_failure_after_navigation_never_inserts_inbox_row_into_sent() {
    let mut state = ready_for_search();
    state.selected_message_id = Some(MessageId::gmail(2));
    let (request_id, generation, message_id, mutation) =
        mutation_effect(state.dispatch(Action::MoveToTrash));
    assert!(matches!(mutation, MessageMutation::MoveToTrash { .. }));
    load_folder_for_test(&mut state, FolderId::Sent, "[Gmail]/Sent Mail", "Sent only");

    state.dispatch(Action::Worker(WorkerEvent::MutationFailed {
        request_id,
        generation,
        message_id,
        uncertain: false,
    }));
    let snapshot = state.snapshot();
    assert_eq!(snapshot.selected_folder_id, FolderId::Sent);
    assert_eq!(snapshot.visible_messages.len(), 1);
    assert_eq!(snapshot.visible_messages[0].subject, "Sent only");
    assert_eq!(snapshot.visible_messages[0].folder_id, FolderId::Sent);
}

#[test]
fn label_failure_after_navigation_does_not_mutate_the_label_view() {
    let mut state = ready_for_search();
    state.selected_message_id = Some(MessageId::gmail(1));
    let (request_id, generation, message_id, mutation) =
        mutation_effect(state.dispatch(Action::ToggleLabel("Projects/Rust".into())));
    assert!(matches!(mutation, MessageMutation::SetLabel { .. }));
    load_folder_for_test(
        &mut state,
        FolderId::Label("Projects/Rust".into()),
        "Projects/Rust",
        "Label only",
    );
    let labels_before = state.snapshot().visible_messages[0].labels.clone();

    state.dispatch(Action::Worker(WorkerEvent::MutationFailed {
        request_id,
        generation,
        message_id,
        uncertain: false,
    }));
    assert_eq!(state.snapshot().visible_messages[0].labels, labels_before);
}

#[test]
fn trash_and_label_optimistic_changes_roll_back_only_in_the_originating_inbox() {
    let mut state = ready_for_search();
    state.selected_message_id = Some(MessageId::gmail(1));
    let (label_request, generation, label_id, _) =
        mutation_effect(state.dispatch(Action::ToggleLabel("Projects/Rust".into())));
    assert!(
        state
            .selected_message()
            .unwrap()
            .labels
            .contains(&"Projects/Rust".into())
    );
    state.dispatch(Action::Worker(WorkerEvent::MutationFailed {
        request_id: label_request,
        generation,
        message_id: label_id,
        uncertain: false,
    }));
    assert!(state.selected_message().unwrap().labels.is_empty());

    state.selected_message_id = Some(MessageId::gmail(2));
    let (trash_request, generation, trashed_id, _) =
        mutation_effect(state.dispatch(Action::MoveToTrash));
    assert!(!state.visible_message_ids().contains(&trashed_id));
    state.dispatch(Action::Worker(WorkerEvent::MutationFailed {
        request_id: trash_request,
        generation,
        message_id: trashed_id.clone(),
        uncertain: false,
    }));
    assert_eq!(
        state
            .mailbox
            .as_ref()
            .unwrap()
            .messages
            .iter()
            .filter(|message| message.id == trashed_id)
            .count(),
        1
    );
}

#[test]
fn reconciliation_after_navigation_never_inserts_an_inbox_row_into_trash() {
    let mut state = ready_for_search();
    state.selected_message_id = Some(MessageId::gmail(2));
    let (request_id, generation, message_id, _) = mutation_effect(state.dispatch(Action::Archive));
    load_folder_for_test(&mut state, FolderId::Trash, "[Gmail]/Trash", "Trash only");
    state.dispatch(Action::Worker(WorkerEvent::MutationReconciled {
        request_id,
        generation,
        message_id,
        state: Some(ReconciledMessageState {
            unread: true,
            starred: false,
            in_inbox: true,
            in_trash: false,
            labels: vec!["Work".into()],
        }),
    }));
    assert_eq!(state.snapshot().visible_messages.len(), 1);
    assert_eq!(state.snapshot().visible_messages[0].subject, "Trash only");
    assert_eq!(
        state.snapshot().visible_messages[0].folder_id,
        FolderId::Trash
    );
}

#[test]
fn trash_uncertain_and_missing_target_settle_without_reinsert_or_duplicate() {
    let mut state = ready_for_search();
    state.selected_message_id = Some(MessageId::gmail(2));
    let (request_id, generation, message_id, _) =
        mutation_effect(state.dispatch(Action::MoveToTrash));
    state.dispatch(Action::Worker(WorkerEvent::MutationFailed {
        request_id,
        generation,
        message_id: message_id.clone(),
        uncertain: true,
    }));
    assert!(!state.visible_message_ids().contains(&message_id));

    state.selected_message_id = Some(MessageId::gmail(1));
    let (request_id, generation, missing, _) = mutation_effect(state.dispatch(Action::ToggleStar));
    state
        .mailbox
        .as_mut()
        .unwrap()
        .messages
        .retain(|message| message.id != missing);
    state.dispatch(Action::Worker(WorkerEvent::MutationFailed {
        request_id,
        generation,
        message_id: missing.clone(),
        uncertain: false,
    }));
    assert!(!state.visible_message_ids().contains(&missing));
}

#[test]
fn second_change_to_same_dimension_waits_for_settlement() {
    let mut state = AppState::new();
    ready(&mut state);
    state.selected_message_id = Some(MessageId::gmail(1));
    mutation_effect(state.dispatch(Action::ToggleStar));
    let second = state.dispatch(Action::ToggleStar);
    assert!(second.effects.is_empty());
    assert_eq!(second.feedback, Some("That change is already in progress"));
}
#[test]
fn complete_sync_replaces_mailbox_and_searches_loaded_messages() {
    let mut state = AppState::new();
    ready(&mut state);
    assert_eq!(state.snapshot().folders.len(), 1);
    assert_eq!(state.visible_message_ids().len(), 3);
    state.dispatch(Action::SetSearch("roadmap".into()));
    assert_eq!(state.visible_message_ids(), vec![MessageId::gmail(2)]);
    state.dispatch(Action::SetSearch("missing".into()));
    assert_eq!(state.snapshot().status, ViewStatus::NoSearchResults);
}

#[test]
fn render_snapshot_omits_rows_when_list_revision_is_unchanged() {
    let mut state = AppState::new();
    ready(&mut state);

    let snapshot = state.snapshot_for_render(false);

    assert!(snapshot.visible_messages.is_empty());
    assert_eq!(snapshot.status, ViewStatus::Ready);
    assert!(snapshot.selected_message.is_some());
}

#[test]
fn summary_sync_does_not_fetch_until_deliberate_open() {
    let mut state = AppState::new();
    ready(&mut state);
    assert!(matches!(state.snapshot().reader, ReaderState::Closed));
    let list_revision = state.snapshot().list_revision;
    let id = MessageId::gmail(1);
    let update = state.dispatch(Action::SelectMessage(id.clone()));
    let (request_id, generation) = match &update.effects[0] {
        Effect::SendWorker(WorkerCommand::FetchBody {
            request_id,
            generation,
            message_id,
            ..
        }) if message_id == &id => (*request_id, *generation),
        other => panic!("unexpected effect: {other:?}"),
    };
    assert_eq!(state.snapshot().list_revision, list_revision + 1);
    let body_revision = state.snapshot().reader_revision;
    state.dispatch(Action::Worker(WorkerEvent::BodyLoaded {
        request_id,
        generation,
        message_id: id.clone(),
        body: body(),
        usage: crate::model::CacheUsage {
            body_bytes: 12,
            body_count: 1,
            available: true,
            ..Default::default()
        },
        saved: true,
    }));
    let snapshot = state.snapshot();
    assert!(matches!(snapshot.reader, ReaderState::Loaded { id: loaded, .. } if loaded == id));
    assert_eq!(snapshot.list_revision, list_revision + 1);
    assert!(snapshot.reader_revision > body_revision);
    assert_eq!(snapshot.cache_usage.body_count, 1);
}

#[test]
fn retention_increase_after_decrease_refreshes_again() {
    let mut state = AppState::new();
    ready(&mut state);
    state.cache_limit = 500;
    state.mailbox.as_mut().unwrap().metadata.requested_limit = 500;

    state.dispatch(Action::SetCacheLimit(50));
    assert_eq!(state.snapshot().sync_metadata.unwrap().requested_limit, 50);

    let update = state.dispatch(Action::SetCacheLimit(100));
    assert!(update.effects.iter().any(|effect| matches!(
        effect,
        Effect::SendWorker(WorkerCommand::FetchFolder { .. })
    )));
    assert_eq!(state.snapshot().sync_metadata.unwrap().requested_limit, 100);
}

#[test]
fn offline_session_classifies_missing_runtime_auth_as_offline() {
    let mut state = AppState::new();
    ready(&mut state);
    let message_id = MessageId::gmail(1);
    let update = state.dispatch(Action::SelectMessage(message_id.clone()));
    let (request_id, generation) = match update.effects[0] {
        Effect::SendWorker(WorkerCommand::FetchBody {
            request_id,
            generation,
            ..
        }) => (request_id, generation),
        _ => panic!(),
    };
    let refresh = state.dispatch(Action::Refresh);
    let (folder_request, folder_generation, folder_id) = folder_effect(refresh);
    state.dispatch(Action::Worker(WorkerEvent::FolderFailed {
        request_id: folder_request,
        generation: folder_generation,
        folder_id,
        failure: BodyFailure::Offline,
    }));
    state.dispatch(Action::Worker(WorkerEvent::BodyFailed {
        request_id,
        generation,
        message_id: message_id.clone(),
        failure: BodyFailure::AuthorizationRequired,
    }));
    assert!(matches!(
        state.snapshot().reader,
        ReaderState::Failed {
            id,
            failure: BodyFailure::Offline
        } if id == message_id
    ));
}

#[test]
fn cached_folder_rows_survive_refresh_failures_and_stale_failures_are_ignored() {
    for failure in [
        BodyFailure::Offline,
        BodyFailure::TimedOut,
        BodyFailure::Protocol,
    ] {
        let mut state = ready_for_search();
        let (request_id, generation, folder_id) =
            folder_effect(state.dispatch(Action::SelectFolder(FolderId::Sent)));
        let cached = folder_snapshot(FolderId::Sent, "[Gmail]/Sent Mail", "Cached sent");
        state.dispatch(Action::Worker(WorkerEvent::FolderCacheLoaded {
            request_id,
            generation,
            folder_id: folder_id.clone(),
            snapshot: cached,
        }));
        state.dispatch(Action::Worker(WorkerEvent::FolderFailed {
            request_id,
            generation,
            folder_id,
            failure,
        }));
        assert_eq!(state.snapshot().visible_messages[0].subject, "Cached sent");

        let (new_request, new_generation, new_folder) =
            folder_effect(state.dispatch(Action::SelectFolder(FolderId::Trash)));
        state.dispatch(Action::Worker(WorkerEvent::FolderFailed {
            request_id,
            generation,
            folder_id: FolderId::Sent,
            failure: BodyFailure::Protocol,
        }));
        assert_eq!(state.snapshot().selected_folder_id, FolderId::Trash);
        state.dispatch(Action::Worker(WorkerEvent::FolderFailed {
            request_id: new_request,
            generation: new_generation,
            folder_id: new_folder,
            failure: BodyFailure::Protocol,
        }));
    }
}

#[test]
fn worker_failure_terminates_an_in_flight_reader_request() {
    let mut state = AppState::new();
    ready(&mut state);
    state.dispatch(Action::SelectMessage(MessageId::gmail(1)));
    let revision = state.snapshot().reader_revision;

    state.dispatch(Action::WorkerUnavailable);

    let snapshot = state.snapshot();
    assert!(matches!(
        snapshot.reader,
        ReaderState::Failed {
            failure: BodyFailure::Offline,
            ..
        }
    ));
    assert!(snapshot.reader_revision > revision);
}

#[test]
fn stale_body_results_and_post_clear_completions_are_ignored() {
    let mut state = AppState::new();
    ready(&mut state);
    let first = MessageId::gmail(1);
    let first_update = state.dispatch(Action::SelectMessage(first.clone()));
    let (first_request, old_generation) = match first_update.effects[0] {
        Effect::SendWorker(WorkerCommand::FetchBody {
            request_id,
            generation,
            ..
        }) => (request_id, generation),
        _ => panic!(),
    };
    state.dispatch(Action::SelectMessage(MessageId::gmail(2)));
    state.dispatch(Action::Worker(WorkerEvent::BodyLoaded {
        request_id: first_request,
        generation: old_generation,
        message_id: first.clone(),
        body: body(),
        usage: Default::default(),
        saved: true,
    }));
    assert!(!matches!(state.snapshot().reader, ReaderState::Loaded { id, .. } if id == first));

    let clear = state.dispatch(Action::ConfirmClearCache);
    assert!(matches!(
        clear.effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::ClearBodyCache { .. })]
    ));
    state.dispatch(Action::Worker(WorkerEvent::BodyLoaded {
        request_id: first_request,
        generation: old_generation,
        message_id: first,
        body: body(),
        usage: Default::default(),
        saved: true,
    }));
    assert!(matches!(state.snapshot().reader, ReaderState::Closed));
}
#[test]
fn stale_events_never_mutate_state() {
    let mut state = AppState::new();
    let current = ready(&mut state);
    state.dispatch(Action::Worker(WorkerEvent::Disconnected {
        id: OperationId(current.0 + 99),
    }));
    assert!(state.snapshot().account.is_some());
}
#[test]
fn refresh_failure_preserves_mail_and_enters_offline() {
    let mut state = AppState::new();
    ready(&mut state);
    let update = state.dispatch(Action::Refresh);
    let (request_id, generation, folder_id) = folder_effect(update);
    state.dispatch(Action::Worker(WorkerEvent::FolderFailed {
        request_id,
        generation,
        folder_id,
        failure: BodyFailure::Offline,
    }));
    assert_eq!(state.snapshot().status, ViewStatus::Offline);
    assert_eq!(state.visible_message_ids().len(), 3);
}
#[test]
fn disconnect_hides_local_mail_immediately_and_keeps_account_for_cleanup_retry() {
    let mut state = AppState::new();
    ready(&mut state);
    let update = state.dispatch(Action::ConfirmDisconnect);
    let id = match update.effects[0] {
        Effect::SendWorker(WorkerCommand::Disconnect { id, .. }) => id,
        _ => panic!(),
    };
    assert!(state.snapshot().visible_messages.is_empty());
    assert!(state.snapshot().selected_message.is_none());
    assert!(state.snapshot().account.is_some());
    state.dispatch(Action::Worker(WorkerEvent::Failed {
        id,
        failure: ServiceFailure {
            kind: FailureKind::DisconnectFailed,
            retryable: true,
            preserve_mail: true,
            cleanup_failed: false,
            config_path: None,
        },
    }));
    assert!(state.snapshot().account.is_some());
    let update = state.dispatch(Action::Retry);
    let id = match update.effects[0] {
        Effect::SendWorker(WorkerCommand::Disconnect { id, .. }) => id,
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::Disconnected { id }));
    assert!(state.snapshot().account.is_none());
    assert_eq!(state.snapshot().status, ViewStatus::Disconnected);
}

#[test]
fn failed_disconnect_retry_keeps_worker_and_reducer_generations_aligned() {
    let mut state = AppState::new();
    ready(&mut state);
    let first = state.dispatch(Action::ConfirmDisconnect);
    let (first_id, first_generation, first_draft_generation) = match first.effects[0] {
        Effect::SendWorker(WorkerCommand::Disconnect {
            id,
            generation,
            draft_generation,
            ..
        }) => (id, generation, draft_generation),
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::Failed {
        id: first_id,
        failure: ServiceFailure {
            kind: FailureKind::DisconnectFailed,
            retryable: true,
            preserve_mail: true,
            cleanup_failed: true,
            config_path: None,
        },
    }));

    let open = state.dispatch(Action::SelectMessage(MessageId::gmail(1)));
    assert!(open.effects.is_empty());
    assert!(state.snapshot().visible_messages.is_empty());

    let retry = state.dispatch(Action::Retry);
    let (second_generation, second_draft_generation) = match retry.effects[0] {
        Effect::SendWorker(WorkerCommand::Disconnect {
            generation,
            draft_generation,
            ..
        }) => (generation, draft_generation),
        _ => panic!(),
    };
    assert_eq!(second_generation, first_generation.wrapping_add(1));
    assert_eq!(
        second_draft_generation,
        first_draft_generation.wrapping_add(1)
    );
}

#[test]
fn account_switch_drops_old_mailbox_and_reader_before_body_requests() {
    let mut state = AppState::new();
    ready(&mut state);
    state.dispatch(Action::SelectMessage(MessageId::gmail(1)));
    let connect = state.dispatch(Action::Connect);
    let id = match connect.effects[0] {
        Effect::SendWorker(WorkerCommand::Connect { id }) => id,
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::AccountPersisted {
        id,
        account: AccountIdentity {
            provider: MailProvider::Gmail,
            email: "other@example.com".into(),
        },
    }));

    let snapshot = state.snapshot();
    assert!(snapshot.visible_messages.is_empty());
    assert!(snapshot.selected_message.is_none());
    assert!(matches!(snapshot.reader, ReaderState::Closed));
    assert_eq!(snapshot.account.unwrap().email, "other@example.com");
}

#[test]
fn verified_account_switch_hides_old_mail_even_if_credential_save_then_fails() {
    let mut state = AppState::new();
    let id = ready(&mut state);
    state.active_operation = Some(id);
    state.dispatch(Action::Worker(WorkerEvent::IdentityVerified {
        id,
        account: AccountIdentity {
            provider: MailProvider::Gmail,
            email: "other@example.com".into(),
        },
    }));
    assert!(state.snapshot().visible_messages.is_empty());
    assert_eq!(state.snapshot().account.unwrap().email, "other@example.com");

    state.dispatch(Action::Worker(WorkerEvent::Failed {
        id,
        failure: ServiceFailure {
            kind: FailureKind::CredentialSaveFailed,
            retryable: true,
            preserve_mail: false,
            cleanup_failed: false,
            config_path: None,
        },
    }));
    assert!(state.snapshot().visible_messages.is_empty());
}
#[test]
fn filters_and_navigation_are_safe() {
    let mut state = AppState::new();
    ready(&mut state);
    state.dispatch(Action::SetFilter(MessageFilter::Unread));
    assert_eq!(state.visible_message_ids().len(), 2);
    state.dispatch(Action::SelectNext);
    state.dispatch(Action::SelectPrevious);
    assert!(state.selected_message_id().is_some());
}
#[test]
fn escape_precedence_is_pure() {
    assert_eq!(
        escape_outcome(EscapeContext {
            search_active: true,
            reader_visible: true,
            folders_visible: true
        }),
        EscapeOutcome::ClearSearch
    );
    assert_eq!(
        escape_outcome(EscapeContext {
            search_active: false,
            reader_visible: true,
            folders_visible: true
        }),
        EscapeOutcome::ShowMessageList
    );
}

#[test]
fn authorization_event_order_keeps_cancel_and_reopen_enabled() {
    let mut state = AppState::new();
    let update = state.dispatch(Action::Connect);
    let id = match update.effects[0] {
        Effect::SendWorker(WorkerCommand::Connect { id }) => id,
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::AuthorizationRequired {
        id,
        url: crate::oauth::AuthorizationUrl::new("https://example.invalid".into()),
        deadline: SystemTime::UNIX_EPOCH,
    }));
    state.dispatch(Action::Worker(WorkerEvent::Phase {
        id,
        phase: WorkerPhase::WaitingForBrowser,
    }));
    let snapshot = state.snapshot();
    assert!(matches!(snapshot.session, SessionState::Authorizing { .. }));
    assert!(snapshot.can_cancel && snapshot.can_reopen);
}

#[test]
fn browser_launch_failure_survives_worker_cancellation_acknowledgement() {
    let mut state = AppState::new();
    let update = state.dispatch(Action::Connect);
    let authorization_id = match update.effects[0] {
        Effect::SendWorker(WorkerCommand::Connect { id }) => id,
        _ => panic!(),
    };
    let update = state.dispatch(Action::BrowserLaunchFailed(authorization_id));
    let cancel_id = match update.effects[0] {
        Effect::SendWorker(WorkerCommand::Cancel { id }) => id,
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::Cancelled { id: cancel_id }));
    let snapshot = state.snapshot();
    assert!(matches!(
        snapshot.session,
        SessionState::ServiceError { failure }
            if failure.kind == FailureKind::BrowserLaunchFailed
    ));
    assert!(snapshot.can_retry);
}

#[test]
fn auth_required_retry_reconnects_and_offline_retains_rows() {
    let mut state = AppState::new();
    ready(&mut state);
    let update = state.dispatch(Action::Refresh);
    let (request_id, generation, folder_id) = folder_effect(update);
    state.dispatch(Action::Worker(WorkerEvent::FolderFailed {
        request_id,
        generation,
        folder_id,
        failure: BodyFailure::AuthorizationRequired,
    }));
    let degraded = state.snapshot();
    assert_eq!(degraded.status, ViewStatus::Degraded);
    assert_eq!(degraded.visible_messages.len(), 3);
    assert!(degraded.selected_message.is_some());
    let retry = state.dispatch(Action::Retry);
    assert!(matches!(
        retry.effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::Connect { .. })]
    ));

    let id = match retry.effects[0] {
        Effect::SendWorker(WorkerCommand::Connect { id }) => id,
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::Failed {
        id,
        failure: ServiceFailure {
            kind: FailureKind::Network,
            retryable: true,
            preserve_mail: true,
            cleanup_failed: false,
            config_path: None,
        },
    }));
    let snapshot = state.snapshot();
    assert_eq!(snapshot.status, ViewStatus::Offline);
    assert_eq!(snapshot.visible_messages.len(), 3);
    assert!(snapshot.selected_message.is_some());
}

#[test]
fn failed_disconnect_keeps_local_mail_hidden_and_offers_cleanup_retry() {
    let mut state = AppState::new();
    ready(&mut state);
    let update = state.dispatch(Action::ConfirmDisconnect);
    let id = match update.effects[0] {
        Effect::SendWorker(WorkerCommand::Disconnect { id, .. }) => id,
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::Failed {
        id,
        failure: ServiceFailure {
            kind: FailureKind::DisconnectFailed,
            retryable: true,
            preserve_mail: true,
            cleanup_failed: true,
            config_path: None,
        },
    }));
    let snapshot = state.snapshot();
    assert_eq!(snapshot.status, ViewStatus::Error);
    assert!(snapshot.visible_messages.is_empty());
    assert!(snapshot.selected_message.is_none());
    assert!(snapshot.can_retry);
}

#[test]
fn failed_partial_save_offers_cleanup_without_connecting_again() {
    let mut state = AppState::new();
    let update = state.dispatch(Action::Connect);
    let id = match update.effects[0] {
        Effect::SendWorker(WorkerCommand::Connect { id }) => id,
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::Failed {
        id,
        failure: ServiceFailure {
            kind: FailureKind::DisconnectFailed,
            retryable: true,
            preserve_mail: false,
            cleanup_failed: true,
            config_path: None,
        },
    }));
    let snapshot = state.snapshot();
    assert!(snapshot.can_retry);
    assert!(!snapshot.can_connect);
    assert!(matches!(
        state.dispatch(Action::Retry).effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::Disconnect { .. })]
    ));
}

#[test]
fn worker_channel_failure_never_leaves_a_spinner() {
    let mut state = AppState::new();
    state.dispatch(Action::Startup);
    state.dispatch(Action::WorkerUnavailable);
    let snapshot = state.snapshot();
    assert!(
        matches!(snapshot.session, SessionState::ServiceError { failure } if failure.kind == FailureKind::WorkerUnavailable)
    );
    assert_eq!(snapshot.status, ViewStatus::Error);
    assert!(!snapshot.can_retry);
    assert!(!snapshot.can_connect);
}

#[test]
fn worker_channel_failure_keeps_loaded_mail_stale() {
    let mut state = AppState::new();
    ready(&mut state);
    state.dispatch(Action::WorkerUnavailable);
    let snapshot = state.snapshot();
    assert_eq!(snapshot.status, ViewStatus::Offline);
    assert_eq!(snapshot.visible_messages.len(), 3);
    assert!(!snapshot.can_retry);
    assert!(!snapshot.can_connect);
}

#[test]
fn retry_preserves_restore_and_refresh_operations() {
    let mut state = AppState::new();
    let update = state.dispatch(Action::Startup);
    let restore_id = match update.effects[0] {
        Effect::SendWorker(WorkerCommand::Restore { id }) => id,
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::Failed {
        id: restore_id,
        failure: ServiceFailure {
            kind: FailureKind::Network,
            retryable: true,
            preserve_mail: false,
            cleanup_failed: false,
            config_path: None,
        },
    }));
    assert!(matches!(
        state.dispatch(Action::Retry).effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::Restore { .. })]
    ));

    let mut state = AppState::new();
    ready(&mut state);
    let update = state.dispatch(Action::Refresh);
    let (request_id, generation, folder_id) = folder_effect(update);
    state.dispatch(Action::Worker(WorkerEvent::FolderFailed {
        request_id,
        generation,
        folder_id,
        failure: BodyFailure::Offline,
    }));
    assert!(matches!(
        state.dispatch(Action::Retry).effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::FetchFolder { .. })]
    ));
}

#[test]
fn startup_configuration_error_only_retries_saved_token_restore() {
    let mut state = AppState::new();
    let update = state.dispatch(Action::Startup);
    let restore_id = match update.effects[0] {
        Effect::SendWorker(WorkerCommand::Restore { id }) => id,
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::Failed {
        id: restore_id,
        failure: ServiceFailure {
            kind: FailureKind::ConfigurationInvalid,
            retryable: true,
            preserve_mail: false,
            cleanup_failed: false,
            config_path: Some("/safe/google-oauth.json".into()),
        },
    }));

    let snapshot = state.snapshot();
    assert!(matches!(
        snapshot.session,
        SessionState::ConfigurationError { .. }
    ));
    assert!(snapshot.can_retry);
    assert!(!snapshot.can_connect);
    assert!(matches!(
        state.dispatch(Action::Retry).effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::Restore { .. })]
    ));
}

#[test]
fn authentication_rejections_recover_with_connect() {
    for _kind in [
        FailureKind::AuthorizationExpired,
        FailureKind::IdentityInvalid,
        FailureKind::ImapAuthenticationFailed,
    ] {
        let mut state = AppState::new();
        ready(&mut state);
        let update = state.dispatch(Action::Refresh);
        let (request_id, generation, folder_id) = folder_effect(update);
        state.dispatch(Action::Worker(WorkerEvent::FolderFailed {
            request_id,
            generation,
            folder_id,
            failure: BodyFailure::AuthorizationRequired,
        }));
        assert!(matches!(
            state.snapshot().session,
            SessionState::AuthRequired { .. }
        ));
        assert!(matches!(
            state.dispatch(Action::Retry).effects.as_slice(),
            [Effect::SendWorker(WorkerCommand::Connect { .. })]
        ));
    }
}

#[test]
fn loaded_message_search_handles_unicode_and_rtl() {
    let mut state = AppState::new();
    ready(&mut state);
    state.mailbox.as_mut().unwrap().messages[0].sender = "ليلى Müller".into();
    state.dispatch(Action::SetSearch("MÜLLER".into()));
    assert_eq!(state.visible_message_ids().len(), 1);
    state.dispatch(Action::SetSearch("ليلى".into()));
    assert_eq!(state.visible_message_ids().len(), 1);
}

#[test]
fn reply_requires_loaded_context_and_prevents_duplicate_send() {
    let mut state = AppState::new();
    ready(&mut state);
    assert_eq!(
        state.dispatch(Action::BeginReply).feedback,
        Some("Load the message before replying or forwarding")
    );

    load_replyable(&mut state);
    state.dispatch(Action::BeginReply);
    assert!(matches!(
        state.snapshot().composer,
        ComposerState::Editing { .. }
    ));
    state.dispatch(Action::UpdateMessageBody("Thanks".into()));
    let send = state.dispatch(Action::SendMessage);
    let (request_id, generation) = match send.effects.as_slice() {
        [
            Effect::SendWorker(WorkerCommand::SendMessage {
                request_id,
                generation,
                submission,
                ..
            }),
        ] if submission.draft.text == "Thanks" => (*request_id, *generation),
        other => panic!("unexpected send effect: {other:?}"),
    };
    assert!(state.dispatch(Action::SendMessage).effects.is_empty());
    assert!(
        matches!(state.snapshot().composer, ComposerState::Sending { request_id: current, .. } if current == request_id)
    );

    state.dispatch(Action::Worker(WorkerEvent::MessageSendFailed {
        request_id,
        generation,
        failure: SendFailure::Rejected,
    }));
    assert!(
        matches!(state.snapshot().composer, ComposerState::Failed { ref draft, failure: SendFailure::Rejected } if draft.body == "Thanks")
    );
}

#[test]
fn reply_rejects_empty_and_ignores_stale_completion_then_refreshes_on_success() {
    let mut state = AppState::new();
    load_replyable(&mut state);
    state.dispatch(Action::BeginReply);
    let empty = state.dispatch(Action::SendMessage);
    assert!(empty.effects.is_empty());
    assert!(matches!(
        state.snapshot().composer,
        ComposerState::Failed {
            failure: SendFailure::Empty,
            ..
        }
    ));

    state.dispatch(Action::UpdateMessageBody("Hello".into()));
    let send = state.dispatch(Action::SendMessage);
    let (request_id, generation) = match send.effects[0] {
        Effect::SendWorker(WorkerCommand::SendMessage {
            request_id,
            generation,
            ..
        }) => (request_id, generation),
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::MessageSent {
        request_id: SendRequestId(request_id.0 + 1),
        generation,
    }));
    assert!(matches!(
        state.snapshot().composer,
        ComposerState::Sending { .. }
    ));

    let complete = state.dispatch(Action::Worker(WorkerEvent::MessageSent {
        request_id,
        generation,
    }));
    assert_eq!(complete.feedback, Some("Message sent"));
    assert!(complete.effects.iter().any(|effect| matches!(
        effect,
        Effect::SendWorker(WorkerCommand::FetchFolder { .. })
    )));
    assert!(complete.effects.iter().any(|effect| matches!(
        effect,
        Effect::SendWorker(WorkerCommand::DeleteDraft { .. })
    )));
    assert!(matches!(state.snapshot().composer, ComposerState::Closed));
}

#[test]
fn uncertain_reply_requires_explicit_resend_confirmation() {
    let mut state = AppState::new();
    load_replyable(&mut state);
    state.dispatch(Action::BeginReply);
    state.dispatch(Action::UpdateMessageBody("Hello".into()));
    let send = state.dispatch(Action::SendMessage);
    let (request_id, generation) = match send.effects[0] {
        Effect::SendWorker(WorkerCommand::SendMessage {
            request_id,
            generation,
            ..
        }) => (request_id, generation),
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::MessageSendFailed {
        request_id,
        generation,
        failure: SendFailure::DeliveryUncertain,
    }));

    assert!(matches!(
        state.dispatch(Action::SendMessage).effects.as_slice(),
        [Effect::PresentUncertainResendConfirmation]
    ));
    assert!(matches!(
        state.snapshot().composer,
        ComposerState::Failed {
            failure: SendFailure::DeliveryUncertain,
            ..
        }
    ));
    assert!(matches!(
        state.dispatch(Action::ConfirmResend).effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::SendMessage { .. })]
    ));
}

#[test]
fn disconnect_is_blocked_without_discarding_an_in_flight_reply() {
    let mut state = AppState::new();
    load_replyable(&mut state);
    state.dispatch(Action::BeginReply);
    state.dispatch(Action::UpdateMessageBody("Keep this draft".into()));
    state.dispatch(Action::SendMessage);

    let request = state.dispatch(Action::RequestDisconnect);
    assert_eq!(
        request.feedback,
        Some("Wait for the message to finish sending before disconnecting")
    );
    assert!(request.effects.is_empty());
    let confirm = state.dispatch(Action::ConfirmDisconnect);
    assert!(confirm.effects.is_empty());
    assert!(matches!(
        state.snapshot().composer,
        ComposerState::Sending { ref draft, .. } if draft.body == "Keep this draft"
    ));
    assert!(!state.snapshot().can_disconnect);
}

#[test]
fn reconnect_is_blocked_until_smtp_has_a_definite_outcome() {
    let mut state = AppState::new();
    load_replyable(&mut state);
    state.dispatch(Action::BeginReply);
    state.dispatch(Action::UpdateMessageBody("Still sending".into()));
    state.dispatch(Action::SendMessage);

    let connect = state.dispatch(Action::Connect);
    assert!(connect.effects.is_empty());
    assert_eq!(
        connect.feedback,
        Some("Wait for the message to finish sending before reconnecting")
    );
    assert!(matches!(
        state.snapshot().composer,
        ComposerState::Sending { .. }
    ));
}

#[test]
fn reply_all_and_forward_create_distinct_generic_drafts() {
    let mut state = AppState::new();
    load_replyable(&mut state);
    state.dispatch(Action::BeginReplyAll);
    let snapshot = state.snapshot();
    let ComposerState::Editing { draft } = snapshot.composer else {
        panic!()
    };
    assert!(matches!(
        draft.compose.kind,
        crate::composer::ComposeKind::ReplyAll { .. }
    ));
    assert!(draft.compose.thread.is_some());
    state.dispatch(Action::DiscardDraft);

    state.dispatch(Action::BeginForward);
    let snapshot = state.snapshot();
    let ComposerState::Editing { draft } = snapshot.composer else {
        panic!()
    };
    assert!(matches!(
        draft.compose.kind,
        crate::composer::ComposeKind::Forward { .. }
    ));
    assert!(draft.compose.to.is_empty());
    assert!(draft.compose.thread.is_none());
}

#[test]
fn standalone_compose_is_blank_unthreaded_and_does_not_need_an_open_message() {
    let mut state = AppState::new();
    ready(&mut state);
    finish_draft_restore(&mut state, Vec::new());
    state.reader = ReaderState::Closed;

    let update = state.dispatch(Action::BeginNewMessage);
    assert!(matches!(
        update.effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::SaveDraft { .. })]
    ));
    let ComposerState::Editing { draft } = state.snapshot().composer else {
        panic!("composer did not open")
    };
    assert_eq!(draft.compose.kind, crate::composer::ComposeKind::New);
    assert!(draft.source_message_id.is_none());
    assert!(draft.context.is_none());
    assert!(draft.compose.to.is_empty());
    assert!(draft.compose.subject.is_empty());
    assert!(draft.compose.thread.is_none());
    assert!(!state.snapshot().can_compose);
}

#[test]
fn standalone_send_reuses_generic_smtp_submission_without_thread_headers() {
    let mut state = AppState::new();
    ready(&mut state);
    finish_draft_restore(&mut state, Vec::new());
    state.dispatch(Action::BeginNewMessage);
    state.dispatch(Action::UpdateRecipients {
        to: vec![crate::composer::Recipient {
            name: Some("Friend".into()),
            email: "friend@example.com".into(),
        }],
        cc: Vec::new(),
        bcc: Vec::new(),
    });
    state.dispatch(Action::UpdateSubject("Hello".into()));
    state.dispatch(Action::UpdateMessageBody("A standalone message".into()));

    let sent = state.dispatch(Action::SendMessage);
    let (request_id, generation, submission) = match sent.effects.into_iter().next().unwrap() {
        Effect::SendWorker(WorkerCommand::SendMessage {
            request_id,
            generation,
            submission,
        }) => (request_id, generation, submission),
        other => panic!("unexpected effect: {other:?}"),
    };
    assert_eq!(submission.draft.kind, crate::composer::ComposeKind::New);
    assert!(submission.draft.thread.is_none());
    assert_eq!(submission.draft.to[0].email, "friend@example.com");

    let completed = state.dispatch(Action::Worker(WorkerEvent::MessageSent {
        request_id,
        generation,
    }));
    assert!(matches!(state.snapshot().composer, ComposerState::Closed));
    assert!(completed.effects.iter().any(|effect| matches!(
        effect,
        Effect::SendWorker(WorkerCommand::FetchFolder { .. })
    )));
}

#[test]
fn repeated_new_message_actions_create_isolated_drafts() {
    let mut state = AppState::new();
    ready(&mut state);
    finish_draft_restore(&mut state, Vec::new());
    state.dispatch(Action::BeginNewMessage);
    let first = match state.snapshot().composer {
        ComposerState::Editing { draft } => draft.compose.id,
        _ => panic!(),
    };
    state.dispatch(Action::CancelCompose);
    state.dispatch(Action::BeginNewMessage);
    let second = match state.snapshot().composer {
        ComposerState::Editing { draft } => draft.compose.id,
        _ => panic!(),
    };
    assert_ne!(first, second);
    assert_eq!(state.saved_drafts.len(), 2);
}

#[test]
fn hiding_saves_and_resume_restores_local_draft_then_discard_deletes_it() {
    let mut state = AppState::new();
    load_replyable(&mut state);
    state.dispatch(Action::BeginReply);
    state.dispatch(Action::UpdateMessageBody("kept locally".into()));
    let hidden = state.dispatch(Action::HideComposer);
    let (operation_id, draft_id, revision) = hidden
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::SendWorker(WorkerCommand::SaveDraft {
                operation_id,
                draft,
                ..
            }) => Some((*operation_id, draft.id.clone(), draft.dirty_revision)),
            _ => None,
        })
        .expect("close save");
    assert!(matches!(state.snapshot().composer, ComposerState::Closed));
    assert!(!state.snapshot().composer_close_pending);
    let saved = state.dispatch(Action::Worker(WorkerEvent::DraftSaved {
        operation_id,
        generation: state.draft_generation,
        account_email: "person@example.com".into(),
        draft_id,
        revision,
    }));
    assert_eq!(saved.feedback, None);
    assert!(matches!(state.snapshot().composer, ComposerState::Closed));

    state.dispatch(Action::ResumeDraft);
    assert!(matches!(state.snapshot().composer,
        ComposerState::Editing { ref draft } if draft.body == "kept locally"));
    let discarded = state.dispatch(Action::DiscardDraft);
    assert!(discarded.effects.iter().any(|effect| matches!(
        effect,
        Effect::SendWorker(WorkerCommand::DeleteDraft { .. })
    )));
    assert!(matches!(state.snapshot().composer, ComposerState::Closed));
}

#[test]
fn staging_blocks_send_until_the_attachment_finishes_or_fails() {
    let mut state = AppState::new();
    load_replyable(&mut state);
    state.dispatch(Action::BeginReply);
    state.dispatch(Action::UpdateMessageBody("with a file".into()));
    let stage = state.dispatch(Action::StageAttachment {
        source: "/tmp/report.pdf".into(),
        display_name: "report.pdf".into(),
        media_type: "application/pdf".into(),
    });
    let operation_id = stage
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::SendWorker(WorkerCommand::StageAttachment { operation_id, .. }) => {
                Some(*operation_id)
            }
            _ => None,
        })
        .expect("stage operation");
    assert_eq!(state.snapshot().pending_attachment_staging, 1);
    let blocked = state.dispatch(Action::SendMessage);
    assert_eq!(
        blocked.feedback,
        Some("Wait for attachments to finish loading")
    );
    assert!(blocked.effects.is_empty());
    assert!(matches!(
        state.snapshot().composer,
        ComposerState::Editing { .. }
    ));

    state.dispatch(Action::Worker(WorkerEvent::DraftOperationFailed {
        operation_id,
        generation: state.draft_generation,
        account_email: "person@example.com".into(),
    }));
    assert_eq!(state.snapshot().pending_attachment_staging, 0);
    assert!(matches!(
        state.dispatch(Action::SendMessage).effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::SendMessage { .. })]
    ));
}

#[test]
fn close_waits_for_acknowledged_save_and_failure_keeps_composer_visible() {
    let mut state = AppState::new();
    load_replyable(&mut state);
    state.dispatch(Action::BeginReply);
    state.dispatch(Action::UpdateMessageBody("must survive".into()));
    let closing = state.dispatch(Action::HideComposerAndCloseApp);
    let operation_id = closing
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::SendWorker(WorkerCommand::SaveDraft { operation_id, .. }) => {
                Some(*operation_id)
            }
            _ => None,
        })
        .expect("save before close");
    assert!(state.snapshot().composer_close_pending);
    assert_eq!(state.snapshot().draft_save_state, DraftSaveState::Saving);

    state.dispatch(Action::Worker(WorkerEvent::DraftOperationFailed {
        operation_id,
        generation: state.draft_generation,
        account_email: "person@example.com".into(),
    }));
    let snapshot = state.snapshot();
    assert!(!snapshot.composer_close_pending);
    assert_eq!(snapshot.draft_save_state, DraftSaveState::Failed);
    assert!(matches!(snapshot.composer, ComposerState::Editing { .. }));

    let retry = state.dispatch(Action::HideComposerAndCloseApp);
    let (operation_id, draft_id, revision) = retry
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::SendWorker(WorkerCommand::SaveDraft {
                operation_id,
                draft,
                ..
            }) => Some((*operation_id, draft.id.clone(), draft.dirty_revision)),
            _ => None,
        })
        .expect("retry save");
    let saved = state.dispatch(Action::Worker(WorkerEvent::DraftSaved {
        operation_id,
        generation: state.draft_generation,
        account_email: "person@example.com".into(),
        draft_id,
        revision,
    }));
    assert!(matches!(
        saved.effects.as_slice(),
        [Effect::CloseApplicationWindow]
    ));
    assert!(matches!(state.snapshot().composer, ComposerState::Closed));
}

#[test]
fn worker_loss_releases_composer_save_and_staging_locks() {
    let mut state = AppState::new();
    load_replyable(&mut state);
    state.dispatch(Action::BeginReply);
    state.dispatch(Action::StageAttachment {
        source: "/tmp/report.pdf".into(),
        display_name: "report.pdf".into(),
        media_type: "application/pdf".into(),
    });
    state.dispatch(Action::HideComposer);
    assert!(state.snapshot().pending_attachment_staging > 0);
    state.dispatch(Action::WorkerUnavailable);
    let snapshot = state.snapshot();
    assert_eq!(snapshot.pending_attachment_staging, 0);
    assert!(!snapshot.composer_close_pending);
    assert_eq!(snapshot.draft_save_state, DraftSaveState::Failed);
    assert!(matches!(snapshot.composer, ComposerState::Editing { .. }));
}

#[test]
fn inline_stage_creates_cid_marker_and_resume_summary_is_exposed() {
    let mut state = AppState::new();
    load_replyable(&mut state);
    state.dispatch(Action::BeginReply);
    let draft_id = match state.snapshot().composer {
        ComposerState::Editing { draft } => draft.compose.id,
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::AttachmentStaged {
        account_email: "person@example.com".into(),
        operation_id: DraftOperationId(77),
        generation: state.draft_generation,
        draft_id,
        attachment: crate::composer::DraftAttachment {
            id: "image-1".into(),
            display_name: "photo.png".into(),
            media_type: "image/png".into(),
            staged_file: "image-1".into(),
            bytes: 42,
        },
        inline: true,
    }));
    let snapshot = state.snapshot();
    let ComposerState::Editing { draft } = snapshot.composer else {
        panic!()
    };
    assert_eq!(
        draft.compose.inline_images[0].content_id,
        "whitford-image-1@local"
    );
    assert!(
        draft
            .compose
            .html
            .contains("src=\"cid:whitford-image-1@local\"")
    );
    assert!(snapshot.saved_draft.is_some());
    let html = draft.compose.html.clone();
    state.dispatch(Action::UpdateHtml {
        html,
        text: "inline image".into(),
    });
    assert!(matches!(state.snapshot().composer,
        ComposerState::Editing { draft }
            if draft.compose.html.contains("cid:whitford-image-1@local")));
    let removal = state.dispatch(Action::RemoveAttachment("image-1".into()));
    assert!(removal.effects.iter().any(|effect| matches!(
        effect,
        Effect::SendWorker(WorkerCommand::RemoveStaged { .. })
    )));
}

#[test]
fn manual_recipients_are_deduplicated_across_visible_and_bcc_fields() {
    let mut state = AppState::new();
    load_replyable(&mut state);
    state.dispatch(Action::BeginReply);
    let recipient = |email: &str| crate::composer::Recipient {
        name: None,
        email: email.into(),
    };
    state.dispatch(Action::UpdateRecipients {
        to: vec![recipient("same@example.com"), recipient("SAME@example.com")],
        cc: vec![recipient("same@example.com"), recipient("cc@example.com")],
        bcc: vec![recipient("CC@example.com"), recipient("secret@example.com")],
    });
    let ComposerState::Editing { draft } = state.snapshot().composer else {
        panic!()
    };
    assert_eq!(draft.compose.to.len(), 1);
    assert_eq!(draft.compose.cc.len(), 1);
    assert_eq!(draft.compose.bcc.len(), 1);
}

#[test]
fn oversized_editor_snapshot_is_rejected_without_replacing_the_draft() {
    let mut state = AppState::new();
    load_replyable(&mut state);
    state.dispatch(Action::BeginReply);
    let before = match state.snapshot().composer {
        ComposerState::Editing { draft } => draft.compose.html,
        _ => panic!(),
    };
    let update = state.dispatch(Action::UpdateHtml {
        html: "x".repeat(crate::composer::MAX_HTML_BYTES + 1),
        text: "x".into(),
    });
    assert_eq!(update.feedback, Some("Message body is too large"));
    assert!(matches!(state.snapshot().composer,
        ComposerState::Editing { draft } if draft.compose.html == before));
}

#[test]
fn beginning_another_kind_creates_an_isolated_draft() {
    let mut state = AppState::new();
    load_replyable(&mut state);
    state.dispatch(Action::BeginReply);
    let original_id = match state.snapshot().composer {
        ComposerState::Editing { draft } => draft.compose.id,
        _ => panic!(),
    };
    let hidden = state.dispatch(Action::HideComposer);
    let (operation_id, revision) = hidden
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::SendWorker(WorkerCommand::SaveDraft {
                operation_id,
                draft,
                ..
            }) => Some((*operation_id, draft.dirty_revision)),
            _ => None,
        })
        .expect("close save");
    state.dispatch(Action::Worker(WorkerEvent::DraftSaved {
        operation_id,
        generation: state.draft_generation,
        account_email: "person@example.com".into(),
        draft_id: original_id.clone(),
        revision,
    }));
    let update = state.dispatch(Action::BeginForward);
    assert_eq!(update.feedback, None);
    assert!(matches!(state.snapshot().composer,
        ComposerState::Editing { ref draft } if draft.compose.id != original_id));
    assert_eq!(state.saved_drafts.len(), 2);
}

#[test]
fn stale_cross_account_draft_and_signature_events_are_ignored() {
    let mut state = AppState::new();
    ready(&mut state);
    let draft_op = state.pending_draft_load.expect("draft load");
    let signature_op = state.pending_signature_load.expect("signature load");
    state.dispatch(Action::Worker(WorkerEvent::DraftsLoaded {
        operation_id: draft_op,
        generation: state.draft_generation,
        account_email: "other@example.com".into(),
        drafts: vec![],
    }));
    state.dispatch(Action::Worker(WorkerEvent::SignatureLoaded {
        operation_id: signature_op,
        generation: state.draft_generation,
        account_email: "other@example.com".into(),
        preference: crate::drafts::SignaturePreference {
            html: "<b>Other account</b>".into(),
            enabled: true,
        },
    }));
    let snapshot = state.snapshot();
    assert!(snapshot.saved_draft.is_none());
    assert!(snapshot.signature.html.is_empty());
}

#[test]
fn signature_is_sanitized_saved_and_only_applied_to_new_drafts() {
    let mut state = AppState::new();
    load_replyable(&mut state);
    let update = state.dispatch(Action::UpdateSignature {
        html: "<b>Ryan</b><script>bad()</script>".into(),
        enabled: true,
    });
    assert!(update.effects.iter().any(|effect| matches!(
        effect,
        Effect::SendWorker(WorkerCommand::SaveSignature { .. })
    )));
    state.dispatch(Action::BeginReply);
    let snapshot = state.snapshot();
    let ComposerState::Editing { draft } = snapshot.composer else {
        panic!()
    };
    assert!(draft.compose.html.contains("<b>Ryan</b>"));
    assert!(!draft.compose.html.contains("script"));
    assert_eq!(snapshot.signature.html, "<b>Ryan</b>");
}

fn ready_for_search() -> AppState {
    let mut state = AppState::new();
    ready(&mut state);
    state.mailbox.as_mut().unwrap().folder_catalog = browsing_catalog();
    state
}

#[test]
fn typing_filters_locally_without_starting_a_gmail_search() {
    let mut state = ready_for_search();
    let update = state.dispatch(Action::SetSearch("alice".into()));
    assert!(update.effects.is_empty());
    assert_eq!(state.snapshot().server_search, ServerSearchView::Idle);
}

#[test]
fn submitted_search_uses_discovered_all_mail_and_results_stay_ephemeral() {
    let mut state = ready_for_search();
    let authoritative = state.mailbox.as_ref().unwrap().messages.clone();
    state.dispatch(Action::SetSearch("from:alice has:attachment".into()));
    let update = state.dispatch(Action::SubmitServerSearch);
    let (request_id, generation) = match &update.effects[0] {
        Effect::SendWorker(WorkerCommand::SearchGmail {
            request_id,
            generation,
            folder,
            query,
            ..
        }) => {
            assert_eq!(folder.id, FolderId::AllMail);
            assert_eq!(folder.mailbox, "[Gmail]/All Mail");
            assert_eq!(query, "from:alice has:attachment");
            (*request_id, *generation)
        }
        effect => panic!("unexpected effect: {effect:?}"),
    };
    assert_eq!(state.snapshot().server_search, ServerSearchView::Loading);

    let result = folder_snapshot(FolderId::AllMail, "[Gmail]/All Mail", "Search hit")
        .messages
        .remove(0);
    state.dispatch(Action::Worker(WorkerEvent::SearchLoaded {
        request_id,
        generation,
        messages: vec![result.clone()],
        truncated: true,
        skipped_count: 2,
    }));
    let snapshot = state.snapshot();
    assert_eq!(snapshot.visible_messages, vec![result]);
    assert_eq!(
        snapshot.server_search,
        ServerSearchView::Results {
            count: 1,
            truncated: true,
            skipped_count: 2
        }
    );
    assert_eq!(state.mailbox.as_ref().unwrap().messages, authoritative);
}

#[test]
fn server_search_message_opens_and_replies_without_current_folder_membership() {
    let mut state = ready_for_search();
    finish_draft_restore(&mut state, Vec::new());
    state.dispatch(Action::SetSearch("from:sender".into()));
    let submitted = state.dispatch(Action::SubmitServerSearch);
    let (request_id, generation) = submitted
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::SendWorker(WorkerCommand::SearchGmail {
                request_id,
                generation,
                ..
            }) => Some((*request_id, *generation)),
            _ => None,
        })
        .unwrap();
    let mut hit = folder_snapshot(FolderId::AllMail, "[Gmail]/All Mail", "Search-only subject")
        .messages
        .remove(0);
    hit.id = MessageId::gmail(99);
    state.dispatch(Action::Worker(WorkerEvent::SearchLoaded {
        request_id,
        generation,
        messages: vec![hit.clone()],
        truncated: false,
        skipped_count: 0,
    }));

    let opened = state.dispatch(Action::SelectMessage(hit.id.clone()));
    let (body_request, body_generation) = match opened.effects.as_slice() {
        [
            Effect::SendWorker(WorkerCommand::FetchBody {
                request_id,
                generation,
                locator,
                ..
            }),
        ] => {
            assert_eq!(locator.folder_id, FolderId::AllMail);
            (*request_id, *generation)
        }
        effect => panic!("expected a cache-first body request, got {effect:?}"),
    };
    state.dispatch(Action::Worker(WorkerEvent::BodyLoaded {
        request_id: body_request,
        generation: body_generation,
        message_id: hit.id,
        body: reply_body(),
        usage: Default::default(),
        saved: true,
    }));

    assert!(state.dispatch(Action::BeginReply).feedback.is_none());
    assert!(matches!(
        state.snapshot().composer,
        ComposerState::Editing { ref draft } if draft.subject == "Re: Search-only subject"
    ));
}

#[test]
fn server_search_rows_are_mutable_and_optimistic_updates_reach_all_visible_copies() {
    let mut state = ready_for_search();
    state.dispatch(Action::SetSearch("from:alice".into()));
    let submitted = state.dispatch(Action::SubmitServerSearch);
    let (request_id, generation) = submitted
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::SendWorker(WorkerCommand::SearchGmail {
                request_id,
                generation,
                ..
            }) => Some((*request_id, *generation)),
            _ => None,
        })
        .unwrap();
    let hit = folder_snapshot(FolderId::AllMail, "[Gmail]/All Mail", "Search hit")
        .messages
        .remove(0);
    state.dispatch(Action::Worker(WorkerEvent::SearchLoaded {
        request_id,
        generation,
        messages: vec![hit.clone()],
        truncated: false,
        skipped_count: 0,
    }));
    state.selected_message_id = Some(hit.id.clone());

    let (_, _, message_id, mutation) = mutation_effect(state.dispatch(Action::ToggleStar));
    assert_eq!(message_id, hit.id);
    assert!(matches!(mutation, MessageMutation::SetStarred(true)));
    assert!(state.snapshot().can_mutate);
    assert!(state.snapshot().selected_message.unwrap().starred);
    assert!(matches!(
        state.server_search,
        ServerSearchState::Loaded { ref messages, .. } if messages[0].starred
    ));
}

#[test]
fn server_search_loading_clears_stale_selection_and_reader_actions() {
    let mut state = ready_for_search();
    let id = MessageId::gmail(1);
    state.selected_message_id = Some(id.clone());
    state.reader = ReaderState::Loaded {
        id,
        body: reply_body(),
    };
    state.dispatch(Action::SetSearch("from:alice".into()));
    state.dispatch(Action::SubmitServerSearch);
    assert!(state.selected_message_id().is_none());
    assert!(matches!(state.snapshot().reader, ReaderState::Closed));
    assert!(!state.snapshot().can_mutate);
    assert!(state.dispatch(Action::Archive).effects.is_empty());
}

#[test]
fn server_search_from_trash_uses_the_search_hit_not_the_selected_folder_for_eligibility() {
    let mut state = ready_for_search();
    load_folder_for_test(&mut state, FolderId::Trash, "[Gmail]/Trash", "Trash only");
    assert_eq!(state.snapshot().selected_folder_id, FolderId::Trash);
    state.dispatch(Action::SetSearch("in:anywhere".into()));
    let submitted = state.dispatch(Action::SubmitServerSearch);
    let (request_id, generation) = submitted
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::SendWorker(WorkerCommand::SearchGmail {
                request_id,
                generation,
                ..
            }) => Some((*request_id, *generation)),
            _ => None,
        })
        .unwrap();
    let hit = folder_snapshot(FolderId::AllMail, "[Gmail]/All Mail", "Search hit")
        .messages
        .remove(0);
    state.dispatch(Action::Worker(WorkerEvent::SearchLoaded {
        request_id,
        generation,
        messages: vec![hit.clone()],
        truncated: false,
        skipped_count: 0,
    }));
    state.selected_message_id = Some(hit.id.clone());
    assert!(state.snapshot().can_archive);
    assert!(mutation_effect(state.dispatch(Action::Archive)).3 == MessageMutation::Archive);
}

#[test]
fn editing_search_cancels_loading_and_generation_gates_stale_results() {
    let mut state = ready_for_search();
    state.dispatch(Action::SetSearch("first".into()));
    let submitted = state.dispatch(Action::SubmitServerSearch);
    let (request_id, generation) = submitted
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::SendWorker(WorkerCommand::SearchGmail {
                request_id,
                generation,
                ..
            }) => Some((*request_id, *generation)),
            _ => None,
        })
        .unwrap();
    let edit = state.dispatch(Action::SetSearch("second".into()));
    assert!(edit.effects.iter().any(|effect| matches!(
        effect,
        Effect::SendWorker(WorkerCommand::CancelSearch { request_id: id, .. }) if *id == request_id
    )));
    assert_eq!(state.snapshot().server_search, ServerSearchView::Idle);

    state.dispatch(Action::Worker(WorkerEvent::SearchLoaded {
        request_id,
        generation,
        messages: folder_snapshot(FolderId::AllMail, "[Gmail]/All Mail", "Stale").messages,
        truncated: false,
        skipped_count: 0,
    }));
    assert_eq!(state.snapshot().server_search, ServerSearchView::Idle);
}

#[test]
fn failed_search_can_retry_without_exposing_query_in_status() {
    let mut state = ready_for_search();
    state.dispatch(Action::SetSearch("secret project".into()));
    let submitted = state.dispatch(Action::SubmitServerSearch);
    let (request_id, generation) = submitted
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::SendWorker(WorkerCommand::SearchGmail {
                request_id,
                generation,
                ..
            }) => Some((*request_id, *generation)),
            _ => None,
        })
        .unwrap();
    state.dispatch(Action::Worker(WorkerEvent::SearchFailed {
        request_id,
        generation,
        failure: BodyFailure::TimedOut,
    }));
    assert_eq!(
        state.snapshot().server_search,
        ServerSearchView::Failed(BodyFailure::TimedOut)
    );
    let retry = state.dispatch(Action::RetryServerSearch);
    assert!(retry.effects.iter().any(|effect| matches!(
        effect,
        Effect::SendWorker(WorkerCommand::SearchGmail { query, .. }) if query == "secret project"
    )));
}

#[test]
fn folder_navigation_invalidates_ephemeral_search_results() {
    let mut state = ready_for_search();
    state.dispatch(Action::SetSearch("in:anywhere".into()));
    let submitted = state.dispatch(Action::SubmitServerSearch);
    let (request_id, generation) = submitted
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::SendWorker(WorkerCommand::SearchGmail {
                request_id,
                generation,
                ..
            }) => Some((*request_id, *generation)),
            _ => None,
        })
        .unwrap();
    state.dispatch(Action::Worker(WorkerEvent::SearchLoaded {
        request_id,
        generation,
        messages: folder_snapshot(FolderId::AllMail, "[Gmail]/All Mail", "Anywhere").messages,
        truncated: false,
        skipped_count: 0,
    }));

    let navigation = state.dispatch(Action::SelectFolder(FolderId::Sent));
    assert_eq!(state.snapshot().server_search, ServerSearchView::Idle);
    assert!(navigation.effects.iter().any(|effect| matches!(
        effect,
        Effect::SendWorker(WorkerCommand::FetchFolder { folder, .. }) if folder.id == FolderId::Sent
    )));
}

#[test]
fn appearance_updates_immediately_and_only_matching_save_acknowledges_it() {
    let mut state = AppState::new();
    let first = state.dispatch(Action::SetAppearance(
        crate::cache::AppearancePreference::Light,
    ));
    let first_id = match first.effects.as_slice() {
        [
            Effect::SendWorker(WorkerCommand::SavePreferences {
                request_id,
                preferences,
            }),
        ] => {
            assert_eq!(
                preferences.appearance,
                crate::cache::AppearancePreference::Light
            );
            *request_id
        }
        other => panic!("unexpected effects: {other:?}"),
    };
    assert_eq!(
        state.snapshot().appearance,
        crate::cache::AppearancePreference::Light
    );
    assert_eq!(
        state.snapshot().preferences_save_state,
        PreferencesSaveState::Saving
    );

    let second = state.dispatch(Action::SetAppearance(
        crate::cache::AppearancePreference::Dark,
    ));
    let second_id = match second.effects.as_slice() {
        [
            Effect::SendWorker(WorkerCommand::SavePreferences {
                request_id,
                preferences,
            }),
        ] => {
            assert_eq!(
                preferences.appearance,
                crate::cache::AppearancePreference::Dark
            );
            *request_id
        }
        other => panic!("unexpected effects: {other:?}"),
    };
    assert_ne!(first_id, second_id);
    state.dispatch(Action::Worker(WorkerEvent::PreferencesSaved {
        request_id: first_id,
    }));
    assert_eq!(
        state.snapshot().preferences_save_state,
        PreferencesSaveState::Saving
    );
    state.dispatch(Action::Worker(WorkerEvent::PreferencesSaved {
        request_id: second_id,
    }));
    assert_eq!(
        state.snapshot().preferences_save_state,
        PreferencesSaveState::Saved
    );
}

#[test]
fn appearance_save_failure_keeps_selection_and_can_retry() {
    let mut state = AppState::new();
    let appearance = match state.snapshot().appearance {
        crate::cache::AppearancePreference::Dark => crate::cache::AppearancePreference::Light,
        _ => crate::cache::AppearancePreference::Dark,
    };
    let update = state.dispatch(Action::SetAppearance(appearance));
    let request_id = match update.effects.as_slice() {
        [Effect::SendWorker(WorkerCommand::SavePreferences { request_id, .. })] => *request_id,
        other => panic!("unexpected effects: {other:?}"),
    };
    let failed = state.dispatch(Action::Worker(WorkerEvent::PreferencesSaveFailed {
        request_id,
    }));
    assert_eq!(
        failed.feedback,
        Some("Settings changed, but could not be saved")
    );
    assert_eq!(state.snapshot().appearance, appearance);
    assert_eq!(
        state.snapshot().preferences_save_state,
        PreferencesSaveState::Failed
    );
    assert!(matches!(
        state.dispatch(Action::RetryPreferencesSave).effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::SavePreferences { preferences, .. })]
            if preferences.appearance == appearance
    ));
}

#[test]
fn sidebar_projection_separates_primary_labels_and_suppresses_special_duplicates() {
    let mut state = AppState::new();
    state.mailbox = Some(folder_snapshot(FolderId::Inbox, "INBOX", "Inbox"));
    state.mailbox.as_mut().unwrap().folder_catalog = crate::model::FolderCatalog::bounded(vec![
        crate::model::FolderDescriptor {
            id: FolderId::Inbox,
            mailbox: "INBOX".into(),
            display_name: "Inbox".into(),
            kind: crate::model::FolderKind::Inbox,
        },
        crate::model::FolderDescriptor {
            id: FolderId::Sent,
            mailbox: "Sent".into(),
            display_name: "Sent".into(),
            kind: crate::model::FolderKind::Sent,
        },
        crate::model::FolderDescriptor {
            id: FolderId::Label("Inbox label".into()),
            mailbox: "Inbox label".into(),
            display_name: "inBOX".into(),
            kind: crate::model::FolderKind::Label,
        },
        crate::model::FolderDescriptor {
            id: FolderId::Label("Projects".into()),
            mailbox: "Projects".into(),
            display_name: "Projects".into(),
            kind: crate::model::FolderKind::Label,
        },
    ]);
    let sidebar = state.snapshot().sidebar_folders;
    assert_eq!(
        sidebar
            .primary
            .iter()
            .map(|folder| &folder.name)
            .collect::<Vec<_>>(),
        ["Inbox", "Sent"]
    );
    assert_eq!(
        sidebar
            .labels
            .iter()
            .map(|folder| &folder.name)
            .collect::<Vec<_>>(),
        ["Projects"]
    );
}
