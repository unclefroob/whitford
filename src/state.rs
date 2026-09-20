use crate::{
    model::{AccountIdentity, Folder, FolderId, INBOX_FOLDER, MailboxSnapshot, Message, MessageId},
    worker::{
        FailureKind, OperationId, ServiceFailure, SyncKind, WorkerCommand, WorkerEvent, WorkerPhase,
    },
};
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct AppState {
    session: SessionState,
    active_operation: Option<OperationId>,
    next_operation: u64,
    account: Option<AccountIdentity>,
    mailbox: Option<MailboxSnapshot>,
    selected_message_id: Option<MessageId>,
    search_query: String,
    message_filter: MessageFilter,
    recovery: Option<RecoveryAction>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecoveryAction {
    Sync(SyncKind),
    Disconnect,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionState {
    Disconnected,
    Authorizing { deadline: SystemTime },
    Syncing { kind: SyncKind, phase: WorkerPhase },
    Ready,
    Offline { failure: ServiceFailure },
    AuthRequired { cleanup_failed: bool },
    Disconnecting,
    ConfigurationError { failure: ServiceFailure },
    ServiceError { failure: ServiceFailure },
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MessageFilter {
    #[default]
    All,
    Unread,
    Attachments,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewStatus {
    Disconnected,
    Loading,
    Ready,
    Offline,
    Degraded,
    EmptyInbox,
    NoSearchResults,
    Error,
}
#[derive(Debug)]
pub enum Action {
    Startup,
    Connect,
    Refresh,
    CancelAuthorization,
    ReopenAuthorization,
    RequestDisconnect,
    ConfirmDisconnect,
    Retry,
    SelectFolder(FolderId),
    SelectMessage(MessageId),
    SetSearch(String),
    SetFilter(MessageFilter),
    SelectNext,
    SelectPrevious,
    BrowserLaunchFailed(OperationId),
    WorkerUnavailable,
    Worker(WorkerEvent),
}
#[derive(Debug)]
pub enum Effect {
    SendWorker(WorkerCommand),
    LaunchAuthorization { id: OperationId },
    ClearAuthorization { id: OperationId },
    PresentDisconnectConfirmation,
}
#[derive(Default, Debug)]
pub struct Update {
    pub feedback: Option<&'static str>,
    pub effects: Vec<Effect>,
}
#[derive(Clone, Debug)]
pub struct ViewSnapshot {
    pub folders: Vec<Folder>,
    pub visible_messages: Vec<Message>,
    pub selected_message: Option<Message>,
    pub selected_folder_id: FolderId,
    pub search_query: String,
    pub message_filter: MessageFilter,
    pub status: ViewStatus,
    pub folder_counts: Vec<(FolderId, usize)>,
    pub session: SessionState,
    pub account: Option<AccountIdentity>,
    pub sync_metadata: Option<crate::model::SyncMetadata>,
    pub can_connect: bool,
    pub can_refresh: bool,
    pub can_disconnect: bool,
    pub can_reopen: bool,
    pub can_cancel: bool,
    pub can_retry: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EscapeContext {
    pub search_active: bool,
    pub reader_visible: bool,
    pub folders_visible: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EscapeOutcome {
    ClearSearch,
    ShowMessageList,
    HideFolders,
    None,
}
pub fn escape_outcome(c: EscapeContext) -> EscapeOutcome {
    if c.search_active {
        EscapeOutcome::ClearSearch
    } else if c.reader_visible {
        EscapeOutcome::ShowMessageList
    } else if c.folders_visible {
        EscapeOutcome::HideFolders
    } else {
        EscapeOutcome::None
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
impl AppState {
    pub fn new() -> Self {
        Self {
            session: SessionState::Disconnected,
            active_operation: None,
            next_operation: 1,
            account: None,
            mailbox: None,
            selected_message_id: None,
            search_query: String::new(),
            message_filter: MessageFilter::All,
            recovery: None,
        }
    }
    pub fn dispatch(&mut self, action: Action) -> Update {
        match action {
            Action::Startup => self.start(SyncKind::Restore),
            Action::Connect => self.start(SyncKind::Connect),
            Action::Refresh => {
                if self.account.is_some() {
                    self.start(SyncKind::Refresh)
                } else {
                    Update {
                        feedback: Some("Connect Gmail first"),
                        ..Default::default()
                    }
                }
            }
            Action::CancelAuthorization => self.cancel(),
            Action::ReopenAuthorization => {
                self.active_operation
                    .map_or_else(Update::default, |id| Update {
                        effects: vec![Effect::LaunchAuthorization { id }],
                        ..Default::default()
                    })
            }
            Action::RequestDisconnect => Update {
                effects: vec![Effect::PresentDisconnectConfirmation],
                ..Default::default()
            },
            Action::ConfirmDisconnect => self.disconnect(),
            Action::Retry => match self.recovery {
                Some(RecoveryAction::Sync(kind)) => self.start(kind),
                Some(RecoveryAction::Disconnect) => self.disconnect(),
                None => Update {
                    feedback: Some("Restart Whitford to restore the mail service"),
                    ..Default::default()
                },
            },
            Action::BrowserLaunchFailed(id) => {
                if self.active_operation == Some(id) {
                    self.session = SessionState::ServiceError {
                        failure: ServiceFailure {
                            kind: FailureKind::BrowserLaunchFailed,
                            retryable: true,
                            preserve_mail: false,
                            cleanup_failed: false,
                            config_path: None,
                        },
                    };
                    let cancel = self
                        .allocate()
                        .map(|new_id| Effect::SendWorker(WorkerCommand::Cancel { id: new_id }));
                    self.active_operation = None;
                    Update {
                        feedback: Some("Could not open your browser"),
                        effects: cancel
                            .into_iter()
                            .chain([Effect::ClearAuthorization { id }])
                            .collect(),
                    }
                } else {
                    Update::default()
                }
            }
            Action::WorkerUnavailable => {
                self.active_operation = None;
                self.recovery = None;
                let failure = ServiceFailure {
                    kind: FailureKind::WorkerUnavailable,
                    retryable: false,
                    preserve_mail: self.mailbox.is_some(),
                    cleanup_failed: false,
                    config_path: None,
                };
                self.session = if self.mailbox.is_some() {
                    SessionState::Offline { failure }
                } else {
                    SessionState::ServiceError { failure }
                };
                Update {
                    feedback: Some("The mail worker stopped unexpectedly"),
                    ..Default::default()
                }
            }
            Action::Worker(event) => self.worker_event(event),
            Action::SelectFolder(_) => Update::default(),
            Action::SelectMessage(id) => self.select_message(id),
            Action::SetSearch(query) => {
                self.search_query = query.trim().to_owned();
                self.normalize();
                Update::default()
            }
            Action::SetFilter(filter) => {
                self.message_filter = filter;
                self.normalize();
                Update::default()
            }
            Action::SelectNext => {
                self.move_selection(1);
                Update::default()
            }
            Action::SelectPrevious => {
                self.move_selection(-1);
                Update::default()
            }
        }
    }
    fn start(&mut self, kind: SyncKind) -> Update {
        let Some(id) = self.allocate() else {
            return Update {
                feedback: Some("Mail worker is unavailable"),
                ..Default::default()
            };
        };
        self.session = SessionState::Syncing {
            kind,
            phase: if kind == SyncKind::Restore {
                WorkerPhase::OpeningKeyring
            } else {
                WorkerPhase::LoadingConfiguration
            },
        };
        self.recovery = Some(RecoveryAction::Sync(kind));
        let command = match kind {
            SyncKind::Restore => WorkerCommand::Restore { id },
            SyncKind::Connect => WorkerCommand::Connect { id },
            SyncKind::Refresh => WorkerCommand::Refresh { id },
        };
        Update {
            effects: vec![Effect::SendWorker(command)],
            ..Default::default()
        }
    }
    fn cancel(&mut self) -> Update {
        let old = self.active_operation;
        let Some(id) = self.allocate() else {
            return Update::default();
        };
        self.session = SessionState::Disconnected;
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::Cancel { id })]
                .into_iter()
                .chain(old.map(|id| Effect::ClearAuthorization { id }))
                .collect(),
            ..Default::default()
        }
    }
    fn disconnect(&mut self) -> Update {
        let Some(id) = self.allocate() else {
            return Update::default();
        };
        self.session = SessionState::Disconnecting;
        self.recovery = Some(RecoveryAction::Disconnect);
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::Disconnect { id })],
            ..Default::default()
        }
    }
    fn allocate(&mut self) -> Option<OperationId> {
        let current = self.next_operation;
        self.next_operation = self.next_operation.checked_add(1)?;
        let id = OperationId(current);
        self.active_operation = Some(id);
        Some(id)
    }
    fn worker_event(&mut self, event: WorkerEvent) -> Update {
        let id = match &event {
            WorkerEvent::Phase { id, .. }
            | WorkerEvent::AuthorizationRequired { id, .. }
            | WorkerEvent::AccountPersisted { id, .. }
            | WorkerEvent::NoStoredAccount { id }
            | WorkerEvent::SyncComplete { id, .. }
            | WorkerEvent::Disconnected { id }
            | WorkerEvent::Cancelled { id }
            | WorkerEvent::Failed { id, .. } => *id,
        };
        if self.active_operation != Some(id) {
            return Update::default();
        }
        match event {
            WorkerEvent::Phase { phase, .. } => {
                if phase == WorkerPhase::WaitingForBrowser
                    && matches!(self.session, SessionState::Authorizing { .. })
                {
                    return Update::default();
                }
                let kind = match self.session {
                    SessionState::Syncing { kind, .. } => kind,
                    _ => SyncKind::Connect,
                };
                self.session = SessionState::Syncing { kind, phase };
                Update::default()
            }
            WorkerEvent::AuthorizationRequired { deadline, .. } => {
                self.session = SessionState::Authorizing { deadline };
                Update {
                    effects: vec![Effect::LaunchAuthorization { id }],
                    ..Default::default()
                }
            }
            WorkerEvent::AccountPersisted { account, .. } => {
                self.account = Some(account);
                self.session = SessionState::Syncing {
                    kind: SyncKind::Connect,
                    phase: WorkerPhase::ConnectingImap,
                };
                Update::default()
            }
            WorkerEvent::Cancelled { .. }
                if matches!(
                    self.session,
                    SessionState::ServiceError {
                        failure: ServiceFailure {
                            kind: FailureKind::BrowserLaunchFailed,
                            ..
                        }
                    }
                ) =>
            {
                self.active_operation = None;
                Update::default()
            }
            WorkerEvent::NoStoredAccount { .. } | WorkerEvent::Cancelled { .. } => {
                self.session = SessionState::Disconnected;
                self.active_operation = None;
                self.recovery = None;
                Update::default()
            }
            WorkerEvent::SyncComplete {
                account, snapshot, ..
            } => {
                self.account = Some(account);
                self.mailbox = Some(snapshot);
                self.session = SessionState::Ready;
                self.active_operation = None;
                self.recovery = None;
                self.normalize();
                Update {
                    effects: vec![Effect::ClearAuthorization { id }],
                    ..Default::default()
                }
            }
            WorkerEvent::Disconnected { .. } => {
                self.account = None;
                self.mailbox = None;
                self.selected_message_id = None;
                self.session = SessionState::Disconnected;
                self.active_operation = None;
                self.recovery = None;
                Update::default()
            }
            WorkerEvent::Failed { failure, .. } => {
                if matches!(
                    failure.kind,
                    FailureKind::AuthorizationExpired
                        | FailureKind::IdentityInvalid
                        | FailureKind::ImapAuthenticationFailed
                        | FailureKind::CredentialSaveFailed
                ) {
                    self.recovery = Some(RecoveryAction::Sync(SyncKind::Connect));
                } else if failure.kind == FailureKind::DisconnectFailed {
                    self.recovery = Some(RecoveryAction::Disconnect);
                }
                self.session = if failure.kind == FailureKind::DisconnectFailed {
                    SessionState::ServiceError { failure }
                } else if matches!(
                    failure.kind,
                    FailureKind::AuthorizationExpired
                        | FailureKind::IdentityInvalid
                        | FailureKind::ImapAuthenticationFailed
                ) {
                    SessionState::AuthRequired {
                        cleanup_failed: failure.cleanup_failed,
                    }
                } else if failure.preserve_mail && self.mailbox.is_some() {
                    SessionState::Offline { failure }
                } else if failure.kind.is_configuration() {
                    SessionState::ConfigurationError { failure }
                } else {
                    SessionState::ServiceError { failure }
                };
                self.active_operation = None;
                Update {
                    effects: vec![Effect::ClearAuthorization { id }],
                    ..Default::default()
                }
            }
        }
    }
    pub fn snapshot(&self) -> ViewSnapshot {
        let visible = self
            .visible_messages()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let status = match self.session {
            SessionState::Disconnected
            | SessionState::AuthRequired { .. }
            | SessionState::ConfigurationError { .. }
                if self.mailbox.is_none() =>
            {
                ViewStatus::Disconnected
            }
            SessionState::Syncing { .. }
            | SessionState::Authorizing { .. }
            | SessionState::Disconnecting
                if self.mailbox.is_none() =>
            {
                ViewStatus::Loading
            }
            SessionState::Offline { .. } => ViewStatus::Offline,
            SessionState::AuthRequired { .. } => ViewStatus::Degraded,
            SessionState::ServiceError { ref failure }
                if failure.kind == FailureKind::DisconnectFailed && self.mailbox.is_some() =>
            {
                ViewStatus::Degraded
            }
            SessionState::ServiceError { .. } | SessionState::ConfigurationError { .. } => {
                ViewStatus::Error
            }
            _ if visible.is_empty()
                && (!self.search_query.is_empty() || self.message_filter != MessageFilter::All) =>
            {
                ViewStatus::NoSearchResults
            }
            _ if visible.is_empty() => ViewStatus::EmptyInbox,
            _ => ViewStatus::Ready,
        };
        ViewSnapshot {
            folders: vec![INBOX_FOLDER.clone()],
            visible_messages: visible,
            selected_message: self.selected_message().cloned(),
            selected_folder_id: FolderId::Inbox,
            search_query: self.search_query.clone(),
            message_filter: self.message_filter,
            status,
            folder_counts: vec![(FolderId::Inbox, self.folder_count(FolderId::Inbox))],
            session: self.session.clone(),
            account: self.account.clone(),
            sync_metadata: self
                .mailbox
                .as_ref()
                .map(|mailbox| mailbox.metadata.clone()),
            can_connect: matches!(
                self.session,
                SessionState::Disconnected | SessionState::AuthRequired { .. }
            ) || (self.account.is_none()
                && matches!(
                    self.session,
                    SessionState::ServiceError { ref failure }
                        if !matches!(failure.kind, FailureKind::DisconnectFailed | FailureKind::WorkerUnavailable)
                )),
            can_refresh: self.account.is_some()
                && matches!(
                    self.session,
                    SessionState::Ready | SessionState::Offline { .. }
                ),
            can_disconnect: self.account.is_some()
                && !matches!(self.session, SessionState::Disconnecting),
            can_reopen: matches!(self.session, SessionState::Authorizing { .. }),
            can_cancel: matches!(self.session, SessionState::Authorizing { .. }),
            can_retry: match &self.session {
                SessionState::AuthRequired { .. } => true,
                SessionState::Offline { failure }
                | SessionState::ConfigurationError { failure }
                | SessionState::ServiceError { failure } => failure.retryable,
                _ => false,
            },
        }
    }
    pub fn selected_message_id(&self) -> Option<MessageId> {
        self.selected_message_id.clone()
    }
    pub fn selected_message(&self) -> Option<&Message> {
        let id = self.selected_message_id.as_ref()?;
        self.mailbox
            .as_ref()?
            .messages
            .iter()
            .find(|message| &message.id == id)
    }
    pub fn visible_message_ids(&self) -> Vec<MessageId> {
        self.visible_messages()
            .into_iter()
            .map(|m| m.id.clone())
            .collect()
    }
    pub fn folder_count(&self, _: FolderId) -> usize {
        self.mailbox
            .as_ref()
            .map_or(0, |m| m.messages.iter().filter(|m| m.unread).count())
    }
    fn visible_messages(&self) -> Vec<&Message> {
        let query = self.search_query.to_lowercase();
        self.mailbox
            .as_ref()
            .map(|mailbox| {
                mailbox
                    .messages
                    .iter()
                    .filter(|m| {
                        (query.is_empty()
                            || m.sender.to_lowercase().contains(&query)
                            || m.subject.to_lowercase().contains(&query)
                            || m.preview
                                .as_ref()
                                .is_some_and(|p| p.to_lowercase().contains(&query)))
                            && match self.message_filter {
                                MessageFilter::All => true,
                                MessageFilter::Unread => m.unread,
                                MessageFilter::Attachments => !m.attachments.is_empty(),
                            }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    fn select_message(&mut self, id: MessageId) -> Update {
        if self.visible_message_ids().contains(&id) {
            self.selected_message_id = Some(id);
            Update::default()
        } else {
            Update {
                feedback: Some("Message is unavailable"),
                ..Default::default()
            }
        }
    }
    fn normalize(&mut self) {
        let visible = self.visible_message_ids();
        if self
            .selected_message_id
            .as_ref()
            .is_none_or(|id| !visible.contains(id))
        {
            self.selected_message_id = visible.first().cloned();
        }
    }
    fn move_selection(&mut self, step: isize) {
        let visible = self.visible_message_ids();
        if visible.is_empty() {
            return;
        }
        let index = self
            .selected_message_id
            .as_ref()
            .and_then(|id| visible.iter().position(|item| item == id))
            .unwrap_or(0)
            .saturating_add_signed(step)
            .min(visible.len() - 1);
        self.selected_message_id = Some(visible[index].clone());
    }
}

#[cfg(test)]
mod tests;
