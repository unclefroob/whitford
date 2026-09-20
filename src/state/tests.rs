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
        used_fallback: false,
    })
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
