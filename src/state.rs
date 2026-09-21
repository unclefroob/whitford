use crate::{
    cache::{self, AppearancePreference, Preferences},
    composer::{self, ComposeDraft, Recipient},
    model::{
        AccountFolderId, AccountId, AccountIdentity, AccountMessageId, CacheUsage, Folder,
        FolderId, FolderKind, MAX_ACCOUNTS, MailboxSnapshot, MessageBody, MessageId,
        MessageMutation, MessageSummary, MutationDimension, ReconciledMessageState,
    },
    smtp,
    worker::{
        AttachmentDestination, AttachmentFailure, AttachmentJobId, BackgroundSyncRequestId,
        BodyFailure, BodyRequestId, CacheOperationId, DraftOperationId, FailureKind,
        FolderRequestId, MutationRequestId, OperationId, PreferencesRequestId,
        RestoredAccountMailbox, SearchRequestId, SendFailure, SendRequestId, ServiceFailure,
        SyncKind, WorkerCommand, WorkerEvent, WorkerPhase,
    },
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
    time::{Duration, SystemTime},
};

const BACKGROUND_SYNC_FLOOR: Duration = Duration::from_secs(60);
const BACKGROUND_SYNC_CAP: Duration = Duration::from_secs(15 * 60);
const MAX_EMITTED_BACKGROUND_IDS: usize = cache::MAX_SUMMARIES_PER_FOLDER;

#[derive(Clone, Debug)]
pub struct AppState {
    session: SessionState,
    active_operation: Option<OperationId>,
    // Add-account OAuth is deliberately independent of the singleton
    // lifecycle operation. Its events can update only the account-aware
    // projection and must never replace `account`, `mailbox`, or `session`.
    adding_account_operation: Option<OperationId>,
    next_operation: u64,
    account: Option<AccountIdentity>,
    mailbox: Option<MailboxSnapshot>,
    selected_message_id: Option<MessageId>,
    selected_folder_id: FolderId,
    next_folder_request: u64,
    folder_generation: u64,
    active_folder_request: Option<FolderRequestId>,
    folder_loading: bool,
    search_query: String,
    next_search_request: u64,
    search_generation: u64,
    server_search: ServerSearchState,
    message_filter: MessageFilter,
    recovery: Option<RecoveryAction>,
    cache_limit: usize,
    appearance: AppearancePreference,
    preferences_save_state: PreferencesSaveState,
    next_preferences_request: u64,
    pending_preferences_request: Option<PreferencesRequestId>,
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
    // Deliberately process-local for this milestone. The cache is changed only by worker
    // confirmations, so restart drops an in-flight Undo rather than claiming it completed.
    // A durable journal needs worker events carrying the restored summary/locator before it can
    // safely recreate an Inbox/Trash projection after restart.
    undo_operation: Option<UndoOperation>,
    next_attachment_job: u64,
    attachment_jobs: HashMap<AttachmentJobId, AttachmentDownload>,
    background_sync: BackgroundSyncState,
    // This is deliberately a projection-only registry for the incremental
    // multi-account rollout. The existing singleton fields above still drive
    // worker commands until they become account keyed. Keeping this separate
    // prevents a partially migrated worker event from changing another
    // account's visible data.
    account_mailboxes: BTreeMap<AccountId, AccountMailbox>,
    selected_mailbox_view: MailboxView,
    selected_account_message: Option<AccountMessageId>,
    // Account-scoped reconnect/removal requests use the same monotonically
    // increasing operation IDs, but never occupy the singleton lifecycle
    // slot. This lets one account recover without interrupting the rest.
    account_operations: BTreeMap<AccountId, OperationId>,
}

/// The account-scoped navigation identity. `UnifiedInbox` has no provider
/// folder counterpart; every other destination is explicitly namespaced by an
/// opaque account ID.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum MailboxView {
    #[default]
    UnifiedInbox,
    AccountFolder(AccountFolderId),
}

/// A mailbox snapshot belonging to one account. It is intentionally separate
/// from the singleton `AppState::mailbox` while worker/cache protocols are
/// migrated incrementally.
#[derive(Clone, Debug)]
struct AccountMailbox {
    identity: AccountIdentity,
    session: SessionState,
    inbox: Option<MailboxSnapshot>,
    folders: HashMap<FolderId, MailboxSnapshot>,
}

/// A compact account row for sidebar/settings consumers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountViewSummary {
    pub id: AccountId,
    pub identity: AccountIdentity,
    pub session: SessionState,
    pub unread_count: usize,
}

/// A message in a state projection. The account-scoped ID is required even
/// where Gmail's message ID looks globally unique: Gmail only guarantees it
/// within an account.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountMessageSummary {
    pub id: AccountMessageId,
    pub account: AccountIdentity,
    pub message: MessageSummary,
}

#[derive(Clone, Debug)]
struct BackgroundSyncState {
    in_flight: Option<BackgroundSyncRequestId>,
    next_request: u64,
    consecutive_failures: u8,
    schedule_generation: u64,
    pending_tick: bool,
    paused: bool,
    emitted_ids: HashSet<MessageId>,
}

impl Default for BackgroundSyncState {
    fn default() -> Self {
        Self {
            in_flight: None,
            next_request: 1,
            consecutive_failures: 0,
            schedule_generation: 1,
            pending_tick: false,
            paused: true,
            emitted_ids: HashSet::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BackgroundSyncStatus {
    #[default]
    Idle,
    Syncing,
    BackingOff,
    Paused,
}

/// Whether the current in-memory preferences have been acknowledged by the
/// background persistence worker. A failed save is deliberately recoverable:
/// the UI continues using the selected value and can ask for a retry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PreferencesSaveState {
    #[default]
    Saved,
    Saving,
    Failed,
}

#[derive(Clone, Debug)]
struct PendingMutation {
    request_id: MutationRequestId,
    origin_folder_id: FolderId,
    mutation: MessageMutation,
    previous: PendingValue,
}

#[derive(Clone, Debug)]
struct UndoOperation {
    id: u64,
    message_id: MessageId,
    forward: MessageMutation,
    previous: PendingValue,
    locator: crate::model::MessageLocator,
    catalog: crate::model::FolderCatalog,
    title: String,
    phase: UndoPhase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UndoPhase {
    ForwardPending {
        request_id: MutationRequestId,
        requested: bool,
    },
    Available,
    UndoPending {
        request_id: MutationRequestId,
    },
}

#[derive(Clone, Debug)]
enum PendingValue {
    Bool(bool),
    Label(bool),
    Removed(Vec<RemovedMessage>),
}

#[derive(Clone, Debug)]
struct RemovedMessage {
    view: RemovedMessageView,
    message: MessageSummary,
    index: usize,
}

#[derive(Clone, Copy, Debug)]
enum RemovedMessageView {
    Folder,
    Search,
}

fn undo_labels(previous: &PendingValue) -> Vec<String> {
    let PendingValue::Removed(rows) = previous else {
        return Vec::new();
    };
    rows.first()
        .map(|row| row.message.labels.clone())
        .unwrap_or_default()
}

fn undo_was_in_inbox(previous: &PendingValue) -> bool {
    let PendingValue::Removed(rows) = previous else {
        return false;
    };
    rows.iter().any(|row| row.message.in_inbox)
}
#[derive(Clone, Debug, Eq, PartialEq)]
enum RecoveryAction {
    Sync(SyncKind),
    Folder(FolderId),
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
    /// The account ID remains part of every Unified Inbox reader state. A
    /// Gmail message ID is only unique inside that account.
    AccountLoading {
        id: AccountMessageId,
        request_id: BodyRequestId,
        generation: u64,
    },
    AccountLoaded {
        id: AccountMessageId,
        body: Arc<MessageBody>,
    },
    AccountFailed {
        id: AccountMessageId,
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
#[derive(Clone, Debug, Eq, PartialEq)]
enum ServerSearchState {
    Idle,
    Loading {
        request_id: SearchRequestId,
        generation: u64,
        query: String,
    },
    Loaded {
        messages: Vec<MessageSummary>,
        truncated: bool,
        skipped_count: usize,
    },
    Failed {
        query: String,
        failure: BodyFailure,
    },
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ServerSearchView {
    #[default]
    Idle,
    Loading,
    Results {
        count: usize,
        truncated: bool,
        skipped_count: usize,
    },
    Failed(BodyFailure),
}
#[derive(Debug)]
pub enum Action {
    Startup,
    /// Inserts or replaces one account's independently loaded mailbox
    /// projection. This is a state boundary, not a worker protocol: worker
    /// migration can begin feeding it without changing existing commands.
    UpsertAccountMailbox {
        account_id: AccountId,
        identity: AccountIdentity,
        session: SessionState,
        mailbox: Option<MailboxSnapshot>,
    },
    /// Stores a loaded non-unified folder without replacing the account's
    /// Inbox projection. This lets a foreground folder view coexist with the
    /// deterministic Unified Inbox.
    UpsertAccountFolder {
        folder: AccountFolderId,
        snapshot: MailboxSnapshot,
    },
    /// Bridges the verified legacy singleton account into the account-aware
    /// projection. It does not change the singleton account, worker runtime,
    /// or credential ownership.
    RegisterLegacyAccount {
        account_id: AccountId,
        identity: AccountIdentity,
    },
    /// Starts additive Gmail OAuth. This is separate from `Connect`, which
    /// retains its established singleton session semantics.
    AddAccount,
    ReconnectAccount {
        account_id: AccountId,
    },
    RemoveAccount {
        account_id: AccountId,
    },
    RemoveAccountMailbox {
        account_id: AccountId,
    },
    SelectMailboxView(MailboxView),
    SelectAccountMessage(AccountMessageId),
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
    SubmitServerSearch,
    CancelServerSearch,
    RetryServerSearch,
    SetFilter(MessageFilter),
    ToggleRead,
    ToggleStar,
    Archive,
    MoveToTrash,
    UndoMessageOperation,
    ToggleLabel(String),
    SetCacheLimit(usize),
    SetAppearance(AppearancePreference),
    RetryPreferencesSave,
    RetryBody,
    OpenAttachment(crate::model::Attachment),
    SaveAttachment {
        attachment: crate::model::Attachment,
        destination: std::path::PathBuf,
    },
    CancelAttachment(AttachmentJobId),
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
    BackgroundSyncTimer {
        schedule_generation: u64,
    },
    Worker(WorkerEvent),
}
#[allow(clippy::large_enum_variant)] // Worker commands own private staged-file paths.
#[derive(Debug)]
pub enum Effect {
    SendWorker(WorkerCommand),
    ScheduleBackgroundSync {
        after: Duration,
        schedule_generation: u64,
    },
    CancelBackgroundSyncTimer,
    NotifyNewUnread {
        count: usize,
    },
    LaunchAuthorization {
        id: OperationId,
    },
    ClearAuthorization {
        id: OperationId,
    },
    PresentDisconnectConfirmation,
    PresentClearCacheConfirmation,
    PresentUncertainResendConfirmation,
    LaunchAttachment(std::path::PathBuf),
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
    /// The mail navigation catalog, arranged for the sidebar. `folders` is
    /// retained for the moment for non-sidebar consumers; new sidebar code
    /// should use this semantic projection so special folders and labels do
    /// not get mixed together.
    pub sidebar_folders: SidebarFolders,
    pub visible_messages: Vec<MessageSummary>,
    pub selected_message: Option<MessageSummary>,
    pub selected_folder_id: FolderId,
    pub search_query: String,
    pub server_search: ServerSearchView,
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
    pub appearance: AppearancePreference,
    pub preferences_save_state: PreferencesSaveState,
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
    /// Action-specific availability is deliberately independent of the folder being viewed.
    /// `can_mutate` remains as a compatibility summary for existing renderers.
    pub can_archive: bool,
    pub can_move_to_trash: bool,
    pub undo_message_operation: Option<UndoMessageOperationView>,
    pub label_options: Vec<(String, String, bool)>,
    pub attachment_downloads: Vec<AttachmentDownload>,
    pub background_sync_status: BackgroundSyncStatus,
    /// Account registry used by account-aware UI. It is ordered by opaque
    /// account ID, giving settings/sidebar consumers a stable result.
    pub accounts: Vec<AccountViewSummary>,
    /// The selected account-aware navigation destination. The legacy
    /// `selected_folder_id` remains available for singleton renderers.
    pub mailbox_view: MailboxView,
    /// Messages for `mailbox_view`. In Unified Inbox this is the deterministic
    /// newest-first merge across all account Inbox projections.
    pub account_visible_messages: Vec<AccountMessageSummary>,
    pub selected_account_message: Option<AccountMessageId>,
}

/// A UI-ready projection of the provider folder catalog.
///
/// Primary folders retain the catalog's special-folder ordering. Labels are
/// kept in their supplied order, except that labels indistinguishable from a
/// displayed special folder are omitted and colliding label names are made
/// unambiguous with their mailbox name.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SidebarFolders {
    pub primary: Vec<Folder>,
    pub labels: Vec<Folder>,
}

fn sidebar_folders(catalog: Option<&crate::model::FolderCatalog>) -> SidebarFolders {
    let Some(catalog) = catalog else {
        return SidebarFolders::default();
    };

    let folder = |descriptor: &crate::model::FolderDescriptor| Folder {
        id: descriptor.id.clone(),
        name: descriptor.display_name.clone(),
        icon: match descriptor.kind {
            FolderKind::Inbox => "mail-unread-symbolic",
            FolderKind::Sent => "mail-send-symbolic",
            FolderKind::AllMail => "mail-read-symbolic",
            FolderKind::Trash => "user-trash-symbolic",
            FolderKind::Starred => "starred-symbolic",
            FolderKind::Label => "tag-symbolic",
        },
    };
    let primary = catalog
        .folders
        .iter()
        .filter(|descriptor| descriptor.kind != FolderKind::Label)
        .map(folder)
        .collect::<Vec<_>>();
    let primary_names = primary
        .iter()
        .map(|entry| entry.name.to_lowercase())
        .collect::<HashSet<_>>();

    // Gmail can expose two distinct label mailboxes with the same friendly
    // name. Keeping both destinations is useful, but rendering two identical
    // rows is not; disambiguate only those collisions without changing the
    // order supplied by the catalog.
    let mut label_name_counts = HashMap::<String, usize>::new();
    for descriptor in &catalog.folders {
        if descriptor.kind == FolderKind::Label
            && !primary_names.contains(&descriptor.display_name.to_lowercase())
        {
            *label_name_counts
                .entry(descriptor.display_name.to_lowercase())
                .or_default() += 1;
        }
    }
    let labels = catalog
        .folders
        .iter()
        .filter(|descriptor| {
            descriptor.kind == FolderKind::Label
                && !primary_names.contains(&descriptor.display_name.to_lowercase())
        })
        .map(|descriptor| {
            let mut entry = folder(descriptor);
            if label_name_counts
                .get(&descriptor.display_name.to_lowercase())
                .copied()
                .unwrap_or_default()
                > 1
            {
                entry.name = format!("{} ({})", descriptor.display_name, descriptor.mailbox);
            }
            entry
        })
        .collect();
    SidebarFolders { primary, labels }
}

fn message_is_in_inbox(message: &MessageSummary) -> bool {
    // Older cached Inbox snapshots did not retain the system-label bit. Their
    // folder identity is sufficient for this compatibility projection.
    message.in_inbox || message.folder_id == FolderId::Inbox
}

fn message_is_in_folder(message: &MessageSummary, folder_id: &FolderId) -> bool {
    match folder_id {
        FolderId::Inbox => message_is_in_inbox(message),
        _ => message.folder_id == *folder_id,
    }
}

/// Newest first, with a total ordering for equal or missing provider dates.
/// The opaque account ID comes before the Gmail message ID so two accounts
/// containing the same Gmail ID remain distinct and consistently ordered.
fn compare_account_messages_newest_first(
    left: &AccountMessageSummary,
    right: &AccountMessageSummary,
) -> std::cmp::Ordering {
    right
        .message
        .received_at_unix
        .cmp(&left.message.received_at_unix)
        .then_with(|| left.id.account_id.cmp(&right.id.account_id))
        .then_with(|| left.id.message_id.0.cmp(&right.id.message_id.0))
        .then_with(|| {
            left.message
                .locator
                .uid_validity
                .cmp(&right.message.locator.uid_validity)
        })
        .then_with(|| left.message.locator.uid.cmp(&right.message.locator.uid))
        .then_with(|| left.message.subject.cmp(&right.message.subject))
}

/// Only lifecycle events carry an operation ID. Keeping this extraction at
/// the reducer boundary lets additive OAuth consume its own events before the
/// legacy singleton lifecycle reducer sees them.
fn worker_operation_id(event: &WorkerEvent) -> Option<OperationId> {
    match event {
        WorkerEvent::AccountMailboxesRestored { id, .. }
        | WorkerEvent::AccountSynced { id, .. }
        | WorkerEvent::AccountRemoved { id, .. }
        | WorkerEvent::AccountOperationFailed { id, .. }
        | WorkerEvent::LegacyAccountRegistered { id, .. }
        | WorkerEvent::Phase { id, .. }
        | WorkerEvent::AuthorizationRequired { id, .. }
        | WorkerEvent::IdentityVerified { id, .. }
        | WorkerEvent::DuplicateAccountIdentity { id, .. }
        | WorkerEvent::AccountAdded { id, .. }
        | WorkerEvent::AccountPersisted { id, .. }
        | WorkerEvent::CacheLoaded { id, .. }
        | WorkerEvent::NoStoredAccount { id }
        | WorkerEvent::SyncComplete { id, .. }
        | WorkerEvent::Disconnected { id }
        | WorkerEvent::Cancelled { id }
        | WorkerEvent::Failed { id, .. } => Some(*id),
        _ => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UndoMessageOperationView {
    pub id: u64,
    pub message_id: MessageId,
    pub title: String,
    pub can_undo: bool,
    pub pending: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachmentDownload {
    pub job_id: AttachmentJobId,
    pub message_id: MessageId,
    pub part_path: Vec<u32>,
    pub name: String,
    pub transferred: u64,
    pub total: u64,
    pub open: bool,
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
        let preferences = cache::load_preferences();
        Self {
            session: SessionState::Disconnected,
            active_operation: None,
            adding_account_operation: None,
            next_operation: 1,
            account: None,
            mailbox: None,
            selected_message_id: None,
            selected_folder_id: FolderId::Inbox,
            next_folder_request: 1,
            folder_generation: 1,
            active_folder_request: None,
            folder_loading: false,
            search_query: String::new(),
            next_search_request: 1,
            search_generation: 1,
            server_search: ServerSearchState::Idle,
            message_filter: MessageFilter::All,
            recovery: None,
            cache_limit: preferences.retained_messages,
            appearance: preferences.appearance,
            preferences_save_state: PreferencesSaveState::Saved,
            next_preferences_request: 1,
            pending_preferences_request: None,
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
            undo_operation: None,
            next_attachment_job: 1,
            attachment_jobs: HashMap::new(),
            background_sync: BackgroundSyncState::default(),
            account_mailboxes: BTreeMap::new(),
            selected_mailbox_view: MailboxView::UnifiedInbox,
            selected_account_message: None,
            account_operations: BTreeMap::new(),
        }
    }
    pub fn dispatch(&mut self, action: Action) -> Update {
        match action {
            Action::Startup => self.start(SyncKind::Restore),
            Action::UpsertAccountMailbox {
                account_id,
                identity,
                session,
                mailbox,
            } => self.upsert_account_mailbox(account_id, identity, session, mailbox),
            Action::UpsertAccountFolder { folder, snapshot } => {
                self.upsert_account_folder(folder, snapshot)
            }
            Action::RegisterLegacyAccount {
                account_id,
                identity,
            } => self.register_legacy_account(account_id, identity),
            Action::AddAccount => self.start_add_account(),
            Action::ReconnectAccount { account_id } => self.reconnect_account(account_id),
            Action::RemoveAccount { account_id } => self.remove_account(account_id),
            Action::RemoveAccountMailbox { account_id } => self.remove_account_mailbox(&account_id),
            Action::SelectMailboxView(view) => self.select_mailbox_view(view),
            Action::SelectAccountMessage(id) => self.select_account_message(id),
            Action::Connect => self.start(SyncKind::Connect),
            Action::Refresh => {
                if self.account.is_some() {
                    self.load_folder(self.selected_folder_id.clone())
                } else {
                    Update {
                        feedback: Some("Connect Gmail first"),
                        ..Default::default()
                    }
                }
            }
            Action::CancelAuthorization => self.cancel(),
            Action::ReopenAuthorization => self
                .active_operation
                .or(self.adding_account_operation)
                .map_or_else(Update::default, |id| Update {
                    effects: vec![Effect::LaunchAuthorization { id }],
                    ..Default::default()
                }),
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
            Action::Retry => match self.recovery.clone() {
                Some(RecoveryAction::Sync(kind)) => self.start(kind),
                Some(RecoveryAction::Folder(id)) => self.load_folder(id),
                Some(RecoveryAction::Disconnect) => self.disconnect(),
                None => Update {
                    feedback: Some("Restart Whitford to restore the mail service"),
                    ..Default::default()
                },
            },
            Action::BrowserLaunchFailed(id) => {
                if self.adding_account_operation == Some(id) {
                    self.adding_account_operation = None;
                    Update {
                        feedback: Some("Could not open your browser"),
                        effects: vec![
                            Effect::SendWorker(WorkerCommand::Cancel { id }),
                            Effect::ClearAuthorization { id },
                        ],
                    }
                } else if self.active_operation == Some(id) {
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
                let cancel_background = self.stop_background_sync();
                self.active_operation = None;
                self.adding_account_operation = None;
                self.recovery = None;
                let _ = self.invalidate_server_search();
                self.pending_mutations.clear();
                self.undo_operation = None;
                self.attachment_jobs.clear();
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
                    effects: cancel_background.into_iter().collect(),
                }
            }
            Action::BackgroundSyncTimer {
                schedule_generation,
            } => self.background_sync_timer(schedule_generation),
            Action::Worker(event) => self.worker_event(event),
            Action::SelectFolder(id) => self.load_folder(id),
            Action::SelectMessage(id) => self.select_message(id),
            Action::SetSearch(query) => {
                if self.search_query == query {
                    return Update::default();
                }
                let effects = self.invalidate_server_search();
                self.search_query = query;
                self.normalize();
                self.bump_list();
                Update {
                    effects,
                    ..Default::default()
                }
            }
            Action::SubmitServerSearch => self.submit_server_search(),
            Action::CancelServerSearch => self.cancel_server_search(),
            Action::RetryServerSearch => self.retry_server_search(),
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
            Action::UndoMessageOperation => self.undo_message_operation(),
            Action::ToggleLabel(mailbox) => {
                self.mutate_selected(|message, _| MessageMutation::SetLabel {
                    applied: !message.labels.contains(&mailbox),
                    mailbox,
                })
            }
            Action::SetCacheLimit(limit) => self.set_cache_limit(limit),
            Action::SetAppearance(appearance) => self.set_appearance(appearance),
            Action::RetryPreferencesSave => self.retry_preferences_save(),
            Action::RetryBody => self.retry_body(),
            Action::OpenAttachment(attachment) => {
                self.start_attachment(attachment, AttachmentDestination::Open)
            }
            Action::SaveAttachment {
                attachment,
                destination,
            } => self.start_attachment(attachment, AttachmentDestination::SaveAs(destination)),
            Action::CancelAttachment(job_id) => self.cancel_attachment(job_id),
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
        if self.adding_account_operation.is_some() {
            return Update {
                feedback: Some("Wait for the additional Gmail account to finish connecting"),
                ..Default::default()
            };
        }
        if matches!(self.composer, ComposerState::Sending { .. }) {
            return Update {
                feedback: Some("Wait for the message to finish sending before reconnecting"),
                ..Default::default()
            };
        }
        let Some(id) = self.allocate() else {
            return Update {
                feedback: Some("Mail worker is unavailable"),
                ..Default::default()
            };
        };
        let mut effects = self.invalidate_server_search();
        self.folder_generation = self.folder_generation.wrapping_add(1).max(1);
        self.active_folder_request = None;
        self.folder_loading = false;
        let had_attachment_jobs = !self.attachment_jobs.is_empty();
        self.attachment_jobs.clear();
        let reader_was_loading = matches!(self.reader, ReaderState::Loading { .. });
        if reader_was_loading {
            self.reader = ReaderState::Closed;
        }
        if had_attachment_jobs || reader_was_loading {
            self.bump_reader();
        }
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
        effects.push(Effect::SendWorker(command));
        Update {
            effects,
            ..Default::default()
        }
    }
    fn start_add_account(&mut self) -> Update {
        // The worker has one OAuth listener slot. Rejecting this locally
        // avoids superseding a Connect/Restore operation and, importantly,
        // leaves the singleton mailbox untouched.
        if self.active_operation.is_some() || self.adding_account_operation.is_some() {
            return Update {
                feedback: Some("Wait for the current Gmail connection to finish"),
                ..Default::default()
            };
        }
        let current = self.next_operation;
        let Some(next) = current.checked_add(1) else {
            return Update {
                feedback: Some("Mail worker is unavailable"),
                ..Default::default()
            };
        };
        self.next_operation = next;
        let id = OperationId(current);
        self.adding_account_operation = Some(id);
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::AddAccount { id })],
            ..Default::default()
        }
    }
    fn reconnect_account(&mut self, account_id: AccountId) -> Update {
        if self.account_operations.contains_key(&account_id) {
            return Update {
                feedback: Some("This Gmail account is already being updated"),
                ..Default::default()
            };
        }
        if !self.account_mailboxes.contains_key(&account_id) {
            return Update {
                feedback: Some("That Gmail account is unavailable"),
                ..Default::default()
            };
        }
        let Some(id) = self.allocate_account_operation() else {
            return Update {
                feedback: Some("Mail worker is unavailable"),
                ..Default::default()
            };
        };
        if let Some(account) = self.account_mailboxes.get_mut(&account_id) {
            account.session = SessionState::Syncing {
                kind: SyncKind::Refresh,
                phase: WorkerPhase::OpeningKeyring,
            };
        }
        self.account_operations.insert(account_id.clone(), id);
        self.bump_list();
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::ReconnectAccount {
                id,
                account_id,
            })],
            ..Default::default()
        }
    }
    fn remove_account(&mut self, account_id: AccountId) -> Update {
        if self.account_operations.contains_key(&account_id) {
            return Update {
                feedback: Some("This Gmail account is already being updated"),
                ..Default::default()
            };
        }
        if !self.account_mailboxes.contains_key(&account_id) {
            return Update {
                feedback: Some("That Gmail account is unavailable"),
                ..Default::default()
            };
        }
        let Some(id) = self.allocate_account_operation() else {
            return Update {
                feedback: Some("Mail worker is unavailable"),
                ..Default::default()
            };
        };
        if let Some(account) = self.account_mailboxes.get_mut(&account_id) {
            account.session = SessionState::Disconnecting;
        }
        self.account_operations.insert(account_id.clone(), id);
        self.bump_list();
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::RemoveAccount {
                id,
                account_id,
            })],
            ..Default::default()
        }
    }
    fn cancel(&mut self) -> Update {
        if let Some(id) = self.adding_account_operation.take() {
            return Update {
                effects: vec![
                    Effect::SendWorker(WorkerCommand::Cancel { id }),
                    Effect::ClearAuthorization { id },
                ],
                ..Default::default()
            };
        }
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
        if self.adding_account_operation.is_some() {
            return Update {
                feedback: Some("Wait for the additional Gmail account to finish connecting"),
                ..Default::default()
            };
        }
        let account_email = match self.account.as_ref() {
            Some(account) => account.email.clone(),
            None if self.recovery == Some(RecoveryAction::Disconnect) => String::new(),
            None => return Update::default(),
        };
        let Some(id) = self.allocate() else {
            return Update::default();
        };
        let mut effects = self.invalidate_server_search();
        let cancel_background = self.stop_background_sync();
        self.cache_generation = self.cache_generation.wrapping_add(1);
        self.folder_generation = self.folder_generation.wrapping_add(1).max(1);
        self.active_folder_request = None;
        self.folder_loading = false;
        self.selected_folder_id = FolderId::Inbox;
        self.pending_mutations.clear();
        self.undo_operation = None;
        self.attachment_jobs.clear();
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
        effects.push(Effect::SendWorker(WorkerCommand::Disconnect {
            id,
            generation: self.cache_generation,
            draft_generation: self.draft_generation,
            account_email,
        }));
        if let Some(cancel) = cancel_background {
            effects.push(cancel);
        }
        Update {
            effects,
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
    fn allocate_account_operation(&mut self) -> Option<OperationId> {
        let current = self.next_operation;
        self.next_operation = self.next_operation.checked_add(1)?;
        Some(OperationId(current))
    }
    fn worker_event(&mut self, event: WorkerEvent) -> Update {
        if self
            .adding_account_operation
            .is_some_and(|id| worker_operation_id(&event) == Some(id))
        {
            return self.add_account_worker_event(event);
        }
        if worker_operation_id(&event)
            .is_some_and(|id| self.account_operations.values().any(|value| *value == id))
        {
            return self.account_projection_worker_event(event);
        }
        match event {
            WorkerEvent::AccountMailboxesRestored { id, accounts } => {
                if self.active_operation != Some(id) {
                    return Update::default();
                }
                self.restore_account_mailboxes(accounts)
            }
            WorkerEvent::AttachmentProgress {
                job_id,
                generation,
                transferred,
                total,
            } => {
                if generation == self.cache_generation
                    && let Some(job) = self.attachment_jobs.get_mut(&job_id)
                {
                    job.transferred = transferred.min(total);
                    job.total = total;
                }
                Update::default()
            }
            WorkerEvent::AttachmentCompleted {
                job_id,
                generation,
                path,
                open,
            } => {
                if generation != self.cache_generation
                    || self.attachment_jobs.remove(&job_id).is_none()
                {
                    return Update::default();
                }
                self.bump_reader();
                Update {
                    feedback: (!open).then_some("Attachment saved"),
                    effects: open
                        .then_some(Effect::LaunchAttachment(path))
                        .into_iter()
                        .collect(),
                }
            }
            WorkerEvent::AttachmentFailed {
                job_id,
                generation,
                failure,
            } => {
                if generation != self.cache_generation
                    || self.attachment_jobs.remove(&job_id).is_none()
                {
                    return Update::default();
                }
                self.bump_reader();
                Update {
                    feedback: Some(attachment_failure_feedback(failure)),
                    ..Default::default()
                }
            }
            WorkerEvent::AttachmentCancelled { job_id, generation } => {
                if generation == self.cache_generation
                    && self.attachment_jobs.remove(&job_id).is_some()
                {
                    self.bump_reader();
                }
                Update::default()
            }
            WorkerEvent::SearchLoaded {
                request_id,
                generation,
                messages,
                truncated,
                skipped_count,
            } => self.search_loaded(request_id, generation, messages, truncated, skipped_count),
            WorkerEvent::SearchFailed {
                request_id,
                generation,
                failure,
            } => self.search_failed(request_id, generation, failure),
            WorkerEvent::SearchCancelled { .. } => Update::default(),
            WorkerEvent::FolderCacheLoaded {
                request_id,
                generation,
                folder_id,
                snapshot,
            } => self.folder_loaded_and_project(request_id, generation, folder_id, snapshot, true),
            WorkerEvent::FolderLoaded {
                request_id,
                generation,
                folder_id,
                snapshot,
            } => self.folder_loaded_and_project(request_id, generation, folder_id, snapshot, false),
            WorkerEvent::FolderFailed {
                request_id,
                generation,
                folder_id,
                failure,
            } => self.folder_failed(request_id, generation, &folder_id, failure),
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
            WorkerEvent::AccountBodyLoaded {
                request_id,
                generation,
                message,
                body,
            } => {
                let matches = matches!(
                    &self.reader,
                    ReaderState::AccountLoading { id, request_id: current, generation: current_generation }
                        if id == &message && *current == request_id && *current_generation == generation
                );
                if matches && generation == self.cache_generation {
                    self.reader = ReaderState::AccountLoaded { id: message, body };
                    self.bump_reader();
                }
                Update::default()
            }
            WorkerEvent::AccountBodyFailed {
                request_id,
                generation,
                message,
                failure,
            } => {
                let matches = matches!(
                    &self.reader,
                    ReaderState::AccountLoading { id, request_id: current, generation: current_generation }
                        if id == &message && *current == request_id && *current_generation == generation
                );
                if matches && generation == self.cache_generation {
                    self.reader = ReaderState::AccountFailed {
                        id: message,
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
                        feedback: Some("Downloaded message bodies and attachments cleared"),
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
                let mut update = self.load_folder(self.selected_folder_id.clone());
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
            WorkerEvent::AccountMessageSent {
                request_id,
                generation,
                account_id,
            } => self.finish_account_send(request_id, generation, &account_id, None),
            WorkerEvent::AccountMessageSendFailed {
                request_id,
                generation,
                account_id,
                failure,
            } => self.finish_account_send(request_id, generation, &account_id, Some(failure)),
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
            WorkerEvent::AccountMutationConfirmed {
                message, mutation, ..
            } => {
                // The mutation was intentionally applied only after Gmail
                // confirmed it, so a duplicate Gmail ID in another account
                // can never be changed by this acknowledgement.
                self.apply_account_mutation(&message, &mutation);
                self.bump_list();
                self.bump_reader();
                Update::default()
            }
            WorkerEvent::AccountMutationReconciled { message, state, .. } => {
                self.apply_account_reconciled(&message, state.as_ref());
                self.bump_list();
                self.bump_reader();
                Update::default()
            }
            WorkerEvent::AccountMutationFailed { uncertain, .. } => Update {
                feedback: Some(if uncertain {
                    "Gmail received the change, but its final state could not be verified"
                } else {
                    "Gmail rejected the change"
                }),
                ..Default::default()
            },
            WorkerEvent::PreferencesSaved { request_id } => {
                if self.pending_preferences_request == Some(request_id) {
                    self.pending_preferences_request = None;
                    self.preferences_save_state = PreferencesSaveState::Saved;
                }
                Update::default()
            }
            WorkerEvent::PreferencesSaveFailed { request_id } => {
                if self.pending_preferences_request == Some(request_id) {
                    self.pending_preferences_request = None;
                    self.preferences_save_state = PreferencesSaveState::Failed;
                    return Update {
                        feedback: Some("Settings changed, but could not be saved"),
                        ..Default::default()
                    };
                }
                // A completion is authoritative only for its request. In
                // particular, an old failure must not mark a newer selection
                // as unsaved.
                Update::default()
            }
            event @ (WorkerEvent::BackgroundSyncComplete { .. }
            | WorkerEvent::BackgroundSyncFailed { .. }) => self
                .background_worker_event(event)
                .expect("background event"),
            other => self.account_worker_event(other),
        }
    }

    fn background_sync_delay(failures: u8) -> Duration {
        let multiplier = 1u64.checked_shl(u32::from(failures.min(4))).unwrap_or(16);
        BACKGROUND_SYNC_FLOOR
            .saturating_mul(multiplier as u32)
            .min(BACKGROUND_SYNC_CAP)
    }

    fn arm_background_sync(&mut self, after: Duration) -> Effect {
        self.background_sync.schedule_generation = self
            .background_sync
            .schedule_generation
            .wrapping_add(1)
            .max(1);
        Effect::ScheduleBackgroundSync {
            after,
            schedule_generation: self.background_sync.schedule_generation,
        }
    }

    fn stop_background_sync(&mut self) -> Option<Effect> {
        let was_active = self.background_sync.in_flight.take().is_some()
            || self.background_sync.pending_tick
            || !self.background_sync.paused;
        self.background_sync.pending_tick = false;
        self.background_sync.paused = true;
        self.background_sync.schedule_generation = self
            .background_sync
            .schedule_generation
            .wrapping_add(1)
            .max(1);
        was_active.then_some(Effect::CancelBackgroundSyncTimer)
    }

    /// Gmail message IDs are only stable within an account.  Keep the
    /// process-local notification dedup scoped to the current identity, and
    /// invalidate any old account's request/timer before accepting its state.
    fn reset_background_for_account_transition(&mut self) {
        self.background_sync.in_flight = None;
        self.background_sync.pending_tick = false;
        self.background_sync.consecutive_failures = 0;
        self.background_sync.paused = true;
        self.background_sync.emitted_ids.clear();
        self.background_sync.schedule_generation = self
            .background_sync
            .schedule_generation
            .wrapping_add(1)
            .max(1);
    }

    fn can_begin_background_sync(&self) -> bool {
        matches!(self.session, SessionState::Ready)
            && self.account.is_some()
            && self.mailbox.is_some()
            && self.active_operation.is_none()
            && self.active_folder_request.is_none()
            && !self.folder_loading
            && self.background_sync.in_flight.is_none()
            && !self.background_sync.paused
    }

    fn background_sync_timer(&mut self, schedule_generation: u64) -> Update {
        if schedule_generation != self.background_sync.schedule_generation
            || self.background_sync.paused
        {
            return Update::default();
        }
        if !self.can_begin_background_sync() {
            self.background_sync.pending_tick = true;
            return Update {
                effects: vec![self.arm_background_sync(BACKGROUND_SYNC_FLOOR)],
                ..Default::default()
            };
        }
        let request_id = BackgroundSyncRequestId(self.background_sync.next_request);
        self.background_sync.next_request =
            self.background_sync.next_request.wrapping_add(1).max(1);
        self.background_sync.in_flight = Some(request_id);
        self.background_sync.pending_tick = false;
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::BackgroundSync {
                request_id,
                account_email: self.account.as_ref().expect("ready account").email.clone(),
            })],
            ..Default::default()
        }
    }

    fn background_worker_event(&mut self, event: WorkerEvent) -> Option<Update> {
        match event {
            WorkerEvent::BackgroundSyncComplete {
                request_id,
                account_email,
                snapshot,
                new_unread_ids,
            } => {
                if self.background_sync.in_flight != Some(request_id)
                    || !self
                        .account
                        .as_ref()
                        .is_some_and(|account| account.email.eq_ignore_ascii_case(&account_email))
                {
                    return Some(Update::default());
                }
                self.background_sync.in_flight = None;
                self.background_sync.consecutive_failures = 0;
                let mut count = 0;
                for id in new_unread_ids {
                    if self.background_sync.emitted_ids.len() >= MAX_EMITTED_BACKGROUND_IDS {
                        self.background_sync.emitted_ids.clear();
                    }
                    if self.background_sync.emitted_ids.insert(id) {
                        count += 1;
                    }
                }
                // A background Inbox snapshot must never disturb an interactive
                // search or reader. Publish it only when the Inbox list itself is
                // the whole visible surface.
                if self.selected_folder_id == FolderId::Inbox
                    && matches!(self.server_search, ServerSearchState::Idle)
                    && matches!(self.reader, ReaderState::Closed)
                {
                    self.mailbox = Some(snapshot);
                    self.normalize();
                    self.bump_list();
                    self.mirror_singleton_inbox_projection();
                }
                let mut effects = Vec::new();
                if count > 0 {
                    effects.push(Effect::NotifyNewUnread { count });
                }
                effects.push(self.arm_background_sync(BACKGROUND_SYNC_FLOOR));
                Some(Update {
                    effects,
                    ..Default::default()
                })
            }
            WorkerEvent::BackgroundSyncFailed {
                request_id,
                failure,
            } => {
                if self.background_sync.in_flight != Some(request_id) {
                    return Some(Update::default());
                }
                self.background_sync.in_flight = None;
                if !failure.retryable {
                    self.background_sync.paused = true;
                    return Some(Update::default());
                }
                self.background_sync.consecutive_failures =
                    self.background_sync.consecutive_failures.saturating_add(1);
                let after = Self::background_sync_delay(self.background_sync.consecutive_failures);
                Some(Update {
                    effects: vec![self.arm_background_sync(after)],
                    ..Default::default()
                })
            }
            _ => None,
        }
    }

    fn account_projection_worker_event(&mut self, event: WorkerEvent) -> Update {
        match event {
            WorkerEvent::Phase { id, phase } => {
                let account_id =
                    self.account_operations
                        .iter()
                        .find_map(|(account_id, operation)| {
                            (*operation == id).then(|| account_id.clone())
                        });
                if let Some(account_id) = account_id
                    && let Some(account) = self.account_mailboxes.get_mut(&account_id)
                {
                    account.session = SessionState::Syncing {
                        kind: SyncKind::Refresh,
                        phase,
                    };
                    self.bump_list();
                }
                Update::default()
            }
            WorkerEvent::AccountSynced {
                id,
                account_id,
                account,
                snapshot,
            } => {
                if self.account_operations.get(&account_id) != Some(&id) {
                    return Update::default();
                }
                self.account_operations.remove(&account_id);
                let _ = self.upsert_account_mailbox(
                    account_id,
                    account,
                    SessionState::Ready,
                    Some(snapshot),
                );
                Update {
                    feedback: Some("Gmail account reconnected"),
                    ..Default::default()
                }
            }
            WorkerEvent::AccountRemoved { id, account_id } => {
                if self.account_operations.get(&account_id) != Some(&id) {
                    return Update::default();
                }
                self.account_operations.remove(&account_id);
                self.remove_account_mailbox(&account_id);
                Update {
                    feedback: Some("Gmail account removed"),
                    ..Default::default()
                }
            }
            WorkerEvent::AccountOperationFailed {
                id,
                account_id,
                failure,
            } => {
                if self.account_operations.get(&account_id) != Some(&id) {
                    return Update::default();
                }
                self.account_operations.remove(&account_id);
                if let Some(account) = self.account_mailboxes.get_mut(&account_id) {
                    account.session = if matches!(
                        failure.kind,
                        FailureKind::AuthorizationExpired
                            | FailureKind::IdentityInvalid
                            | FailureKind::ImapAuthenticationFailed
                    ) {
                        SessionState::AuthRequired {
                            cleanup_failed: failure.cleanup_failed,
                        }
                    } else if failure.preserve_mail && account.inbox.is_some() {
                        SessionState::Offline { failure }
                    } else {
                        SessionState::ServiceError { failure }
                    };
                    self.bump_list();
                }
                Update {
                    feedback: Some("Could not update the Gmail account"),
                    ..Default::default()
                }
            }
            // A scoped command cannot legitimately emit singleton lifecycle
            // data. Dropping it keeps a stale/injected event from replacing
            // the active legacy account.
            _ => Update::default(),
        }
    }

    fn add_account_worker_event(&mut self, event: WorkerEvent) -> Update {
        let Some(id) = worker_operation_id(&event) else {
            return Update::default();
        };
        if self.adding_account_operation != Some(id) {
            return Update::default();
        }
        match event {
            WorkerEvent::Phase { .. } | WorkerEvent::IdentityVerified { .. } => Update::default(),
            WorkerEvent::AuthorizationRequired { .. } => Update {
                // The singleton session stays Ready (or whatever it already
                // was); this deadline is only for the browser authorization
                // owned by the additional account.
                effects: vec![Effect::LaunchAuthorization { id }],
                ..Default::default()
            },
            WorkerEvent::DuplicateAccountIdentity { .. } => {
                self.adding_account_operation = None;
                Update {
                    feedback: Some("That Gmail account is already connected"),
                    effects: vec![Effect::ClearAuthorization { id }],
                }
            }
            WorkerEvent::AccountAdded {
                account_id,
                account,
                snapshot,
                ..
            } => {
                self.adding_account_operation = None;
                let _ = self.upsert_account_mailbox(
                    account_id,
                    account,
                    SessionState::Ready,
                    Some(snapshot),
                );
                Update {
                    feedback: Some("Gmail account added"),
                    effects: vec![Effect::ClearAuthorization { id }],
                }
            }
            WorkerEvent::Failed { .. } => {
                self.adding_account_operation = None;
                Update {
                    feedback: Some("Could not add the Gmail account"),
                    effects: vec![Effect::ClearAuthorization { id }],
                }
            }
            WorkerEvent::Cancelled { .. } => {
                self.adding_account_operation = None;
                Update {
                    effects: vec![Effect::ClearAuthorization { id }],
                    ..Default::default()
                }
            }
            // The additive command emits none of the singleton state events.
            // Ignore any stale/injected lifecycle event rather than allowing
            // it to mutate the active legacy account.
            WorkerEvent::LegacyAccountRegistered { .. }
            | WorkerEvent::AccountPersisted { .. }
            | WorkerEvent::CacheLoaded { .. }
            | WorkerEvent::NoStoredAccount { .. }
            | WorkerEvent::SyncComplete { .. }
            | WorkerEvent::Disconnected { .. } => Update::default(),
            _ => Update::default(),
        }
    }

    fn account_worker_event(&mut self, event: WorkerEvent) -> Update {
        let Some(id) = worker_operation_id(&event) else {
            return Update::default();
        };
        if self.active_operation != Some(id) {
            return Update::default();
        }
        match event {
            // `worker_event` consumes this before lifecycle routing so the
            // restored projections cannot disturb singleton state.
            WorkerEvent::AccountMailboxesRestored { .. } => unreachable!(),
            WorkerEvent::AccountSynced { .. }
            | WorkerEvent::AccountRemoved { .. }
            | WorkerEvent::AccountOperationFailed { .. } => Update::default(),
            WorkerEvent::LegacyAccountRegistered {
                account_id,
                account,
                ..
            } => self.register_legacy_account(account_id, account),
            WorkerEvent::DuplicateAccountIdentity { .. } | WorkerEvent::AccountAdded { .. } => {
                unreachable!("add-account event reached singleton reducer")
            }
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
            WorkerEvent::IdentityVerified { account, .. } => {
                let account_is_new = self
                    .account
                    .as_ref()
                    .is_none_or(|current| !current.email.eq_ignore_ascii_case(&account.email));
                if account_is_new {
                    self.reset_background_for_account_transition();
                    self.pending_mutations.clear();
                    self.undo_operation = None;
                    self.search_generation = self.search_generation.wrapping_add(1).max(1);
                    self.server_search = ServerSearchState::Idle;
                    self.reset_draft_session();
                    self.send_generation = self.send_generation.wrapping_add(1);
                    self.composer = ComposerState::Closed;
                    self.mailbox = None;
                    self.selected_message_id = None;
                    self.reader = ReaderState::Closed;
                    self.cache_usage = CacheUsage::default();
                    self.selected_folder_id = FolderId::Inbox;
                    self.folder_generation = self.folder_generation.wrapping_add(1).max(1);
                    self.active_folder_request = None;
                    self.folder_loading = false;
                    self.bump_list();
                    self.bump_reader();
                }
                self.account = Some(account);
                Update::default()
            }
            WorkerEvent::AccountPersisted { account, .. } => {
                let account_is_new = self
                    .account
                    .as_ref()
                    .is_none_or(|current| !current.email.eq_ignore_ascii_case(&account.email));
                if account_is_new {
                    self.reset_background_for_account_transition();
                    self.pending_mutations.clear();
                    self.undo_operation = None;
                    self.search_generation = self.search_generation.wrapping_add(1).max(1);
                    self.server_search = ServerSearchState::Idle;
                    self.reset_draft_session();
                    self.send_generation = self.send_generation.wrapping_add(1);
                    self.composer = ComposerState::Closed;
                    self.mailbox = None;
                    self.selected_message_id = None;
                    self.reader = ReaderState::Closed;
                    self.cache_usage = CacheUsage::default();
                    self.selected_folder_id = FolderId::Inbox;
                    self.folder_generation = self.folder_generation.wrapping_add(1).max(1);
                    self.active_folder_request = None;
                    self.folder_loading = false;
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
                    self.reset_background_for_account_transition();
                    self.search_generation = self.search_generation.wrapping_add(1).max(1);
                    self.server_search = ServerSearchState::Idle;
                    self.reset_draft_session();
                    self.send_generation = self.send_generation.wrapping_add(1);
                    self.composer = ComposerState::Closed;
                }
                let account_email = account.email.clone();
                self.account = Some(account);
                self.selected_folder_id = FolderId::Inbox;
                self.mailbox = Some(snapshot);
                self.normalize();
                self.bump_list();
                self.mirror_singleton_inbox_projection();
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
                    self.reset_background_for_account_transition();
                    self.search_generation = self.search_generation.wrapping_add(1).max(1);
                    self.server_search = ServerSearchState::Idle;
                    self.reset_draft_session();
                    self.send_generation = self.send_generation.wrapping_add(1);
                    self.composer = ComposerState::Closed;
                    self.selected_message_id = None;
                    self.reader = ReaderState::Closed;
                    self.cache_usage = CacheUsage::default();
                    self.selected_folder_id = FolderId::Inbox;
                    self.folder_generation = self.folder_generation.wrapping_add(1).max(1);
                    self.active_folder_request = None;
                    self.folder_loading = false;
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
                self.mirror_singleton_inbox_projection();
                self.background_sync.paused = false;
                self.background_sync.consecutive_failures = 0;
                let mut effects = vec![Effect::ClearAuthorization { id }];
                effects.extend(self.ensure_local_restore(&draft_account));
                effects.push(self.arm_background_sync(BACKGROUND_SYNC_FLOOR));
                Update {
                    effects,
                    ..Default::default()
                }
            }
            WorkerEvent::Disconnected { .. } => {
                let cancel_background = self.stop_background_sync();
                self.pending_mutations.clear();
                self.undo_operation = None;
                self.search_generation = self.search_generation.wrapping_add(1).max(1);
                self.server_search = ServerSearchState::Idle;
                self.attachment_jobs.clear();
                self.account = None;
                self.selected_folder_id = FolderId::Inbox;
                self.folder_generation = self.folder_generation.wrapping_add(1).max(1);
                self.active_folder_request = None;
                self.folder_loading = false;
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
                Update {
                    effects: cancel_background.into_iter().collect(),
                    ..Default::default()
                }
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
            | WorkerEvent::AttachmentProgress { .. }
            | WorkerEvent::AttachmentCompleted { .. }
            | WorkerEvent::AttachmentFailed { .. }
            | WorkerEvent::AttachmentCancelled { .. }
            | WorkerEvent::FolderCacheLoaded { .. }
            | WorkerEvent::FolderLoaded { .. }
            | WorkerEvent::FolderFailed { .. }
            | WorkerEvent::SearchLoaded { .. }
            | WorkerEvent::SearchFailed { .. }
            | WorkerEvent::SearchCancelled { .. }
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
            | WorkerEvent::MutationFailed { .. }
            | WorkerEvent::PreferencesSaved { .. }
            | WorkerEvent::PreferencesSaveFailed { .. }
            | WorkerEvent::BackgroundSyncComplete { .. }
            | WorkerEvent::BackgroundSyncFailed { .. } => unreachable!(),
            _ => Update::default(),
        }
    }
    fn upsert_account_mailbox(
        &mut self,
        account_id: AccountId,
        identity: AccountIdentity,
        session: SessionState,
        mailbox: Option<MailboxSnapshot>,
    ) -> Update {
        let is_existing = self.account_mailboxes.contains_key(&account_id);
        if !is_existing && self.account_mailboxes.len() >= MAX_ACCOUNTS {
            return Update {
                feedback: Some("Whitford can store up to 32 Gmail accounts"),
                ..Default::default()
            };
        }
        if self.account_mailboxes.iter().any(|(id, account)| {
            id != &account_id && account.identity.email.eq_ignore_ascii_case(&identity.email)
        }) {
            return Update {
                feedback: Some("That Gmail account is already connected"),
                ..Default::default()
            };
        }
        let folders = self
            .account_mailboxes
            .get(&account_id)
            .map(|account| account.folders.clone())
            .unwrap_or_default();
        self.account_mailboxes.insert(
            account_id,
            AccountMailbox {
                identity,
                session,
                inbox: mailbox,
                folders,
            },
        );
        self.reconcile_account_selection();
        self.bump_list();
        Update::default()
    }

    fn restore_account_mailboxes(&mut self, accounts: Vec<RestoredAccountMailbox>) -> Update {
        // Each row came from the durable registry. Keep rows whose snapshots
        // are unavailable too: losing a cache must never look like removing
        // an account. These are local projections only, so `Ready` means
        // "ready to view cached mail", not that a live token was refreshed.
        for restored in accounts {
            let _ = self.upsert_account_mailbox(
                restored.account_id,
                restored.account,
                SessionState::Ready,
                restored.snapshot,
            );
        }
        Update::default()
    }

    fn register_legacy_account(
        &mut self,
        account_id: AccountId,
        identity: AccountIdentity,
    ) -> Update {
        // A stale registration event must not create a projection for an
        // account that is no longer the singleton worker's verified identity.
        if !self
            .account
            .as_ref()
            .is_some_and(|current| current.email.eq_ignore_ascii_case(&identity.email))
        {
            return Update::default();
        }
        // A reconnect may verify while the legacy UI is showing a non-Inbox
        // folder. Never misfile that folder snapshot as this account's Inbox.
        let inbox = if self.selected_folder_id == FolderId::Inbox {
            self.mailbox.clone()
        } else {
            self.account_mailboxes
                .get(&account_id)
                .and_then(|account| account.inbox.clone())
        };
        self.upsert_account_mailbox(account_id, identity, self.session.clone(), inbox)
    }

    /// Keeps the account-aware Inbox projection in step with the only live
    /// worker session. This is intentionally one-way: selecting an account
    /// projection never changes the legacy singleton worker's identity.
    fn mirror_singleton_inbox_projection(&mut self) {
        let Some(identity) = self.account.clone() else {
            return;
        };
        let Some(account_id) = self
            .account_mailboxes
            .iter()
            .find(|(_, account)| account.identity.email.eq_ignore_ascii_case(&identity.email))
            .map(|(id, _)| id.clone())
        else {
            return;
        };
        let _ = self.upsert_account_mailbox(
            account_id,
            identity,
            self.session.clone(),
            self.mailbox.clone(),
        );
    }

    fn project_singleton_folder(&mut self, folder_id: FolderId, snapshot: MailboxSnapshot) {
        let Some(identity) = self.account.as_ref() else {
            return;
        };
        let Some(account_id) = self
            .account_mailboxes
            .iter()
            .find(|(_, account)| account.identity.email.eq_ignore_ascii_case(&identity.email))
            .map(|(id, _)| id.clone())
        else {
            return;
        };
        let _ = self.upsert_account_folder(
            AccountFolderId {
                account_id,
                folder_id,
            },
            snapshot,
        );
    }

    fn upsert_account_folder(
        &mut self,
        folder: AccountFolderId,
        snapshot: MailboxSnapshot,
    ) -> Update {
        let Some(account) = self.account_mailboxes.get_mut(&folder.account_id) else {
            return Update {
                feedback: Some("That Gmail account is unavailable"),
                ..Default::default()
            };
        };
        if folder.folder_id == FolderId::Inbox {
            account.inbox = Some(snapshot);
        } else {
            account.folders.insert(folder.folder_id, snapshot);
        }
        self.reconcile_account_selection();
        self.bump_list();
        Update::default()
    }

    fn remove_account_mailbox(&mut self, account_id: &AccountId) -> Update {
        if self.account_mailboxes.remove(account_id).is_none() {
            return Update::default();
        }
        self.reconcile_account_selection();
        self.bump_list();
        Update::default()
    }

    fn select_mailbox_view(&mut self, view: MailboxView) -> Update {
        if let MailboxView::AccountFolder(scope) = &view {
            let Some(account) = self.account_mailboxes.get(&scope.account_id) else {
                return Update {
                    feedback: Some("That Gmail account is unavailable"),
                    ..Default::default()
                };
            };
            if account
                .inbox
                .as_ref()
                .is_some_and(|mailbox| mailbox.folder_catalog.find(&scope.folder_id).is_none())
            {
                return Update {
                    feedback: Some("That Gmail folder is unavailable"),
                    ..Default::default()
                };
            }
        }
        if self.selected_mailbox_view == view {
            return Update::default();
        }
        self.selected_mailbox_view = view;
        self.reconcile_account_selection();
        self.bump_list();
        Update::default()
    }

    fn select_account_message(&mut self, id: AccountMessageId) -> Update {
        if !self
            .account_visible_messages()
            .iter()
            .any(|message| message.id == id)
        {
            return Update {
                feedback: Some("Message is unavailable"),
                ..Default::default()
            };
        }
        self.selected_account_message = Some(id);
        self.bump_list();
        self.open_selected_account()
    }

    fn selected_account_summary(&self) -> Option<AccountMessageSummary> {
        let id = self.selected_account_message.as_ref()?;
        self.account_visible_messages()
            .into_iter()
            .find(|message| &message.id == id)
    }

    fn open_selected_account(&mut self) -> Update {
        let Some(message) = self.selected_account_summary() else {
            return Update::default();
        };
        if matches!(&self.reader, ReaderState::AccountLoaded { id, .. } if id == &message.id) {
            return Update::default();
        }
        let request_id = BodyRequestId(self.next_body_request);
        self.next_body_request = self.next_body_request.wrapping_add(1).max(1);
        self.reader = ReaderState::AccountLoading {
            id: message.id.clone(),
            request_id,
            generation: self.cache_generation,
        };
        self.bump_reader();
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::FetchAccountBody {
                request_id,
                generation: self.cache_generation,
                message: message.id,
                account_email: message.account.email,
                locator: message.message.locator,
            })],
            ..Default::default()
        }
    }

    fn reconcile_account_selection(&mut self) {
        let selected_view_exists = match &self.selected_mailbox_view {
            MailboxView::UnifiedInbox => true,
            MailboxView::AccountFolder(scope) => {
                self.account_mailboxes.contains_key(&scope.account_id)
            }
        };
        if !selected_view_exists {
            self.selected_mailbox_view = MailboxView::UnifiedInbox;
        }
        if self
            .selected_account_message
            .as_ref()
            .is_some_and(|selected| {
                !self
                    .account_visible_messages()
                    .iter()
                    .any(|message| &message.id == selected)
            })
        {
            self.selected_account_message = None;
        }
    }

    fn account_view_summaries(&self) -> Vec<AccountViewSummary> {
        self.account_mailboxes
            .iter()
            .map(|(id, account)| AccountViewSummary {
                id: id.clone(),
                identity: account.identity.clone(),
                session: account.session.clone(),
                unread_count: account
                    .inbox
                    .as_ref()
                    .map(|mailbox| {
                        mailbox
                            .messages
                            .iter()
                            .filter(|message| message.unread && message_is_in_inbox(message))
                            .count()
                    })
                    .unwrap_or_default(),
            })
            .collect()
    }

    fn account_visible_messages(&self) -> Vec<AccountMessageSummary> {
        let mut messages = match &self.selected_mailbox_view {
            MailboxView::UnifiedInbox => self
                .account_mailboxes
                .iter()
                .flat_map(|(account_id, account)| {
                    account.inbox.iter().flat_map(move |mailbox| {
                        mailbox
                            .messages
                            .iter()
                            .filter(|message| message_is_in_inbox(message))
                            .map(move |message| AccountMessageSummary {
                                id: AccountMessageId {
                                    account_id: account_id.clone(),
                                    message_id: message.id.clone(),
                                },
                                account: account.identity.clone(),
                                message: message.clone(),
                            })
                    })
                })
                .collect::<Vec<_>>(),
            MailboxView::AccountFolder(scope) => {
                let Some(account) = self.account_mailboxes.get(&scope.account_id) else {
                    return Vec::new();
                };
                let mailbox = if scope.folder_id == FolderId::Inbox {
                    account.inbox.as_ref()
                } else {
                    account.folders.get(&scope.folder_id)
                };
                mailbox
                    .map(|mailbox| {
                        mailbox
                            .messages
                            .iter()
                            .filter(|message| message_is_in_folder(message, &scope.folder_id))
                            .map(|message| AccountMessageSummary {
                                id: AccountMessageId {
                                    account_id: scope.account_id.clone(),
                                    message_id: message.id.clone(),
                                },
                                account: account.identity.clone(),
                                message: message.clone(),
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            }
        };
        messages.sort_by(compare_account_messages_newest_first);
        messages
    }

    pub fn snapshot(&self) -> ViewSnapshot {
        self.snapshot_for_render(true)
    }

    pub(crate) fn list_revision(&self) -> u64 {
        self.list_revision
    }

    pub(crate) fn snapshot_for_render(&self, include_visible_messages: bool) -> ViewSnapshot {
        let sidebar_folders =
            sidebar_folders(self.mailbox.as_ref().map(|mailbox| &mailbox.folder_catalog));
        let account_visible_messages = if include_visible_messages {
            self.account_visible_messages()
        } else {
            Vec::new()
        };
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
            _ if self.folder_loading && !has_visible_messages => ViewStatus::Loading,
            _ if !has_visible_messages
                && (!self.search_query.is_empty() || self.message_filter != MessageFilter::All) =>
            {
                ViewStatus::NoSearchResults
            }
            _ if !has_visible_messages => ViewStatus::EmptyInbox,
            _ => ViewStatus::Ready,
        };
        ViewSnapshot {
            // Compatibility flattened form of the rendered sidebar. Message
            // label actions retain the provider's full catalog separately in
            // `label_options` above.
            folders: sidebar_folders
                .primary
                .iter()
                .chain(sidebar_folders.labels.iter())
                .cloned()
                .collect(),
            sidebar_folders,
            can_mutate: self.can_change_selected_message(),
            can_archive: self.can_archive_selected(),
            can_move_to_trash: self.can_trash_selected(),
            undo_message_operation: self.undo_operation.as_ref().map(|undo| {
                UndoMessageOperationView {
                    id: undo.id,
                    message_id: undo.message_id.clone(),
                    title: undo.title.clone(),
                    can_undo: matches!(undo.phase, UndoPhase::Available),
                    pending: !matches!(undo.phase, UndoPhase::Available),
                }
            }),
            label_options: self
                .selected_account_message
                .as_ref()
                .and_then(|id| self.account_mailboxes.get(&id.account_id))
                .and_then(|account| account.inbox.as_ref())
                .map(|mailbox| {
                    (
                        &mailbox.folder_catalog,
                        self.selected_account_summary()
                            .map(|message| message.message),
                    )
                })
                .or_else(|| {
                    self.mailbox
                        .as_ref()
                        .map(|mailbox| (&mailbox.folder_catalog, self.selected_message().cloned()))
                })
                .map(|mailbox| {
                    mailbox
                        .0
                        .folders
                        .iter()
                        .filter_map(|folder| match &folder.id {
                            FolderId::Label(mailbox_name) => Some((
                                mailbox_name.clone(),
                                folder.display_name.clone(),
                                mailbox
                                    .1
                                    .as_ref()
                                    .is_some_and(|message| message.labels.contains(mailbox_name)),
                            )),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            attachment_downloads: {
                let mut jobs = self.attachment_jobs.values().cloned().collect::<Vec<_>>();
                jobs.sort_by_key(|job| job.job_id.0);
                jobs
            },
            background_sync_status: if self.background_sync.paused {
                BackgroundSyncStatus::Paused
            } else if self.background_sync.in_flight.is_some() {
                BackgroundSyncStatus::Syncing
            } else if self.background_sync.consecutive_failures > 0 {
                BackgroundSyncStatus::BackingOff
            } else {
                BackgroundSyncStatus::Idle
            },
            accounts: self.account_view_summaries(),
            mailbox_view: self.selected_mailbox_view.clone(),
            account_visible_messages,
            selected_account_message: self.selected_account_message.clone(),
            visible_messages: visible,
            selected_message: self.selected_message().cloned(),
            selected_folder_id: self.selected_folder_id.clone(),
            search_query: self.search_query.clone(),
            server_search: match &self.server_search {
                ServerSearchState::Idle => ServerSearchView::Idle,
                ServerSearchState::Loading { .. } => ServerSearchView::Loading,
                ServerSearchState::Loaded {
                    messages,
                    truncated,
                    skipped_count,
                    ..
                } => ServerSearchView::Results {
                    count: messages.len(),
                    truncated: *truncated,
                    skipped_count: *skipped_count,
                },
                ServerSearchState::Failed { failure, .. } => ServerSearchView::Failed(*failure),
            },
            message_filter: self.message_filter,
            status,
            folder_counts: vec![(
                self.selected_folder_id.clone(),
                self.folder_count(&self.selected_folder_id),
            )],
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
            appearance: self.appearance,
            preferences_save_state: self.preferences_save_state,
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
        if let ServerSearchState::Loaded { messages, .. } = &self.server_search
            && let Some(message) = messages.iter().find(|message| &message.id == id)
        {
            return Some(message);
        }
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
        if let ServerSearchState::Loaded { messages, .. } = &self.server_search {
            return messages
                .iter()
                .filter(|message| self.matches_filter(message))
                .collect();
        }
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
                            && self.matches_filter(m)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    fn has_visible_messages(&self) -> bool {
        if let ServerSearchState::Loaded { messages, .. } = &self.server_search {
            return messages.iter().any(|message| self.matches_filter(message));
        }
        let query = self.search_query.to_lowercase();
        self.mailbox.as_ref().is_some_and(|mailbox| {
            mailbox.messages.iter().any(|message| {
                (query.is_empty()
                    || message.sender.to_lowercase().contains(&query)
                    || message.subject.to_lowercase().contains(&query))
                    && self.matches_filter(message)
            })
        })
    }
    fn matches_filter(&self, message: &MessageSummary) -> bool {
        match self.message_filter {
            MessageFilter::All => true,
            MessageFilter::Unread => message.unread,
            MessageFilter::Attachments => message.attachment_state.has_attachments(),
        }
    }
    fn invalidate_server_search(&mut self) -> Vec<Effect> {
        let (active, clears_ephemeral_selection) = match self.server_search {
            ServerSearchState::Loading { request_id, .. } => (Some(request_id), false),
            ServerSearchState::Idle => return Vec::new(),
            ServerSearchState::Loaded { .. } => (None, true),
            ServerSearchState::Failed { .. } => (None, false),
        };
        self.search_generation = self.search_generation.wrapping_add(1).max(1);
        self.server_search = ServerSearchState::Idle;
        if clears_ephemeral_selection {
            self.selected_message_id = None;
            self.reader = ReaderState::Closed;
            self.bump_reader();
        }
        active
            .map(|request_id| {
                Effect::SendWorker(WorkerCommand::CancelSearch {
                    request_id,
                    generation: self.search_generation,
                })
            })
            .into_iter()
            .collect()
    }
    fn submit_server_search(&mut self) -> Update {
        let Some(query) =
            crate::gmail::validate_search_query(&self.search_query).map(str::to_owned)
        else {
            return Update {
                feedback: Some("Enter a Gmail search up to 2 KiB"),
                ..Default::default()
            };
        };
        if !matches!(self.session, SessionState::Ready) {
            return Update {
                feedback: Some("Connect Gmail before searching all mail"),
                ..Default::default()
            };
        }
        let Some(account_email) = self.account.as_ref().map(|account| account.email.clone()) else {
            return Update::default();
        };
        let Some(folder) = self
            .mailbox
            .as_ref()
            .and_then(|mailbox| mailbox.folder_catalog.find(&FolderId::AllMail))
            .cloned()
        else {
            return Update {
                feedback: Some("Gmail All Mail is unavailable for this account"),
                ..Default::default()
            };
        };
        let mut effects = self.invalidate_server_search();
        self.search_generation = self.search_generation.wrapping_add(1).max(1);
        let request_id = SearchRequestId(self.next_search_request);
        self.next_search_request = self.next_search_request.wrapping_add(1).max(1);
        self.server_search = ServerSearchState::Loading {
            request_id,
            generation: self.search_generation,
            query: query.clone(),
        };
        // While search has no result set, a folder selection is not a valid action target.
        // Clearing it prevents toolbar/keyboard actions from mutating a stale row.
        self.selected_message_id = None;
        self.reader = ReaderState::Closed;
        self.bump_reader();
        self.bump_list();
        effects.push(Effect::SendWorker(WorkerCommand::SearchGmail {
            request_id,
            generation: self.search_generation,
            account_email,
            folder,
            query,
        }));
        Update {
            effects,
            ..Default::default()
        }
    }
    fn cancel_server_search(&mut self) -> Update {
        if !matches!(self.server_search, ServerSearchState::Loading { .. }) {
            return Update::default();
        }
        let effects = self.invalidate_server_search();
        self.normalize();
        self.bump_list();
        Update {
            feedback: Some("Gmail search cancelled"),
            effects,
        }
    }
    fn retry_server_search(&mut self) -> Update {
        let ServerSearchState::Failed { query, .. } = &self.server_search else {
            return Update::default();
        };
        self.search_query = query.clone();
        self.submit_server_search()
    }
    fn search_loaded(
        &mut self,
        request_id: SearchRequestId,
        generation: u64,
        messages: Vec<MessageSummary>,
        truncated: bool,
        skipped_count: usize,
    ) -> Update {
        let ServerSearchState::Loading {
            request_id: current,
            generation: current_generation,
            ..
        } = &self.server_search
        else {
            return Update::default();
        };
        if *current != request_id || *current_generation != generation {
            return Update::default();
        }
        self.server_search = ServerSearchState::Loaded {
            messages,
            truncated,
            skipped_count,
        };
        self.selected_message_id = None;
        self.reader = ReaderState::Closed;
        self.normalize();
        self.bump_reader();
        self.bump_list();
        Update::default()
    }
    fn search_failed(
        &mut self,
        request_id: SearchRequestId,
        generation: u64,
        failure: BodyFailure,
    ) -> Update {
        let ServerSearchState::Loading {
            request_id: current,
            generation: current_generation,
            query,
        } = &self.server_search
        else {
            return Update::default();
        };
        if *current != request_id || *current_generation != generation {
            return Update::default();
        }
        let query = query.clone();
        self.server_search = ServerSearchState::Failed { query, failure };
        self.normalize();
        Update {
            feedback: Some(match failure {
                BodyFailure::Offline => "Gmail search is offline",
                BodyFailure::TimedOut => "Gmail search took too long",
                BodyFailure::AuthorizationRequired => "Reconnect Gmail to search all mail",
                BodyFailure::MailboxChanged | BodyFailure::Missing | BodyFailure::Protocol => {
                    "Gmail search failed"
                }
            }),
            ..Default::default()
        }
    }
    fn load_folder(&mut self, id: FolderId) -> Update {
        let Some(account_email) = self.account.as_ref().map(|account| account.email.clone()) else {
            return Update {
                feedback: Some("Connect Gmail first"),
                ..Default::default()
            };
        };
        let Some(catalog) = self
            .mailbox
            .as_ref()
            .map(|mailbox| mailbox.folder_catalog.clone())
        else {
            return Update::default();
        };
        let Some(folder) = catalog.find(&id).cloned() else {
            return Update {
                feedback: Some("That Gmail folder is unavailable"),
                ..Default::default()
            };
        };
        let mut effects = self.invalidate_server_search();
        let switching = self.selected_folder_id != id;
        self.folder_generation = self.folder_generation.wrapping_add(1).max(1);
        let request_id = FolderRequestId(self.next_folder_request);
        self.next_folder_request = self.next_folder_request.wrapping_add(1).max(1);
        self.active_folder_request = Some(request_id);
        self.folder_loading = true;
        self.selected_folder_id = id;
        if switching {
            let mut empty = MailboxSnapshot::empty(SystemTime::now());
            empty.folder_catalog = catalog.clone();
            empty.metadata.requested_limit = self.cache_limit;
            self.mailbox = Some(empty);
            self.selected_message_id = None;
            self.reader = ReaderState::Closed;
            self.bump_reader();
        }
        self.normalize();
        self.bump_list();
        effects.push(Effect::SendWorker(WorkerCommand::FetchFolder {
            request_id,
            generation: self.folder_generation,
            account_email,
            folder,
            catalog,
        }));
        Update {
            effects,
            ..Default::default()
        }
    }

    fn folder_loaded(
        &mut self,
        request_id: FolderRequestId,
        generation: u64,
        folder_id: FolderId,
        snapshot: MailboxSnapshot,
        cached: bool,
    ) -> Update {
        if self.active_folder_request != Some(request_id)
            || self.folder_generation != generation
            || self.selected_folder_id != folder_id
        {
            return Update::default();
        }
        self.mailbox = Some(snapshot);
        self.folder_loading = cached;
        if !cached {
            self.active_folder_request = None;
            self.session = SessionState::Ready;
            self.recovery = None;
        }
        self.normalize();
        self.bump_list();
        Update::default()
    }

    fn folder_loaded_and_project(
        &mut self,
        request_id: FolderRequestId,
        generation: u64,
        folder_id: FolderId,
        snapshot: MailboxSnapshot,
        cached: bool,
    ) -> Update {
        // `folder_loaded` owns the stale-event gate. Keep the exact same gate
        // here so a late singleton folder result cannot enter the account
        // projection after the user has navigated elsewhere.
        let accepted = self.active_folder_request == Some(request_id)
            && self.folder_generation == generation
            && self.selected_folder_id == folder_id;
        let update = self.folder_loaded(
            request_id,
            generation,
            folder_id.clone(),
            snapshot.clone(),
            cached,
        );
        if accepted {
            self.project_singleton_folder(folder_id, snapshot);
        }
        update
    }

    fn folder_failed(
        &mut self,
        request_id: FolderRequestId,
        generation: u64,
        folder_id: &FolderId,
        failure: BodyFailure,
    ) -> Update {
        if self.active_folder_request != Some(request_id)
            || self.folder_generation != generation
            || &self.selected_folder_id != folder_id
        {
            return Update::default();
        }
        self.active_folder_request = None;
        self.folder_loading = false;
        self.bump_list();
        match failure {
            BodyFailure::Offline | BodyFailure::TimedOut => {
                self.recovery = Some(RecoveryAction::Folder(folder_id.clone()));
                self.session = SessionState::Offline {
                    failure: ServiceFailure {
                        kind: if failure == BodyFailure::TimedOut {
                            FailureKind::SyncTimedOut
                        } else {
                            FailureKind::Network
                        },
                        retryable: true,
                        preserve_mail: true,
                        cleanup_failed: false,
                        config_path: None,
                    },
                };
            }
            BodyFailure::AuthorizationRequired => {
                self.recovery = Some(RecoveryAction::Sync(SyncKind::Connect));
                self.session = SessionState::AuthRequired {
                    cleanup_failed: false,
                };
            }
            BodyFailure::MailboxChanged | BodyFailure::Missing | BodyFailure::Protocol => {}
        }
        Update {
            feedback: Some(match failure {
                BodyFailure::Offline => "Showing cached mail; Gmail is offline",
                BodyFailure::TimedOut => "Showing cached mail; Gmail took too long to respond",
                BodyFailure::AuthorizationRequired => {
                    "Refresh Gmail authorization to load this folder"
                }
                BodyFailure::MailboxChanged | BodyFailure::Missing | BodyFailure::Protocol => {
                    "Could not refresh this Gmail folder"
                }
            }),
            ..Default::default()
        }
    }
    fn select_message(&mut self, id: MessageId) -> Update {
        if self.visible_message_ids().contains(&id) {
            self.selected_account_message = None;
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
        if self.selected_account_message.is_some() {
            return self.mutate_account_selected(build);
        }
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
        if matches!(mutation, MessageMutation::Archive) && !self.can_archive_message(&message) {
            return Update {
                feedback: Some("This message is not in Inbox"),
                ..Default::default()
            };
        }
        if matches!(mutation, MessageMutation::MoveToTrash { .. })
            && !self.can_trash_message(&message)
        {
            return Update {
                feedback: Some("This message is already in Trash"),
                ..Default::default()
            };
        }
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
        let previous =
            match &mutation {
                MessageMutation::SetRead(_) => PendingValue::Bool(message.unread),
                MessageMutation::SetStarred(_) => PendingValue::Bool(message.starred),
                MessageMutation::SetLabel { mailbox, .. } => {
                    PendingValue::Label(message.labels.contains(mailbox))
                }
                MessageMutation::Archive | MessageMutation::MoveToTrash { .. } => self
                    .remove_from_views(&message.id, matches!(mutation, MessageMutation::Archive)),
                // Restore mutations are internal Undo transport operations and are never built by
                // the public selection path. Keep this arm defensive for future callers.
                MessageMutation::RestoreArchive { .. }
                | MessageMutation::RestoreFromTrash { .. } => PendingValue::Removed(Vec::new()),
            };
        if matches!(
            mutation,
            MessageMutation::Archive | MessageMutation::MoveToTrash { .. }
        ) {
            self.normalize();
            self.bump_reader();
        } else {
            self.apply_to_visible_representations(&message.id, &mutation);
        }
        self.pending_mutations.insert(
            key,
            PendingMutation {
                request_id,
                origin_folder_id: self.selected_folder_id.clone(),
                mutation: mutation.clone(),
                previous,
            },
        );
        if matches!(
            mutation,
            MessageMutation::Archive | MessageMutation::MoveToTrash { .. }
        ) {
            let previous = self
                .pending_mutations
                .values()
                .find(|pending| pending.request_id == request_id)
                .expect("new pending mutation exists")
                .previous
                .clone();
            self.undo_operation = Some(UndoOperation {
                id: request_id.0,
                message_id: message.id.clone(),
                forward: mutation.clone(),
                previous,
                locator: message.locator.clone(),
                catalog: catalog.clone(),
                title: message.subject.clone(),
                phase: UndoPhase::ForwardPending {
                    request_id,
                    requested: false,
                },
            });
        }
        self.bump_list();
        self.bump_reader();
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::MutateMessageInCatalog {
                request_id,
                generation: self.cache_generation,
                account_email,
                message_id: message.id,
                locator: message.locator,
                catalog,
                mutation,
            })],
            ..Default::default()
        }
    }

    fn mutate_account_selected(
        &mut self,
        build: impl FnOnce(&MessageSummary, &crate::model::FolderCatalog) -> MessageMutation,
    ) -> Update {
        let Some(selected) = self.selected_account_summary() else {
            return Update {
                feedback: Some("Message is unavailable"),
                ..Default::default()
            };
        };
        let Some(account) = self.account_mailboxes.get(&selected.id.account_id) else {
            return Update {
                feedback: Some("That Gmail account is unavailable"),
                ..Default::default()
            };
        };
        let Some(catalog) = account
            .inbox
            .as_ref()
            .map(|inbox| inbox.folder_catalog.clone())
        else {
            return Update {
                feedback: Some("That Gmail mailbox is unavailable"),
                ..Default::default()
            };
        };
        let mutation = build(&selected.message, &catalog);
        if matches!(mutation, MessageMutation::Archive)
            && (!selected.message.in_inbox || selected.message.in_trash)
        {
            return Update {
                feedback: Some("This message is not in Inbox"),
                ..Default::default()
            };
        }
        if matches!(mutation, MessageMutation::MoveToTrash { .. }) && selected.message.in_trash {
            return Update {
                feedback: Some("This message is already in Trash"),
                ..Default::default()
            };
        }
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
        let request_id = MutationRequestId(self.next_mutation_request);
        self.next_mutation_request = self.next_mutation_request.wrapping_add(1).max(1);
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::MutateAccountMessage {
                request_id,
                generation: self.cache_generation,
                message: selected.id,
                account_email: selected.account.email,
                locator: selected.message.locator,
                catalog,
                mutation,
            })],
            ..Default::default()
        }
    }

    fn apply_account_mutation(&mut self, id: &AccountMessageId, mutation: &MessageMutation) {
        let Some(account) = self.account_mailboxes.get_mut(&id.account_id) else {
            return;
        };
        let apply = |message: &mut MessageSummary| {
            if message.id == id.message_id {
                mutation.apply(message);
                match mutation {
                    MessageMutation::Archive => message.in_inbox = false,
                    MessageMutation::MoveToTrash { .. } => {
                        message.in_inbox = false;
                        message.in_trash = true;
                    }
                    _ => {}
                }
            }
        };
        if let Some(inbox) = account.inbox.as_mut() {
            inbox.messages.iter_mut().for_each(&apply);
            inbox.messages.retain(message_is_in_inbox);
            inbox.metadata.loaded_count = inbox.messages.len();
        }
        for snapshot in account.folders.values_mut() {
            snapshot.messages.iter_mut().for_each(&apply);
            snapshot.messages.retain(Self::belongs_in_projection);
            snapshot.metadata.loaded_count = snapshot.messages.len();
        }
    }

    fn apply_account_reconciled(
        &mut self,
        id: &AccountMessageId,
        state: Option<&ReconciledMessageState>,
    ) {
        let Some(account) = self.account_mailboxes.get_mut(&id.account_id) else {
            return;
        };
        let reconcile = |message: &mut MessageSummary| {
            if message.id == id.message_id
                && let Some(state) = state
            {
                message.unread = state.unread;
                message.starred = state.starred;
                message.in_inbox = state.in_inbox;
                message.in_trash = state.in_trash;
                message.labels = state.labels.clone();
            }
        };
        if let Some(inbox) = account.inbox.as_mut() {
            inbox.messages.iter_mut().for_each(&reconcile);
            if state.is_none() {
                inbox.messages.retain(|message| message.id != id.message_id);
            } else {
                inbox.messages.retain(message_is_in_inbox);
            }
            inbox.metadata.loaded_count = inbox.messages.len();
        }
        for snapshot in account.folders.values_mut() {
            snapshot.messages.iter_mut().for_each(&reconcile);
            if state.is_none() {
                snapshot
                    .messages
                    .retain(|message| message.id != id.message_id);
            } else {
                snapshot.messages.retain(Self::belongs_in_projection);
            }
            snapshot.metadata.loaded_count = snapshot.messages.len();
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
        if self.undo_operation.as_ref().is_some_and(|undo| {
            matches!(undo.phase, UndoPhase::UndoPending { request_id: current } if current == request_id)
                && undo.message_id == *message_id
        }) {
            return self.finish_undo_mutation(confirmed, reconciled, uncertain, message_id);
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
        let mut preconfirmed_undo_requested = false;
        if let Some(undo) = self.undo_operation.as_mut()
            && undo.message_id == *message_id
            && matches!(undo.phase, UndoPhase::ForwardPending { request_id: current, .. } if current == request_id)
        {
            if confirmed {
                let requested = matches!(
                    undo.phase,
                    UndoPhase::ForwardPending {
                        requested: true,
                        ..
                    }
                );
                undo.phase = UndoPhase::Available;
                if requested {
                    let mut undo = self.undo_operation.take().expect("undo operation exists");
                    return match self.dispatch_undo(&mut undo) {
                        Ok(update) => {
                            self.undo_operation = Some(undo);
                            update
                        }
                        Err(feedback) => Update {
                            feedback: Some(feedback),
                            ..Default::default()
                        },
                    };
                }
                return Update {
                    feedback: Some("Message changed — Undo is available"),
                    ..Default::default()
                };
            }
            // A failed or uncertain forward is handled by the normal rollback/reconciliation
            // path below. An Undo token must not survive a result we could not confirm.
            preconfirmed_undo_requested = matches!(
                undo.phase,
                UndoPhase::ForwardPending {
                    requested: true,
                    ..
                }
            );
            self.undo_operation = None;
        }
        let origin_is_visible = self.selected_folder_id == pending.origin_folder_id;
        let definite_failure = !confirmed && reconciled.is_none() && !uncertain;
        if uncertain && preconfirmed_undo_requested && origin_is_visible {
            // The user restored locally while the forward request was pending. Its outcome is
            // unknown, so retain the safer forward presentation rather than claiming Undo won.
            self.remove_from_views(
                message_id,
                matches!(pending.mutation, MessageMutation::Archive),
            );
        } else if definite_failure && origin_is_visible {
            match pending.previous {
                PendingValue::Bool(value) => {
                    self.restore_boolean_to_visible_representations(
                        message_id,
                        &pending.mutation,
                        value,
                    );
                }
                PendingValue::Label(applied) => {
                    self.restore_label_to_visible_representations(
                        message_id,
                        &pending.mutation,
                        applied,
                    );
                }
                PendingValue::Removed(rows) => self.restore_removed_rows(rows),
            }
        } else if origin_is_visible && let Some(state) = reconciled {
            match state {
                Some(state) => {
                    // Reconciliation says the message remains in Inbox. Put back the exact
                    // projections removed optimistically only where its authoritative system
                    // membership permits them (for example, archive may have succeeded even
                    // though flags/labels were reconciled through All Mail).
                    if let PendingValue::Removed(rows) = &pending.previous {
                        self.restore_reconciled_rows(rows.clone(), &state);
                    }
                    self.apply_reconciled_to_visible_representations(message_id, &state);
                }
                None => {
                    self.remove_message_from_all_visible_representations(message_id);
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

    fn finish_undo_mutation(
        &mut self,
        confirmed: bool,
        reconciled: Option<Option<ReconciledMessageState>>,
        uncertain: bool,
        message_id: &MessageId,
    ) -> Update {
        let undo = self.undo_operation.take().expect("matching undo exists");
        if confirmed {
            self.normalize();
            self.bump_list();
            self.bump_reader();
            return Update {
                feedback: Some("Message action undone"),
                ..Default::default()
            };
        }
        if let Some(state) = reconciled {
            match state {
                Some(state) => {
                    if let PendingValue::Removed(rows) = undo.previous {
                        self.restore_reconciled_rows(rows, &state);
                    }
                    self.apply_reconciled_to_visible_representations(message_id, &state);
                }
                None => self.remove_message_from_all_visible_representations(message_id),
            }
        } else {
            // The local restore was optimistic. If Gmail rejected it, reinstate the forward
            // presentation so the UI does not claim an Undo that never reached the server.
            self.remove_from_views(message_id, matches!(undo.forward, MessageMutation::Archive));
        }
        self.normalize();
        self.bump_list();
        self.bump_reader();
        Update {
            feedback: if uncertain {
                Some("Undo may have reached Gmail, but its final state could not be verified")
            } else {
                Some("Gmail could not undo the message action")
            },
            ..Default::default()
        }
    }

    fn can_change_selected_message(&self) -> bool {
        if self.selected_account_message.is_some() {
            return self.selected_account_summary().is_some();
        }
        matches!(self.session, SessionState::Ready) && self.selected_message().is_some()
    }

    fn undo_message_operation(&mut self) -> Update {
        let Some(mut undo) = self.undo_operation.take() else {
            return Update {
                feedback: Some("There is no message action to undo"),
                ..Default::default()
            };
        };
        match undo.phase {
            UndoPhase::ForwardPending { request_id, .. } => {
                self.restore_pending_value(undo.previous.clone());
                undo.phase = UndoPhase::ForwardPending {
                    request_id,
                    requested: true,
                };
                self.undo_operation = Some(undo);
                self.normalize();
                self.bump_list();
                self.bump_reader();
                Update {
                    feedback: Some("Undo will finish after Gmail confirms the message action"),
                    ..Default::default()
                }
            }
            UndoPhase::Available => match self.dispatch_undo(&mut undo) {
                Ok(update) => {
                    self.undo_operation = Some(undo);
                    update
                }
                Err(feedback) => Update {
                    feedback: Some(feedback),
                    ..Default::default()
                },
            },
            UndoPhase::UndoPending { .. } => {
                self.undo_operation = Some(undo);
                Update {
                    feedback: Some("Undo is already in progress"),
                    ..Default::default()
                }
            }
        }
    }

    fn dispatch_undo(&mut self, undo: &mut UndoOperation) -> Result<Update, &'static str> {
        let mutation = self.inverse_mutation(undo)?;
        let account_email = self
            .account
            .as_ref()
            .map(|account| account.email.clone())
            .ok_or("Connect Gmail before undoing this action")?;
        let request_id = MutationRequestId(self.next_mutation_request);
        self.next_mutation_request = self.next_mutation_request.wrapping_add(1).max(1);
        self.restore_pending_value(undo.previous.clone());
        self.normalize();
        self.bump_list();
        self.bump_reader();
        undo.phase = UndoPhase::UndoPending { request_id };
        Ok(Update {
            effects: vec![Effect::SendWorker(WorkerCommand::MutateMessageInCatalog {
                request_id,
                generation: self.cache_generation,
                account_email,
                message_id: undo.message_id.clone(),
                locator: undo.locator.clone(),
                catalog: undo.catalog.clone(),
                mutation,
            })],
            ..Default::default()
        })
    }

    fn inverse_mutation(&self, undo: &UndoOperation) -> Result<MessageMutation, &'static str> {
        let inbox_mailbox = undo
            .catalog
            .find(&FolderId::Inbox)
            .map(|folder| folder.mailbox.clone())
            .ok_or("Gmail did not expose an Inbox mailbox to restore this message")?;
        match &undo.forward {
            MessageMutation::Archive => Ok(MessageMutation::RestoreArchive { inbox_mailbox }),
            MessageMutation::MoveToTrash { mailbox } => Ok(MessageMutation::RestoreFromTrash {
                inbox_mailbox,
                trash_mailbox: mailbox.clone(),
                restore_inbox: undo_was_in_inbox(&undo.previous),
                labels: undo_labels(&undo.previous),
            }),
            _ => Err("This message action cannot be undone"),
        }
    }

    fn restore_pending_value(&mut self, previous: PendingValue) {
        match previous {
            PendingValue::Removed(rows) => self.restore_removed_rows(rows),
            PendingValue::Bool(_) | PendingValue::Label(_) => {}
        }
    }

    // Gmail search/All Mail rows do not currently carry system-label membership.  They are
    // therefore actionable unless the row is known to be Trash; the transport remains the
    fn can_archive_message(&self, message: &MessageSummary) -> bool {
        self.can_change_selected_message() && message.in_inbox && !message.in_trash
    }

    fn can_trash_message(&self, message: &MessageSummary) -> bool {
        self.can_change_selected_message() && !message.in_trash
    }

    fn can_archive_selected(&self) -> bool {
        if self.selected_account_message.is_some() {
            return self
                .selected_account_summary()
                .is_some_and(|message| message.message.in_inbox && !message.message.in_trash);
        }
        self.selected_message()
            .is_some_and(|message| self.can_archive_message(message))
    }

    fn can_trash_selected(&self) -> bool {
        if self.selected_account_message.is_some() {
            return self
                .selected_account_summary()
                .is_some_and(|message| !message.message.in_trash);
        }
        self.selected_message()
            .is_some_and(|message| self.can_trash_message(message))
    }

    fn apply_to_visible_representations(&mut self, id: &MessageId, mutation: &MessageMutation) {
        if let Some(mailbox) = self.mailbox.as_mut() {
            for message in &mut mailbox.messages {
                if &message.id == id {
                    mutation.apply(message);
                }
            }
        }
        if let ServerSearchState::Loaded { messages, .. } = &mut self.server_search {
            for message in messages {
                if &message.id == id {
                    mutation.apply(message);
                }
            }
        }
    }

    /// Removes only projections that Gmail's operation makes ineligible. Archive leaves All
    /// Mail/label/search rows alone; trash removes every non-Trash projection. The removed
    /// rows are retained exactly so a definite failure can restore their original order.
    fn remove_from_views(&mut self, id: &MessageId, archive: bool) -> PendingValue {
        let should_remove = |message: &MessageSummary| {
            if archive {
                message.folder_id == FolderId::Inbox
            } else {
                message.folder_id != FolderId::Trash
            }
        };
        let mut removed = Vec::new();
        if let Some(mailbox) = self.mailbox.as_mut() {
            let mut index = 0;
            while index < mailbox.messages.len() {
                if mailbox.messages[index].id == *id && should_remove(&mailbox.messages[index]) {
                    removed.push(RemovedMessage {
                        view: RemovedMessageView::Folder,
                        message: mailbox.messages.remove(index),
                        index,
                    });
                } else {
                    index += 1;
                }
            }
            mailbox.metadata.loaded_count = mailbox.messages.len();
        }
        if let ServerSearchState::Loaded { messages, .. } = &mut self.server_search {
            let mut index = 0;
            while index < messages.len() {
                if messages[index].id == *id && should_remove(&messages[index]) {
                    removed.push(RemovedMessage {
                        view: RemovedMessageView::Search,
                        message: messages.remove(index),
                        index,
                    });
                } else {
                    index += 1;
                }
            }
        }
        PendingValue::Removed(removed)
    }

    fn restore_removed_rows(&mut self, rows: Vec<RemovedMessage>) {
        for row in rows {
            match row.view {
                RemovedMessageView::Folder => {
                    if let Some(mailbox) = self.mailbox.as_mut()
                        && !mailbox
                            .messages
                            .iter()
                            .any(|message| message.id == row.message.id)
                    {
                        let index = row.index.min(mailbox.messages.len());
                        mailbox.messages.insert(index, row.message);
                        mailbox.metadata.loaded_count = mailbox.messages.len();
                    }
                }
                RemovedMessageView::Search => {
                    if let ServerSearchState::Loaded { messages, .. } = &mut self.server_search
                        && !messages.iter().any(|message| message.id == row.message.id)
                    {
                        messages.insert(row.index.min(messages.len()), row.message);
                    }
                }
            }
        }
    }

    fn apply_reconciled_to_visible_representations(
        &mut self,
        id: &MessageId,
        state: &ReconciledMessageState,
    ) {
        let update = |message: &mut MessageSummary| {
            if &message.id == id {
                message.unread = state.unread;
                message.starred = state.starred;
                message.in_inbox = state.in_inbox;
                message.in_trash = state.in_trash;
                message.labels = state.labels.clone();
            }
        };
        if let Some(mailbox) = self.mailbox.as_mut() {
            mailbox.messages.iter_mut().for_each(&update);
            mailbox.messages.retain(Self::belongs_in_projection);
            mailbox.metadata.loaded_count = mailbox.messages.len();
        }
        if let ServerSearchState::Loaded { messages, .. } = &mut self.server_search {
            messages.iter_mut().for_each(update);
            messages.retain(Self::belongs_in_projection);
        }
    }

    fn restore_reconciled_rows(
        &mut self,
        rows: Vec<RemovedMessage>,
        state: &ReconciledMessageState,
    ) {
        let rows = rows
            .into_iter()
            .filter_map(|mut row| {
                row.message.unread = state.unread;
                row.message.starred = state.starred;
                row.message.in_inbox = state.in_inbox;
                row.message.in_trash = state.in_trash;
                row.message.labels = state.labels.clone();
                Self::belongs_in_projection(&row.message).then_some(row)
            })
            .collect();
        self.restore_removed_rows(rows);
    }

    fn belongs_in_projection(message: &MessageSummary) -> bool {
        match &message.folder_id {
            FolderId::Inbox => message.in_inbox,
            FolderId::Trash => message.in_trash,
            FolderId::AllMail => !message.in_trash,
            FolderId::Starred => message.starred && !message.in_trash,
            FolderId::Sent => !message.in_trash,
            FolderId::Label(label) => message.labels.contains(label) && !message.in_trash,
        }
    }

    fn restore_boolean_to_visible_representations(
        &mut self,
        id: &MessageId,
        mutation: &MessageMutation,
        value: bool,
    ) {
        let restore = |message: &mut MessageSummary| {
            if &message.id == id {
                match mutation {
                    MessageMutation::SetRead(_) => message.unread = value,
                    MessageMutation::SetStarred(_) => message.starred = value,
                    _ => {}
                }
            }
        };
        if let Some(mailbox) = self.mailbox.as_mut() {
            mailbox.messages.iter_mut().for_each(&restore);
        }
        if let ServerSearchState::Loaded { messages, .. } = &mut self.server_search {
            messages.iter_mut().for_each(restore);
        }
    }

    fn restore_label_to_visible_representations(
        &mut self,
        id: &MessageId,
        mutation: &MessageMutation,
        applied: bool,
    ) {
        let MessageMutation::SetLabel { mailbox, .. } = mutation else {
            return;
        };
        let restore = |message: &mut MessageSummary| {
            if &message.id == id {
                message.labels.retain(|label| label != mailbox);
                if applied {
                    message.labels.push(mailbox.clone());
                    message.labels.sort();
                    message.labels.dedup();
                }
            }
        };
        if let Some(mailbox_view) = self.mailbox.as_mut() {
            mailbox_view.messages.iter_mut().for_each(&restore);
        }
        if let ServerSearchState::Loaded { messages, .. } = &mut self.server_search {
            messages.iter_mut().for_each(restore);
        }
    }

    fn remove_message_from_all_visible_representations(&mut self, id: &MessageId) {
        if let Some(mailbox) = self.mailbox.as_mut() {
            mailbox.messages.retain(|message| &message.id != id);
            mailbox.metadata.loaded_count = mailbox.messages.len();
        }
        if let ServerSearchState::Loaded { messages, .. } = &mut self.server_search {
            messages.retain(|message| &message.id != id);
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
        // Persist the complete snapshot. Saving a retention field on its own
        // would race an appearance change and could erase it on disk.
        let mut effects = self.save_preferences().effects;
        if increased && self.account.is_some() {
            let refresh = self.load_folder(self.selected_folder_id.clone());
            effects.extend(refresh.effects);
        }
        Update {
            feedback: Some("Local mail limit updated"),
            effects,
        }
    }

    fn preferences_snapshot(&self) -> Preferences {
        Preferences::new(self.cache_limit, self.appearance)
    }

    fn save_preferences(&mut self) -> Update {
        let request_id = PreferencesRequestId(self.next_preferences_request);
        self.next_preferences_request = self.next_preferences_request.wrapping_add(1).max(1);
        self.pending_preferences_request = Some(request_id);
        self.preferences_save_state = PreferencesSaveState::Saving;
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::SavePreferences {
                request_id,
                preferences: self.preferences_snapshot(),
            })],
            ..Default::default()
        }
    }

    fn set_appearance(&mut self, appearance: AppearancePreference) -> Update {
        if self.appearance == appearance {
            return if self.preferences_save_state == PreferencesSaveState::Failed {
                self.retry_preferences_save()
            } else {
                Update::default()
            };
        }
        // The UI reads this value directly from the snapshot, so theme changes
        // apply immediately without waiting for filesystem I/O.
        self.appearance = appearance;
        let mut update = self.save_preferences();
        update.feedback = Some("Appearance updated");
        update
    }

    fn retry_preferences_save(&mut self) -> Update {
        if self.preferences_save_state != PreferencesSaveState::Failed {
            return Update::default();
        }
        self.save_preferences()
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
            ReaderState::AccountLoading { .. }
            | ReaderState::AccountLoaded { .. }
            | ReaderState::AccountFailed { .. } => None,
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
        if matches!(self.reader, ReaderState::AccountFailed { .. }) {
            self.open_selected_account()
        } else if matches!(self.reader, ReaderState::Failed { .. }) {
            self.open_selected()
        } else {
            Update::default()
        }
    }
    fn start_attachment(
        &mut self,
        requested: crate::model::Attachment,
        destination: AttachmentDestination,
    ) -> Update {
        if self.selected_account_message.is_some() {
            // The legacy attachment command is keyed by a bare MessageId.
            // Do not let an account-reader attachment fall through to that
            // singleton lane until its transport receives the same scoped
            // contract as body loading.
            return Update {
                feedback: Some("Attachment downloads are not available for this account yet"),
                ..Default::default()
            };
        }
        if self.attachment_jobs.len() >= crate::worker::MAX_CONTENT_JOBS {
            return Update {
                feedback: Some("Wait for an attachment download to finish"),
                ..Default::default()
            };
        }
        let Some(account_email) = self.account.as_ref().map(|value| value.email.clone()) else {
            return Update::default();
        };
        let Some(message) = self.selected_message().cloned() else {
            return Update::default();
        };
        let attachment = match &self.reader {
            ReaderState::Loaded { id, body } if id == &message.id => body
                .attachments
                .iter()
                .find(|attachment| {
                    attachment.part.path == requested.part.path
                        && attachment.part == requested.part
                        && attachment.name == requested.name
                })
                .cloned(),
            _ => None,
        };
        let Some(attachment) = attachment.filter(crate::model::Attachment::is_downloadable) else {
            return Update {
                feedback: Some("This attachment needs the message to be refreshed"),
                ..Default::default()
            };
        };
        if self
            .attachment_jobs
            .values()
            .any(|job| job.message_id == message.id && job.part_path == attachment.part.path)
        {
            return Update {
                feedback: Some("That attachment is already downloading"),
                ..Default::default()
            };
        }
        let job_id = AttachmentJobId(self.next_attachment_job);
        self.next_attachment_job = self.next_attachment_job.wrapping_add(1).max(1);
        let open = matches!(destination, AttachmentDestination::Open);
        self.attachment_jobs.insert(
            job_id,
            AttachmentDownload {
                job_id,
                message_id: message.id.clone(),
                part_path: attachment.part.path.clone(),
                name: attachment.name.clone(),
                transferred: 0,
                total: attachment.part.encoded_octets,
                open,
            },
        );
        self.bump_reader();
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::DownloadAttachment {
                job_id,
                generation: self.cache_generation,
                account_email,
                message_id: message.id,
                locator: message.locator,
                attachment: Box::new(attachment),
                destination,
            })],
            ..Default::default()
        }
    }

    fn cancel_attachment(&mut self, job_id: AttachmentJobId) -> Update {
        if self.attachment_jobs.remove(&job_id).is_none() {
            return Update::default();
        }
        self.bump_reader();
        Update {
            effects: vec![Effect::SendWorker(WorkerCommand::CancelAttachment {
                job_id,
                generation: self.cache_generation,
            })],
            feedback: Some("Attachment download cancelled"),
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
                        None,
                    )),
                    ReaderState::AccountLoaded { id, body } => Some((
                        id.message_id.clone(),
                        body.reply_context.clone(),
                        body.html
                            .clone()
                            .unwrap_or_else(|| format!("<div>{}</div>", escape_html(&body.text))),
                        self.account_mailboxes
                            .get(&id.account_id)
                            .map(|account| account.identity.email.clone()),
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
        let source_id = source.as_ref().map(|(id, _, _, _)| id.clone());
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
                        .is_some_and(|(id, _, _, _)| compose_matches(&draft.kind, start, id))
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
            // The selected row may be an ephemeral Gmail server-search result, not a member
            // of the currently loaded folder. Resolve through the same accessor as selection.
            let subject = self
                .selected_account_summary()
                .filter(|message| &message.id.message_id == id)
                .map(|message| message.message.subject)
                .or_else(|| {
                    self.selected_message()
                        .filter(|message| &message.id == id)
                        .map(|message| message.subject.clone())
                });
            let Some(subject) = subject else {
                return Update {
                    feedback: Some("The source message is no longer in this mailbox"),
                    ..Default::default()
                };
            };
            Some(subject)
        } else {
            None
        };
        let account_email = source
            .as_ref()
            .and_then(|(_, _, _, account)| account.clone())
            .or_else(|| self.account.as_ref().map(|a| a.email.clone()));
        let Some(account_email) = account_email else {
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
            (ComposeStart::Reply, Some((id, context, original_html, _))) => composer::new_reply(
                draft_id,
                &account_email,
                id.clone(),
                source_subject.as_deref().unwrap_or(""),
                context,
                original_html,
                signature,
            ),
            (ComposeStart::ReplyAll, Some((id, context, original_html, _))) => {
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
            (ComposeStart::Forward, Some((id, context, original_html, _))) => {
                composer::new_forward(
                    draft_id,
                    &account_email,
                    id.clone(),
                    source_subject.as_deref().unwrap_or(""),
                    context,
                    original_html,
                    signature,
                )
            }
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
                source_message_id: source.as_ref().map(|(id, _, _, _)| id.clone()),
                recipient,
                subject: compose.subject.clone(),
                body: String::new(),
                context: source.map(|(_, context, _, _)| context),
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
        let draft_account_email = draft.compose.account_email.clone();
        self.composer = ComposerState::Sending {
            draft,
            request_id,
            generation,
        };
        let scoped_account = self
            .account_mailboxes
            .iter()
            .find_map(|(account_id, account)| {
                account
                    .identity
                    .email
                    .eq_ignore_ascii_case(&draft_account_email)
                    .then(|| (account_id.clone(), account.identity.email.clone()))
            });
        let command = match scoped_account {
            Some((account_id, account_email)) => WorkerCommand::SendAccountMessage {
                request_id,
                generation,
                account_id,
                account_email,
                submission: Box::new(submission),
            },
            None => WorkerCommand::SendMessage {
                request_id,
                generation,
                submission: Box::new(submission),
            },
        };
        Update {
            effects: vec![Effect::SendWorker(command)],
            ..Default::default()
        }
    }

    fn finish_account_send(
        &mut self,
        request_id: SendRequestId,
        generation: u64,
        account_id: &AccountId,
        failure: Option<SendFailure>,
    ) -> Update {
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
        let belongs_to_account = self
            .account_mailboxes
            .get(account_id)
            .is_some_and(|account| {
                account
                    .identity
                    .email
                    .eq_ignore_ascii_case(&draft.compose.account_email)
            });
        if !belongs_to_account {
            return Update::default();
        }
        if let Some(failure) = failure {
            self.composer = ComposerState::Failed { draft, failure };
            return Update {
                feedback: Some(send_failure_feedback(failure)),
                ..Default::default()
            };
        }
        self.composer = ComposerState::Closed;
        Update {
            feedback: Some("Message sent"),
            ..Default::default()
        }
    }
    fn clear_body_cache(&mut self) -> Update {
        self.cache_generation = self.cache_generation.wrapping_add(1);
        self.attachment_jobs.clear();
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

fn attachment_failure_feedback(failure: AttachmentFailure) -> &'static str {
    match failure {
        AttachmentFailure::Offline => "Could not download the attachment while offline",
        AttachmentFailure::AuthorizationRequired => {
            "Refresh Gmail authorization to download attachments"
        }
        AttachmentFailure::TooLarge => "Attachments are limited to 100 MB",
        AttachmentFailure::UnsupportedEncoding => {
            "This attachment uses an unsupported mail encoding"
        }
        AttachmentFailure::Filesystem => "Could not save the attachment",
        AttachmentFailure::Protocol => "Gmail returned an invalid attachment",
        AttachmentFailure::TimedOut => "The attachment download timed out",
        AttachmentFailure::Busy => "Two attachment downloads are already running",
    }
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
