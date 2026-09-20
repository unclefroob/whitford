use crate::{
    cache,
    composer::{self, ComposeDraft, Recipient},
    model::{
        AccountIdentity, CacheUsage, Folder, FolderId, MailboxSnapshot, MessageBody, MessageId,
        MessageMutation, MessageSummary, MutationDimension, ReconciledMessageState, inbox_folder,
    },
    smtp,
    worker::{
        BodyFailure, BodyRequestId, CacheOperationId, DraftOperationId, FailureKind,
        MutationRequestId, OperationId, SendFailure, SendRequestId, ServiceFailure, SyncKind,
        WorkerCommand, WorkerEvent, WorkerPhase,
    },
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::SystemTime,
};

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
    cache_limit: usize,
    next_body_request: u64,
    next_send_request: u64,
    send_generation: u64,
    cache_generation: u64,
    pending_cache_clear: Option<CacheOperationId>,
    reader: ReaderState,
    cache_usage: CacheUsage,
    composer: ComposerState,
    saved_drafts: Vec<ComposeDraft>,
    draft_catalog_state: DraftCatalogState,
    draft_generation: u64,
    pending_draft_intent: Option<PendingDraftIntent>,
    signature: crate::drafts::SignaturePreference,
    next_draft_operation: u64,
    pending_draft_load: Option<DraftOperationId>,
    pending_signature_load: Option<DraftOperationId>,
    pending_attachment_staging: HashSet<DraftOperationId>,
    latest_draft_save: Option<DraftOperationId>,
    pending_composer_close: Option<(DraftOperationId, bool)>,
    draft_save_state: DraftSaveState,
    list_revision: u64,
    reader_revision: u64,
    next_mutation_request: u64,
    pending_mutations: HashMap<(MessageId, MutationDimension), PendingMutation>,
}

#[derive(Clone, Debug)]
struct PendingMutation {
    request_id: MutationRequestId,
    mutation: MessageMutation,
    previous: PendingValue,
}

#[derive(Clone, Debug)]
enum PendingValue {
    Bool(bool),
    Label(bool),
    Inbox {
        message: Box<MessageSummary>,
        index: usize,
    },
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReaderState {
    Closed,
    Loading {
        id: MessageId,
        request_id: BodyRequestId,
        generation: u64,
    },
    Loaded {
        id: MessageId,
        body: Arc<MessageBody>,
    },
    Failed {
        id: MessageId,
        failure: BodyFailure,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposeSession {
    pub source_message_id: Option<MessageId>,
    pub recipient: String,
    pub subject: String,
    pub body: String,
    pub context: Option<crate::model::ReplyContext>,
    pub compose: ComposeDraft,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComposerState {
    Closed,
    Editing {
        draft: ComposeSession,
    },
    Sending {
        draft: ComposeSession,
        request_id: SendRequestId,
        generation: u64,
    },
    Failed {
        draft: ComposeSession,
        failure: SendFailure,
    },
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DraftSaveState {
    #[default]
    Saved,
    Saving,
    Failed,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DraftCatalogState {
    #[default]
    NotStarted,
    Loading,
    Ready,
    Failed,
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
    ToggleRead,
    ToggleStar,
    Archive,
    MoveToTrash,
    ToggleLabel(String),
    SetCacheLimit(usize),
    RetryBody,
    RetryDraftRestore,
    BeginNewMessage,
    BeginReply,
    BeginReplyAll,
    BeginForward,
    UpdateMessageBody(String),
    UpdateRecipients {
        to: Vec<Recipient>,
        cc: Vec<Recipient>,
        bcc: Vec<Recipient>,
    },
    UpdateSubject(String),
    UpdateHtml {
        html: String,
        text: String,
    },
    StageAttachment {
        source: std::path::PathBuf,
        display_name: String,
        media_type: String,
    },
    StageInlineImage {
        source: std::path::PathBuf,
        display_name: String,
        media_type: String,
    },
    UpdateSignature {
        html: String,
        enabled: bool,
    },
    RemoveAttachment(String),
    HideComposer,
    HideComposerAndCloseApp,
    ResumeDraft,
    DiscardDraft,
    CancelCompose,
    SendMessage,
    ConfirmResend,
    RequestClearCache,
    ConfirmClearCache,
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
    PresentClearCacheConfirmation,
    PresentUncertainResendConfirmation,
    CloseApplicationWindow,
}
#[derive(Default, Debug)]
pub struct Update {
    pub feedback: Option<&'static str>,
    pub effects: Vec<Effect>,
}
#[derive(Clone, Debug)]
pub struct ViewSnapshot {
    pub folders: Vec<Folder>,
    pub visible_messages: Vec<MessageSummary>,
    pub selected_message: Option<MessageSummary>,
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
    pub can_compose: bool,
    pub cache_limit: usize,
    pub reader: ReaderState,
    pub composer: ComposerState,
    pub saved_draft: Option<SavedDraftSummary>,
    pub draft_catalog_state: DraftCatalogState,
    pub signature: crate::drafts::SignaturePreference,
    pub pending_attachment_staging: usize,
    pub draft_save_state: DraftSaveState,
    pub composer_close_pending: bool,
    pub cache_usage: CacheUsage,
    pub list_revision: u64,
    pub reader_revision: u64,
    pub can_mutate: bool,
    pub label_options: Vec<(String, String, bool)>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedDraftSummary {
    pub id: String,
    pub kind: composer::ComposeKind,
    pub subject: String,
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ComposeStart {
    New,
    Reply,
    ReplyAll,
    Forward,
}
#[derive(Clone, Debug)]
enum PendingDraftIntent {
    Compose(ComposeStart, Option<MessageId>),
    ResumeLatest,
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
            cache_limit: cache::load_limit(),
            next_body_request: 1,
            next_send_request: 1,
            send_generation: 0,
            cache_generation: 0,
            pending_cache_clear: None,
            reader: ReaderState::Closed,
            cache_usage: CacheUsage::default(),
            composer: ComposerState::Closed,
            saved_drafts: Vec::new(),
            draft_catalog_state: DraftCatalogState::NotStarted,
            draft_generation: 1,
            pending_draft_intent: None,
            signature: crate::drafts::SignaturePreference {
                html: String::new(),
                enabled: true,
            },
            next_draft_operation: 1,
            pending_draft_load: None,
            pending_signature_load: None,
            pending_attachment_staging: HashSet::new(),
            latest_draft_save: None,
            pending_composer_close: None,
            draft_save_state: DraftSaveState::Saved,
            list_revision: 1,
            reader_revision: 1,
            next_mutation_request: 1,
            pending_mutations: HashMap::new(),
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
            Action::RequestDisconnect => self.request_disconnect(),
            Action::ConfirmDisconnect => {
                if matches!(self.composer, ComposerState::Sending { .. }) {
                    Update {
                        feedback: Some(
                            "Wait for the message to finish sending before disconnecting",
                        ),
                        ..Default::default()
                    }
                } else {
                    self.disconnect()
                }
            }
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
                self.pending_mutations.clear();
                if let ReaderState::Loading { id, .. } = &self.reader {
                    self.reader = ReaderState::Failed {
                        id: id.clone(),
                        failure: BodyFailure::Offline,
                    };
                    self.bump_reader();
                }
                if let ComposerState::Sending { draft, .. } = &self.composer {
                    self.composer = ComposerState::Failed {
                        draft: draft.clone(),
                        failure: SendFailure::DeliveryUncertain,
                    };
                }
                if self.latest_draft_save.take().is_some()
                    || self.pending_composer_close.take().is_some()
                    || !self.pending_attachment_staging.is_empty()
                {
                    self.pending_attachment_staging.clear();
                    self.draft_save_state = DraftSaveState::Failed;
                }
                if self.pending_draft_load.take().is_some()
                    || self.pending_signature_load.take().is_some()
                {
                    self.draft_catalog_state = DraftCatalogState::Failed;
                }
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
                self.bump_list();
                Update::default()
            }
            Action::SetFilter(filter) => {
                self.message_filter = filter;
                self.normalize();
                self.bump_list();
                Update::default()
            }
            Action::ToggleRead => {
                self.mutate_selected(|message, _| MessageMutation::SetRead(message.unread))
            }
            Action::ToggleStar => {
                self.mutate_selected(|message, _| MessageMutation::SetStarred(!message.starred))
            }
            Action::Archive => self.mutate_selected(|_, _| MessageMutation::Archive),
            Action::MoveToTrash => self.mutate_selected(|_, catalog| {
                catalog
                    .find(&FolderId::Trash)
                    .map(|folder| MessageMutation::MoveToTrash {
                        mailbox: folder.mailbox.clone(),
                    })
                    .unwrap_or(MessageMutation::MoveToTrash {
                        mailbox: String::new(),
                    })
            }),
            Action::ToggleLabel(mailbox) => {
                self.mutate_selected(|message, _| MessageMutation::SetLabel {
                    applied: !message.labels.contains(&mailbox),
                    mailbox,
                })
            }
            Action::SetCacheLimit(limit) => self.set_cache_limit(limit),
            Action::RetryBody => self.retry_body(),
            Action::RetryDraftRestore => self.retry_draft_restore(),
            Action::BeginNewMessage => self.begin_compose(ComposeStart::New),
            Action::BeginReply => self.begin_reply(),
            Action::BeginReplyAll => self.begin_compose(ComposeStart::ReplyAll),
            Action::BeginForward => self.begin_compose(ComposeStart::Forward),
            Action::UpdateMessageBody(body) => self.update_message_body(body),
            Action::UpdateRecipients { to, cc, bcc } => self.update_recipients(to, cc, bcc),
            Action::UpdateSubject(subject) => self.update_subject(subject),
            Action::UpdateHtml { html, text } => self.update_html(html, text),
            Action::StageAttachment {
                source,
                display_name,
                media_type,
            } => self.stage_attachment(source, display_name, media_type),
            Action::StageInlineImage {
                source,
                display_name,
                media_type,
            } => self.stage_file(source, display_name, media_type, true),
            Action::UpdateSignature { html, enabled } => self.update_signature(html, enabled),
            Action::RemoveAttachment(id) => self.remove_attachment(&id),
            Action::HideComposer => self.hide_composer(),
            Action::HideComposerAndCloseApp => self.hide_composer_and_close_app(),
            Action::ResumeDraft => self.resume_draft(),
            Action::DiscardDraft => self.discard_draft(),
            Action::CancelCompose => {
                if !matches!(self.composer, ComposerState::Sending { .. }) {
                    self.composer = ComposerState::Closed;
                }
                Update::default()
            }
            Action::SendMessage => self.send_message(false),
            Action::ConfirmResend => self.send_message(true),
            Action::RequestClearCache => Update {
                effects: vec![Effect::PresentClearCacheConfirmation],
                ..Default::default()
            },
            Action::ConfirmClearCache => self.clear_body_cache(),
            Action::SelectNext => self.move_selection(1),
            Action::SelectPrevious => self.move_selection(-1),
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
        let account_email = match self.account.as_ref() {
            Some(account) => account.email.clone(),
            None if self.recovery == Some(RecoveryAction::Disconnect) => String::new(),
            None => return Update::default(),
        };
        let Some(id) = self.allocate() else {
            return Update::default();
        };
        self.cache_generation = self.cache_generation.wrapping_add(1);
        self.pending_mutations.clear();
        self.send_generation = self.send_generation.wrapping_add(1);
        self.reset_draft_session();
        self.mailbox = None;
        self.selected_message_id = None;
        self.reader = ReaderState::Closed;
        self.cache_usage = CacheUsage::default();
        self.bump_list();
        self.bump_reader();
        self.session = SessionState::Disconnecting;
        self.recovery = Some(RecoveryAction::Disconnect);
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::Disconnect {
                id,
                generation: self.cache_generation,
                draft_generation: self.draft_generation,
                account_email,
            })],
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
        match event {
            WorkerEvent::BodyLoaded {
                request_id,
                generation,
                message_id,
                body,
                usage,
                saved,
            } => {
                let matches = matches!(
                    &self.reader,
                    ReaderState::Loading { id, request_id: current, generation: current_generation }
                        if id == &message_id && *current == request_id && *current_generation == generation
                );
                if matches && generation == self.cache_generation {
                    self.reader = ReaderState::Loaded {
                        id: message_id,
                        body,
                    };
                    self.cache_usage = usage;
                    self.bump_reader();
                    return Update {
                        feedback: (!saved)
                            .then_some("Message loaded but could not be saved offline"),
                        ..Default::default()
                    };
                }
                Update::default()
            }
            WorkerEvent::BodyFailed {
                request_id,
                generation,
                message_id,
                failure,
            } => {
                let matches = matches!(
                    &self.reader,
                    ReaderState::Loading { id, request_id: current, generation: current_generation }
                        if id == &message_id && *current == request_id && *current_generation == generation
                );
                if matches && generation == self.cache_generation {
                    let failure = if failure == BodyFailure::AuthorizationRequired
                        && matches!(self.session, SessionState::Offline { .. })
                    {
                        BodyFailure::Offline
                    } else {
                        failure
                    };
                    self.reader = ReaderState::Failed {
                        id: message_id,
                        failure,
                    };
                    self.bump_reader();
                }
                Update::default()
            }
            WorkerEvent::CacheCleared {
                operation_id,
                generation,
                reclaimed_bytes: _,
                usage,
            } => {
                if self.pending_cache_clear == Some(operation_id)
                    && self.cache_generation == generation
                {
                    self.pending_cache_clear = None;
                    self.cache_usage = usage;
                    self.reader = ReaderState::Closed;
                    self.bump_reader();
                    return Update {
                        feedback: Some("Downloaded message bodies cleared"),
                        ..Default::default()
                    };
                }
                Update::default()
            }
            WorkerEvent::CacheClearFailed {
                operation_id,
                generation,
            } => {
                if self.pending_cache_clear == Some(operation_id)
                    && self.cache_generation == generation
                {
                    self.pending_cache_clear = None;
                    return Update {
                        feedback: Some("Could not fully clear downloaded messages"),
                        ..Default::default()
                    };
                }
                Update::default()
            }
            WorkerEvent::CacheUsageChanged { usage } => {
                self.cache_usage = usage;
                Update::default()
            }
            WorkerEvent::MessageSent {
                request_id,
                generation,
            } => {
                let matches = matches!(&self.composer,
                    ComposerState::Sending { request_id: current, generation: current_generation, .. }
                    if *current == request_id && *current_generation == generation);
                if !matches || generation != self.send_generation {
                    return Update::default();
                }
                let sent = match &self.composer {
                    ComposerState::Sending { draft, .. } => Some(draft.compose.clone()),
                    _ => None,
                };
                self.composer = ComposerState::Closed;
                let mut update = self.start(SyncKind::Refresh);
                update.feedback = Some("Message sent");
                if let Some(draft) = sent {
                    self.saved_drafts.retain(|value| value.id != draft.id);
                    let operation_id = self.allocate_draft_operation();
                    update
                        .effects
                        .push(Effect::SendWorker(WorkerCommand::DeleteDraft {
                            operation_id,
                            generation: self.draft_generation,
                            account_email: draft.account_email,
                            draft_id: draft.id,
                        }));
                }
                update
            }
            WorkerEvent::MessageSendFailed {
                request_id,
                generation,
                failure,
            } => {
                let draft = match &self.composer {
                    ComposerState::Sending {
                        draft,
                        request_id: current,
                        generation: current_generation,
                    } if *current == request_id
                        && *current_generation == generation
                        && generation == self.send_generation =>
                    {
                        draft.clone()
                    }
                    _ => return Update::default(),
                };
                self.composer = ComposerState::Failed { draft, failure };
                Update {
                    feedback: Some(send_failure_feedback(failure)),
                    ..Default::default()
                }
            }
            WorkerEvent::DraftsLoaded {
                operation_id,
                generation,
                account_email,
                drafts,
            } => {
                if self.pending_draft_load != Some(operation_id)
                    || self.draft_generation != generation
                    || !self.current_account_is(&account_email)
                {
                    return Update::default();
                }
                self.pending_draft_load = None;
                self.saved_drafts = drafts;
                self.finish_local_restore()
            }
            WorkerEvent::DraftSaved {
                operation_id,
                generation,
                account_email,
                draft_id,
                revision,
                ..
            } => {
                if generation != self.draft_generation || !self.current_account_is(&account_email) {
                    return Update::default();
                }
                if let Some(draft) = self.saved_drafts.iter_mut().find(|d| d.id == draft_id) {
                    draft.dirty_revision = draft.dirty_revision.max(revision);
                }
                if self.latest_draft_save == Some(operation_id) {
                    self.latest_draft_save = None;
                    self.draft_save_state = DraftSaveState::Saved;
                }
                if self
                    .pending_composer_close
                    .is_some_and(|(pending, _)| pending == operation_id)
                {
                    let (_, close_app) = self.pending_composer_close.take().expect("checked");
                    self.composer = ComposerState::Closed;
                    return Update {
                        feedback: Some("Draft saved on this device"),
                        effects: close_app
                            .then_some(Effect::CloseApplicationWindow)
                            .into_iter()
                            .collect(),
                    };
                }
                Update::default()
            }
            WorkerEvent::DraftDeleted {
                generation,
                account_email,
                draft_id,
                ..
            } => {
                if generation != self.draft_generation || !self.current_account_is(&account_email) {
                    return Update::default();
                }
                self.saved_drafts.retain(|draft| draft.id != draft_id);
                Update::default()
            }
            WorkerEvent::AttachmentStaged {
                operation_id,
                generation,
                draft_id,
                account_email,
                attachment,
                inline,
                ..
            } => {
                if generation != self.draft_generation {
                    return Update::default();
                }
                self.pending_attachment_staging.remove(&operation_id);
                let staged_file = attachment.staged_file.clone();
                if !self.current_account_is(&account_email) {
                    let operation_id = self.allocate_draft_operation();
                    return Update {
                        effects: vec![Effect::SendWorker(WorkerCommand::RemoveStaged {
                            operation_id,
                            generation: self.draft_generation,
                            account_email,
                            draft_id,
                            staged_file,
                        })],
                        ..Default::default()
                    };
                }
                let mut changed = None;
                if let ComposerState::Editing { draft } | ComposerState::Failed { draft, .. } =
                    &mut self.composer
                    && draft.compose.id == draft_id
                {
                    if inline {
                        let content_id = format!("whitford-{}@local", attachment.id);
                        let marker = format!(
                            "<div><img src=\"cid:{}\" alt=\"{}\"></div>",
                            content_id,
                            escape_html(&attachment.display_name)
                        );
                        draft.compose.html =
                            composer::sanitize_html(&format!("{}{}", marker, draft.compose.html));
                        draft
                            .compose
                            .inline_images
                            .push(crate::composer::DraftInlineImage {
                                attachment,
                                content_id,
                            });
                    } else {
                        draft.compose.attachments.push(attachment);
                    }
                    draft.compose.dirty_revision = draft.compose.dirty_revision.wrapping_add(1);
                    changed = Some(draft.compose.clone());
                }
                if let Some(draft) = changed {
                    self.upsert_saved_draft(draft.clone());
                    let mut update = self.queue_draft_save(draft, None);
                    update.feedback = Some("Attachment added");
                    update
                } else {
                    let operation_id = self.allocate_draft_operation();
                    Update {
                        effects: vec![Effect::SendWorker(WorkerCommand::RemoveStaged {
                            operation_id,
                            generation: self.draft_generation,
                            account_email,
                            draft_id,
                            staged_file,
                        })],
                        ..Default::default()
                    }
                }
            }
            WorkerEvent::SignatureLoaded {
                operation_id,
                generation,
                account_email,
                preference,
            } => {
                if self.pending_signature_load != Some(operation_id)
                    || generation != self.draft_generation
                    || !self.current_account_is(&account_email)
                {
                    return Update::default();
                }
                self.pending_signature_load = None;
                self.signature = crate::drafts::SignaturePreference {
                    html: composer::sanitize_html(&preference.html),
                    enabled: preference.enabled,
                };
                self.finish_local_restore()
            }
            WorkerEvent::SignatureSaved {
                generation,
                account_email,
                ..
            } => {
                if generation != self.draft_generation || !self.current_account_is(&account_email) {
                    return Update::default();
                }
                Update::default()
            }
            WorkerEvent::StagedRemoved {
                generation,
                account_email,
                ..
            } => {
                if generation != self.draft_generation || !self.current_account_is(&account_email) {
                    return Update::default();
                }
                Update::default()
            }
            WorkerEvent::DraftOperationFailed {
                operation_id,
                generation,
                account_email,
            } => {
                if generation != self.draft_generation || !self.current_account_is(&account_email) {
                    return Update::default();
                }
                if self.pending_draft_load == Some(operation_id) {
                    self.pending_draft_load = None;
                    self.draft_catalog_state = DraftCatalogState::Failed;
                    return Update {
                        feedback: Some("Could not restore local drafts"),
                        ..Default::default()
                    };
                }
                if self.pending_signature_load == Some(operation_id) {
                    self.pending_signature_load = None;
                    self.draft_catalog_state = DraftCatalogState::Failed;
                    return Update {
                        feedback: Some("Could not restore local signature settings"),
                        ..Default::default()
                    };
                }
                if self.pending_attachment_staging.remove(&operation_id) {
                    return Update {
                        feedback: Some("Could not add the selected attachment"),
                        ..Default::default()
                    };
                }
                if self.latest_draft_save == Some(operation_id) {
                    self.latest_draft_save = None;
                    self.draft_save_state = DraftSaveState::Failed;
                }
                if self
                    .pending_composer_close
                    .is_some_and(|(pending, _)| pending == operation_id)
                {
                    self.pending_composer_close = None;
                }
                Update {
                    feedback: Some("Could not save the local draft"),
                    ..Default::default()
                }
            }
            WorkerEvent::MutationConfirmed {
                request_id,
                generation,
                message_id,
            } => self.finish_mutation(request_id, generation, &message_id, true, None, false),
            WorkerEvent::MutationReconciled {
                request_id,
                generation,
                message_id,
                state,
            } => self.finish_mutation(
                request_id,
                generation,
                &message_id,
                false,
                Some(state),
                false,
            ),
            WorkerEvent::MutationFailed {
                request_id,
                generation,
                message_id,
                uncertain,
            } => self.finish_mutation(request_id, generation, &message_id, false, None, uncertain),
            other => self.account_worker_event(other),
        }
    }

    fn account_worker_event(&mut self, event: WorkerEvent) -> Update {
        let id = match &event {
            WorkerEvent::Phase { id, .. }
            | WorkerEvent::AuthorizationRequired { id, .. }
            | WorkerEvent::AccountPersisted { id, .. }
            | WorkerEvent::CacheLoaded { id, .. }
            | WorkerEvent::NoStoredAccount { id }
            | WorkerEvent::SyncComplete { id, .. }
            | WorkerEvent::Disconnected { id }
            | WorkerEvent::Cancelled { id }
            | WorkerEvent::Failed { id, .. } => *id,
            WorkerEvent::BodyLoaded { .. }
            | WorkerEvent::BodyFailed { .. }
            | WorkerEvent::CacheCleared { .. }
            | WorkerEvent::CacheClearFailed { .. }
            | WorkerEvent::CacheUsageChanged { .. }
            | WorkerEvent::MessageSent { .. }
            | WorkerEvent::MessageSendFailed { .. }
            | WorkerEvent::DraftsLoaded { .. }
            | WorkerEvent::DraftSaved { .. }
            | WorkerEvent::DraftDeleted { .. }
            | WorkerEvent::AttachmentStaged { .. }
            | WorkerEvent::StagedRemoved { .. }
            | WorkerEvent::SignatureLoaded { .. }
            | WorkerEvent::SignatureSaved { .. }
            | WorkerEvent::DraftOperationFailed { .. }
            | WorkerEvent::MutationConfirmed { .. }
            | WorkerEvent::MutationReconciled { .. }
            | WorkerEvent::MutationFailed { .. } => unreachable!(),
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
                let account_is_new = self
                    .account
                    .as_ref()
                    .is_none_or(|current| !current.email.eq_ignore_ascii_case(&account.email));
                if account_is_new {
                    self.pending_mutations.clear();
                    self.reset_draft_session();
                    self.send_generation = self.send_generation.wrapping_add(1);
                    self.composer = ComposerState::Closed;
                    self.mailbox = None;
                    self.selected_message_id = None;
                    self.reader = ReaderState::Closed;
                    self.cache_usage = CacheUsage::default();
                    self.bump_list();
                    self.bump_reader();
                }
                let account_email = account.email.clone();
                self.account = Some(account);
                self.session = SessionState::Syncing {
                    kind: SyncKind::Connect,
                    phase: WorkerPhase::ConnectingImap,
                };
                Update {
                    effects: self.ensure_local_restore(&account_email),
                    ..Default::default()
                }
            }
            WorkerEvent::CacheLoaded {
                account, snapshot, ..
            } => {
                let account_is_new = self
                    .account
                    .as_ref()
                    .is_none_or(|current| !current.email.eq_ignore_ascii_case(&account.email));
                if account_is_new {
                    self.reset_draft_session();
                    self.send_generation = self.send_generation.wrapping_add(1);
                    self.composer = ComposerState::Closed;
                }
                let account_email = account.email.clone();
                self.account = Some(account);
                self.mailbox = Some(snapshot);
                self.normalize();
                self.bump_list();
                Update {
                    effects: self.ensure_local_restore(&account_email),
                    ..Default::default()
                }
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
                let account_is_new = self
                    .account
                    .as_ref()
                    .is_none_or(|current| !current.email.eq_ignore_ascii_case(&account.email));
                if account_is_new {
                    self.reset_draft_session();
                    self.send_generation = self.send_generation.wrapping_add(1);
                    self.composer = ComposerState::Closed;
                    self.selected_message_id = None;
                    self.reader = ReaderState::Closed;
                    self.cache_usage = CacheUsage::default();
                    self.bump_reader();
                }
                let draft_account = account.email.clone();
                self.account = Some(account);
                self.mailbox = Some(snapshot);
                self.session = SessionState::Ready;
                self.active_operation = None;
                self.recovery = None;
                self.normalize();
                self.bump_list();
                let mut effects = vec![Effect::ClearAuthorization { id }];
                effects.extend(self.ensure_local_restore(&draft_account));
                Update {
                    effects,
                    ..Default::default()
                }
            }
            WorkerEvent::Disconnected { .. } => {
                self.pending_mutations.clear();
                self.account = None;
                self.mailbox = None;
                self.selected_message_id = None;
                self.session = SessionState::Disconnected;
                self.active_operation = None;
                self.recovery = None;
                self.reader = ReaderState::Closed;
                self.send_generation = self.send_generation.wrapping_add(1);
                self.composer = ComposerState::Closed;
                self.reset_draft_session();
                self.signature = crate::drafts::SignaturePreference {
                    html: String::new(),
                    enabled: true,
                };
                self.cache_usage = CacheUsage::default();
                self.bump_list();
                self.bump_reader();
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
            WorkerEvent::BodyLoaded { .. }
            | WorkerEvent::BodyFailed { .. }
            | WorkerEvent::CacheCleared { .. }
            | WorkerEvent::CacheClearFailed { .. }
            | WorkerEvent::CacheUsageChanged { .. }
            | WorkerEvent::MessageSent { .. }
            | WorkerEvent::MessageSendFailed { .. }
            | WorkerEvent::DraftsLoaded { .. }
            | WorkerEvent::DraftSaved { .. }
            | WorkerEvent::DraftDeleted { .. }
            | WorkerEvent::AttachmentStaged { .. }
            | WorkerEvent::StagedRemoved { .. }
            | WorkerEvent::SignatureLoaded { .. }
            | WorkerEvent::SignatureSaved { .. }
            | WorkerEvent::DraftOperationFailed { .. } => unreachable!(),
            WorkerEvent::MutationConfirmed { .. }
            | WorkerEvent::MutationReconciled { .. }
            | WorkerEvent::MutationFailed { .. } => unreachable!(),
        }
    }
    pub fn snapshot(&self) -> ViewSnapshot {
        self.snapshot_for_render(true)
    }

    pub(crate) fn list_revision(&self) -> u64 {
        self.list_revision
    }

    pub(crate) fn snapshot_for_render(&self, include_visible_messages: bool) -> ViewSnapshot {
        let has_visible_messages = self.has_visible_messages();
        let visible = if include_visible_messages {
            self.visible_messages()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
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
            _ if !has_visible_messages
                && (!self.search_query.is_empty() || self.message_filter != MessageFilter::All) =>
            {
                ViewStatus::NoSearchResults
            }
            _ if !has_visible_messages => ViewStatus::EmptyInbox,
            _ => ViewStatus::Ready,
        };
        ViewSnapshot {
            can_mutate: matches!(self.session, SessionState::Ready)
                && self.selected_message().is_some(),
            label_options: self
                .mailbox
                .as_ref()
                .map(|mailbox| {
                    mailbox
                        .folder_catalog
                        .folders
                        .iter()
                        .filter_map(|folder| match &folder.id {
                            FolderId::Label(mailbox_name) => Some((
                                mailbox_name.clone(),
                                folder.display_name.clone(),
                                self.selected_message()
                                    .is_some_and(|message| message.labels.contains(mailbox_name)),
                            )),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            folders: vec![inbox_folder()],
            visible_messages: visible,
            selected_message: self.selected_message().cloned(),
            selected_folder_id: FolderId::Inbox,
            search_query: self.search_query.clone(),
            message_filter: self.message_filter,
            status,
            folder_counts: vec![(FolderId::Inbox, self.folder_count(&FolderId::Inbox))],
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
                && !matches!(self.session, SessionState::Disconnecting)
                && !matches!(self.composer, ComposerState::Sending { .. }),
            can_reopen: matches!(self.session, SessionState::Authorizing { .. }),
            can_cancel: matches!(self.session, SessionState::Authorizing { .. }),
            can_retry: match &self.session {
                SessionState::AuthRequired { .. } => true,
                SessionState::Offline { failure }
                | SessionState::ConfigurationError { failure }
                | SessionState::ServiceError { failure } => failure.retryable,
                _ => false,
            },
            can_compose: matches!(self.session, SessionState::Ready)
                && matches!(self.composer, ComposerState::Closed),
            cache_limit: self.cache_limit,
            reader: self.reader.clone(),
            composer: self.composer.clone(),
            saved_draft: (self.draft_catalog_state == DraftCatalogState::Ready)
                .then(|| self.saved_drafts.last())
                .flatten()
                .map(|draft| SavedDraftSummary {
                    id: draft.id.clone(),
                    kind: draft.kind.clone(),
                    subject: draft.subject.clone(),
                }),
            draft_catalog_state: self.draft_catalog_state,
            signature: self.signature.clone(),
            pending_attachment_staging: self.pending_attachment_staging.len(),
            draft_save_state: self.draft_save_state,
            composer_close_pending: self.pending_composer_close.is_some(),
            cache_usage: self.cache_usage,
            list_revision: self.list_revision,
            reader_revision: self.reader_revision,
        }
    }
    pub fn selected_message_id(&self) -> Option<MessageId> {
        self.selected_message_id.clone()
    }
    pub fn selected_message(&self) -> Option<&MessageSummary> {
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
    pub fn folder_count(&self, _: &FolderId) -> usize {
        self.mailbox
            .as_ref()
            .map_or(0, |m| m.messages.iter().filter(|m| m.unread).count())
    }
    fn visible_messages(&self) -> Vec<&MessageSummary> {
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
                            || m.subject.to_lowercase().contains(&query))
                            && match self.message_filter {
                                MessageFilter::All => true,
                                MessageFilter::Unread => m.unread,
                                MessageFilter::Attachments => m.attachment_state.has_attachments(),
                            }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    fn has_visible_messages(&self) -> bool {
        let query = self.search_query.to_lowercase();
        self.mailbox.as_ref().is_some_and(|mailbox| {
            mailbox.messages.iter().any(|message| {
                (query.is_empty()
                    || message.sender.to_lowercase().contains(&query)
                    || message.subject.to_lowercase().contains(&query))
                    && match self.message_filter {
                        MessageFilter::All => true,
                        MessageFilter::Unread => message.unread,
                        MessageFilter::Attachments => message.attachment_state.has_attachments(),
                    }
            })
        })
    }
    fn select_message(&mut self, id: MessageId) -> Update {
        if self.visible_message_ids().contains(&id) {
            self.selected_message_id = Some(id);
            self.bump_list();
            self.open_selected()
        } else {
            Update {
                feedback: Some("Message is unavailable"),
                ..Default::default()
            }
        }
    }
    fn mutate_selected(
        &mut self,
        build: impl FnOnce(&MessageSummary, &crate::model::FolderCatalog) -> MessageMutation,
    ) -> Update {
        if !matches!(self.session, SessionState::Ready) {
            return Update {
                feedback: Some("Connect Gmail before changing messages"),
                ..Default::default()
            };
        }
        let Some(account_email) = self.account.as_ref().map(|account| account.email.clone()) else {
            return Update::default();
        };
        let Some(message) = self.selected_message().cloned() else {
            return Update::default();
        };
        let Some(catalog) = self
            .mailbox
            .as_ref()
            .map(|mailbox| mailbox.folder_catalog.clone())
        else {
            return Update::default();
        };
        let mutation = build(&message, &catalog);
        if matches!(&mutation, MessageMutation::MoveToTrash { mailbox } if mailbox.is_empty()) {
            return Update {
                feedback: Some("Gmail did not expose a Trash mailbox"),
                ..Default::default()
            };
        }
        if let MessageMutation::SetLabel { mailbox, .. } = &mutation
            && !matches!(catalog.find(&FolderId::Label(mailbox.clone())), Some(folder) if folder.mailbox == *mailbox)
        {
            return Update {
                feedback: Some("That Gmail label is unavailable"),
                ..Default::default()
            };
        }
        let dimension = mutation.dimension();
        let key = (message.id.clone(), dimension.clone());
        if self.pending_mutations.contains_key(&key) {
            return Update {
                feedback: Some("That change is already in progress"),
                ..Default::default()
            };
        }
        let request_id = MutationRequestId(self.next_mutation_request);
        self.next_mutation_request = self.next_mutation_request.wrapping_add(1).max(1);
        let previous = match &mutation {
            MessageMutation::SetRead(_) => PendingValue::Bool(message.unread),
            MessageMutation::SetStarred(_) => PendingValue::Bool(message.starred),
            MessageMutation::SetLabel { mailbox, .. } => {
                PendingValue::Label(message.labels.contains(mailbox))
            }
            MessageMutation::Archive | MessageMutation::MoveToTrash { .. } => {
                let index = self
                    .mailbox
                    .as_ref()
                    .and_then(|mailbox| {
                        mailbox
                            .messages
                            .iter()
                            .position(|item| item.id == message.id)
                    })
                    .unwrap_or(0);
                PendingValue::Inbox {
                    message: Box::new(message.clone()),
                    index,
                }
            }
        };
        if matches!(
            mutation,
            MessageMutation::Archive | MessageMutation::MoveToTrash { .. }
        ) {
            if let Some(mailbox) = self.mailbox.as_mut() {
                mailbox.messages.retain(|item| item.id != message.id);
                mailbox.metadata.loaded_count = mailbox.messages.len();
            }
            self.normalize();
            self.bump_reader();
        } else if let Some(current) = self.mailbox.as_mut().and_then(|mailbox| {
            mailbox
                .messages
                .iter_mut()
                .find(|item| item.id == message.id)
        }) {
            mutation.apply(current);
        }
        self.pending_mutations.insert(
            key,
            PendingMutation {
                request_id,
                mutation: mutation.clone(),
                previous,
            },
        );
        self.bump_list();
        self.bump_reader();
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::MutateMessage {
                request_id,
                generation: self.cache_generation,
                account_email,
                message_id: message.id,
                locator: message.locator,
                mutation,
            })],
            ..Default::default()
        }
    }

    fn finish_mutation(
        &mut self,
        request_id: MutationRequestId,
        generation: u64,
        message_id: &MessageId,
        confirmed: bool,
        reconciled: Option<Option<ReconciledMessageState>>,
        uncertain: bool,
    ) -> Update {
        if generation != self.cache_generation {
            return Update::default();
        }
        let key = self.pending_mutations.iter().find_map(|(key, pending)| {
            (pending.request_id == request_id && &key.0 == message_id).then(|| key.clone())
        });
        let Some(key) = key else {
            return Update::default();
        };
        let pending = self
            .pending_mutations
            .remove(&key)
            .expect("pending mutation exists");
        let definite_failure = !confirmed && reconciled.is_none() && !uncertain;
        if definite_failure {
            match pending.previous {
                PendingValue::Bool(value) => {
                    if let Some(message) = self.mailbox.as_mut().and_then(|mailbox| {
                        mailbox
                            .messages
                            .iter_mut()
                            .find(|message| &message.id == message_id)
                    }) {
                        match pending.mutation {
                            MessageMutation::SetRead(_) => message.unread = value,
                            MessageMutation::SetStarred(_) => message.starred = value,
                            _ => {}
                        }
                    }
                }
                PendingValue::Label(applied) => {
                    if let MessageMutation::SetLabel { mailbox, .. } = pending.mutation
                        && let Some(message) = self.mailbox.as_mut().and_then(|mailbox| {
                            mailbox
                                .messages
                                .iter_mut()
                                .find(|message| &message.id == message_id)
                        })
                    {
                        message.labels.retain(|label| label != &mailbox);
                        if applied {
                            message.labels.push(mailbox);
                            message.labels.sort();
                            message.labels.dedup();
                        }
                    }
                }
                PendingValue::Inbox { message, index } => {
                    if let Some(mailbox) = self.mailbox.as_mut() {
                        let index = index.min(mailbox.messages.len());
                        mailbox.messages.insert(index, *message);
                        mailbox.metadata.loaded_count = mailbox.messages.len();
                    }
                }
            }
        } else if let Some(state) = reconciled {
            match state {
                Some(state) => {
                    if let Some(mailbox) = self.mailbox.as_mut() {
                        if let Some(message) = mailbox
                            .messages
                            .iter_mut()
                            .find(|message| &message.id == message_id)
                        {
                            message.unread = state.unread;
                            message.starred = state.starred;
                            message.labels = state.labels;
                        } else if let PendingValue::Inbox { message, index } = &pending.previous {
                            let mut message = (**message).clone();
                            message.unread = state.unread;
                            message.starred = state.starred;
                            message.labels = state.labels;
                            mailbox
                                .messages
                                .insert((*index).min(mailbox.messages.len()), message);
                            mailbox.metadata.loaded_count = mailbox.messages.len();
                        }
                    }
                }
                None => {
                    if let Some(mailbox) = self.mailbox.as_mut() {
                        mailbox.messages.retain(|message| &message.id != message_id);
                        mailbox.metadata.loaded_count = mailbox.messages.len();
                    }
                }
            }
        }
        self.normalize();
        self.bump_list();
        self.bump_reader();
        Update {
            feedback: if uncertain {
                Some("Gmail received the change, but its final state could not be verified")
            } else if definite_failure {
                Some("Gmail rejected the change; it was restored")
            } else {
                None
            },
            ..Default::default()
        }
    }
    fn set_cache_limit(&mut self, limit: usize) -> Update {
        if !cache::is_valid_limit(limit) || self.cache_limit == limit {
            return Update::default();
        }
        self.cache_limit = limit;
        let increased = limit
            > self
                .mailbox
                .as_ref()
                .map_or(0, |mailbox| mailbox.metadata.requested_limit);
        if let Some(mailbox) = self.mailbox.as_mut() {
            mailbox.messages.truncate(limit);
            mailbox.metadata.requested_limit = limit;
            mailbox.metadata.loaded_count = mailbox.messages.len();
        }
        self.normalize();
        self.bump_list();
        let mut effects = vec![Effect::SendWorker(WorkerCommand::SetCacheLimit { limit })];
        if increased && self.account.is_some() {
            let refresh = self.start(SyncKind::Refresh);
            effects.extend(refresh.effects);
        }
        Update {
            feedback: Some("Local mail limit updated"),
            effects,
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
        let reader_id = match &self.reader {
            ReaderState::Closed => None,
            ReaderState::Loading { id, .. }
            | ReaderState::Loaded { id, .. }
            | ReaderState::Failed { id, .. } => Some(id),
        };
        if reader_id.is_some_and(|id| !visible.contains(id)) {
            self.reader = ReaderState::Closed;
            self.bump_reader();
        }
    }
    fn move_selection(&mut self, step: isize) -> Update {
        let visible = self.visible_message_ids();
        if visible.is_empty() {
            return Update::default();
        }
        let index = self
            .selected_message_id
            .as_ref()
            .and_then(|id| visible.iter().position(|item| item == id))
            .unwrap_or(0)
            .saturating_add_signed(step)
            .min(visible.len() - 1);
        self.selected_message_id = Some(visible[index].clone());
        self.bump_list();
        self.open_selected()
    }
    fn open_selected(&mut self) -> Update {
        let Some(id) = self.selected_message_id.clone() else {
            self.reader = ReaderState::Closed;
            self.bump_reader();
            return Update::default();
        };
        let Some(locator) = self
            .selected_message()
            .map(|message| message.locator.clone())
        else {
            return Update::default();
        };
        if matches!(&self.reader, ReaderState::Loaded { id: loaded, .. } if loaded == &id) {
            return Update::default();
        }
        let request_id = BodyRequestId(self.next_body_request);
        self.next_body_request = self.next_body_request.wrapping_add(1).max(1);
        self.reader = ReaderState::Loading {
            id: id.clone(),
            request_id,
            generation: self.cache_generation,
        };
        self.bump_reader();
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::FetchBody {
                request_id,
                generation: self.cache_generation,
                account_email: self
                    .account
                    .as_ref()
                    .map(|account| account.email.clone())
                    .unwrap_or_default(),
                message_id: id,
                locator,
            })],
            ..Default::default()
        }
    }
    fn retry_body(&mut self) -> Update {
        if matches!(self.reader, ReaderState::Failed { .. }) {
            self.open_selected()
        } else {
            Update::default()
        }
    }
    fn begin_reply(&mut self) -> Update {
        self.begin_compose(ComposeStart::Reply)
    }
    fn begin_compose(&mut self, start: ComposeStart) -> Update {
        self.begin_compose_for(start, None)
    }
    fn begin_compose_for(&mut self, start: ComposeStart, expected_id: Option<MessageId>) -> Update {
        if !matches!(self.composer, ComposerState::Closed) {
            return Update::default();
        }
        let source = match start {
            ComposeStart::New => None,
            ComposeStart::Reply | ComposeStart::ReplyAll | ComposeStart::Forward => {
                match &self.reader {
                    ReaderState::Loaded { id, body } => Some((
                        id.clone(),
                        body.reply_context.clone(),
                        body.html
                            .clone()
                            .unwrap_or_else(|| format!("<div>{}</div>", escape_html(&body.text))),
                    )),
                    _ => {
                        return Update {
                            feedback: Some("Load the message before replying or forwarding"),
                            ..Default::default()
                        };
                    }
                }
            }
        };
        let source_id = source.as_ref().map(|(id, _, _)| id.clone());
        if expected_id != source_id && expected_id.is_some() {
            return Update {
                feedback: Some("Reopen the message to continue composing"),
                ..Default::default()
            };
        }
        match self.draft_catalog_state {
            DraftCatalogState::NotStarted | DraftCatalogState::Loading => {
                self.pending_draft_intent = Some(PendingDraftIntent::Compose(start, source_id));
                return Update {
                    feedback: Some("Restoring local drafts before composing"),
                    ..Default::default()
                };
            }
            DraftCatalogState::Failed => {
                return Update {
                    feedback: Some("Retry local draft recovery before composing"),
                    ..Default::default()
                };
            }
            DraftCatalogState::Ready => {}
        }
        if start != ComposeStart::New
            && let Some(compose) = self
                .saved_drafts
                .iter()
                .rev()
                .find(|draft| {
                    source
                        .as_ref()
                        .is_some_and(|(id, _, _)| compose_matches(&draft.kind, start, id))
                })
                .cloned()
        {
            self.composer = ComposerState::Editing {
                draft: compatibility_draft(compose),
            };
            return Update {
                feedback: Some("Resumed the matching draft saved on this device"),
                ..Default::default()
            };
        }
        let source_subject = if let Some(id) = source_id.as_ref() {
            let Some(subject) = self.mailbox.as_ref().and_then(|mailbox| {
                mailbox
                    .messages
                    .iter()
                    .find(|message| &message.id == id)
                    .map(|message| message.subject.clone())
            }) else {
                return Update {
                    feedback: Some("The source message is no longer in this mailbox"),
                    ..Default::default()
                };
            };
            Some(subject)
        } else {
            None
        };
        let Some(account_email) = self.account.as_ref().map(|a| a.email.clone()) else {
            return Update {
                feedback: Some("Connect Gmail before composing"),
                ..Default::default()
            };
        };
        let mut random = [0_u8; 16];
        if getrandom::fill(&mut random).is_err() {
            return Update {
                feedback: Some("Could not create a private draft identifier"),
                ..Default::default()
            };
        }
        let draft_id = format!(
            "draft-{}",
            random
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let signature = if self.signature.enabled {
            self.signature.html.as_str()
        } else {
            ""
        };
        let built = match (start, source.as_ref()) {
            (ComposeStart::New, _) => composer::new_message(draft_id, &account_email, signature),
            (ComposeStart::Reply, Some((id, context, original_html))) => composer::new_reply(
                draft_id,
                &account_email,
                id.clone(),
                source_subject.as_deref().unwrap_or(""),
                context,
                original_html,
                signature,
            ),
            (ComposeStart::ReplyAll, Some((id, context, original_html))) => {
                composer::new_reply_all(
                    draft_id,
                    &account_email,
                    id.clone(),
                    source_subject.as_deref().unwrap_or(""),
                    context,
                    original_html,
                    signature,
                )
            }
            (ComposeStart::Forward, Some((id, context, original_html))) => composer::new_forward(
                draft_id,
                &account_email,
                id.clone(),
                source_subject.as_deref().unwrap_or(""),
                context,
                original_html,
                signature,
            ),
            _ => return Update::default(),
        };
        let Ok(compose) = built else {
            return Update {
                feedback: Some("This message has no usable recipients"),
                ..Default::default()
            };
        };
        let recipient = compose
            .to
            .iter()
            .map(display_recipient)
            .collect::<Vec<_>>()
            .join(", ");
        self.composer = ComposerState::Editing {
            draft: ComposeSession {
                source_message_id: source.as_ref().map(|(id, _, _)| id.clone()),
                recipient,
                subject: compose.subject.clone(),
                body: String::new(),
                context: source.map(|(_, context, _)| context),
                compose: compose.clone(),
            },
        };
        self.upsert_saved_draft(compose.clone());
        self.queue_draft_save(compose, None)
    }
    fn update_message_body(&mut self, body: String) -> Update {
        match &mut self.composer {
            ComposerState::Editing { draft } | ComposerState::Failed { draft, .. } => {
                draft.body = body.clone();
                draft.compose.html = format!("<div>{}</div>", escape_html(&body));
                draft.compose.text = body;
                draft.compose.dirty_revision = draft.compose.dirty_revision.wrapping_add(1);
            }
            ComposerState::Closed | ComposerState::Sending { .. } => {}
        }
        Update::default()
    }
    fn update_recipients(
        &mut self,
        mut to: Vec<Recipient>,
        mut cc: Vec<Recipient>,
        mut bcc: Vec<Recipient>,
    ) -> Update {
        let mut seen = std::collections::HashSet::new();
        for values in [&mut to, &mut cc, &mut bcc] {
            values.retain(|recipient| {
                let key = recipient.email.trim().to_ascii_lowercase();
                !key.is_empty() && seen.insert(key)
            });
        }
        self.mutate_compose(|draft| {
            draft.to = to;
            draft.cc = cc;
            draft.bcc = bcc;
        })
    }
    fn update_subject(&mut self, subject: String) -> Update {
        self.mutate_compose(|draft| {
            draft.subject = subject.chars().take(composer::MAX_SUBJECT_CHARS).collect()
        })
    }
    fn update_html(&mut self, html: String, text: String) -> Update {
        if html.len() > composer::MAX_HTML_BYTES || text.len() > composer::MAX_HTML_BYTES {
            return Update {
                feedback: Some("Message body is too large"),
                ..Default::default()
            };
        }
        self.mutate_compose(|draft| {
            // Keep GTK responsive: bounded editor output is sanitized by the
            // draft and SMTP workers before it crosses a trust boundary.
            draft.html = html;
            draft.text = text;
        })
    }
    fn mutate_compose(&mut self, change: impl FnOnce(&mut ComposeDraft)) -> Update {
        let compose = match &mut self.composer {
            ComposerState::Editing { draft } | ComposerState::Failed { draft, .. } => {
                change(&mut draft.compose);
                draft.compose.dirty_revision = draft.compose.dirty_revision.wrapping_add(1);
                draft.subject = draft.compose.subject.clone();
                draft.recipient = draft
                    .compose
                    .to
                    .iter()
                    .map(display_recipient)
                    .collect::<Vec<_>>()
                    .join(", ");
                draft.body = draft.compose.text.clone();
                draft.compose.clone()
            }
            _ => return Update::default(),
        };
        self.upsert_saved_draft(compose.clone());
        self.queue_draft_save(compose, None)
    }
    fn hide_composer(&mut self) -> Update {
        if !self.pending_attachment_staging.is_empty() {
            return Update {
                feedback: Some("Wait for attachments to finish loading"),
                ..Default::default()
            };
        }
        let compose = match &self.composer {
            ComposerState::Editing { draft } | ComposerState::Failed { draft, .. } => {
                Some(draft.compose.clone())
            }
            _ => None,
        };
        let Some(compose) = compose else {
            return Update::default();
        };
        self.upsert_saved_draft(compose.clone());
        self.queue_draft_save(compose, Some(false))
    }
    fn hide_composer_and_close_app(&mut self) -> Update {
        if !self.pending_attachment_staging.is_empty() {
            return Update {
                feedback: Some("Wait for attachments to finish loading"),
                ..Default::default()
            };
        }
        let compose = match &self.composer {
            ComposerState::Editing { draft } | ComposerState::Failed { draft, .. } => {
                Some(draft.compose.clone())
            }
            _ => None,
        };
        let Some(compose) = compose else {
            return Update {
                effects: vec![Effect::CloseApplicationWindow],
                ..Default::default()
            };
        };
        self.upsert_saved_draft(compose.clone());
        self.queue_draft_save(compose, Some(true))
    }
    fn queue_draft_save(&mut self, compose: ComposeDraft, close_app: Option<bool>) -> Update {
        if self.draft_catalog_state != DraftCatalogState::Ready {
            return Update {
                feedback: Some("Restore local drafts before saving"),
                ..Default::default()
            };
        }
        let operation_id = self.allocate_draft_operation();
        self.latest_draft_save = Some(operation_id);
        self.draft_save_state = DraftSaveState::Saving;
        if let Some(close_app) = close_app {
            self.pending_composer_close = Some((operation_id, close_app));
        }
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::SaveDraft {
                operation_id,
                generation: self.draft_generation,
                draft: Box::new(compose),
            })],
            ..Default::default()
        }
    }
    fn resume_draft(&mut self) -> Update {
        if !matches!(self.composer, ComposerState::Closed) {
            return Update::default();
        }
        match self.draft_catalog_state {
            DraftCatalogState::NotStarted | DraftCatalogState::Loading => {
                self.pending_draft_intent = Some(PendingDraftIntent::ResumeLatest);
                return Update {
                    feedback: Some("Restoring local drafts before composing"),
                    ..Default::default()
                };
            }
            DraftCatalogState::Failed => {
                return Update {
                    feedback: Some("Retry local draft recovery before composing"),
                    ..Default::default()
                };
            }
            DraftCatalogState::Ready => {}
        }
        let Some(compose) = self.saved_drafts.last().cloned() else {
            return Update::default();
        };
        self.composer = ComposerState::Editing {
            draft: compatibility_draft(compose),
        };
        Update::default()
    }
    fn discard_draft(&mut self) -> Update {
        let compose = match &self.composer {
            ComposerState::Editing { draft } | ComposerState::Failed { draft, .. } => {
                Some(draft.compose.clone())
            }
            _ => self.saved_drafts.last().cloned(),
        };
        let Some(compose) = compose else {
            return Update::default();
        };
        self.composer = ComposerState::Closed;
        self.saved_drafts.retain(|draft| draft.id != compose.id);
        let operation_id = self.allocate_draft_operation();
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::DeleteDraft {
                operation_id,
                generation: self.draft_generation,
                account_email: compose.account_email,
                draft_id: compose.id,
            })],
            ..Default::default()
        }
    }
    fn stage_attachment(
        &mut self,
        source: std::path::PathBuf,
        display_name: String,
        media_type: String,
    ) -> Update {
        self.stage_file(source, display_name, media_type, false)
    }
    fn stage_file(
        &mut self,
        source: std::path::PathBuf,
        display_name: String,
        media_type: String,
        inline: bool,
    ) -> Update {
        let (account_email, draft_id) = match &self.composer {
            ComposerState::Editing { draft } | ComposerState::Failed { draft, .. } => (
                draft.compose.account_email.clone(),
                draft.compose.id.clone(),
            ),
            _ => return Update::default(),
        };
        let operation_id = self.allocate_draft_operation();
        self.pending_attachment_staging.insert(operation_id);
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::StageAttachment {
                operation_id,
                generation: self.draft_generation,
                account_email,
                draft_id,
                source,
                display_name,
                media_type,
                inline,
            })],
            ..Default::default()
        }
    }
    fn update_signature(&mut self, html: String, enabled: bool) -> Update {
        if self.draft_catalog_state != DraftCatalogState::Ready
            || self.pending_signature_load.is_some()
        {
            return Update {
                feedback: Some("Restore local account settings before changing the signature"),
                ..Default::default()
            };
        }
        let Some(account_email) = self.account.as_ref().map(|a| a.email.clone()) else {
            return Update::default();
        };
        self.signature = crate::drafts::SignaturePreference {
            html: composer::sanitize_html(&html),
            enabled,
        };
        let operation_id = self.allocate_draft_operation();
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::SaveSignature {
                operation_id,
                generation: self.draft_generation,
                account_email,
                preference: self.signature.clone(),
            })],
            ..Default::default()
        }
    }
    fn remove_attachment(&mut self, id: &str) -> Update {
        let removed = match &self.composer {
            ComposerState::Editing { draft } | ComposerState::Failed { draft, .. } => draft
                .compose
                .attachments
                .iter()
                .chain(draft.compose.inline_images.iter().map(|v| &v.attachment))
                .find(|a| a.id == id)
                .map(|a| {
                    (
                        draft.compose.account_email.clone(),
                        draft.compose.id.clone(),
                        a.staged_file.clone(),
                    )
                }),
            _ => None,
        };
        let mut update = self.mutate_compose(|d| {
            d.attachments.retain(|a| a.id != id);
            d.inline_images.retain(|a| a.attachment.id != id);
        });
        if let Some((account_email, draft_id, staged_file)) = removed {
            let operation_id = self.allocate_draft_operation();
            update
                .effects
                .push(Effect::SendWorker(WorkerCommand::RemoveStaged {
                    operation_id,
                    generation: self.draft_generation,
                    account_email,
                    draft_id,
                    staged_file,
                }));
        }
        update
    }
    fn allocate_draft_operation(&mut self) -> DraftOperationId {
        let id = DraftOperationId(self.next_draft_operation);
        self.next_draft_operation = self.next_draft_operation.wrapping_add(1).max(1);
        id
    }
    fn reset_draft_session(&mut self) {
        self.draft_generation = self.draft_generation.wrapping_add(1).max(1);
        self.saved_drafts.clear();
        self.draft_catalog_state = DraftCatalogState::NotStarted;
        self.pending_draft_intent = None;
        self.pending_draft_load = None;
        self.pending_signature_load = None;
        self.pending_attachment_staging.clear();
        self.latest_draft_save = None;
        self.pending_composer_close = None;
        self.draft_save_state = DraftSaveState::Saved;
        self.signature = crate::drafts::SignaturePreference {
            html: String::new(),
            enabled: true,
        };
        self.composer = ComposerState::Closed;
    }
    fn ensure_local_restore(&mut self, account_email: &str) -> Vec<Effect> {
        if self.draft_catalog_state != DraftCatalogState::NotStarted {
            return Vec::new();
        }
        self.draft_catalog_state = DraftCatalogState::Loading;
        let draft_operation = self.allocate_draft_operation();
        let signature_operation = self.allocate_draft_operation();
        self.pending_draft_load = Some(draft_operation);
        self.pending_signature_load = Some(signature_operation);
        vec![
            Effect::SendWorker(WorkerCommand::LoadDrafts {
                operation_id: draft_operation,
                generation: self.draft_generation,
                account_email: account_email.to_owned(),
            }),
            Effect::SendWorker(WorkerCommand::LoadSignature {
                operation_id: signature_operation,
                generation: self.draft_generation,
                account_email: account_email.to_owned(),
            }),
        ]
    }
    fn retry_draft_restore(&mut self) -> Update {
        if self.draft_catalog_state != DraftCatalogState::Failed {
            return Update::default();
        }
        let Some(account_email) = self.account.as_ref().map(|value| value.email.clone()) else {
            return Update::default();
        };
        self.draft_generation = self.draft_generation.wrapping_add(1).max(1);
        self.pending_draft_load = None;
        self.pending_signature_load = None;
        self.draft_catalog_state = DraftCatalogState::NotStarted;
        Update {
            effects: self.ensure_local_restore(&account_email),
            ..Default::default()
        }
    }
    fn finish_local_restore(&mut self) -> Update {
        if self.draft_catalog_state != DraftCatalogState::Loading
            || self.pending_draft_load.is_some()
            || self.pending_signature_load.is_some()
        {
            return Update::default();
        }
        self.draft_catalog_state = DraftCatalogState::Ready;
        match self.pending_draft_intent.take() {
            Some(PendingDraftIntent::Compose(start, id)) => self.begin_compose_for(start, id),
            Some(PendingDraftIntent::ResumeLatest) => self.resume_draft(),
            None => Update::default(),
        }
    }
    fn upsert_saved_draft(&mut self, draft: ComposeDraft) {
        self.saved_drafts.retain(|value| value.id != draft.id);
        self.saved_drafts.push(draft);
    }
    fn request_disconnect(&self) -> Update {
        if matches!(self.composer, ComposerState::Sending { .. }) {
            Update {
                feedback: Some("Wait for the message to finish sending before disconnecting"),
                ..Default::default()
            }
        } else {
            Update {
                effects: vec![Effect::PresentDisconnectConfirmation],
                ..Default::default()
            }
        }
    }

    fn send_message(&mut self, confirmed_uncertain_resend: bool) -> Update {
        if !self.pending_attachment_staging.is_empty() {
            return Update {
                feedback: Some("Wait for attachments to finish loading"),
                ..Default::default()
            };
        }
        if self.pending_composer_close.is_some() {
            return Update::default();
        }
        let draft = match &self.composer {
            ComposerState::Failed {
                failure: SendFailure::DeliveryUncertain,
                ..
            } if !confirmed_uncertain_resend => {
                return Update {
                    effects: vec![Effect::PresentUncertainResendConfirmation],
                    ..Default::default()
                };
            }
            ComposerState::Editing { draft } | ComposerState::Failed { draft, .. } => draft,
            ComposerState::Closed | ComposerState::Sending { .. } => return Update::default(),
        };
        let Some(account_email) = self.account.as_ref().map(|account| account.email.clone()) else {
            return Update {
                feedback: Some("Connect Gmail before sending"),
                ..Default::default()
            };
        };
        if draft.body.trim().is_empty()
            && draft.compose.attachments.is_empty()
            && draft.compose.inline_images.is_empty()
        {
            let draft = match std::mem::replace(&mut self.composer, ComposerState::Closed) {
                ComposerState::Editing { draft } | ComposerState::Failed { draft, .. } => draft,
                other => {
                    self.composer = other;
                    return Update::default();
                }
            };
            self.composer = ComposerState::Failed {
                draft,
                failure: SendFailure::Empty,
            };
            return Update {
                feedback: Some(send_failure_feedback(SendFailure::Empty)),
                ..Default::default()
            };
        }
        if let Err(error) = smtp::build_message(&smtp::MailSubmission {
            account_email: account_email.clone(),
            to: draft.compose.to.clone(),
            cc: draft.compose.cc.clone(),
            bcc: draft.compose.bcc.clone(),
            subject: draft.compose.subject.clone(),
            html: draft.compose.html.clone(),
            attachments: Vec::new(),
            inline_images: Vec::new(),
            thread: draft.compose.thread.clone(),
        }) {
            let failure = map_send_failure(error);
            let draft = match std::mem::replace(&mut self.composer, ComposerState::Closed) {
                ComposerState::Editing { draft } | ComposerState::Failed { draft, .. } => draft,
                other => {
                    self.composer = other;
                    return Update::default();
                }
            };
            self.composer = ComposerState::Failed { draft, failure };
            return Update {
                feedback: Some(send_failure_feedback(failure)),
                ..Default::default()
            };
        }
        let request_id = SendRequestId(self.next_send_request);
        self.next_send_request = self.next_send_request.wrapping_add(1).max(1);
        let generation = self.send_generation;
        let draft = match std::mem::replace(&mut self.composer, ComposerState::Closed) {
            ComposerState::Editing { draft } | ComposerState::Failed { draft, .. } => draft,
            other => {
                self.composer = other;
                return Update::default();
            }
        };
        let submission = crate::worker::ComposeSubmission {
            draft: draft.compose.clone(),
        };
        self.composer = ComposerState::Sending {
            draft,
            request_id,
            generation,
        };
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::SendMessage {
                request_id,
                generation,
                submission: Box::new(submission),
            })],
            ..Default::default()
        }
    }
    fn clear_body_cache(&mut self) -> Update {
        self.cache_generation = self.cache_generation.wrapping_add(1);
        let operation_id = CacheOperationId(self.next_body_request);
        self.next_body_request = self.next_body_request.wrapping_add(1).max(1);
        self.pending_cache_clear = Some(operation_id);
        self.reader = ReaderState::Closed;
        self.bump_reader();
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::ClearBodyCache {
                operation_id,
                generation: self.cache_generation,
            })],
            ..Default::default()
        }
    }
    fn bump_list(&mut self) {
        self.list_revision = self.list_revision.wrapping_add(1);
    }
    fn bump_reader(&mut self) {
        self.reader_revision = self.reader_revision.wrapping_add(1);
    }

    fn current_account_is(&self, email: &str) -> bool {
        self.account
            .as_ref()
            .is_some_and(|account| account.email.eq_ignore_ascii_case(email))
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
fn display_recipient(value: &Recipient) -> String {
    value
        .name
        .as_ref()
        .filter(|n| !n.trim().is_empty())
        .map_or_else(
            || value.email.clone(),
            |name| format!("{name} <{}>", value.email),
        )
}
fn compatibility_draft(compose: ComposeDraft) -> ComposeSession {
    let source_message_id = match &compose.kind {
        composer::ComposeKind::New => None,
        composer::ComposeKind::Reply { original }
        | composer::ComposeKind::ReplyAll { original }
        | composer::ComposeKind::Forward { original } => Some(original.clone()),
    };
    ComposeSession {
        source_message_id,
        recipient: compose
            .to
            .iter()
            .map(display_recipient)
            .collect::<Vec<_>>()
            .join(", "),
        subject: compose.subject.clone(),
        body: compose.text.clone(),
        context: None,
        compose,
    }
}

fn compose_matches(kind: &composer::ComposeKind, start: ComposeStart, id: &MessageId) -> bool {
    matches!(
        (kind, start),
        (composer::ComposeKind::Reply { original }, ComposeStart::Reply)
            | (
                composer::ComposeKind::ReplyAll { original },
                ComposeStart::ReplyAll
            )
            | (
                composer::ComposeKind::Forward { original },
                ComposeStart::Forward
            ) if original == id
    )
}

fn map_send_failure(error: smtp::SmtpError) -> SendFailure {
    match error {
        smtp::SmtpError::EmptyBody => SendFailure::Empty,
        smtp::SmtpError::BodyTooLarge => SendFailure::TooLarge,
        smtp::SmtpError::MissingRecipient
        | smtp::SmtpError::InvalidAddress
        | smtp::SmtpError::TooManyRecipients => SendFailure::InvalidRecipient,
        smtp::SmtpError::Authentication => SendFailure::AuthorizationRequired,
        smtp::SmtpError::Rejected => SendFailure::Rejected,
        smtp::SmtpError::DeliveryUncertain => SendFailure::DeliveryUncertain,
        smtp::SmtpError::Build => SendFailure::Protocol,
    }
}

fn send_failure_feedback(failure: SendFailure) -> &'static str {
    match failure {
        SendFailure::Empty => "Write a message before sending",
        SendFailure::TooLarge => "Message is too large to send",
        SendFailure::InvalidRecipient => "This message has no valid recipients",
        SendFailure::AuthorizationRequired => "Refresh Gmail authorization before sending",
        SendFailure::Rejected => "Gmail rejected the message",
        SendFailure::DeliveryUncertain => "Delivery is uncertain; check Sent before retrying",
        SendFailure::Protocol => "Could not construct or send the message",
    }
}

#[cfg(test)]
mod tests;
