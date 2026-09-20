use super::*;
use crate::model::{MailProvider, SyncMetadata, fixture_messages};

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
        Effect::SendWorker(WorkerCommand::Disconnect { id }) => id,
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
        Effect::SendWorker(WorkerCommand::Disconnect { id }) => id,
        _ => panic!(),
    };
    state.dispatch(Action::Worker(WorkerEvent::Disconnected { id }));
    assert!(state.snapshot().account.is_none());
    assert_eq!(state.snapshot().status, ViewStatus::Disconnected);
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
        Effect::SendWorker(WorkerCommand::Disconnect { id }) => id,
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
