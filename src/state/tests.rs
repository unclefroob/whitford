use super::*;
use crate::model::{MailProvider, SyncMetadata, fixture_messages};
use std::sync::Arc;

fn account() -> AccountIdentity {
    AccountIdentity {
        provider: MailProvider::Gmail,
        email: "person@example.com".into(),
    }
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
    let id = MessageId::gmail(1, 1);
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
fn complete_sync_replaces_mailbox_and_searches_loaded_messages() {
    let mut state = AppState::new();
    ready(&mut state);
    assert_eq!(state.snapshot().folders.len(), 1);
    assert_eq!(state.visible_message_ids().len(), 3);
    state.dispatch(Action::SetSearch("roadmap".into()));
    assert_eq!(state.visible_message_ids(), vec![MessageId::gmail(1, 2)]);
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
    let id = MessageId::gmail(1, 1);
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
    assert!(
        update
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::SendWorker(WorkerCommand::Refresh { .. })))
    );
    assert_eq!(state.snapshot().sync_metadata.unwrap().requested_limit, 100);
}

#[test]
fn offline_session_classifies_missing_runtime_auth_as_offline() {
    let mut state = AppState::new();
    ready(&mut state);
    let message_id = MessageId::gmail(1, 1);
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
    let refresh_id = match refresh.effects[0] {
        Effect::SendWorker(WorkerCommand::Refresh { id }) => id,
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::Failed {
        id: refresh_id,
        failure: ServiceFailure {
            kind: FailureKind::Network,
            retryable: true,
            preserve_mail: true,
            cleanup_failed: false,
            config_path: None,
        },
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
fn worker_failure_terminates_an_in_flight_reader_request() {
    let mut state = AppState::new();
    ready(&mut state);
    state.dispatch(Action::SelectMessage(MessageId::gmail(1, 1)));
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
    let first = MessageId::gmail(1, 1);
    let first_update = state.dispatch(Action::SelectMessage(first.clone()));
    let (first_request, old_generation) = match first_update.effects[0] {
        Effect::SendWorker(WorkerCommand::FetchBody {
            request_id,
            generation,
            ..
        }) => (request_id, generation),
        _ => panic!(),
    };
    state.dispatch(Action::SelectMessage(MessageId::gmail(1, 2)));
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
    let id = match update.effects[0] {
        Effect::SendWorker(WorkerCommand::Refresh { id }) => id,
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
    assert_eq!(state.snapshot().status, ViewStatus::Offline);
    assert_eq!(state.visible_message_ids().len(), 3);
}
#[test]
fn disconnect_only_clears_after_success() {
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
    let (first_id, first_generation) = match first.effects[0] {
        Effect::SendWorker(WorkerCommand::Disconnect { id, generation }) => (id, generation),
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

    let open = state.dispatch(Action::SelectMessage(MessageId::gmail(1, 1)));
    assert!(matches!(
        open.effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::FetchBody { generation, account_email, .. })]
            if *generation == first_generation && account_email == "person@example.com"
    ));

    let retry = state.dispatch(Action::Retry);
    let second_generation = match retry.effects[0] {
        Effect::SendWorker(WorkerCommand::Disconnect { generation, .. }) => generation,
        _ => panic!(),
    };
    assert_eq!(second_generation, first_generation.wrapping_add(1));
}

#[test]
fn account_switch_drops_old_mailbox_and_reader_before_body_requests() {
    let mut state = AppState::new();
    ready(&mut state);
    state.dispatch(Action::SelectMessage(MessageId::gmail(1, 1)));
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
    let id = match update.effects[0] {
        Effect::SendWorker(WorkerCommand::Refresh { id }) => id,
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::Failed {
        id,
        failure: ServiceFailure {
            kind: FailureKind::AuthorizationExpired,
            retryable: false,
            preserve_mail: true,
            cleanup_failed: false,
            config_path: None,
        },
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
fn failed_disconnect_is_degraded_and_keeps_reader_content() {
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
    assert_eq!(snapshot.status, ViewStatus::Degraded);
    assert_eq!(snapshot.visible_messages.len(), 3);
    assert!(snapshot.selected_message.is_some());
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
    let refresh_id = match update.effects[0] {
        Effect::SendWorker(WorkerCommand::Refresh { id }) => id,
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::Failed {
        id: refresh_id,
        failure: ServiceFailure {
            kind: FailureKind::Network,
            retryable: true,
            preserve_mail: true,
            cleanup_failed: false,
            config_path: None,
        },
    }));
    assert!(matches!(
        state.dispatch(Action::Retry).effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::Refresh { .. })]
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
    for kind in [
        FailureKind::AuthorizationExpired,
        FailureKind::IdentityInvalid,
        FailureKind::ImapAuthenticationFailed,
    ] {
        let mut state = AppState::new();
        ready(&mut state);
        let update = state.dispatch(Action::Refresh);
        let id = match update.effects[0] {
            Effect::SendWorker(WorkerCommand::Refresh { id }) => id,
            _ => panic!(),
        };
        state.dispatch(Action::Worker(WorkerEvent::Failed {
            id,
            failure: ServiceFailure {
                kind,
                retryable: false,
                preserve_mail: true,
                cleanup_failed: false,
                config_path: None,
            },
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
        Some("Load the message before replying")
    );

    load_replyable(&mut state);
    state.dispatch(Action::BeginReply);
    assert!(matches!(
        state.snapshot().composer,
        ComposerState::Editing { .. }
    ));
    state.dispatch(Action::UpdateReplyBody("Thanks".into()));
    let send = state.dispatch(Action::SendReply);
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
    assert!(state.dispatch(Action::SendReply).effects.is_empty());
    assert!(
        matches!(state.snapshot().composer, ComposerState::Sending { request_id: current, .. } if current == request_id)
    );

    state.dispatch(Action::Worker(WorkerEvent::ReplyFailed {
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
    let empty = state.dispatch(Action::SendReply);
    assert!(empty.effects.is_empty());
    assert!(matches!(
        state.snapshot().composer,
        ComposerState::Failed {
            failure: SendFailure::Empty,
            ..
        }
    ));

    state.dispatch(Action::UpdateReplyBody("Hello".into()));
    let send = state.dispatch(Action::SendReply);
    let (request_id, generation) = match send.effects[0] {
        Effect::SendWorker(WorkerCommand::SendMessage {
            request_id,
            generation,
            ..
        }) => (request_id, generation),
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::ReplySent {
        request_id: SendRequestId(request_id.0 + 1),
        generation,
    }));
    assert!(matches!(
        state.snapshot().composer,
        ComposerState::Sending { .. }
    ));

    let complete = state.dispatch(Action::Worker(WorkerEvent::ReplySent {
        request_id,
        generation,
    }));
    assert_eq!(complete.feedback, Some("Message sent"));
    assert!(
        complete
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::SendWorker(WorkerCommand::Refresh { .. })))
    );
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
    state.dispatch(Action::UpdateReplyBody("Hello".into()));
    let send = state.dispatch(Action::SendReply);
    let (request_id, generation) = match send.effects[0] {
        Effect::SendWorker(WorkerCommand::SendMessage {
            request_id,
            generation,
            ..
        }) => (request_id, generation),
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::ReplyFailed {
        request_id,
        generation,
        failure: SendFailure::DeliveryUncertain,
    }));

    assert!(matches!(
        state.dispatch(Action::SendReply).effects.as_slice(),
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
    state.dispatch(Action::UpdateReplyBody("Keep this draft".into()));
    state.dispatch(Action::SendReply);

    let request = state.dispatch(Action::RequestDisconnect);
    assert_eq!(
        request.feedback,
        Some("Wait for the reply to finish before disconnecting")
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
fn hiding_saves_and_resume_restores_local_draft_then_discard_deletes_it() {
    let mut state = AppState::new();
    load_replyable(&mut state);
    state.dispatch(Action::BeginReply);
    state.dispatch(Action::UpdateReplyBody("kept locally".into()));
    let hidden = state.dispatch(Action::HideComposer);
    let (operation_id, draft_id, revision) = hidden
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::SendWorker(WorkerCommand::SaveDraft {
                operation_id,
                draft,
            }) => Some((*operation_id, draft.id.clone(), draft.dirty_revision)),
            _ => None,
        })
        .expect("close save");
    assert!(matches!(
        state.snapshot().composer,
        ComposerState::Editing { .. }
    ));
    assert!(state.snapshot().composer_close_pending);
    let saved = state.dispatch(Action::Worker(WorkerEvent::DraftSaved {
        operation_id,
        account_email: "person@example.com".into(),
        draft_id,
        revision,
    }));
    assert_eq!(saved.feedback, Some("Draft saved on this device"));
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
    state.dispatch(Action::UpdateReplyBody("with a file".into()));
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
    let blocked = state.dispatch(Action::SendReply);
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
        account_email: "person@example.com".into(),
    }));
    assert_eq!(state.snapshot().pending_attachment_staging, 0);
    assert!(matches!(
        state.dispatch(Action::SendReply).effects.as_slice(),
        [Effect::SendWorker(WorkerCommand::SendMessage { .. })]
    ));
}

#[test]
fn close_waits_for_acknowledged_save_and_failure_keeps_composer_visible() {
    let mut state = AppState::new();
    load_replyable(&mut state);
    state.dispatch(Action::BeginReply);
    state.dispatch(Action::UpdateReplyBody("must survive".into()));
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
            }) => Some((*operation_id, draft.id.clone(), draft.dirty_revision)),
            _ => None,
        })
        .expect("retry save");
    let saved = state.dispatch(Action::Worker(WorkerEvent::DraftSaved {
        operation_id,
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
fn beginning_another_message_resumes_the_single_saved_draft() {
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
            }) => Some((*operation_id, draft.dirty_revision)),
            _ => None,
        })
        .expect("close save");
    state.dispatch(Action::Worker(WorkerEvent::DraftSaved {
        operation_id,
        account_email: "person@example.com".into(),
        draft_id: original_id.clone(),
        revision,
    }));
    let update = state.dispatch(Action::BeginForward);
    assert_eq!(
        update.feedback,
        Some("Resumed the draft already saved on this device")
    );
    assert!(matches!(state.snapshot().composer,
        ComposerState::Editing { draft } if draft.compose.id == original_id));
}

#[test]
fn stale_cross_account_draft_and_signature_events_are_ignored() {
    let mut state = AppState::new();
    load_replyable(&mut state);
    let draft_op = state.pending_draft_load.expect("draft load");
    let signature_op = state.pending_signature_load.expect("signature load");
    state.dispatch(Action::Worker(WorkerEvent::DraftsLoaded {
        operation_id: draft_op,
        account_email: "other@example.com".into(),
        drafts: vec![],
    }));
    state.dispatch(Action::Worker(WorkerEvent::SignatureLoaded {
        operation_id: signature_op,
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
