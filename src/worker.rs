use crate::{
    cache,
    composer::{ComposeDraft, DraftAttachment},
    config, drafts, gmail, message,
    model::{
        AccountIdentity, CacheUsage, FolderCatalog, FolderDescriptor, FolderId, FolderKind,
        MailProvider, MailboxSnapshot, MessageBody, MessageId, MessageLocator, MessageMutation,
        ReconciledMessageState, SyncMetadata,
    },
    oauth::{self, AuthorizationUrl},
    secrets::{self, RefreshToken},
    smtp,
};
use futures_util::FutureExt;

const KEYRING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
pub const MAX_CONTENT_JOBS: usize = 2;
use std::{
    collections::HashMap,
    fmt,
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle as ThreadJoinHandle},
    time::SystemTime,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
    task::JoinHandle,
    time::{Instant, timeout, timeout_at},
};
use zeroize::Zeroizing;

const MAX_CALLBACK_HEADERS: usize = 8192;
const CALLBACK_SUCCESS_BODY: &[u8] =
    b"<!doctype html><title>Whitford</title>Authorization received. You may close this tab.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationId(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BodyRequestId(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheOperationId(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SendRequestId(pub u64);
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DraftOperationId(pub u64);
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MutationRequestId(pub u64);
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FolderRequestId(pub u64);
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SearchRequestId(pub u64);
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AttachmentJobId(pub u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttachmentDestination {
    Open,
    SaveAs(std::path::PathBuf),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachmentFailure {
    Offline,
    AuthorizationRequired,
    TooLarge,
    UnsupportedEncoding,
    Filesystem,
    Protocol,
    TimedOut,
    Busy,
}

fn folder_debug_kind(id: &FolderId) -> &'static str {
    match id {
        FolderId::Inbox => "inbox",
        FolderId::Sent => "sent",
        FolderId::AllMail => "all-mail",
        FolderId::Trash => "trash",
        FolderId::Starred => "starred",
        FolderId::Label(_) => "label",
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncKind {
    Restore,
    Connect,
    Refresh,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerPhase {
    LoadingConfiguration,
    WaitingForBrowser,
    ExchangingCode,
    OpeningKeyring,
    RefreshingToken,
    VerifyingIdentity,
    ConnectingImap,
    FetchingInbox,
    Disconnecting,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureKind {
    ConfigurationDirectoryUnavailable,
    ConfigurationMissing,
    ConfigurationUnreadable,
    ConfigurationTooLarge,
    ConfigurationInvalid,
    ConfigurationWrongProject,
    BrowserLaunchFailed,
    AuthorizationDenied,
    AuthorizationTimedOut,
    AuthorizationInvalid,
    Network,
    ProviderUnavailable,
    RateLimited,
    AuthorizationExpired,
    IdentityInvalid,
    KeyringUnavailable,
    CredentialSaveFailed,
    DisconnectFailed,
    TlsFailed,
    ImapAuthenticationFailed,
    InboxUnavailable,
    ImapProtocol,
    SyncTimedOut,
    WorkerUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyFailure {
    Offline,
    TimedOut,
    AuthorizationRequired,
    MailboxChanged,
    Missing,
    Protocol,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendFailure {
    Empty,
    TooLarge,
    InvalidRecipient,
    AuthorizationRequired,
    Rejected,
    DeliveryUncertain,
    Protocol,
}
/// A generic composer submission. It intentionally carries only private staged
/// file references; attachment bytes are loaded on the blocking worker lane.
pub struct ComposeSubmission {
    pub draft: ComposeDraft,
}
impl FailureKind {
    pub fn is_configuration(self) -> bool {
        matches!(
            self,
            Self::ConfigurationDirectoryUnavailable
                | Self::ConfigurationMissing
                | Self::ConfigurationUnreadable
                | Self::ConfigurationTooLarge
                | Self::ConfigurationInvalid
                | Self::ConfigurationWrongProject
        )
    }
}
#[derive(Clone, Eq, PartialEq)]
pub struct ServiceFailure {
    pub kind: FailureKind,
    pub retryable: bool,
    pub preserve_mail: bool,
    pub cleanup_failed: bool,
    pub config_path: Option<String>,
}
impl fmt::Debug for ServiceFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceFailure")
            .field("kind", &self.kind)
            .field("retryable", &self.retryable)
            .field("preserve_mail", &self.preserve_mail)
            .field("cleanup_failed", &self.cleanup_failed)
            .field(
                "config_path",
                &self.config_path.as_ref().map(|_| "[REDACTED PATH]"),
            )
            .finish()
    }
}

pub enum WorkerCommand {
    Restore {
        id: OperationId,
    },
    Connect {
        id: OperationId,
    },
    Refresh {
        id: OperationId,
    },
    Disconnect {
        id: OperationId,
        generation: u64,
        draft_generation: u64,
        account_email: String,
    },
    Cancel {
        id: OperationId,
    },
    SetCacheLimit {
        limit: usize,
    },
    FetchFolder {
        request_id: FolderRequestId,
        generation: u64,
        account_email: String,
        folder: FolderDescriptor,
        catalog: FolderCatalog,
    },
    SearchGmail {
        request_id: SearchRequestId,
        generation: u64,
        account_email: String,
        folder: FolderDescriptor,
        query: String,
    },
    CancelSearch {
        request_id: SearchRequestId,
        generation: u64,
    },
    FetchBody {
        request_id: BodyRequestId,
        generation: u64,
        account_email: String,
        message_id: MessageId,
        locator: MessageLocator,
    },
    DownloadAttachment {
        job_id: AttachmentJobId,
        generation: u64,
        account_email: String,
        message_id: MessageId,
        locator: MessageLocator,
        attachment: Box<crate::model::Attachment>,
        destination: AttachmentDestination,
    },
    CancelAttachment {
        job_id: AttachmentJobId,
        generation: u64,
    },
    ClearBodyCache {
        operation_id: CacheOperationId,
        generation: u64,
    },
    SendMessage {
        request_id: SendRequestId,
        generation: u64,
        submission: Box<ComposeSubmission>,
    },
    LoadDrafts {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
    },
    SaveDraft {
        operation_id: DraftOperationId,
        generation: u64,
        draft: Box<ComposeDraft>,
    },
    DeleteDraft {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
        draft_id: String,
    },
    StageAttachment {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
        draft_id: String,
        source: std::path::PathBuf,
        display_name: String,
        media_type: String,
        inline: bool,
    },
    RemoveStaged {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
        draft_id: String,
        staged_file: String,
    },
    LoadSignature {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
    },
    SaveSignature {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
        preference: drafts::SignaturePreference,
    },
    MutateMessage {
        request_id: MutationRequestId,
        generation: u64,
        account_email: String,
        message_id: MessageId,
        locator: MessageLocator,
        mutation: MessageMutation,
    },
    /// Folder-independent mutation contract.  The catalog supplies the
    /// account's discovered special-use mailbox paths; callers must not infer
    /// localized Gmail paths from a FolderId.
    MutateMessageInCatalog {
        request_id: MutationRequestId,
        generation: u64,
        account_email: String,
        message_id: MessageId,
        locator: MessageLocator,
        catalog: FolderCatalog,
        mutation: MessageMutation,
    },
    Shutdown,
}
impl fmt::Debug for WorkerCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Restore { id } => f.debug_tuple("Restore").field(id).finish(),
            Self::Connect { id } => f.debug_tuple("Connect").field(id).finish(),
            Self::Refresh { id } => f.debug_tuple("Refresh").field(id).finish(),
            Self::Disconnect { id, generation, .. } => f
                .debug_struct("Disconnect")
                .field("id", id)
                .field("generation", generation)
                .finish(),
            Self::Cancel { id } => f.debug_tuple("Cancel").field(id).finish(),
            Self::SetCacheLimit { limit } => f
                .debug_struct("SetCacheLimit")
                .field("limit", limit)
                .finish(),
            Self::FetchFolder {
                request_id,
                generation,
                folder,
                ..
            } => f
                .debug_struct("FetchFolder")
                .field("request_id", request_id)
                .field("generation", generation)
                .field("folder_kind", &folder_debug_kind(&folder.id))
                .finish(),
            Self::SearchGmail {
                request_id,
                generation,
                query,
                ..
            } => f
                .debug_struct("SearchGmail")
                .field("request_id", request_id)
                .field("generation", generation)
                .field("query_bytes", &query.len())
                .finish(),
            Self::CancelSearch {
                request_id,
                generation,
            } => f
                .debug_struct("CancelSearch")
                .field("request_id", request_id)
                .field("generation", generation)
                .finish(),
            Self::FetchBody {
                request_id,
                generation,
                ..
            } => f
                .debug_struct("FetchBody")
                .field("request_id", request_id)
                .field("generation", generation)
                .finish(),
            Self::DownloadAttachment {
                job_id,
                generation,
                destination,
                ..
            } => f
                .debug_struct("DownloadAttachment")
                .field("job_id", job_id)
                .field("generation", generation)
                .field(
                    "destination",
                    &match destination {
                        AttachmentDestination::Open => "open",
                        AttachmentDestination::SaveAs(_) => "save-as",
                    },
                )
                .finish(),
            Self::CancelAttachment { job_id, generation } => f
                .debug_struct("CancelAttachment")
                .field("job_id", job_id)
                .field("generation", generation)
                .finish(),
            Self::ClearBodyCache {
                operation_id,
                generation,
            } => f
                .debug_struct("ClearBodyCache")
                .field("operation_id", operation_id)
                .field("generation", generation)
                .finish(),
            Self::SendMessage {
                request_id,
                generation,
                ..
            } => f
                .debug_struct("SendMessage")
                .field("request_id", request_id)
                .field("generation", generation)
                .field("content", &"[REDACTED]")
                .finish(),
            Self::LoadDrafts { operation_id, .. } => f
                .debug_struct("LoadDrafts")
                .field("operation_id", operation_id)
                .finish(),
            Self::SaveDraft { operation_id, .. } => f
                .debug_struct("SaveDraft")
                .field("operation_id", operation_id)
                .finish(),
            Self::DeleteDraft { operation_id, .. } => f
                .debug_struct("DeleteDraft")
                .field("operation_id", operation_id)
                .finish(),
            Self::StageAttachment { operation_id, .. } => f
                .debug_struct("StageAttachment")
                .field("operation_id", operation_id)
                .finish(),
            Self::RemoveStaged { operation_id, .. } => {
                f.debug_tuple("RemoveStaged").field(operation_id).finish()
            }
            Self::LoadSignature { operation_id, .. } => {
                f.debug_tuple("LoadSignature").field(operation_id).finish()
            }
            Self::SaveSignature { operation_id, .. } => {
                f.debug_tuple("SaveSignature").field(operation_id).finish()
            }
            Self::MutateMessage {
                request_id,
                generation,
                mutation,
                ..
            } => f
                .debug_struct("MutateMessage")
                .field("request_id", request_id)
                .field("generation", generation)
                .field(
                    "kind",
                    &match mutation {
                        MessageMutation::SetRead(_) => "read",
                        MessageMutation::SetStarred(_) => "starred",
                        MessageMutation::Archive => "archive",
                        MessageMutation::MoveToTrash { .. } => "trash",
                        MessageMutation::RestoreArchive { .. } => "restore-archive",
                        MessageMutation::RestoreFromTrash { .. } => "restore-trash",
                        MessageMutation::SetLabel { .. } => "label",
                    },
                )
                .finish(),
            Self::MutateMessageInCatalog {
                request_id,
                generation,
                mutation,
                ..
            } => f
                .debug_struct("MutateMessageInCatalog")
                .field("request_id", request_id)
                .field("generation", generation)
                .field(
                    "kind",
                    &match mutation {
                        MessageMutation::SetRead(_) => "read",
                        MessageMutation::SetStarred(_) => "starred",
                        MessageMutation::Archive => "archive",
                        MessageMutation::MoveToTrash { .. } => "trash",
                        MessageMutation::RestoreArchive { .. } => "restore-archive",
                        MessageMutation::RestoreFromTrash { .. } => "restore-trash",
                        MessageMutation::SetLabel { .. } => "label",
                    },
                )
                .finish(),
            Self::Shutdown => f.write_str("Shutdown"),
        }
    }
}

pub enum WorkerEvent {
    Phase {
        id: OperationId,
        phase: WorkerPhase,
    },
    AuthorizationRequired {
        id: OperationId,
        url: AuthorizationUrl,
        deadline: SystemTime,
    },
    IdentityVerified {
        id: OperationId,
        account: AccountIdentity,
    },
    AccountPersisted {
        id: OperationId,
        account: AccountIdentity,
    },
    CacheLoaded {
        id: OperationId,
        account: AccountIdentity,
        snapshot: MailboxSnapshot,
    },
    NoStoredAccount {
        id: OperationId,
    },
    SyncComplete {
        id: OperationId,
        account: AccountIdentity,
        snapshot: MailboxSnapshot,
    },
    FolderCacheLoaded {
        request_id: FolderRequestId,
        generation: u64,
        folder_id: FolderId,
        snapshot: MailboxSnapshot,
    },
    FolderLoaded {
        request_id: FolderRequestId,
        generation: u64,
        folder_id: FolderId,
        snapshot: MailboxSnapshot,
    },
    FolderFailed {
        request_id: FolderRequestId,
        generation: u64,
        folder_id: FolderId,
        failure: BodyFailure,
    },
    SearchLoaded {
        request_id: SearchRequestId,
        generation: u64,
        messages: Vec<crate::model::MessageSummary>,
        truncated: bool,
        skipped_count: usize,
    },
    SearchFailed {
        request_id: SearchRequestId,
        generation: u64,
        failure: BodyFailure,
    },
    SearchCancelled {
        request_id: SearchRequestId,
        generation: u64,
    },
    Disconnected {
        id: OperationId,
    },
    Cancelled {
        id: OperationId,
    },
    Failed {
        id: OperationId,
        failure: ServiceFailure,
    },
    BodyLoaded {
        request_id: BodyRequestId,
        generation: u64,
        message_id: MessageId,
        body: Arc<MessageBody>,
        usage: CacheUsage,
        saved: bool,
    },
    BodyFailed {
        request_id: BodyRequestId,
        generation: u64,
        message_id: MessageId,
        failure: BodyFailure,
    },
    AttachmentProgress {
        job_id: AttachmentJobId,
        generation: u64,
        transferred: u64,
        total: u64,
    },
    AttachmentCompleted {
        job_id: AttachmentJobId,
        generation: u64,
        path: std::path::PathBuf,
        open: bool,
    },
    AttachmentFailed {
        job_id: AttachmentJobId,
        generation: u64,
        failure: AttachmentFailure,
    },
    AttachmentCancelled {
        job_id: AttachmentJobId,
        generation: u64,
    },
    CacheCleared {
        operation_id: CacheOperationId,
        generation: u64,
        reclaimed_bytes: u64,
        usage: CacheUsage,
    },
    CacheClearFailed {
        operation_id: CacheOperationId,
        generation: u64,
    },
    CacheUsageChanged {
        usage: CacheUsage,
    },
    MessageSent {
        request_id: SendRequestId,
        generation: u64,
    },
    MessageSendFailed {
        request_id: SendRequestId,
        generation: u64,
        failure: SendFailure,
    },
    DraftsLoaded {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
        drafts: Vec<ComposeDraft>,
    },
    DraftSaved {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
        draft_id: String,
        revision: u64,
    },
    DraftDeleted {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
        draft_id: String,
    },
    AttachmentStaged {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
        draft_id: String,
        attachment: DraftAttachment,
        inline: bool,
    },
    StagedRemoved {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
    },
    SignatureLoaded {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
        preference: drafts::SignaturePreference,
    },
    SignatureSaved {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
    },
    DraftOperationFailed {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
    },
    MutationConfirmed {
        request_id: MutationRequestId,
        generation: u64,
        message_id: MessageId,
    },
    MutationReconciled {
        request_id: MutationRequestId,
        generation: u64,
        message_id: MessageId,
        state: Option<ReconciledMessageState>,
    },
    MutationFailed {
        request_id: MutationRequestId,
        generation: u64,
        message_id: MessageId,
        uncertain: bool,
    },
}
impl fmt::Debug for WorkerEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Phase { id, phase } => f
                .debug_struct("Phase")
                .field("id", id)
                .field("phase", phase)
                .finish(),
            Self::AuthorizationRequired { id, .. } => f
                .debug_struct("AuthorizationRequired")
                .field("id", id)
                .finish(),
            Self::IdentityVerified { id, .. } => {
                f.debug_struct("IdentityVerified").field("id", id).finish()
            }
            Self::AccountPersisted { id, .. } => {
                f.debug_struct("AccountPersisted").field("id", id).finish()
            }
            Self::CacheLoaded { id, snapshot, .. } => f
                .debug_struct("CacheLoaded")
                .field("id", id)
                .field("loaded", &snapshot.metadata.loaded_count)
                .finish(),
            Self::NoStoredAccount { id } => {
                f.debug_struct("NoStoredAccount").field("id", id).finish()
            }
            Self::SyncComplete { id, snapshot, .. } => f
                .debug_struct("SyncComplete")
                .field("id", id)
                .field("loaded", &snapshot.metadata.loaded_count)
                .finish(),
            Self::FolderCacheLoaded {
                request_id,
                folder_id,
                snapshot,
                ..
            }
            | Self::FolderLoaded {
                request_id,
                folder_id,
                snapshot,
                ..
            } => f
                .debug_struct("FolderLoaded")
                .field("request_id", request_id)
                .field("folder_kind", &folder_debug_kind(folder_id))
                .field("loaded", &snapshot.metadata.loaded_count)
                .finish(),
            Self::FolderFailed {
                request_id,
                folder_id,
                failure,
                ..
            } => f
                .debug_struct("FolderFailed")
                .field("request_id", request_id)
                .field("folder_kind", &folder_debug_kind(folder_id))
                .field("failure", failure)
                .finish(),
            Self::SearchLoaded {
                request_id,
                generation,
                messages,
                truncated,
                skipped_count,
            } => f
                .debug_struct("SearchLoaded")
                .field("request_id", request_id)
                .field("generation", generation)
                .field("loaded", &messages.len())
                .field("truncated", truncated)
                .field("skipped_count", skipped_count)
                .finish(),
            Self::SearchFailed {
                request_id,
                generation,
                failure,
            } => f
                .debug_struct("SearchFailed")
                .field("request_id", request_id)
                .field("generation", generation)
                .field("failure", failure)
                .finish(),
            Self::SearchCancelled {
                request_id,
                generation,
            } => f
                .debug_struct("SearchCancelled")
                .field("request_id", request_id)
                .field("generation", generation)
                .finish(),
            Self::Disconnected { id } => f.debug_struct("Disconnected").field("id", id).finish(),
            Self::Cancelled { id } => f.debug_struct("Cancelled").field("id", id).finish(),
            Self::Failed { id, failure } => f
                .debug_struct("Failed")
                .field("id", id)
                .field("failure", failure)
                .finish(),
            Self::BodyLoaded {
                request_id,
                generation,
                saved,
                ..
            } => f
                .debug_struct("BodyLoaded")
                .field("request_id", request_id)
                .field("generation", generation)
                .field("saved", saved)
                .finish(),
            Self::BodyFailed {
                request_id,
                generation,
                failure,
                ..
            } => f
                .debug_struct("BodyFailed")
                .field("request_id", request_id)
                .field("generation", generation)
                .field("failure", failure)
                .finish(),
            Self::AttachmentProgress {
                job_id,
                generation,
                transferred,
                total,
            } => f
                .debug_struct("AttachmentProgress")
                .field("job_id", job_id)
                .field("generation", generation)
                .field("transferred", transferred)
                .field("total", total)
                .finish(),
            Self::AttachmentCompleted {
                job_id,
                generation,
                open,
                ..
            } => f
                .debug_struct("AttachmentCompleted")
                .field("job_id", job_id)
                .field("generation", generation)
                .field("open", open)
                .finish(),
            Self::AttachmentFailed {
                job_id,
                generation,
                failure,
            } => f
                .debug_struct("AttachmentFailed")
                .field("job_id", job_id)
                .field("generation", generation)
                .field("failure", failure)
                .finish(),
            Self::AttachmentCancelled { job_id, generation } => f
                .debug_struct("AttachmentCancelled")
                .field("job_id", job_id)
                .field("generation", generation)
                .finish(),
            Self::CacheCleared {
                operation_id,
                generation,
                reclaimed_bytes,
                ..
            } => f
                .debug_struct("CacheCleared")
                .field("operation_id", operation_id)
                .field("generation", generation)
                .field("reclaimed_bytes", reclaimed_bytes)
                .finish(),
            Self::CacheClearFailed {
                operation_id,
                generation,
            } => f
                .debug_struct("CacheClearFailed")
                .field("operation_id", operation_id)
                .field("generation", generation)
                .finish(),
            Self::CacheUsageChanged { usage } => f
                .debug_struct("CacheUsageChanged")
                .field("body_bytes", &usage.body_bytes)
                .field("body_count", &usage.body_count)
                .finish(),
            Self::MessageSent {
                request_id,
                generation,
            } => f
                .debug_struct("MessageSent")
                .field("request_id", request_id)
                .field("generation", generation)
                .finish(),
            Self::MessageSendFailed {
                request_id,
                generation,
                failure,
            } => f
                .debug_struct("MessageSendFailed")
                .field("request_id", request_id)
                .field("generation", generation)
                .field("failure", failure)
                .finish(),
            Self::DraftsLoaded {
                operation_id,
                drafts,
                ..
            } => f
                .debug_struct("DraftsLoaded")
                .field("operation_id", operation_id)
                .field("count", &drafts.len())
                .finish(),
            Self::DraftSaved {
                operation_id,
                revision,
                ..
            } => f
                .debug_struct("DraftSaved")
                .field("operation_id", operation_id)
                .field("revision", revision)
                .finish(),
            Self::DraftDeleted { operation_id, .. } => f
                .debug_struct("DraftDeleted")
                .field("operation_id", operation_id)
                .finish(),
            Self::AttachmentStaged {
                operation_id,
                attachment,
                ..
            } => f
                .debug_struct("AttachmentStaged")
                .field("operation_id", operation_id)
                .field("bytes", &attachment.bytes)
                .finish(),
            Self::StagedRemoved { operation_id, .. } => {
                f.debug_tuple("StagedRemoved").field(operation_id).finish()
            }
            Self::SignatureLoaded { operation_id, .. } => f
                .debug_tuple("SignatureLoaded")
                .field(operation_id)
                .finish(),
            Self::SignatureSaved { operation_id, .. } => {
                f.debug_tuple("SignatureSaved").field(operation_id).finish()
            }
            Self::DraftOperationFailed { operation_id, .. } => f
                .debug_struct("DraftOperationFailed")
                .field("operation_id", operation_id)
                .finish(),
            Self::MutationConfirmed { request_id, .. } => f
                .debug_tuple("MutationConfirmed")
                .field(request_id)
                .finish(),
            Self::MutationReconciled {
                request_id, state, ..
            } => f
                .debug_struct("MutationReconciled")
                .field("request_id", request_id)
                .field("in_inbox", &state.is_some())
                .finish(),
            Self::MutationFailed {
                request_id,
                uncertain,
                ..
            } => f
                .debug_struct("MutationFailed")
                .field("request_id", request_id)
                .field("uncertain", uncertain)
                .finish(),
        }
    }
}

pub struct WorkerHandle {
    pub sender: mpsc::UnboundedSender<WorkerCommand>,
    thread: Option<ThreadJoinHandle<()>>,
}
impl WorkerHandle {
    pub fn start() -> (Self, mpsc::UnboundedReceiver<WorkerEvent>) {
        let (commands_tx, commands_rx) = mpsc::unbounded_channel();
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let thread = thread::Builder::new()
            .name("whitford-mail-worker".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                if let Ok(runtime) = runtime {
                    runtime.block_on(controller(commands_rx, events_tx));
                }
            })
            .ok();
        (
            Self {
                sender: commands_tx,
                thread,
            },
            events_rx,
        )
    }

    pub fn shutdown_and_join(mut self) -> thread::Result<()> {
        let _ = self.sender.send(WorkerCommand::Shutdown);
        drop(self.sender);
        self.thread.take().map_or(Ok(()), ThreadJoinHandle::join)
    }
}

async fn controller(
    mut commands: mpsc::UnboundedReceiver<WorkerCommand>,
    events: mpsc::UnboundedSender<WorkerEvent>,
) {
    struct Active {
        id: OperationId,
        task: JoinHandle<()>,
        cleanup: Arc<AtomicBool>,
    }
    let mut active: Option<Active> = None;
    let auth = Arc::new(Mutex::new(None::<RuntimeAuth>));
    let cache_io = Arc::new(Mutex::new(()));
    let metadata_lane = Arc::new(tokio::sync::Mutex::new(()));
    let cache_generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let folder_generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let search_generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let send_generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let content_slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONTENT_JOBS));
    let mut body_task: Option<JoinHandle<()>> = None;
    let mut send_task: Option<JoinHandle<()>> = None;
    let mut folder_task: Option<JoinHandle<()>> = None;
    let mut search_task: Option<(SearchRequestId, JoinHandle<()>)> = None;
    let mut attachment_tasks = HashMap::<AttachmentJobId, JoinHandle<()>>::new();
    let (draft_tx, draft_rx) = mpsc::unbounded_channel();
    let draft_events = events.clone();
    let draft_task = tokio::spawn(draft_io_actor(draft_rx, draft_events));
    let (mutation_tx, mutation_rx) = mpsc::unbounded_channel();
    let mutation_task = tokio::spawn(mutation_actor(
        mutation_rx,
        events.clone(),
        auth.clone(),
        cache_io.clone(),
        cache_generation.clone(),
        metadata_lane.clone(),
    ));
    let _ = cache_blocking(cache_io.clone(), cache::cleanup_attachment_partials).await;
    loop {
        let command = if let Some(current) = active.as_mut() {
            tokio::select! {
                biased;
                command = commands.recv() => command,
                result = &mut current.task => {
                    let id = current.id;
                    if join_failed(&result) {
                        let cleanup_failed = if current.cleanup.load(Ordering::Acquire) {
                            !matches!(timeout(KEYRING_TIMEOUT, secrets::delete()).await, Ok(Ok(())))
                        } else {
                            false
                        };
                        if cleanup_failed {
                            fail_cleanup(&events, id, true);
                        } else {
                            fail(&events, id, FailureKind::WorkerUnavailable, false, true);
                        }
                    }
                    active = None;
                    continue;
                }
            }
        } else {
            commands.recv().await
        };
        let Some(command) = command else {
            if let Some(current) = active.take() {
                let Active { id, task, cleanup } = current;
                let cleanup_failed = abort_with_cleanup(task, &cleanup, async {
                    matches!(
                        timeout(KEYRING_TIMEOUT, secrets::delete()).await,
                        Ok(Ok(()))
                    )
                })
                .await;
                if cleanup_failed {
                    fail_cleanup(&events, id, true);
                }
            }
            if let Some(task) = body_task.take() {
                task.abort();
                let _ = task.await;
            }
            if let Some(task) = folder_task.take() {
                task.abort();
                let _ = task.await;
            }
            if let Some((_, task)) = search_task.take() {
                task.abort();
                let _ = task.await;
            }
            for (_, task) in attachment_tasks.drain() {
                task.abort();
                let _ = task.await;
            }
            break;
        };
        if let WorkerCommand::SetCacheLimit { limit } = &command {
            let limit = *limit;
            let result = cache_blocking(cache_io.clone(), move || {
                let _ = cache::save_limit_and_prune(limit);
                cache::usage()
            })
            .await;
            if let Ok(Ok(usage)) = result {
                let _ = events.send(WorkerEvent::CacheUsageChanged { usage });
            }
            continue;
        }
        attachment_tasks.retain(|_, task| !task.is_finished());
        if let WorkerCommand::CancelAttachment { job_id, generation } = &command {
            if let Some(task) = attachment_tasks.remove(job_id) {
                task.abort();
                let _ = task.await;
            }
            let _ = events.send(WorkerEvent::AttachmentCancelled {
                job_id: *job_id,
                generation: *generation,
            });
            continue;
        }
        if let WorkerCommand::DownloadAttachment {
            job_id,
            generation,
            account_email,
            message_id,
            locator,
            attachment,
            destination,
        } = &command
        {
            if attachment_tasks.len() >= MAX_CONTENT_JOBS {
                let _ = events.send(WorkerEvent::AttachmentFailed {
                    job_id: *job_id,
                    generation: *generation,
                    failure: AttachmentFailure::Busy,
                });
                continue;
            }
            let request = AttachmentLoadRequest {
                job_id: *job_id,
                generation: *generation,
                account_email: account_email.clone(),
                message_id: message_id.clone(),
                locator: locator.clone(),
                attachment: (**attachment).clone(),
                destination: destination.clone(),
            };
            let tx = events.clone();
            let task_auth = auth.clone();
            let task_generation = cache_generation.clone();
            let slots = content_slots.clone();
            let failed_job_id = *job_id;
            let failed_generation = *generation;
            attachment_tasks.insert(
                *job_id,
                tokio::spawn(async move {
                    let Ok(_slot) = slots.acquire_owned().await else {
                        return;
                    };
                    let gate = task_generation.clone();
                    let failed_tx = tx.clone();
                    let result = std::panic::AssertUnwindSafe(download_attachment(
                        request,
                        tx,
                        task_auth,
                        task_generation,
                    ))
                    .catch_unwind()
                    .await;
                    if result.is_err() && gate.load(Ordering::Acquire) == failed_generation {
                        let _ = failed_tx.send(WorkerEvent::AttachmentFailed {
                            job_id: failed_job_id,
                            generation: failed_generation,
                            failure: AttachmentFailure::Protocol,
                        });
                    }
                }),
            );
            continue;
        }
        if let WorkerCommand::FetchFolder {
            request_id,
            generation,
            account_email,
            folder,
            catalog,
        } = &command
        {
            folder_generation.store(*generation, Ordering::Release);
            if let Some(task) = folder_task.take() {
                task.abort();
                let _ = task.await;
            }
            folder_task = Some(tokio::spawn(fetch_folder_view(
                FolderLoadRequest {
                    request_id: *request_id,
                    generation: *generation,
                    account_email: account_email.clone(),
                    folder: folder.clone(),
                    catalog: catalog.clone(),
                },
                events.clone(),
                auth.clone(),
                cache_io.clone(),
                folder_generation.clone(),
                metadata_lane.clone(),
            )));
            continue;
        }
        if let WorkerCommand::CancelSearch {
            request_id,
            generation,
        } = &command
        {
            search_generation.store(*generation, Ordering::Release);
            if let Some((active_id, task)) = search_task.take() {
                if active_id == *request_id {
                    task.abort();
                    let _ = task.await;
                } else {
                    search_task = Some((active_id, task));
                }
            }
            let _ = events.send(WorkerEvent::SearchCancelled {
                request_id: *request_id,
                generation: *generation,
            });
            continue;
        }
        if let WorkerCommand::SearchGmail {
            request_id,
            generation,
            account_email,
            folder,
            query,
        } = &command
        {
            search_generation.store(*generation, Ordering::Release);
            if let Some((_, task)) = search_task.take() {
                task.abort();
                let _ = task.await;
            }
            let request_id = *request_id;
            search_task = Some((
                request_id,
                tokio::spawn(search_gmail(
                    request_id,
                    *generation,
                    account_email.clone(),
                    folder.clone(),
                    query.clone(),
                    events.clone(),
                    auth.clone(),
                    search_generation.clone(),
                )),
            ));
            continue;
        }
        if matches!(
            &command,
            WorkerCommand::MutateMessage { .. } | WorkerCommand::MutateMessageInCatalog { .. }
        ) {
            let (request_id, generation, account_email, message_id, locator, catalog, mutation) =
                match command {
                    WorkerCommand::MutateMessage {
                        request_id,
                        generation,
                        account_email,
                        message_id,
                        locator,
                        mutation,
                    } => {
                        let catalog = legacy_mutation_catalog(&locator, &mutation);
                        (
                            request_id,
                            generation,
                            account_email,
                            message_id,
                            locator,
                            catalog,
                            mutation,
                        )
                    }
                    WorkerCommand::MutateMessageInCatalog {
                        request_id,
                        generation,
                        account_email,
                        message_id,
                        locator,
                        catalog,
                        mutation,
                    } => (
                        request_id,
                        generation,
                        account_email,
                        message_id,
                        locator,
                        catalog,
                        mutation,
                    ),
                    _ => unreachable!(),
                };
            if let Err(error) =
                mutation_tx.send(MutationActorCommand::Mutate(Box::new(MutationIo {
                    request_id,
                    generation,
                    account_email,
                    message_id,
                    locator,
                    catalog,
                    mutation,
                })))
            {
                let MutationActorCommand::Mutate(failed) = error.0 else {
                    unreachable!()
                };
                let _ = events.send(WorkerEvent::MutationFailed {
                    request_id: failed.request_id,
                    generation: failed.generation,
                    message_id: failed.message_id,
                    uncertain: false,
                });
            }
            continue;
        }
        if matches!(
            command,
            WorkerCommand::LoadDrafts { .. }
                | WorkerCommand::SaveDraft { .. }
                | WorkerCommand::DeleteDraft { .. }
                | WorkerCommand::StageAttachment { .. }
                | WorkerCommand::RemoveStaged { .. }
                | WorkerCommand::LoadSignature { .. }
                | WorkerCommand::SaveSignature { .. }
        ) {
            let operation = match command {
                WorkerCommand::LoadDrafts {
                    operation_id,
                    generation,
                    account_email,
                } => DraftIo::Load {
                    operation_id,
                    generation,
                    account_email,
                },
                WorkerCommand::SaveDraft {
                    operation_id,
                    generation,
                    draft,
                } => DraftIo::Save {
                    operation_id,
                    generation,
                    draft,
                },
                WorkerCommand::DeleteDraft {
                    operation_id,
                    generation,
                    account_email,
                    draft_id,
                } => DraftIo::Delete {
                    operation_id,
                    generation,
                    account_email,
                    draft_id,
                },
                WorkerCommand::StageAttachment {
                    operation_id,
                    generation,
                    account_email,
                    draft_id,
                    source,
                    display_name,
                    media_type,
                    inline,
                } => DraftIo::Stage {
                    operation_id,
                    generation,
                    account_email,
                    draft_id,
                    source,
                    display_name,
                    media_type,
                    inline,
                },
                WorkerCommand::RemoveStaged {
                    operation_id,
                    generation,
                    account_email,
                    draft_id,
                    staged_file,
                } => DraftIo::RemoveStaged {
                    operation_id,
                    generation,
                    account_email,
                    draft_id,
                    staged_file,
                },
                WorkerCommand::LoadSignature {
                    operation_id,
                    generation,
                    account_email,
                } => DraftIo::LoadSignature {
                    operation_id,
                    generation,
                    account_email,
                },
                WorkerCommand::SaveSignature {
                    operation_id,
                    generation,
                    account_email,
                    preference,
                } => DraftIo::SaveSignature {
                    operation_id,
                    generation,
                    account_email,
                    preference,
                },
                _ => unreachable!(),
            };
            if let Err(error) = draft_tx.send(operation) {
                let generation = match &error.0 {
                    DraftIo::Load { generation, .. }
                    | DraftIo::Save { generation, .. }
                    | DraftIo::Delete { generation, .. }
                    | DraftIo::Stage { generation, .. }
                    | DraftIo::RemoveStaged { generation, .. }
                    | DraftIo::LoadSignature { generation, .. }
                    | DraftIo::SaveSignature { generation, .. }
                    | DraftIo::PurgeAccount { generation, .. } => *generation,
                };
                let account_email = match &error.0 {
                    DraftIo::Load { account_email, .. }
                    | DraftIo::Delete { account_email, .. }
                    | DraftIo::Stage { account_email, .. }
                    | DraftIo::RemoveStaged { account_email, .. }
                    | DraftIo::LoadSignature { account_email, .. }
                    | DraftIo::SaveSignature { account_email, .. } => account_email.clone(),
                    DraftIo::Save { draft, .. } => draft.account_email.clone(),
                    DraftIo::PurgeAccount { account_email, .. } => account_email.clone(),
                };
                let operation_id = match error.0 {
                    DraftIo::Load { operation_id, .. }
                    | DraftIo::Save { operation_id, .. }
                    | DraftIo::Delete { operation_id, .. }
                    | DraftIo::Stage { operation_id, .. }
                    | DraftIo::RemoveStaged { operation_id, .. }
                    | DraftIo::LoadSignature { operation_id, .. }
                    | DraftIo::SaveSignature { operation_id, .. } => operation_id,
                    DraftIo::PurgeAccount { .. } => DraftOperationId(0),
                };
                let _ = events.send(WorkerEvent::DraftOperationFailed {
                    operation_id,
                    generation,
                    account_email,
                });
            }
            continue;
        }
        if matches!(&command, WorkerCommand::SendMessage { .. }) {
            let WorkerCommand::SendMessage {
                request_id,
                generation,
                submission,
            } = command
            else {
                unreachable!()
            };
            if let Some(task) = send_task.take() {
                if !task.is_finished() {
                    send_task = Some(task);
                    let _ = events.send(WorkerEvent::MessageSendFailed {
                        request_id,
                        generation,
                        failure: SendFailure::Protocol,
                    });
                    continue;
                }
                let _ = task.await;
            }
            send_generation.store(generation, Ordering::Release);
            let draft = submission.draft;
            let credentials = auth
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_ref()
                .filter(|value| value.email.eq_ignore_ascii_case(&draft.account_email))
                .map(|value| {
                    (
                        value.email.clone(),
                        Zeroizing::new(value.access_token.as_str().to_owned()),
                    )
                });
            let tx = events.clone();
            let gate = send_generation.clone();
            send_task = Some(tokio::spawn(async move {
                let result = guard_smtp_send(async move {
                    let Some((email, token)) = credentials else {
                        return Err(smtp::SmtpError::Authentication);
                    };
                    let submission =
                        tokio::task::spawn_blocking(move || materialize_submission(draft))
                            .await
                            .map_err(|_| smtp::SmtpError::Build)??;
                    let message = smtp::build_message(&submission)?;
                    smtp::send_message(&email, token.as_str(), message).await
                })
                .await;
                if gate.load(Ordering::Acquire) != generation {
                    return;
                }
                let event = match result {
                    Ok(()) => WorkerEvent::MessageSent {
                        request_id,
                        generation,
                    },
                    Err(error) => WorkerEvent::MessageSendFailed {
                        request_id,
                        generation,
                        failure: map_send_failure(error),
                    },
                };
                let _ = tx.send(event);
            }));
            continue;
        }
        if let WorkerCommand::FetchBody {
            request_id,
            generation,
            account_email,
            message_id,
            locator,
        } = &command
        {
            if *generation != cache_generation.load(Ordering::Acquire) {
                continue;
            }
            if let Some(task) = body_task.take() {
                task.abort();
                let _ = task.await;
            }
            let tx = events.clone();
            let request_id = *request_id;
            let generation = *generation;
            let account_email = account_email.clone();
            let message_id = message_id.clone();
            let locator = locator.clone();
            let failure_message_id = message_id.clone();
            let auth = auth.clone();
            let cache_generation = cache_generation.clone();
            let body_cache_generation = cache_generation.clone();
            let cache_io = cache_io.clone();
            let slots = content_slots.clone();
            body_task = Some(tokio::spawn(async move {
                let Ok(_slot) = slots.acquire_owned().await else {
                    return;
                };
                let result = std::panic::AssertUnwindSafe(fetch_body(
                    BodyLoadRequest {
                        request_id,
                        generation,
                        account_email,
                        message_id,
                        locator,
                    },
                    &tx,
                    &auth,
                    BodyCacheServices {
                        generation: body_cache_generation,
                        io: cache_io,
                    },
                ))
                .catch_unwind()
                .await;
                if result.is_err() && cache_generation.load(Ordering::Acquire) == generation {
                    send_body_failure(
                        &tx,
                        request_id,
                        generation,
                        failure_message_id,
                        BodyFailure::Protocol,
                    );
                }
            }));
            continue;
        }
        if let WorkerCommand::ClearBodyCache {
            operation_id,
            generation,
        } = &command
        {
            cache_generation.store(*generation, Ordering::Release);
            if let Some(task) = body_task.take() {
                task.abort();
                let _ = task.await;
            }
            for (_, task) in attachment_tasks.drain() {
                task.abort();
                let _ = task.await;
            }
            let cleared = cache_blocking(cache_io.clone(), || {
                let reclaimed = cache::clear_bodies()?;
                let usage = cache::usage().unwrap_or_default();
                Ok::<_, std::io::Error>((reclaimed, usage))
            })
            .await;
            match cleared {
                Ok(Ok((reclaimed_bytes, usage))) => {
                    let _ = events.send(WorkerEvent::CacheCleared {
                        operation_id: *operation_id,
                        generation: *generation,
                        reclaimed_bytes,
                        usage,
                    });
                }
                Ok(Err(_)) | Err(_) => {
                    let _ = events.send(WorkerEvent::CacheClearFailed {
                        operation_id: *operation_id,
                        generation: *generation,
                    });
                }
            }
            continue;
        }
        let account_boundary = matches!(
            &command,
            WorkerCommand::Connect { .. }
                | WorkerCommand::Restore { .. }
                | WorkerCommand::Refresh { .. }
                | WorkerCommand::Disconnect { .. }
        );
        if account_boundary {
            // The barrier is queued behind every transmitted mutation. It is
            // deliberately awaited before auth replacement or local purge.
            if !drain_mutations(&mutation_tx).await {
                let id = command_id(&command).unwrap_or(OperationId(0));
                if matches!(&command, WorkerCommand::Disconnect { .. }) {
                    fail_cleanup(&events, id, false);
                } else {
                    fail(&events, id, FailureKind::WorkerUnavailable, false, true);
                }
                continue;
            }
            // State blocks account transitions during SMTP. This defensive
            // wait preserves a definite send outcome if a command is injected.
            if let Some(task) = send_task.take() {
                let _ = task.await;
            }
            if let Some(task) = body_task.take() {
                task.abort();
                let _ = task.await;
            }
            if let Some(task) = folder_task.take() {
                task.abort();
                let _ = task.await;
            }
            if let Some((_, task)) = search_task.take() {
                task.abort();
                let _ = task.await;
            }
            for (_, task) in attachment_tasks.drain() {
                task.abort();
                let _ = task.await;
            }
        }
        if let WorkerCommand::Disconnect { generation, .. } = &command {
            cache_generation.store(*generation, Ordering::Release);
            send_generation.store(*generation, Ordering::Release);
            folder_generation.fetch_add(1, Ordering::AcqRel);
            search_generation.fetch_add(1, Ordering::AcqRel);
            if let Some(task) = body_task.take() {
                task.abort();
                let _ = task.await;
            }
            if let Some(task) = folder_task.take() {
                task.abort();
                let _ = task.await;
            }
            if let Some((_, task)) = search_task.take() {
                task.abort();
                let _ = task.await;
            }
            for (_, task) in attachment_tasks.drain() {
                task.abort();
                let _ = task.await;
            }
            auth.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
        }
        if matches!(command, WorkerCommand::Shutdown) {
            cache_generation.fetch_add(1, Ordering::AcqRel);
            send_generation.fetch_add(1, Ordering::AcqRel);
            folder_generation.fetch_add(1, Ordering::AcqRel);
            search_generation.fetch_add(1, Ordering::AcqRel);
            if let Some(task) = body_task.take() {
                task.abort();
                let _ = task.await;
            }
            if let Some(task) = folder_task.take() {
                task.abort();
                let _ = task.await;
            }
            if let Some((_, task)) = search_task.take() {
                task.abort();
                let _ = task.await;
            }
            for (_, task) in attachment_tasks.drain() {
                task.abort();
                let _ = task.await;
            }
            auth.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
        }
        let disconnecting = matches!(command, WorkerCommand::Disconnect { .. });
        let cleanup_failed = if let Some(current) = active.take() {
            let Active { task, cleanup, .. } = current;
            if disconnecting {
                // Disconnect owns credential deletion and must perform it only
                // after every local account artifact has been purged.
                task.abort();
                let _ = task.await;
                false
            } else {
                abort_with_cleanup(task, &cleanup, async {
                    matches!(
                        timeout(KEYRING_TIMEOUT, secrets::delete()).await,
                        Ok(Ok(()))
                    )
                })
                .await
            }
        } else {
            false
        };
        if matches!(command, WorkerCommand::Shutdown) {
            break;
        }
        if let WorkerCommand::Cancel { id } = command {
            if cleanup_failed {
                fail_cleanup(&events, id, true);
            } else {
                let _ = events.send(WorkerEvent::Cancelled { id });
            }
            continue;
        }
        if cleanup_failed {
            let id = command_id(&command).unwrap_or(OperationId(0));
            fail_cleanup(&events, id, true);
            continue;
        }
        let id = command_id(&command).unwrap_or(OperationId(0));
        let cleanup = Arc::new(AtomicBool::new(false));
        let task = match command {
            WorkerCommand::Disconnect {
                id,
                draft_generation,
                account_email,
                ..
            } => {
                let tx = events.clone();
                let cache_io = cache_io.clone();
                let draft_tx = draft_tx.clone();
                tokio::spawn(async move {
                    emit_phase(&tx, id, WorkerPhase::Disconnecting);
                    let draft_cleanup = async {
                        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
                        if draft_tx
                            .send(DraftIo::PurgeAccount {
                                generation: draft_generation,
                                account_email,
                                result: result_tx,
                            })
                            .is_err()
                        {
                            return false;
                        }
                        if !matches!(timeout(KEYRING_TIMEOUT, result_rx).await, Ok(Ok(Ok(())))) {
                            return false;
                        }
                        true
                    };
                    let cache_cleanup = async {
                        matches!(
                            cache_blocking(cache_io, cache::clear_all_mail).await,
                            Ok(Ok(()))
                        )
                    };
                    let token_cleanup = async {
                        matches!(
                            timeout(KEYRING_TIMEOUT, secrets::delete()).await,
                            Ok(Ok(()))
                        )
                    };
                    if cleanup_account_token_last(draft_cleanup, cache_cleanup, token_cleanup).await
                    {
                        let _ = tx.send(WorkerEvent::Disconnected { id });
                    } else {
                        fail_cleanup(&tx, id, false);
                    }
                })
            }
            WorkerCommand::Connect { id } => {
                let tx = events.clone();
                let cleanup_task = cleanup.clone();
                let auth = auth.clone();
                let cache_io = cache_io.clone();
                let metadata_lane = metadata_lane.clone();
                tokio::spawn(async move {
                    let _lane = metadata_lane.lock().await;
                    let result = connect(id, &tx, &cleanup_task, &auth, &cache_io).await;
                    if !result.as_ref().is_err_and(|failure| failure.cleanup_failed) {
                        cleanup_task.store(false, Ordering::Release);
                    }
                    if let Err(failure) = result {
                        let _ = tx.send(WorkerEvent::Failed { id, failure });
                    }
                })
            }
            WorkerCommand::Restore { id } => {
                let tx = events.clone();
                let cleanup_task = cleanup.clone();
                let auth = auth.clone();
                let cache_io = cache_io.clone();
                let metadata_lane = metadata_lane.clone();
                tokio::spawn(async move {
                    let _lane = metadata_lane.lock().await;
                    let result =
                        restore_or_refresh(id, &tx, true, &cleanup_task, &auth, &cache_io).await;
                    if !result.as_ref().is_err_and(|failure| failure.cleanup_failed) {
                        cleanup_task.store(false, Ordering::Release);
                    }
                    if let Err(failure) = result {
                        let _ = tx.send(WorkerEvent::Failed { id, failure });
                    }
                })
            }
            WorkerCommand::Refresh { id } => {
                let tx = events.clone();
                let cleanup_task = cleanup.clone();
                let auth = auth.clone();
                let cache_io = cache_io.clone();
                let metadata_lane = metadata_lane.clone();
                tokio::spawn(async move {
                    let _lane = metadata_lane.lock().await;
                    let result =
                        restore_or_refresh(id, &tx, false, &cleanup_task, &auth, &cache_io).await;
                    if !result.as_ref().is_err_and(|failure| failure.cleanup_failed) {
                        cleanup_task.store(false, Ordering::Release);
                    }
                    if let Err(failure) = result {
                        let _ = tx.send(WorkerEvent::Failed { id, failure });
                    }
                })
            }
            WorkerCommand::Cancel { .. }
            | WorkerCommand::SetCacheLimit { .. }
            | WorkerCommand::FetchFolder { .. }
            | WorkerCommand::SearchGmail { .. }
            | WorkerCommand::CancelSearch { .. }
            | WorkerCommand::FetchBody { .. }
            | WorkerCommand::DownloadAttachment { .. }
            | WorkerCommand::CancelAttachment { .. }
            | WorkerCommand::ClearBodyCache { .. }
            | WorkerCommand::SendMessage { .. }
            | WorkerCommand::LoadDrafts { .. }
            | WorkerCommand::SaveDraft { .. }
            | WorkerCommand::DeleteDraft { .. }
            | WorkerCommand::StageAttachment { .. }
            | WorkerCommand::RemoveStaged { .. }
            | WorkerCommand::LoadSignature { .. }
            | WorkerCommand::SaveSignature { .. }
            | WorkerCommand::MutateMessage { .. }
            | WorkerCommand::MutateMessageInCatalog { .. }
            | WorkerCommand::Shutdown => unreachable!(),
        };
        active = Some(Active { id, task, cleanup });
    }
    draft_task.abort();
    mutation_task.abort();
}

#[allow(clippy::too_many_arguments)]
async fn search_gmail(
    request_id: SearchRequestId,
    generation: u64,
    account_email: String,
    folder: FolderDescriptor,
    query: String,
    events: mpsc::UnboundedSender<WorkerEvent>,
    auth: Arc<Mutex<Option<RuntimeAuth>>>,
    generation_gate: Arc<std::sync::atomic::AtomicU64>,
) {
    let credentials = auth
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .filter(|value| value.email.eq_ignore_ascii_case(&account_email))
        .map(|value| Zeroizing::new(value.access_token.as_str().to_owned()));
    let Some(access_token) = credentials else {
        let _ = events.send(WorkerEvent::SearchFailed {
            request_id,
            generation,
            failure: BodyFailure::AuthorizationRequired,
        });
        return;
    };
    let fetched =
        gmail::search_all_mail(&account_email, access_token.as_str(), folder, &query).await;
    if generation_gate.load(Ordering::Acquire) != generation {
        return;
    }
    match fetched {
        Ok(fetched) => {
            let (messages, fallback_count) = message::map_summaries(fetched.records);
            let _ = events.send(WorkerEvent::SearchLoaded {
                request_id,
                generation,
                messages,
                truncated: fetched.truncated,
                skipped_count: fetched.skipped_count.saturating_add(fallback_count),
            });
        }
        Err(error) => {
            let _ = events.send(WorkerEvent::SearchFailed {
                request_id,
                generation,
                failure: map_body_failure(error),
            });
        }
    }
}

struct MutationIo {
    request_id: MutationRequestId,
    generation: u64,
    account_email: String,
    message_id: MessageId,
    locator: MessageLocator,
    catalog: FolderCatalog,
    mutation: MessageMutation,
}

fn legacy_mutation_catalog(locator: &MessageLocator, mutation: &MessageMutation) -> FolderCatalog {
    let mut folders = vec![FolderDescriptor {
        id: locator.folder_id.clone(),
        mailbox: locator.mailbox.clone(),
        display_name: locator.mailbox.clone(),
        kind: match &locator.folder_id {
            FolderId::Inbox => FolderKind::Inbox,
            FolderId::Sent => FolderKind::Sent,
            FolderId::AllMail => FolderKind::AllMail,
            FolderId::Trash => FolderKind::Trash,
            FolderId::Starred => FolderKind::Starred,
            FolderId::Label(_) => FolderKind::Label,
        },
    }];
    // The legacy command already supplies a target resolved by the state
    // catalog.  Preserve that path without hard-coding Gmail's Trash name.
    if let MessageMutation::MoveToTrash { mailbox } = mutation {
        folders.push(FolderDescriptor {
            id: FolderId::Trash,
            mailbox: mailbox.clone(),
            display_name: mailbox.clone(),
            kind: FolderKind::Trash,
        });
    }
    if let MessageMutation::RestoreArchive { inbox_mailbox } = mutation {
        folders.push(FolderDescriptor {
            id: FolderId::Inbox,
            mailbox: inbox_mailbox.clone(),
            display_name: inbox_mailbox.clone(),
            kind: FolderKind::Inbox,
        });
    }
    if let MessageMutation::RestoreFromTrash {
        inbox_mailbox,
        trash_mailbox,
        ..
    } = mutation
    {
        folders.push(FolderDescriptor {
            id: FolderId::Inbox,
            mailbox: inbox_mailbox.clone(),
            display_name: inbox_mailbox.clone(),
            kind: FolderKind::Inbox,
        });
        folders.push(FolderDescriptor {
            id: FolderId::Trash,
            mailbox: trash_mailbox.clone(),
            display_name: trash_mailbox.clone(),
            kind: FolderKind::Trash,
        });
    }
    FolderCatalog::bounded(folders)
}

enum MutationActorCommand {
    Mutate(Box<MutationIo>),
    Barrier(tokio::sync::oneshot::Sender<()>),
}

async fn mutation_actor(
    mut commands: mpsc::UnboundedReceiver<MutationActorCommand>,
    events: mpsc::UnboundedSender<WorkerEvent>,
    auth: Arc<Mutex<Option<RuntimeAuth>>>,
    cache_io: Arc<Mutex<()>>,
    generation_gate: Arc<std::sync::atomic::AtomicU64>,
    metadata_lane: Arc<tokio::sync::Mutex<()>>,
) {
    while let Some(actor_command) = commands.recv().await {
        let command = match actor_command {
            MutationActorCommand::Mutate(command) => command,
            MutationActorCommand::Barrier(done) => {
                let _ = done.send(());
                continue;
            }
        };
        if generation_gate.load(Ordering::Acquire) != command.generation {
            continue;
        }
        let _lane = metadata_lane.lock().await;
        if generation_gate.load(Ordering::Acquire) != command.generation {
            continue;
        }
        let credentials = auth
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .filter(|value| value.email.eq_ignore_ascii_case(&command.account_email))
            .map(|value| Zeroizing::new(value.access_token.as_str().to_owned()));
        let outcome = if let Some(token) = credentials {
            gmail::mutate_message(
                &command.account_email,
                token.as_str(),
                &command.message_id,
                &command.locator,
                &command.catalog,
                &command.mutation,
            )
            .await
        } else {
            gmail::MutationResult::DefiniteFailure(gmail::GmailError::AuthenticationFailed)
        };
        if generation_gate.load(Ordering::Acquire) != command.generation {
            continue;
        }
        let persist = match &outcome {
            gmail::MutationResult::Confirmed => {
                let account = command.account_email.clone();
                let id = command.message_id.clone();
                let mutation = command.mutation.clone();
                let gate = generation_gate.clone();
                let generation = command.generation;
                Some(
                    cache_blocking(cache_io.clone(), move || {
                        (gate.load(Ordering::Acquire) == generation)
                            .then(|| cache::persist_confirmed_mutation(&account, &id, &mutation))
                            .transpose()
                    })
                    .await,
                )
            }
            gmail::MutationResult::Reconciled(state) => {
                let account = command.account_email.clone();
                let id = command.message_id.clone();
                let state = state.clone();
                let gate = generation_gate.clone();
                let generation = command.generation;
                Some(
                    cache_blocking(cache_io.clone(), move || {
                        (gate.load(Ordering::Acquire) == generation)
                            .then(|| cache::persist_reconciled_inbox(&account, &id, state.as_ref()))
                            .transpose()
                    })
                    .await,
                )
            }
            _ => None,
        };
        if generation_gate.load(Ordering::Acquire) != command.generation {
            continue;
        }
        if persist.is_some_and(|result| !matches!(result, Ok(Ok(Some(()))))) {
            tracing::warn!("authoritative Gmail mutation could not be saved to the local cache");
        }
        let event = match outcome {
            gmail::MutationResult::Confirmed => WorkerEvent::MutationConfirmed {
                request_id: command.request_id,
                generation: command.generation,
                message_id: command.message_id,
            },
            gmail::MutationResult::Reconciled(state) => WorkerEvent::MutationReconciled {
                request_id: command.request_id,
                generation: command.generation,
                message_id: command.message_id,
                state,
            },
            gmail::MutationResult::DefiniteFailure(_) => WorkerEvent::MutationFailed {
                request_id: command.request_id,
                generation: command.generation,
                message_id: command.message_id,
                uncertain: false,
            },
            gmail::MutationResult::Uncertain => WorkerEvent::MutationFailed {
                request_id: command.request_id,
                generation: command.generation,
                message_id: command.message_id,
                uncertain: true,
            },
        };
        let _ = events.send(event);
    }
}

async fn drain_mutations(tx: &mpsc::UnboundedSender<MutationActorCommand>) -> bool {
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    tx.send(MutationActorCommand::Barrier(done_tx)).is_ok() && done_rx.await.is_ok()
}

enum DraftIo {
    Load {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
    },
    Save {
        operation_id: DraftOperationId,
        generation: u64,
        draft: Box<ComposeDraft>,
    },
    Delete {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
        draft_id: String,
    },
    Stage {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
        draft_id: String,
        source: std::path::PathBuf,
        display_name: String,
        media_type: String,
        inline: bool,
    },
    RemoveStaged {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
        draft_id: String,
        staged_file: String,
    },
    LoadSignature {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
    },
    SaveSignature {
        operation_id: DraftOperationId,
        generation: u64,
        account_email: String,
        preference: drafts::SignaturePreference,
    },
    PurgeAccount {
        generation: u64,
        account_email: String,
        result: tokio::sync::oneshot::Sender<std::io::Result<()>>,
    },
}

async fn draft_io_actor(
    mut rx: mpsc::UnboundedReceiver<DraftIo>,
    events: mpsc::UnboundedSender<WorkerEvent>,
) {
    while let Some(first) = rx.recv().await {
        let mut batch = vec![first];
        while let Ok(operation) = rx.try_recv() {
            batch.push(operation);
        }
        for index in 0..batch.len() {
            let obsolete = match &batch[index] {
                DraftIo::Save { draft, .. } => batch[index + 1..].iter().any(|later| {
                    matches!(later, DraftIo::Save { draft: candidate, .. }
                        if candidate.id == draft.id && candidate.dirty_revision >= draft.dirty_revision)
                }),
                _ => false,
            };
            if obsolete {
                continue;
            }
            // Replacing with a harmless sentinel lets us move the operation
            // without cloning multi-megabyte rich text.
            let operation = std::mem::replace(
                &mut batch[index],
                DraftIo::Load {
                    operation_id: DraftOperationId(0),
                    generation: 0,
                    account_email: String::new(),
                },
            );
            if let DraftIo::PurgeAccount {
                account_email,
                result,
                ..
            } = operation
            {
                let outcome = if account_email.is_empty() {
                    Ok(())
                } else {
                    tokio::task::spawn_blocking(move || drafts::purge_account(&account_email))
                        .await
                        .unwrap_or_else(|_| Err(std::io::Error::other("draft purge task failed")))
                };
                let _ = result.send(outcome);
                continue;
            }
            handle_draft_io(operation, &events).await;
        }
    }
}

async fn handle_draft_io(operation: DraftIo, events: &mpsc::UnboundedSender<WorkerEvent>) {
    let operation_id = match &operation {
        DraftIo::Load { operation_id, .. }
        | DraftIo::Save { operation_id, .. }
        | DraftIo::Delete { operation_id, .. }
        | DraftIo::Stage { operation_id, .. }
        | DraftIo::RemoveStaged { operation_id, .. }
        | DraftIo::LoadSignature { operation_id, .. }
        | DraftIo::SaveSignature { operation_id, .. } => *operation_id,
        DraftIo::PurgeAccount { .. } => unreachable!(),
    };
    let account_email = match &operation {
        DraftIo::Load { account_email, .. }
        | DraftIo::Delete { account_email, .. }
        | DraftIo::Stage { account_email, .. }
        | DraftIo::RemoveStaged { account_email, .. }
        | DraftIo::LoadSignature { account_email, .. }
        | DraftIo::SaveSignature { account_email, .. } => account_email.clone(),
        DraftIo::Save { draft, .. } => draft.account_email.clone(),
        DraftIo::PurgeAccount { .. } => unreachable!(),
    };
    let generation = match &operation {
        DraftIo::Load { generation, .. }
        | DraftIo::Save { generation, .. }
        | DraftIo::Delete { generation, .. }
        | DraftIo::Stage { generation, .. }
        | DraftIo::RemoveStaged { generation, .. }
        | DraftIo::LoadSignature { generation, .. }
        | DraftIo::SaveSignature { generation, .. } => *generation,
        DraftIo::PurgeAccount { .. } => unreachable!(),
    };
    let failure_account = account_email.clone();
    let result = tokio::task::spawn_blocking(move || match operation {
        DraftIo::Load {
            operation_id,
            generation,
            account_email,
        } => drafts::load_all(&account_email).map(|mut drafts| {
            for draft in &mut drafts {
                draft.html = crate::composer::sanitize_html(&draft.html);
                draft.text = crate::composer::html_to_plain(&draft.html);
            }
            WorkerEvent::DraftsLoaded {
                operation_id,
                generation,
                account_email,
                drafts,
            }
        }),
        DraftIo::Save {
            operation_id,
            generation,
            mut draft,
        } => {
            draft.html = crate::composer::sanitize_html(&draft.html);
            draft.text = crate::composer::html_to_plain(&draft.html);
            drafts::save(&draft.account_email, &draft)?;
            Ok(WorkerEvent::DraftSaved {
                operation_id,
                generation,
                account_email: draft.account_email.clone(),
                draft_id: draft.id.clone(),
                revision: draft.dirty_revision,
            })
        }
        DraftIo::Delete {
            operation_id,
            generation,
            account_email,
            draft_id,
        } => {
            drafts::delete(&account_email, &draft_id)?;
            Ok(WorkerEvent::DraftDeleted {
                operation_id,
                generation,
                account_email,
                draft_id,
            })
        }
        DraftIo::Stage {
            operation_id,
            generation,
            account_email,
            draft_id,
            source,
            display_name,
            media_type,
            inline,
        } => {
            let attachment = drafts::stage_file(
                &account_email,
                &draft_id,
                &source,
                &display_name,
                &media_type,
            )?;
            Ok(WorkerEvent::AttachmentStaged {
                operation_id,
                generation,
                account_email,
                draft_id,
                attachment,
                inline,
            })
        }
        DraftIo::RemoveStaged {
            operation_id,
            generation,
            account_email,
            draft_id,
            staged_file,
        } => {
            drafts::remove_staged(&account_email, &draft_id, &staged_file)?;
            Ok(WorkerEvent::StagedRemoved {
                operation_id,
                generation,
                account_email,
            })
        }
        DraftIo::LoadSignature {
            operation_id,
            generation,
            account_email,
        } => {
            drafts::load_signature(&account_email).map(|preference| WorkerEvent::SignatureLoaded {
                operation_id,
                generation,
                account_email,
                preference,
            })
        }
        DraftIo::SaveSignature {
            operation_id,
            generation,
            account_email,
            preference,
        } => {
            drafts::save_signature(&account_email, &preference)?;
            Ok(WorkerEvent::SignatureSaved {
                operation_id,
                generation,
                account_email,
            })
        }
        DraftIo::PurgeAccount { .. } => unreachable!(),
    })
    .await;
    match result {
        Ok(Ok(event)) => {
            let _ = events.send(event);
        }
        _ => {
            let _ = events.send(WorkerEvent::DraftOperationFailed {
                operation_id,
                generation,
                account_email: failure_account,
            });
        }
    }
}

fn materialize_submission(draft: ComposeDraft) -> Result<smtp::MailSubmission, smtp::SmtpError> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    fn load(
        account: &str,
        id: &str,
        attachment: &DraftAttachment,
    ) -> Result<smtp::SubmissionPart, smtp::SmtpError> {
        let path = drafts::staged_path(account, id, &attachment.staged_file)
            .map_err(|_| smtp::SmtpError::Build)?;
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| smtp::SmtpError::Build)?;
        let metadata = file.metadata().map_err(|_| smtp::SmtpError::Build)?;
        if !metadata.is_file() || metadata.len() != attachment.bytes {
            return Err(smtp::SmtpError::Build);
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take(attachment.bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| smtp::SmtpError::Build)?;
        if bytes.len() as u64 != attachment.bytes {
            return Err(smtp::SmtpError::Build);
        }
        Ok(smtp::SubmissionPart {
            display_name: attachment.display_name.clone(),
            media_type: attachment.media_type.clone(),
            bytes,
        })
    }
    let all_parts = draft
        .attachments
        .iter()
        .chain(draft.inline_images.iter().map(|inline| &inline.attachment));
    let (count, total) = all_parts.fold((0_usize, 0_u64), |(count, total), part| {
        (count.saturating_add(1), total.saturating_add(part.bytes))
    });
    if count > drafts::MAX_STAGED_FILES || total > drafts::MAX_STAGED_TOTAL_BYTES {
        return Err(smtp::SmtpError::BodyTooLarge);
    }
    let attachments = draft
        .attachments
        .iter()
        .map(|part| load(&draft.account_email, &draft.id, part))
        .collect::<Result<Vec<_>, _>>()?;
    let inline_images = draft
        .inline_images
        .iter()
        .map(|inline| {
            Ok(smtp::InlineSubmissionPart {
                part: load(&draft.account_email, &draft.id, &inline.attachment)?,
                content_id: inline.content_id.clone(),
            })
        })
        .collect::<Result<Vec<_>, smtp::SmtpError>>()?;
    Ok(smtp::MailSubmission {
        account_email: draft.account_email,
        to: draft.to,
        cc: draft.cc,
        bcc: draft.bcc,
        subject: draft.subject,
        html: draft.html,
        attachments,
        inline_images,
        thread: draft.thread,
    })
}

struct RuntimeAuth {
    email: String,
    access_token: Zeroizing<String>,
}

struct BodyCacheServices {
    generation: Arc<std::sync::atomic::AtomicU64>,
    io: Arc<Mutex<()>>,
}

struct BodyLoadRequest {
    request_id: BodyRequestId,
    generation: u64,
    account_email: String,
    message_id: MessageId,
    locator: MessageLocator,
}

struct FolderLoadRequest {
    request_id: FolderRequestId,
    generation: u64,
    account_email: String,
    folder: FolderDescriptor,
    catalog: FolderCatalog,
}

struct AttachmentLoadRequest {
    job_id: AttachmentJobId,
    generation: u64,
    account_email: String,
    message_id: MessageId,
    locator: MessageLocator,
    attachment: crate::model::Attachment,
    destination: AttachmentDestination,
}

async fn guard_smtp_send<F>(send: F) -> Result<(), smtp::SmtpError>
where
    F: Future<Output = Result<(), smtp::SmtpError>>,
{
    std::panic::AssertUnwindSafe(send)
        .catch_unwind()
        .await
        // A panic may happen after SMTP accepted DATA; never imply retry is safe.
        .unwrap_or(Err(smtp::SmtpError::DeliveryUncertain))
}

async fn cache_blocking<T, F>(
    cache_io: Arc<Mutex<()>>,
    operation: F,
) -> Result<T, tokio::task::JoinError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let _guard = cache_io
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        operation()
    })
    .await
}

fn join_failed(result: &Result<(), tokio::task::JoinError>) -> bool {
    result.as_ref().is_err_and(|error| !error.is_cancelled())
}

async fn abort_with_cleanup<F: Future<Output = bool>>(
    task: JoinHandle<()>,
    cleanup_required: &AtomicBool,
    cleanup: F,
) -> bool {
    task.abort();
    let _ = task.await;
    if !cleanup_required.load(Ordering::Acquire) {
        return false;
    }
    let cleaned = cleanup.await;
    if cleaned {
        cleanup_required.store(false, Ordering::Release);
    }
    !cleaned
}

async fn cleanup_token_last<Local, Token>(local_cleanup: Local, token_cleanup: Token) -> bool
where
    Local: Future<Output = bool>,
    Token: Future<Output = bool>,
{
    if !local_cleanup.await {
        return false;
    }
    token_cleanup.await
}

async fn cleanup_account_token_last<Drafts, Cache, Token>(
    drafts_cleanup: Drafts,
    cache_cleanup: Cache,
    token_cleanup: Token,
) -> bool
where
    Drafts: Future<Output = bool>,
    Cache: Future<Output = bool>,
    Token: Future<Output = bool>,
{
    if !drafts_cleanup.await {
        return false;
    }
    cleanup_token_last(cache_cleanup, token_cleanup).await
}

async fn save_with_cleanup<Save, SaveFuture, Cleanup, CleanupFuture>(
    cleanup_required: &AtomicBool,
    preserve_mail: bool,
    save: Save,
    cleanup: Cleanup,
) -> Result<(), ServiceFailure>
where
    Save: FnOnce() -> SaveFuture,
    SaveFuture: Future<Output = bool>,
    Cleanup: FnOnce() -> CleanupFuture,
    CleanupFuture: Future<Output = bool>,
{
    cleanup_required.store(true, Ordering::Release);
    if save().await {
        return Ok(());
    }
    if cleanup().await {
        cleanup_required.store(false, Ordering::Release);
        Err(failure(
            FailureKind::CredentialSaveFailed,
            true,
            preserve_mail,
        ))
    } else {
        Err(ServiceFailure {
            kind: FailureKind::DisconnectFailed,
            retryable: true,
            preserve_mail,
            cleanup_failed: true,
            config_path: None,
        })
    }
}

fn command_id(command: &WorkerCommand) -> Option<OperationId> {
    match command {
        WorkerCommand::Restore { id }
        | WorkerCommand::Connect { id }
        | WorkerCommand::Refresh { id }
        | WorkerCommand::Disconnect { id, .. }
        | WorkerCommand::Cancel { id } => Some(*id),
        WorkerCommand::SetCacheLimit { .. }
        | WorkerCommand::FetchFolder { .. }
        | WorkerCommand::SearchGmail { .. }
        | WorkerCommand::CancelSearch { .. }
        | WorkerCommand::FetchBody { .. }
        | WorkerCommand::DownloadAttachment { .. }
        | WorkerCommand::CancelAttachment { .. }
        | WorkerCommand::ClearBodyCache { .. }
        | WorkerCommand::SendMessage { .. }
        | WorkerCommand::LoadDrafts { .. }
        | WorkerCommand::SaveDraft { .. }
        | WorkerCommand::DeleteDraft { .. }
        | WorkerCommand::StageAttachment { .. }
        | WorkerCommand::RemoveStaged { .. }
        | WorkerCommand::LoadSignature { .. }
        | WorkerCommand::SaveSignature { .. }
        | WorkerCommand::MutateMessage { .. }
        | WorkerCommand::MutateMessageInCatalog { .. }
        | WorkerCommand::Shutdown => None,
    }
}

async fn connect(
    id: OperationId,
    tx: &mpsc::UnboundedSender<WorkerEvent>,
    cleanup_required: &AtomicBool,
    auth: &Arc<Mutex<Option<RuntimeAuth>>>,
    cache_io: &Arc<Mutex<()>>,
) -> Result<(), ServiceFailure> {
    emit_phase(tx, id, WorkerPhase::LoadingConfiguration);
    let config = config::load().map_err(|error| map_config(error, false))?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| failure(FailureKind::AuthorizationInvalid, true, false))?;
    let port = listener
        .local_addr()
        .map_err(|_| failure(FailureKind::AuthorizationInvalid, true, false))?
        .port();
    let request =
        oauth::authorization_request(&config, port, SystemTime::now()).map_err(map_oauth)?;
    let url = AuthorizationUrl::new(request.url.expose().to_owned());
    let _ = tx.send(WorkerEvent::AuthorizationRequired {
        id,
        url,
        deadline: request.deadline,
    });
    emit_phase(tx, id, WorkerPhase::WaitingForBrowser);
    let host = format!("127.0.0.1:{port}");
    let deadline = Instant::now() + oauth::CALLBACK_TIMEOUT;
    let (mut socket, code) = loop {
        let (mut socket, peer) = timeout_at(deadline, listener.accept())
            .await
            .map_err(|_| failure(FailureKind::AuthorizationTimedOut, true, false))?
            .map_err(|_| failure(FailureKind::AuthorizationInvalid, true, false))?;
        let bytes = read_callback_headers(&mut socket, deadline)
            .await
            .map_err(map_oauth)?;
        match oauth::parse_callback_request(&bytes, peer, &host, request.csrf_state.expose()) {
            Ok(code) => break (socket, code),
            Err(oauth::OAuthError::CallbackWrongPath) => {
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
            }
            Err(error) => return Err(map_oauth(error)),
        }
    };
    let response = callback_success_header();
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.write_all(CALLBACK_SUCCESS_BODY).await;
    emit_phase(tx, id, WorkerPhase::ExchangingCode);
    let client = oauth::http_client().map_err(map_oauth)?;
    let grant = oauth::exchange_code(&config, &request, &code, &client)
        .await
        .map_err(map_oauth)?;
    emit_phase(tx, id, WorkerPhase::VerifyingIdentity);
    let account = oauth::fetch_identity(&grant.access_token, &client)
        .await
        .map_err(map_oauth)?;
    cache_blocking(cache_io.clone(), {
        let email = account.email.clone();
        move || cache::prepare_verified_account(&email)
    })
    .await
    .map_err(|_| failure(FailureKind::WorkerUnavailable, false, false))?
    .map_err(|_| failure(FailureKind::WorkerUnavailable, true, false))?;
    let _ = tx.send(WorkerEvent::IdentityVerified {
        id,
        account: account.clone(),
    });
    *auth.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(RuntimeAuth {
        email: account.email.clone(),
        access_token: Zeroizing::new(grant.access_token.expose().to_owned()),
    });
    let refresh = oauth::require_initial_refresh_token(&grant)
        .map_err(|_| failure(FailureKind::AuthorizationExpired, false, false))?;
    let token = RefreshToken::new(refresh.expose().to_owned())
        .map_err(|_| failure(FailureKind::CredentialSaveFailed, true, false))?;
    let requested_limit = cache_blocking(cache_io.clone(), cache::load_limit)
        .await
        .map_err(|_| failure(FailureKind::WorkerUnavailable, false, false))?;
    emit_phase(tx, id, WorkerPhase::OpeningKeyring);
    let fetched = persist_then_fetch(
        || async {
            save_with_cleanup(
                cleanup_required,
                false,
                || async {
                    matches!(
                        timeout(KEYRING_TIMEOUT, secrets::replace(&token)).await,
                        Ok(Ok(()))
                    )
                },
                || async {
                    matches!(
                        timeout(KEYRING_TIMEOUT, secrets::delete()).await,
                        Ok(Ok(()))
                    )
                },
            )
            .await
        },
        || {
            let _ = tx.send(WorkerEvent::AccountPersisted {
                id,
                account: account.clone(),
            });
            emit_phase(tx, id, WorkerPhase::ConnectingImap);
        },
        || async {
            gmail::fetch_inbox(&account.email, grant.access_token.expose(), requested_limit)
                .await
                .map_err(map_gmail)
        },
    )
    .await?;
    complete_sync(id, tx, account, fetched, cache_io).await
}

fn callback_success_header() -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        CALLBACK_SUCCESS_BODY.len()
    )
}

async fn persist_then_fetch<Save, SaveFuture, Notify, Fetch, FetchFuture>(
    save: Save,
    notify: Notify,
    fetch: Fetch,
) -> Result<gmail::InboxFetch, ServiceFailure>
where
    Save: FnOnce() -> SaveFuture,
    SaveFuture: Future<Output = Result<(), ServiceFailure>>,
    Notify: FnOnce(),
    Fetch: FnOnce() -> FetchFuture,
    FetchFuture: Future<Output = Result<gmail::InboxFetch, ServiceFailure>>,
{
    save().await?;
    notify();
    fetch().await
}

async fn read_callback_headers<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    deadline: Instant,
) -> Result<Vec<u8>, oauth::OAuthError> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = timeout_at(deadline, reader.read(&mut chunk))
            .await
            .map_err(|_| oauth::OAuthError::AuthorizationTimedOut)?
            .map_err(|_| oauth::OAuthError::CallbackInvalid)?;
        if read == 0 {
            return Err(oauth::OAuthError::CallbackInvalid);
        }
        if request.len().saturating_add(read) > MAX_CALLBACK_HEADERS {
            return Err(oauth::OAuthError::CallbackInvalid);
        }
        request.extend_from_slice(&chunk[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(request);
        }
    }
}

async fn restore_or_refresh(
    id: OperationId,
    tx: &mpsc::UnboundedSender<WorkerEvent>,
    restore: bool,
    cleanup_required: &AtomicBool,
    auth: &Arc<Mutex<Option<RuntimeAuth>>>,
    cache_io: &Arc<Mutex<()>>,
) -> Result<(), ServiceFailure> {
    emit_phase(tx, id, WorkerPhase::OpeningKeyring);
    let Some(token) = timeout(KEYRING_TIMEOUT, secrets::load())
        .await
        .map_err(|_| failure(FailureKind::KeyringUnavailable, true, true))?
        .map_err(|_| failure(FailureKind::KeyringUnavailable, true, true))?
    else {
        if restore {
            let _ = tx.send(WorkerEvent::NoStoredAccount { id });
            return Ok(());
        }
        return Err(failure(FailureKind::AuthorizationExpired, false, true));
    };
    let cached = cache_blocking(cache_io.clone(), || {
        let latest = cache::load_latest(cache::load_limit());
        let usage = cache::usage();
        (latest, usage)
    })
    .await;
    emit_phase(tx, id, WorkerPhase::LoadingConfiguration);
    let config = config::load().map_err(|error| map_config(error, true))?;
    let client = oauth::http_client().map_err(map_oauth)?;
    emit_phase(tx, id, WorkerPhase::RefreshingToken);
    let grant = match oauth::refresh(&config, token.expose(), &client).await {
        Ok(grant) => grant,
        Err(oauth::OAuthError::AuthorizationExpired) => {
            emit_phase(tx, id, WorkerPhase::OpeningKeyring);
            let cleanup_failed = !matches!(
                timeout(KEYRING_TIMEOUT, secrets::delete()).await,
                Ok(Ok(()))
            );
            return Err(if cleanup_failed {
                ServiceFailure {
                    kind: FailureKind::DisconnectFailed,
                    retryable: true,
                    preserve_mail: true,
                    cleanup_failed: true,
                    config_path: None,
                }
            } else {
                ServiceFailure {
                    kind: FailureKind::AuthorizationExpired,
                    retryable: false,
                    preserve_mail: true,
                    cleanup_failed: false,
                    config_path: None,
                }
            });
        }
        Err(error) => return Err(map_oauth(error)),
    };
    if let Some(replacement) = grant.refresh_token.as_ref() {
        let token = RefreshToken::new(replacement.expose().to_owned())
            .map_err(|_| failure(FailureKind::CredentialSaveFailed, true, true))?;
        save_with_cleanup(
            cleanup_required,
            true,
            || async {
                matches!(
                    timeout(KEYRING_TIMEOUT, secrets::replace(&token)).await,
                    Ok(Ok(()))
                )
            },
            || async {
                matches!(
                    timeout(KEYRING_TIMEOUT, secrets::delete()).await,
                    Ok(Ok(()))
                )
            },
        )
        .await?;
    }
    emit_phase(tx, id, WorkerPhase::VerifyingIdentity);
    let account = oauth::fetch_identity(&grant.access_token, &client)
        .await
        .map_err(map_oauth)?;
    cache_blocking(cache_io.clone(), {
        let email = account.email.clone();
        move || cache::prepare_verified_account(&email)
    })
    .await
    .map_err(|_| failure(FailureKind::WorkerUnavailable, false, false))?
    .map_err(|_| failure(FailureKind::WorkerUnavailable, true, false))?;
    let _ = tx.send(WorkerEvent::IdentityVerified {
        id,
        account: account.clone(),
    });
    if let Ok((Ok(Some((cached_account, snapshot))), usage)) = cached
        && cached_account.email.eq_ignore_ascii_case(&account.email)
    {
        let _ = tx.send(WorkerEvent::CacheLoaded {
            id,
            account: cached_account,
            snapshot,
        });
        if let Ok(usage) = usage {
            let _ = tx.send(WorkerEvent::CacheUsageChanged { usage });
        }
    }
    *auth.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(RuntimeAuth {
        email: account.email.clone(),
        access_token: Zeroizing::new(grant.access_token.expose().to_owned()),
    });
    sync(id, tx, account, grant.access_token.expose(), cache_io).await
}

async fn sync(
    id: OperationId,
    tx: &mpsc::UnboundedSender<WorkerEvent>,
    account: AccountIdentity,
    access_token: &str,
    cache_io: &Arc<Mutex<()>>,
) -> Result<(), ServiceFailure> {
    emit_phase(tx, id, WorkerPhase::ConnectingImap);
    let requested_limit = cache_blocking(cache_io.clone(), cache::load_limit)
        .await
        .map_err(|_| failure(FailureKind::WorkerUnavailable, false, true))?;
    let fetched = gmail::fetch_inbox(&account.email, access_token, requested_limit)
        .await
        .map_err(map_gmail)?;
    complete_sync(id, tx, account, fetched, cache_io).await
}

async fn complete_sync(
    id: OperationId,
    tx: &mpsc::UnboundedSender<WorkerEvent>,
    account: AccountIdentity,
    fetched: gmail::InboxFetch,
    cache_io: &Arc<Mutex<()>>,
) -> Result<(), ServiceFailure> {
    emit_phase(tx, id, WorkerPhase::FetchingInbox);
    let account_for_cache = account.clone();
    let result = cache_blocking(cache_io.clone(), move || {
        let folder_catalog = fetched.folder_catalog;
        let (messages, fallback_count) = message::map_summaries(fetched.records);
        let requested_limit = cache::load_limit();
        let fresh = MailboxSnapshot {
            folder_catalog,
            metadata: SyncMetadata {
                completed_at: SystemTime::now(),
                requested_limit,
                loaded_count: messages.len(),
                fallback_count,
                skipped_count: fetched.skipped_count,
            },
            messages,
        };
        let snapshot = cache::replace_and_save(&account_for_cache, fresh, requested_limit)?;
        let usage = cache::usage();
        Ok::<_, std::io::Error>((snapshot, usage))
    })
    .await
    .map_err(|_| failure(FailureKind::WorkerUnavailable, false, true))?;
    let (snapshot, usage) =
        result.map_err(|_| failure(FailureKind::WorkerUnavailable, true, true))?;
    let _ = tx.send(WorkerEvent::SyncComplete {
        id,
        account,
        snapshot,
    });
    if let Ok(usage) = usage {
        let _ = tx.send(WorkerEvent::CacheUsageChanged { usage });
    }
    Ok(())
}

async fn fetch_folder_view(
    request: FolderLoadRequest,
    tx: mpsc::UnboundedSender<WorkerEvent>,
    auth: Arc<Mutex<Option<RuntimeAuth>>>,
    cache_io: Arc<Mutex<()>>,
    generation_gate: Arc<std::sync::atomic::AtomicU64>,
    metadata_lane: Arc<tokio::sync::Mutex<()>>,
) {
    let FolderLoadRequest {
        request_id,
        generation,
        account_email,
        folder,
        catalog,
    } = request;
    let folder_id = folder.id.clone();
    let cache_account = account_email.clone();
    let cache_folder = folder_id.clone();
    let cached = cache_blocking(cache_io.clone(), move || {
        cache::load_folder(&cache_account, &cache_folder, cache::load_limit())
    })
    .await;
    if generation_gate.load(Ordering::Acquire) != generation {
        return;
    }
    if let Ok(Ok(Some(mut snapshot))) = cached {
        // Keep navigation on the catalog discovered by the current account
        // sync. A cached view may predate label additions or removals.
        snapshot.folder_catalog = catalog.clone();
        let _ = tx.send(WorkerEvent::FolderCacheLoaded {
            request_id,
            generation,
            folder_id: folder_id.clone(),
            snapshot,
        });
    }
    let credentials = auth
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .filter(|value| value.email.eq_ignore_ascii_case(&account_email))
        .map(|value| Zeroizing::new(value.access_token.as_str().to_owned()));
    let Some(access_token) = credentials else {
        let _ = tx.send(WorkerEvent::FolderFailed {
            request_id,
            generation,
            folder_id,
            failure: BodyFailure::AuthorizationRequired,
        });
        return;
    };
    let _lane = metadata_lane.lock().await;
    if generation_gate.load(Ordering::Acquire) != generation {
        return;
    }
    let limit = match cache_blocking(cache_io.clone(), cache::load_limit).await {
        Ok(limit) => limit,
        Err(_) => {
            let _ = tx.send(WorkerEvent::FolderFailed {
                request_id,
                generation,
                folder_id,
                failure: BodyFailure::Protocol,
            });
            return;
        }
    };
    let fetched =
        match gmail::fetch_folder(&account_email, access_token.as_str(), folder, limit).await {
            Ok(fetched) => fetched,
            Err(error) => {
                if generation_gate.load(Ordering::Acquire) == generation {
                    let _ = tx.send(WorkerEvent::FolderFailed {
                        request_id,
                        generation,
                        folder_id,
                        failure: map_body_failure(error),
                    });
                }
                return;
            }
        };
    let account = AccountIdentity {
        provider: MailProvider::Gmail,
        email: account_email,
    };
    let save_account = account.clone();
    let save_folder = folder_id.clone();
    let save_gate = generation_gate.clone();
    let saved = cache_blocking(cache_io, move || {
        if save_gate.load(Ordering::Acquire) != generation {
            return None;
        }
        let (messages, fallback_count) = message::map_summaries(fetched.records);
        let fresh = MailboxSnapshot {
            folder_catalog: catalog,
            metadata: SyncMetadata {
                completed_at: SystemTime::now(),
                requested_limit: limit,
                loaded_count: messages.len(),
                fallback_count,
                skipped_count: fetched.skipped_count,
            },
            messages,
        };
        Some(
            cache::replace_folder_and_save(&save_account, save_folder, fresh.clone(), limit)
                .unwrap_or(fresh),
        )
    })
    .await;
    if generation_gate.load(Ordering::Acquire) != generation {
        return;
    }
    match saved {
        Ok(Some(snapshot)) => {
            let _ = tx.send(WorkerEvent::FolderLoaded {
                request_id,
                generation,
                folder_id,
                snapshot,
            });
        }
        _ => {
            let _ = tx.send(WorkerEvent::FolderFailed {
                request_id,
                generation,
                folder_id,
                failure: BodyFailure::Protocol,
            });
        }
    }
}

async fn download_attachment(
    request: AttachmentLoadRequest,
    tx: mpsc::UnboundedSender<WorkerEvent>,
    auth: Arc<Mutex<Option<RuntimeAuth>>>,
    generation_gate: Arc<std::sync::atomic::AtomicU64>,
) {
    let AttachmentLoadRequest {
        job_id,
        generation,
        account_email,
        message_id,
        locator,
        attachment,
        destination,
    } = request;
    if generation_gate.load(Ordering::Acquire) != generation {
        return;
    }
    let (mut output, open) = match destination {
        AttachmentDestination::Open => {
            match cache::prepare_open_attachment(&account_email, &message_id, &attachment) {
                Ok(cache::OpenAttachmentTarget::Cached(path)) => {
                    let _ = cache::prune_attachment_cache(&path);
                    let _ = tx.send(WorkerEvent::AttachmentCompleted {
                        job_id,
                        generation,
                        path,
                        open: true,
                    });
                    return;
                }
                Ok(cache::OpenAttachmentTarget::Download(output)) => (output, true),
                Err(_) => {
                    let _ = tx.send(WorkerEvent::AttachmentFailed {
                        job_id,
                        generation,
                        failure: AttachmentFailure::Filesystem,
                    });
                    return;
                }
            }
        }
        AttachmentDestination::SaveAs(path) => match cache::prepare_save_attachment(&path) {
            Ok(output) => (output, false),
            Err(_) => {
                let _ = tx.send(WorkerEvent::AttachmentFailed {
                    job_id,
                    generation,
                    failure: AttachmentFailure::Filesystem,
                });
                return;
            }
        },
    };
    let credentials = auth
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .filter(|value| value.email.eq_ignore_ascii_case(&account_email))
        .map(|value| Zeroizing::new(value.access_token.as_str().to_owned()));
    let Some(access_token) = credentials else {
        let _ = tx.send(WorkerEvent::AttachmentFailed {
            job_id,
            generation,
            failure: AttachmentFailure::AuthorizationRequired,
        });
        return;
    };
    let progress_tx = tx.clone();
    let result = gmail::fetch_attachment(
        &account_email,
        access_token.as_str(),
        &message_id,
        &locator,
        &attachment,
        |bytes| output.write_all(bytes),
        |transferred, total| {
            if generation_gate.load(Ordering::Acquire) == generation {
                let _ = progress_tx.send(WorkerEvent::AttachmentProgress {
                    job_id,
                    generation,
                    transferred,
                    total,
                });
            }
        },
    )
    .await;
    if generation_gate.load(Ordering::Acquire) != generation {
        return;
    }
    if let Err(error) = result {
        let _ = tx.send(WorkerEvent::AttachmentFailed {
            job_id,
            generation,
            failure: map_attachment_failure(error),
        });
        return;
    }
    match output.finish() {
        Ok(path) => {
            if open {
                let _ = cache::prune_attachment_cache(&path);
            }
            let _ = tx.send(WorkerEvent::AttachmentCompleted {
                job_id,
                generation,
                path,
                open,
            });
            if open && let Ok(usage) = cache::usage() {
                let _ = tx.send(WorkerEvent::CacheUsageChanged { usage });
            }
        }
        Err(_) => {
            let _ = tx.send(WorkerEvent::AttachmentFailed {
                job_id,
                generation,
                failure: AttachmentFailure::Filesystem,
            });
        }
    }
}

fn map_attachment_failure(error: gmail::AttachmentFetchError) -> AttachmentFailure {
    match error {
        gmail::AttachmentFetchError::TooLarge => AttachmentFailure::TooLarge,
        gmail::AttachmentFetchError::InvalidEncoding => AttachmentFailure::UnsupportedEncoding,
        gmail::AttachmentFetchError::WriteFailed => AttachmentFailure::Filesystem,
        gmail::AttachmentFetchError::Gmail(gmail::GmailError::Offline) => {
            AttachmentFailure::Offline
        }
        gmail::AttachmentFetchError::Gmail(gmail::GmailError::AuthenticationFailed) => {
            AttachmentFailure::AuthorizationRequired
        }
        gmail::AttachmentFetchError::Gmail(gmail::GmailError::TimedOut) => {
            AttachmentFailure::TimedOut
        }
        gmail::AttachmentFetchError::Gmail(_) => AttachmentFailure::Protocol,
    }
}

async fn fetch_body(
    request: BodyLoadRequest,
    tx: &mpsc::UnboundedSender<WorkerEvent>,
    auth: &Arc<Mutex<Option<RuntimeAuth>>>,
    cache: BodyCacheServices,
) {
    let BodyLoadRequest {
        request_id,
        generation,
        account_email,
        message_id,
        locator,
    } = request;
    let cache_generation = cache.generation;
    let cache_io = cache.io;
    let load_account = account_email.clone();
    let load_id = message_id.clone();
    let loaded = cache_blocking(cache_io.clone(), move || {
        let body = cache::load_body(&load_account, &load_id);
        let usage = body
            .as_ref()
            .ok()
            .and_then(|body| body.as_ref())
            .map(|_| cache::usage().unwrap_or_default());
        (body, usage)
    })
    .await;
    if let Ok((Ok(Some(body)), usage)) = loaded {
        if cache_generation.load(Ordering::Acquire) == generation {
            let _ = tx.send(WorkerEvent::BodyLoaded {
                request_id,
                generation,
                message_id,
                body: Arc::new(body),
                usage: usage.unwrap_or_default(),
                saved: true,
            });
        }
        return;
    }
    if message_id.gmail_value().is_none() || !locator.is_valid() {
        send_body_failure(
            tx,
            request_id,
            generation,
            message_id,
            BodyFailure::Protocol,
        );
        return;
    }
    let credentials = auth
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .map(|value| {
            (
                value.email.clone(),
                Zeroizing::new(value.access_token.as_str().to_owned()),
            )
        });
    let Some((email, access_token)) = credentials else {
        send_body_failure(
            tx,
            request_id,
            generation,
            message_id,
            BodyFailure::AuthorizationRequired,
        );
        return;
    };
    if !email.eq_ignore_ascii_case(&account_email) {
        send_body_failure(
            tx,
            request_id,
            generation,
            message_id,
            BodyFailure::AuthorizationRequired,
        );
        return;
    }
    let raw = match gmail::fetch_body(&email, access_token.as_str(), &message_id, &locator).await {
        Ok(raw) => raw,
        Err(error) => {
            send_body_failure(
                tx,
                request_id,
                generation,
                message_id,
                map_body_failure(error),
            );
            return;
        }
    };
    if cache_generation.load(Ordering::Acquire) != generation {
        return;
    }
    let body = message::map_body(raw);
    let save_account = account_email.clone();
    let save_id = message_id.clone();
    let save_generation = cache_generation.clone();
    let saved = cache_blocking(cache_io, move || {
        if save_generation.load(Ordering::Acquire) != generation {
            return None;
        }
        let saved_usage = cache::save_body(&save_account, &save_id, &body);
        let (usage, saved) = match saved_usage {
            Ok(usage) => (usage, true),
            Err(_) => (cache::usage().unwrap_or_default(), false),
        };
        Some((body, usage, saved))
    })
    .await;
    if cache_generation.load(Ordering::Acquire) != generation {
        return;
    }
    let Ok(Some((body, usage, saved))) = saved else {
        return;
    };
    let _ = tx.send(WorkerEvent::BodyLoaded {
        request_id,
        generation,
        message_id,
        body: Arc::new(body),
        usage,
        saved,
    });
}

fn send_body_failure(
    tx: &mpsc::UnboundedSender<WorkerEvent>,
    request_id: BodyRequestId,
    generation: u64,
    message_id: MessageId,
    failure: BodyFailure,
) {
    let _ = tx.send(WorkerEvent::BodyFailed {
        request_id,
        generation,
        message_id,
        failure,
    });
}

fn map_body_failure(error: gmail::GmailError) -> BodyFailure {
    match error {
        gmail::GmailError::Offline | gmail::GmailError::TlsFailed => BodyFailure::Offline,
        gmail::GmailError::TimedOut => BodyFailure::TimedOut,
        gmail::GmailError::AuthenticationFailed => BodyFailure::AuthorizationRequired,
        gmail::GmailError::MailboxChanged => BodyFailure::MailboxChanged,
        gmail::GmailError::MessageMissing => BodyFailure::Missing,
        gmail::GmailError::InboxUnavailable | gmail::GmailError::Protocol => BodyFailure::Protocol,
    }
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

fn emit_phase(tx: &mpsc::UnboundedSender<WorkerEvent>, id: OperationId, phase: WorkerPhase) {
    let _ = tx.send(WorkerEvent::Phase { id, phase });
}
fn failure(kind: FailureKind, retryable: bool, preserve_mail: bool) -> ServiceFailure {
    ServiceFailure {
        kind,
        retryable,
        preserve_mail,
        cleanup_failed: false,
        config_path: None,
    }
}

fn map_config(error: config::ConfigLoadError, preserve_mail: bool) -> ServiceFailure {
    let kind = match error.kind {
        config::ConfigError::DirectoryUnavailable => FailureKind::ConfigurationDirectoryUnavailable,
        config::ConfigError::Missing => FailureKind::ConfigurationMissing,
        config::ConfigError::Unreadable => FailureKind::ConfigurationUnreadable,
        config::ConfigError::TooLarge => FailureKind::ConfigurationTooLarge,
        config::ConfigError::Invalid => FailureKind::ConfigurationInvalid,
        config::ConfigError::WrongProject => FailureKind::ConfigurationWrongProject,
    };
    ServiceFailure {
        kind,
        retryable: true,
        preserve_mail,
        cleanup_failed: false,
        config_path: error.path.map(|path| path.to_string_lossy().into_owned()),
    }
}
fn fail(
    tx: &mpsc::UnboundedSender<WorkerEvent>,
    id: OperationId,
    kind: FailureKind,
    retryable: bool,
    preserve_mail: bool,
) {
    let _ = tx.send(WorkerEvent::Failed {
        id,
        failure: failure(kind, retryable, preserve_mail),
    });
}
fn fail_cleanup(tx: &mpsc::UnboundedSender<WorkerEvent>, id: OperationId, preserve_mail: bool) {
    let _ = tx.send(WorkerEvent::Failed {
        id,
        failure: ServiceFailure {
            kind: FailureKind::DisconnectFailed,
            retryable: true,
            preserve_mail,
            cleanup_failed: true,
            config_path: None,
        },
    });
}
fn map_oauth(error: oauth::OAuthError) -> ServiceFailure {
    let kind = match error {
        oauth::OAuthError::AuthorizationDenied => FailureKind::AuthorizationDenied,
        oauth::OAuthError::AuthorizationTimedOut => FailureKind::AuthorizationTimedOut,
        oauth::OAuthError::StateMismatch | oauth::OAuthError::CallbackInvalid => {
            FailureKind::AuthorizationInvalid
        }
        oauth::OAuthError::Network => FailureKind::Network,
        oauth::OAuthError::ProviderUnavailable => FailureKind::ProviderUnavailable,
        oauth::OAuthError::RateLimited => FailureKind::RateLimited,
        oauth::OAuthError::AuthorizationExpired => FailureKind::AuthorizationExpired,
        oauth::OAuthError::IdentityInvalid => FailureKind::IdentityInvalid,
        _ => FailureKind::AuthorizationInvalid,
    };
    failure(
        kind,
        !matches!(kind, FailureKind::AuthorizationDenied),
        true,
    )
}
fn map_gmail(error: gmail::GmailError) -> ServiceFailure {
    let kind = match error {
        gmail::GmailError::Offline => FailureKind::Network,
        gmail::GmailError::TlsFailed => FailureKind::TlsFailed,
        gmail::GmailError::AuthenticationFailed => FailureKind::ImapAuthenticationFailed,
        gmail::GmailError::InboxUnavailable => FailureKind::InboxUnavailable,
        gmail::GmailError::MailboxChanged | gmail::GmailError::MessageMissing => {
            FailureKind::ImapProtocol
        }
        gmail::GmailError::Protocol => FailureKind::ImapProtocol,
        gmail::GmailError::TimedOut => FailureKind::SyncTimedOut,
    };
    failure(kind, true, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    #[test]
    fn debug_contract_redacts_payloads() {
        let command = WorkerCommand::Refresh { id: OperationId(3) };
        assert!(!format!("{command:?}").contains("token"));
        let body_command = WorkerCommand::FetchBody {
            request_id: BodyRequestId(4),
            generation: 1,
            account_email: "canary@example.com".into(),
            message_id: MessageId::gmail(9),
            locator: MessageLocator {
                folder_id: crate::model::FolderId::Inbox,
                mailbox: "INBOX".into(),
                uid_validity: 7,
                uid: 9,
            },
        };
        assert!(!format!("{body_command:?}").contains("canary@example.com"));
        let folder_command = WorkerCommand::FetchFolder {
            request_id: FolderRequestId(8),
            generation: 2,
            account_email: "folder-canary@example.com".into(),
            folder: FolderDescriptor {
                id: FolderId::Label("Secret label".into()),
                mailbox: "Secret label".into(),
                display_name: "Secret label".into(),
                kind: crate::model::FolderKind::Label,
            },
            catalog: FolderCatalog::inbox_only(),
        };
        let folder_debug = format!("{folder_command:?}");
        assert_eq!(command_id(&folder_command), None);
        assert!(!folder_debug.contains("folder-canary@example.com"));
        assert!(!folder_debug.contains("Secret label"));
        let search_command = WorkerCommand::SearchGmail {
            request_id: SearchRequestId(11),
            generation: 4,
            account_email: "search-canary@example.com".into(),
            folder: FolderDescriptor {
                id: FolderId::AllMail,
                mailbox: "[Gmail]/All Mail".into(),
                display_name: "All Mail".into(),
                kind: crate::model::FolderKind::AllMail,
            },
            query: "from:top-secret@example.com".into(),
        };
        let search_debug = format!("{search_command:?}");
        assert_eq!(command_id(&search_command), None);
        assert!(search_debug.contains("query_bytes"));
        assert!(!search_debug.contains("top-secret"));
        assert!(!search_debug.contains("search-canary"));
        let attachment_command = WorkerCommand::DownloadAttachment {
            job_id: AttachmentJobId(2),
            generation: 1,
            account_email: "attachment-canary@example.com".into(),
            message_id: MessageId::gmail(42),
            locator: MessageLocator {
                folder_id: FolderId::Inbox,
                mailbox: "INBOX".into(),
                uid_validity: 7,
                uid: 9,
            },
            attachment: Box::new(crate::model::Attachment {
                name: "secret-report.pdf".into(),
                media_type: Some("application/pdf".into()),
                octets: Some(12),
                part: crate::model::MimePartDescriptor {
                    path: vec![2],
                    encoding: crate::model::TransferEncoding::Base64,
                    encoded_octets: 12,
                },
            }),
            destination: AttachmentDestination::SaveAs(
                "/private/location/secret-report.pdf".into(),
            ),
        };
        let attachment_debug = format!("{attachment_command:?}");
        assert_eq!(command_id(&attachment_command), None);
        assert!(!attachment_debug.contains("attachment-canary"));
        assert!(!attachment_debug.contains("secret-report"));
        assert!(!attachment_debug.contains("/private"));
        let draft = crate::composer::new_message(
            "draft-canary".into(),
            "canary@example.com",
            "<b>secret body</b>",
        )
        .unwrap();
        let send_command = WorkerCommand::SendMessage {
            request_id: SendRequestId(5),
            generation: 1,
            submission: Box::new(ComposeSubmission { draft }),
        };
        let debug = format!("{send_command:?}");
        assert!(!debug.contains("canary@example.com"));
        assert!(!debug.contains("secret body"));
        let event = WorkerEvent::Failed {
            id: OperationId(3),
            failure: failure(FailureKind::Network, true, true),
        };
        assert!(!format!("{event:?}").contains("canary@example.com"));
    }
    #[test]
    fn configuration_failures_keep_category_and_safe_path() {
        let mapped = map_config(
            config::ConfigLoadError {
                kind: config::ConfigError::WrongProject,
                path: Some("/home/person/.config/whitford/google-oauth.json".into()),
            },
            true,
        );
        assert_eq!(mapped.kind, FailureKind::ConfigurationWrongProject);
        assert!(mapped.retryable);
        assert!(mapped.preserve_mail);
        assert_eq!(
            mapped.config_path.as_deref(),
            Some("/home/person/.config/whitford/google-oauth.json")
        );
        assert!(!format!("{mapped:?}").contains("/home/person"));
    }
    #[test]
    fn callback_success_content_length_matches_body() {
        let header = callback_success_header();
        let length = header
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .unwrap()
            .parse::<usize>()
            .unwrap();
        assert_eq!(length, CALLBACK_SUCCESS_BODY.len());
    }
    #[test]
    fn callback_reader_supports_fragments_exact_limit_and_timeout() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (mut client, mut server) = tokio::io::duplex(16_384);
            let writer = tokio::spawn(async move {
                for part in [
                    b"GET /oauth/callback?code=a".as_slice(),
                    b"&state=s HTTP/1.1\r\nHost: 127.0.0.1:1\r\n",
                    b"\r\n",
                ] {
                    server.write_all(part).await.unwrap();
                }
            });
            let request = read_callback_headers(
                &mut client,
                Instant::now() + std::time::Duration::from_secs(1),
            )
            .await
            .unwrap();
            writer.await.unwrap();
            assert!(request.ends_with(b"\r\n\r\n"));

            let base =
                b"GET /oauth/callback?code=a&state=s HTTP/1.1\r\nHost: 127.0.0.1:1\r\nX-Pad: ";
            let mut exact = base.to_vec();
            exact.extend(std::iter::repeat_n(
                b'x',
                MAX_CALLBACK_HEADERS - base.len() - 4,
            ));
            exact.extend_from_slice(b"\r\n\r\n");
            let (mut reader, mut writer) = tokio::io::duplex(16_384);
            writer.write_all(&exact).await.unwrap();
            assert_eq!(
                read_callback_headers(
                    &mut reader,
                    Instant::now() + std::time::Duration::from_secs(1)
                )
                .await
                .unwrap()
                .len(),
                MAX_CALLBACK_HEADERS
            );
            let (mut reader, mut writer) = tokio::io::duplex(16_384);
            writer
                .write_all(&vec![b'x'; MAX_CALLBACK_HEADERS + 1])
                .await
                .unwrap();
            assert_eq!(
                read_callback_headers(
                    &mut reader,
                    Instant::now() + std::time::Duration::from_secs(1)
                )
                .await
                .unwrap_err(),
                oauth::OAuthError::CallbackInvalid
            );
            let (mut reader, _writer) = tokio::io::duplex(16);
            assert_eq!(
                read_callback_headers(
                    &mut reader,
                    Instant::now() + std::time::Duration::from_millis(1)
                )
                .await
                .unwrap_err(),
                oauth::OAuthError::AuthorizationTimedOut
            );
        });
    }

    #[test]
    fn first_connect_and_rotation_persist_before_notification_and_imap() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
            let save_calls = calls.clone();
            let notify_calls = calls.clone();
            let fetch_calls = calls.clone();
            let result = persist_then_fetch(
                move || async move {
                    save_calls.lock().unwrap().push("persist");
                    Ok(())
                },
                move || notify_calls.lock().unwrap().push("account-persisted"),
                move || async move {
                    fetch_calls.lock().unwrap().push("imap");
                    Ok(gmail::InboxFetch {
                        records: vec![],
                        skipped_count: 0,
                        folder_catalog: crate::model::FolderCatalog::inbox_only(),
                    })
                },
            )
            .await
            .unwrap();
            assert!(result.records.is_empty());
            assert_eq!(
                *calls.lock().unwrap(),
                ["persist", "account-persisted", "imap"]
            );
        });
    }

    #[test]
    fn credential_save_and_boundary_failure_short_circuit_imap() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let fetched = Arc::new(AtomicBool::new(false));
            let fetched_call = fetched.clone();
            let result = persist_then_fetch(
                || async { Err(failure(FailureKind::CredentialSaveFailed, true, false)) },
                || panic!("must not notify"),
                move || async move {
                    fetched_call.store(true, Ordering::Release);
                    Ok(gmail::InboxFetch {
                        records: vec![],
                        skipped_count: 0,
                        folder_catalog: crate::model::FolderCatalog::inbox_only(),
                    })
                },
            )
            .await;
            assert_eq!(result.unwrap_err().kind, FailureKind::CredentialSaveFailed);
            assert!(!fetched.load(Ordering::Acquire));
        });
    }

    #[test]
    fn indeterminate_save_requires_confirmed_cleanup() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let cleanup_required = AtomicBool::new(false);
            let partial_save = Arc::new(AtomicBool::new(false));
            let partial_save_call = partial_save.clone();
            let result = save_with_cleanup(
                &cleanup_required,
                false,
                move || async move {
                    partial_save_call.store(true, Ordering::Release);
                    false
                },
                || async { true },
            )
            .await;
            assert!(partial_save.load(Ordering::Acquire));
            assert_eq!(result.unwrap_err().kind, FailureKind::CredentialSaveFailed);
            assert!(!cleanup_required.load(Ordering::Acquire));

            let result = save_with_cleanup(
                &cleanup_required,
                true,
                || async { false },
                || async { false },
            )
            .await;
            let failure = result.unwrap_err();
            assert_eq!(failure.kind, FailureKind::DisconnectFailed);
            assert!(failure.cleanup_failed);
            assert!(failure.preserve_mail);
            assert!(cleanup_required.load(Ordering::Acquire));
        });
    }

    #[test]
    fn controller_cancel_shutdown_and_channel_close_terminate_cleanly() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (command_tx, command_rx) = mpsc::unbounded_channel();
            let (event_tx, mut event_rx) = mpsc::unbounded_channel();
            let task = tokio::spawn(controller(command_rx, event_tx));
            command_tx
                .send(WorkerCommand::Cancel { id: OperationId(9) })
                .unwrap();
            assert!(matches!(
                event_rx.recv().await,
                Some(WorkerEvent::Cancelled { id: OperationId(9) })
            ));
            command_tx.send(WorkerCommand::Shutdown).unwrap();
            task.await.unwrap();
            assert!(event_rx.recv().await.is_none());

            let (command_tx, command_rx) = mpsc::unbounded_channel();
            let (event_tx, _event_rx) = mpsc::unbounded_channel();
            let task = tokio::spawn(controller(command_rx, event_tx));
            drop(command_tx);
            task.await.unwrap();
        });
    }

    #[test]
    fn lifecycle_owner_sends_shutdown_and_joins_worker_thread() {
        let (worker, mut events) = WorkerHandle::start();
        worker
            .sender
            .send(WorkerCommand::Cancel {
                id: OperationId(17),
            })
            .unwrap();
        assert!(matches!(
            events.blocking_recv(),
            Some(WorkerEvent::Cancelled {
                id: OperationId(17)
            })
        ));

        worker.shutdown_and_join().unwrap();
        assert!(events.blocking_recv().is_none());
    }
    #[test]
    fn cancellation_cleanup_is_stage_aware_and_reports_cleanup_failure() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let waiting = AtomicBool::new(false);
            let called = Arc::new(AtomicBool::new(false));
            let called_cleanup = called.clone();
            let task = tokio::spawn(std::future::pending::<()>());
            assert!(
                !abort_with_cleanup(task, &waiting, async move {
                    called_cleanup.store(true, Ordering::Release);
                    true
                })
                .await
            );
            assert!(!called.load(Ordering::Acquire));

            let persisted = AtomicBool::new(true);
            let task = tokio::spawn(std::future::pending::<()>());
            assert!(!abort_with_cleanup(task, &persisted, async { true }).await);
            assert!(!persisted.load(Ordering::Acquire));
            persisted.store(true, Ordering::Release);
            let task = tokio::spawn(std::future::pending::<()>());
            assert!(abort_with_cleanup(task, &persisted, async { false }).await);
            assert!(persisted.load(Ordering::Acquire));
        });
    }
    #[test]
    fn disconnect_cleanup_deletes_token_only_after_local_data() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let local_calls = calls.clone();
            let token_calls = calls.clone();
            assert!(
                cleanup_token_last(
                    async move {
                        local_calls.lock().unwrap().push("local");
                        true
                    },
                    async move {
                        token_calls.lock().unwrap().push("token");
                        true
                    },
                )
                .await
            );
            assert_eq!(*calls.lock().unwrap(), ["local", "token"]);

            let calls = Arc::new(Mutex::new(Vec::new()));
            let local_calls = calls.clone();
            let token_calls = calls.clone();
            assert!(
                !cleanup_token_last(
                    async move {
                        local_calls.lock().unwrap().push("local");
                        false
                    },
                    async move {
                        token_calls.lock().unwrap().push("token");
                        true
                    },
                )
                .await
            );
            assert_eq!(*calls.lock().unwrap(), ["local"]);

            let calls = Arc::new(Mutex::new(Vec::new()));
            let first_local = calls.clone();
            let first_token = calls.clone();
            assert!(
                !cleanup_token_last(
                    async move {
                        first_local.lock().unwrap().push("local-failed");
                        false
                    },
                    async move {
                        first_token.lock().unwrap().push("token-too-early");
                        true
                    },
                )
                .await
            );
            let retry_local = calls.clone();
            let retry_token = calls.clone();
            assert!(
                cleanup_token_last(
                    async move {
                        retry_local.lock().unwrap().push("local-retried");
                        true
                    },
                    async move {
                        retry_token.lock().unwrap().push("token");
                        true
                    },
                )
                .await
            );
            assert_eq!(
                *calls.lock().unwrap(),
                ["local-failed", "local-retried", "token"]
            );

            let calls = Arc::new(Mutex::new(Vec::new()));
            let draft_calls = calls.clone();
            let cache_calls = calls.clone();
            let token_calls = calls.clone();
            assert!(
                !cleanup_account_token_last(
                    async move {
                        draft_calls.lock().unwrap().push("drafts");
                        true
                    },
                    async move {
                        cache_calls.lock().unwrap().push("cache-failed");
                        false
                    },
                    async move {
                        token_calls.lock().unwrap().push("token-too-early");
                        true
                    },
                )
                .await
            );
            let retry_drafts = calls.clone();
            let retry_cache = calls.clone();
            let retry_token = calls.clone();
            assert!(
                cleanup_account_token_last(
                    async move {
                        retry_drafts.lock().unwrap().push("drafts-retried");
                        true
                    },
                    async move {
                        retry_cache.lock().unwrap().push("cache-retried");
                        true
                    },
                    async move {
                        retry_token.lock().unwrap().push("token");
                        true
                    },
                )
                .await
            );
            assert_eq!(
                *calls.lock().unwrap(),
                [
                    "drafts",
                    "cache-failed",
                    "drafts-retried",
                    "cache-retried",
                    "token"
                ]
            );
        });
    }
    #[test]
    fn mutation_barrier_settles_every_prior_command_before_account_boundary() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (command_tx, command_rx) = mpsc::unbounded_channel();
            let (event_tx, mut event_rx) = mpsc::unbounded_channel();
            let generation = Arc::new(std::sync::atomic::AtomicU64::new(7));
            let actor = tokio::spawn(mutation_actor(
                command_rx,
                event_tx,
                Arc::new(Mutex::new(None)),
                Arc::new(Mutex::new(())),
                generation,
                Arc::new(tokio::sync::Mutex::new(())),
            ));
            command_tx
                .send(MutationActorCommand::Mutate(Box::new(MutationIo {
                    request_id: MutationRequestId(1),
                    generation: 7,
                    account_email: "person@example.com".into(),
                    message_id: MessageId::gmail(9),
                    locator: MessageLocator {
                        folder_id: FolderId::Inbox,
                        mailbox: "INBOX".into(),
                        uid_validity: 4,
                        uid: 9,
                    },
                    catalog: FolderCatalog::inbox_only(),
                    mutation: MessageMutation::SetStarred(true),
                })))
                .unwrap();

            assert!(drain_mutations(&command_tx).await);
            assert!(matches!(
                event_rx.try_recv(),
                Ok(WorkerEvent::MutationFailed {
                    request_id: MutationRequestId(1),
                    uncertain: false,
                    ..
                })
            ));
            drop(command_tx);
            actor.await.unwrap();
        });
    }
    #[test]
    fn panicked_worker_task_is_classified_as_unavailable() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let result = tokio::spawn(async { panic!("test panic") }).await;
            assert!(join_failed(&result));
        });
    }

    #[test]
    fn panicked_smtp_task_finishes_as_delivery_uncertain() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(guard_smtp_send(async {
            panic!("test SMTP panic");
            #[allow(unreachable_code)]
            Ok(())
        }));
        assert_eq!(result, Err(smtp::SmtpError::DeliveryUncertain));
    }

    #[test]
    fn mutation_commands_are_non_interrupting_and_redact_user_labels() {
        let command = WorkerCommand::MutateMessage {
            request_id: MutationRequestId(9),
            generation: 3,
            account_email: "private@example.com".into(),
            message_id: MessageId::gmail(42),
            locator: MessageLocator {
                folder_id: crate::model::FolderId::Inbox,
                mailbox: "INBOX".into(),
                uid_validity: 7,
                uid: 11,
            },
            mutation: MessageMutation::SetLabel {
                mailbox: "Private project".into(),
                applied: true,
            },
        };
        assert_eq!(command_id(&command), None);
        let debug = format!("{command:?}");
        assert!(debug.contains("label"));
        assert!(!debug.contains("Private project"));
        assert!(!debug.contains("private@example.com"));
        assert!(!debug.contains("gmail-msg"));
    }

    #[test]
    fn catalog_mutation_command_redacts_catalog_and_keeps_special_use_target() {
        let locator = MessageLocator {
            folder_id: FolderId::AllMail,
            mailbox: "[Gmail]/All Mail".into(),
            uid_validity: 7,
            uid: 11,
        };
        let catalog = legacy_mutation_catalog(
            &locator,
            &MessageMutation::MoveToTrash {
                mailbox: "[Gmail]/Trash".into(),
            },
        );
        assert_eq!(
            catalog
                .find(&FolderId::Trash)
                .map(|folder| folder.mailbox.as_str()),
            Some("[Gmail]/Trash")
        );
        let command = WorkerCommand::MutateMessageInCatalog {
            request_id: MutationRequestId(10),
            generation: 3,
            account_email: "private@example.com".into(),
            message_id: MessageId::gmail(42),
            locator,
            catalog,
            mutation: MessageMutation::MoveToTrash {
                mailbox: "[Gmail]/Trash".into(),
            },
        };
        let debug = format!("{command:?}");
        assert!(debug.contains("trash"));
        assert!(!debug.contains("private@example.com"));
        assert!(!debug.contains("[Gmail]/Trash"));
    }
}
