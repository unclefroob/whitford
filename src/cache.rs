use crate::model::{
    AccountId, AccountIdentity, AccountRecord, AccountRegistry, AccountRegistryError, Attachment,
    CacheUsage, FolderCatalog, FolderId, MailProvider, MailboxSnapshot, MessageBody, MessageId,
    MessageMutation, MessageSummary, ReconciledMessageState, SyncMetadata,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    env, fs,
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub const RETENTION_OPTIONS: [usize; 4] = [50, 100, 250, 500];
pub const DEFAULT_RETENTION: usize = 100;
pub const BODY_CACHE_BUDGET_BYTES: u64 = 128 * 1024 * 1024;
pub const ATTACHMENT_CACHE_BUDGET_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_CACHED_FOLDER_VIEWS: usize = 8;
pub const MAX_SUMMARIES_PER_FOLDER: usize = 500;
const MAX_CACHE_JSON_BYTES: u64 = 128 * 1024 * 1024;
const MAILBOX_VERSION: u8 = 4;
const PREVIOUS_MAILBOX_VERSION: u8 = 3;
const BODY_VERSION: u8 = 5;
const MANIFEST_VERSION: u8 = 3;
const ACCOUNT_CLEANUP_VERSION: u8 = 1;
const PREFERENCES_VERSION: u8 = 1;
const ACCOUNT_REGISTRY_VERSION: u8 = 1;
const LEGACY_CACHE_MIGRATION_VERSION: u8 = 1;
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

/// A physically separate, account-owned cache tree. Its path is constructed
/// only from a validated opaque AccountId, never an email address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountCacheNamespace {
    pub account_id: AccountId,
    pub root: PathBuf,
}

impl AccountCacheNamespace {
    pub fn mailbox_path(&self) -> PathBuf {
        mailbox_path(&self.root)
    }

    pub fn bodies_path(&self) -> PathBuf {
        bodies_path(&self.root)
    }

    pub fn attachments_path(&self) -> PathBuf {
        attachments_path(&self.root)
    }
}

#[derive(Deserialize, Serialize)]
struct StoredAccountRegistry {
    version: u8,
    registry: AccountRegistry,
}

#[derive(Deserialize, Serialize)]
struct PendingLegacyCacheMigration {
    version: u8,
    account_id: AccountId,
}

/// The application's colour-scheme choice. `System` deliberately maps to
/// libadwaita's default scheme at the UI boundary, so desktop theme changes
/// remain live.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum AppearancePreference {
    #[default]
    System,
    Light,
    Dark,
}

/// The complete, private preferences snapshot persisted in preferences.json.
///
/// Callers must save this whole value rather than individual fields. That
/// prevents a retention change and an appearance change from overwriting one
/// another when they are queued close together.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Preferences {
    #[serde(default)]
    version: u8,
    pub retained_messages: usize,
    #[serde(default, deserialize_with = "deserialize_appearance_preference")]
    pub appearance: AppearancePreference,
}

impl Preferences {
    pub fn new(retained_messages: usize, appearance: AppearancePreference) -> Self {
        Self {
            version: PREFERENCES_VERSION,
            retained_messages,
            appearance,
        }
    }
}

impl Default for Preferences {
    fn default() -> Self {
        Self::new(DEFAULT_RETENTION, AppearancePreference::System)
    }
}

#[derive(Deserialize, Serialize)]
struct StoredMailbox {
    version: u8,
    account_email: String,
    folder_catalog: FolderCatalog,
    views: Vec<StoredFolderView>,
    /// v3 caches deliberately deserialize as uninitialized.  The first successful
    /// Inbox write then becomes a baseline rather than announcing historic mail.
    #[serde(default)]
    inbox_watermark: InboxWatermark,
}

/// Durable, bounded Inbox delivery baseline.  This is intentionally identities only:
/// neither subjects nor addresses are needed to decide whether to notify.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct InboxWatermark {
    initialized: bool,
    observed_ids: Vec<MessageId>,
}

/// Result of atomically replacing the Inbox cache and advancing its notification
/// baseline.  The worker owns turning `new_unread_ids` into a background event.
#[derive(Clone, Debug)]
pub struct BackgroundInboxCommit {
    pub snapshot: MailboxSnapshot,
    pub new_unread_ids: Vec<MessageId>,
}

#[derive(Clone, Deserialize, Serialize)]
struct StoredFolderView {
    folder_id: FolderId,
    completed_at_unix: u64,
    requested_limit: usize,
    skipped_count: usize,
    last_accessed_unix: u64,
    messages: Vec<MessageSummary>,
}

#[derive(Deserialize)]
struct StoredBody {
    version: u8,
    account_email: String,
    id: MessageId,
    body: MessageBody,
}

#[derive(Serialize)]
struct StoredBodyRef<'a> {
    version: u8,
    account_email: &'a str,
    id: &'a MessageId,
    body: &'a MessageBody,
}

#[derive(Clone, Deserialize, Serialize)]
struct ManifestEntry {
    account_email: String,
    id: MessageId,
    file_name: String,
    bytes: u64,
    last_accessed_unix: u64,
}

#[derive(Deserialize, Serialize)]
struct BodyManifest {
    version: u8,
    entries: Vec<ManifestEntry>,
}

#[derive(Serialize)]
struct PendingAccountCleanup<'a> {
    version: u8,
    previous_account_email: &'a str,
    next_account_email: &'a str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccountCleanupStage {
    MailboxIndexes,
    BodyCaches,
    AttachmentCache,
    Marker,
}

pub fn is_valid_limit(limit: usize) -> bool {
    RETENTION_OPTIONS.contains(&limit)
}

pub fn selected_index(limit: usize) -> u32 {
    RETENTION_OPTIONS
        .iter()
        .position(|candidate| *candidate == limit)
        .unwrap_or(1) as u32
}

pub fn limit_at(index: u32) -> Option<usize> {
    RETENTION_OPTIONS.get(index as usize).copied()
}

pub fn load_limit() -> usize {
    load_preferences().retained_messages
}

pub fn save_limit_and_prune(limit: usize) -> io::Result<()> {
    let mut preferences = load_preferences();
    preferences.retained_messages = limit;
    save_preferences_and_prune(preferences)
}

/// Loads the complete preferences snapshot. A missing, corrupt, unsupported,
/// or invalid-retention record falls back to safe defaults. An old
/// retention-only record is migrated in memory by defaulting appearance to
/// `System`; it is written in the current shape on the next successful save.
pub fn load_preferences() -> Preferences {
    load_preferences_from(&config_root())
}

/// Persists a complete preferences snapshot and applies its retention setting
/// to cached summaries/bodies. This is intentionally one operation so callers
/// cannot lose one preference field while updating another.
pub fn save_preferences_and_prune(preferences: Preferences) -> io::Result<()> {
    save_preferences_and_prune_at(&config_root(), &cache_root(), preferences)
}

/// Loads the durable account registry. A missing registry represents a
/// pre-multi-account installation and is intentionally not an error.
pub fn load_account_registry() -> io::Result<AccountRegistry> {
    load_account_registry_at(&config_root())
}

pub fn load_account_registry_at(config: &Path) -> io::Result<AccountRegistry> {
    match read_json::<StoredAccountRegistry>(&account_registry_path(config))? {
        None => Ok(AccountRegistry::default()),
        Some(stored)
            if stored.version == ACCOUNT_REGISTRY_VERSION && stored.registry.is_valid() =>
        {
            Ok(stored.registry)
        }
        Some(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid account registry",
        )),
    }
}

pub fn save_account_registry(registry: &AccountRegistry) -> io::Result<()> {
    save_account_registry_at(&config_root(), registry)
}

pub fn save_account_registry_at(config: &Path, registry: &AccountRegistry) -> io::Result<()> {
    if !registry.is_valid() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid account registry",
        ));
    }
    write_private_json(
        &account_registry_path(config),
        &StoredAccountRegistry {
            version: ACCOUNT_REGISTRY_VERSION,
            registry: registry.clone(),
        },
    )
}

/// Adds an account atomically with respect to the registry file. Duplicate
/// Gmail identities are rejected before any existing registry is overwritten.
pub fn add_account_record(record: AccountRecord) -> io::Result<AccountRegistry> {
    add_account_record_at(&config_root(), record)
}

pub fn add_account_record_at(config: &Path, record: AccountRecord) -> io::Result<AccountRegistry> {
    let mut registry = load_account_registry_at(config)?;
    registry.add(record).map_err(account_registry_error)?;
    save_account_registry_at(config, &registry)?;
    Ok(registry)
}

fn account_registry_error(error: AccountRegistryError) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("account registry: {error:?}"),
    )
}

/// Returns the isolated cache root for an account. This does not create it.
pub fn account_cache_namespace(account_id: &AccountId) -> AccountCacheNamespace {
    account_cache_namespace_at(&cache_root(), account_id)
}

pub fn account_cache_namespace_at(cache: &Path, account_id: &AccountId) -> AccountCacheNamespace {
    AccountCacheNamespace {
        account_id: account_id.clone(),
        root: accounts_cache_root(cache).join(account_id.as_str()),
    }
}

/// Removes only the specified account's namespaced cache. Registry and every
/// other account tree are intentionally untouched.
pub fn clear_account_cache(account_id: &AccountId) -> io::Result<()> {
    clear_account_cache_at(&cache_root(), account_id)
}

pub fn clear_account_cache_at(cache: &Path, account_id: &AccountId) -> io::Result<()> {
    let namespace = account_cache_namespace_at(cache, account_id);
    remove_dir_if_exists(&namespace.root)
}

/// Account-scoped equivalents of the singleton cache API. They deliberately
/// route the existing, mature mailbox/body formats through a distinct cache
/// root so equal Gmail message IDs cannot collide across accounts.
pub fn load_latest_for_account(
    account_id: &AccountId,
    limit: usize,
) -> io::Result<Option<(AccountIdentity, MailboxSnapshot)>> {
    load_latest_from(&account_cache_namespace(account_id).root, limit)
}

pub fn load_folder_for_account(
    account_id: &AccountId,
    account_email: &str,
    folder_id: &FolderId,
    limit: usize,
) -> io::Result<Option<MailboxSnapshot>> {
    load_folder_from(
        &account_cache_namespace(account_id).root,
        account_email,
        folder_id,
        limit,
        now_unix(),
    )
}

pub fn replace_and_save_for_account(
    account_id: &AccountId,
    account: &AccountIdentity,
    fresh: MailboxSnapshot,
    limit: usize,
) -> io::Result<MailboxSnapshot> {
    replace_and_save_at(
        &account_cache_namespace(account_id).root,
        account,
        fresh,
        limit,
    )
}

pub fn replace_folder_and_save_for_account(
    account_id: &AccountId,
    account: &AccountIdentity,
    folder_id: FolderId,
    fresh: MailboxSnapshot,
    limit: usize,
) -> io::Result<MailboxSnapshot> {
    replace_folder_and_save_at(
        &account_cache_namespace(account_id).root,
        account,
        folder_id,
        fresh,
        limit,
        now_unix(),
    )
}

pub fn load_body_for_account(
    account_id: &AccountId,
    account_email: &str,
    id: &MessageId,
) -> io::Result<Option<MessageBody>> {
    load_body_at(
        &account_cache_namespace(account_id).root,
        account_email,
        id,
        now_unix(),
    )
}

pub fn save_body_for_account(
    account_id: &AccountId,
    account_email: &str,
    id: &MessageId,
    body: &MessageBody,
) -> io::Result<CacheUsage> {
    save_body_at(
        &account_cache_namespace(account_id).root,
        account_email,
        id,
        body,
        now_unix(),
        BODY_CACHE_BUDGET_BYTES,
    )
}

pub fn persist_confirmed_mutation_for_account(
    account_id: &AccountId,
    account_email: &str,
    id: &MessageId,
    mutation: &MessageMutation,
) -> io::Result<()> {
    persist_confirmed_mutation_at(
        &account_cache_namespace(account_id).root,
        account_email,
        id,
        mutation,
    )
}

/// Copies the singleton cache into a new account namespace without deleting
/// any source files. Call `finalize_legacy_cache_migration` only after the
/// account registry and scoped Secret Service token have both been committed.
/// Repeating this operation after interruption is safe.
pub fn stage_legacy_cache_migration(account_id: &AccountId) -> io::Result<AccountCacheNamespace> {
    stage_legacy_cache_migration_at(&cache_root(), account_id)
}

pub fn stage_legacy_cache_migration_at(
    cache: &Path,
    account_id: &AccountId,
) -> io::Result<AccountCacheNamespace> {
    let marker = legacy_cache_migration_marker_path(cache);
    match read_json::<PendingLegacyCacheMigration>(&marker)? {
        Some(pending)
            if pending.version == LEGACY_CACHE_MIGRATION_VERSION
                && pending.account_id != *account_id =>
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "legacy cache migration belongs to another account",
            ));
        }
        Some(pending) if pending.version != LEGACY_CACHE_MIGRATION_VERSION => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid legacy cache migration marker",
            ));
        }
        None => write_private_json(
            &marker,
            &PendingLegacyCacheMigration {
                version: LEGACY_CACHE_MIGRATION_VERSION,
                account_id: account_id.clone(),
            },
        )?,
        _ => {}
    }
    let namespace = account_cache_namespace_at(cache, account_id);
    ensure_private_dir(&namespace.root)?;
    for (source, destination) in [
        (mailbox_path(cache), namespace.mailbox_path()),
        (v2_mailbox_path(cache), v2_mailbox_path(&namespace.root)),
        (
            legacy_mailbox_path(cache),
            legacy_mailbox_path(&namespace.root),
        ),
    ] {
        copy_private_file_if_exists(&source, &destination)?;
    }
    copy_private_dir_if_exists(&bodies_path(cache), &namespace.bodies_path())?;
    copy_private_dir_if_exists(
        &legacy_bodies_path(cache),
        &legacy_bodies_path(&namespace.root),
    )?;
    copy_private_dir_if_exists(&attachments_path(cache), &namespace.attachments_path())?;
    sync_mail_cache_dir(cache)?;
    Ok(namespace)
}

/// Finishes a previously staged migration. It refuses to remove singleton
/// data unless the marker belongs to `account_id`; this makes retry and crash
/// recovery deterministic and prevents cross-account cleanup.
pub fn finalize_legacy_cache_migration(account_id: &AccountId) -> io::Result<()> {
    finalize_legacy_cache_migration_at(&cache_root(), account_id)
}

pub fn finalize_legacy_cache_migration_at(cache: &Path, account_id: &AccountId) -> io::Result<()> {
    let marker = legacy_cache_migration_marker_path(cache);
    let Some(pending) = read_json::<PendingLegacyCacheMigration>(&marker)? else {
        return Ok(());
    };
    if pending.version != LEGACY_CACHE_MIGRATION_VERSION || pending.account_id != *account_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "legacy cache migration marker mismatch",
        ));
    }
    for path in [
        mailbox_path(cache),
        v2_mailbox_path(cache),
        legacy_mailbox_path(cache),
    ] {
        remove_file_if_exists(&path)?;
    }
    for path in [
        bodies_path(cache),
        legacy_bodies_path(cache),
        attachments_path(cache),
    ] {
        remove_dir_if_exists(&path)?;
    }
    remove_file_if_exists(&marker)?;
    sync_mail_cache_dir(cache)
}

pub fn load_latest(limit: usize) -> io::Result<Option<(AccountIdentity, MailboxSnapshot)>> {
    load_latest_from(&cache_root(), limit)
}

/// Removes every cached artifact when the verified Gmail identity changes.
/// This runs before the new account is exposed to the UI, so a later save
/// failure can never leave the previous account's mailbox available.
pub fn prepare_verified_account(account_email: &str) -> io::Result<()> {
    prepare_verified_account_at(&cache_root(), account_email)
}

pub fn load_folder(
    account_email: &str,
    folder_id: &FolderId,
    limit: usize,
) -> io::Result<Option<MailboxSnapshot>> {
    load_folder_from(&cache_root(), account_email, folder_id, limit, now_unix())
}

/// Saves the server's authoritative newest-N membership. Old absent rows are not merged.
pub fn replace_and_save(
    account: &AccountIdentity,
    fresh: MailboxSnapshot,
    limit: usize,
) -> io::Result<MailboxSnapshot> {
    replace_and_save_at(&cache_root(), account, fresh, limit)
}

/// Replaces the authoritative Inbox view and advances the durable notification
/// watermark in the same cache commit.  The first write after a migration is a
/// baseline, so restoring an old cache can never generate a notification storm.
pub fn replace_inbox_and_save_with_notification_gate(
    account: &AccountIdentity,
    fresh: MailboxSnapshot,
    limit: usize,
) -> io::Result<BackgroundInboxCommit> {
    replace_inbox_and_save_with_notification_gate_at(&cache_root(), account, fresh, limit)
}

pub fn replace_folder_and_save(
    account: &AccountIdentity,
    folder_id: FolderId,
    fresh: MailboxSnapshot,
    limit: usize,
) -> io::Result<MailboxSnapshot> {
    replace_folder_and_save_at(&cache_root(), account, folder_id, fresh, limit, now_unix())
}

/// Updates only data established by a successful IMAP mutation response.
pub fn persist_confirmed_mutation(
    account_email: &str,
    id: &MessageId,
    mutation: &MessageMutation,
) -> io::Result<()> {
    persist_confirmed_message_mutation(account_email, id, mutation)
}

/// Applies a confirmed Gmail mutation to every cached folder projection containing the
/// canonical message ID. This deliberately does not assume that the action originated in
/// Inbox: Gmail IDs can legitimately appear in several cached views with different locators.
pub fn persist_confirmed_message_mutation(
    account_email: &str,
    id: &MessageId,
    mutation: &MessageMutation,
) -> io::Result<()> {
    persist_confirmed_mutation_at(&cache_root(), account_email, id, mutation)
}

fn persist_confirmed_mutation_at(
    cache: &Path,
    account_email: &str,
    id: &MessageId,
    mutation: &MessageMutation,
) -> io::Result<()> {
    mutate_stored_mailbox_at(cache, account_email, |view| {
        for message in &mut view.messages {
            if &message.id == id {
                match mutation {
                    MessageMutation::Archive => message.in_inbox = false,
                    MessageMutation::MoveToTrash { .. } => {
                        message.in_inbox = false;
                        message.in_trash = true;
                    }
                    MessageMutation::RestoreArchive { .. } => {
                        message.in_inbox = true;
                        message.in_trash = false;
                    }
                    MessageMutation::RestoreFromTrash { restore_inbox, .. } => {
                        message.in_inbox = *restore_inbox;
                        message.in_trash = false;
                    }
                    MessageMutation::SetRead(_)
                    | MessageMutation::SetStarred(_)
                    | MessageMutation::SetLabel { .. } => {}
                }
            }
        }
        match mutation {
            // Archive removes only the Inbox projection. All Mail/label views remain useful
            // local representations of the same canonical Gmail message.
            MessageMutation::Archive if view.folder_id == FolderId::Inbox => {
                view.messages.retain(|message| &message.id != id);
            }
            // Gmail MOVE to Trash makes every non-Trash cached projection stale. We cannot
            // synthesize a new Trash row without an authoritative Trash locator.
            MessageMutation::MoveToTrash { .. } if view.folder_id != FolderId::Trash => {
                view.messages.retain(|message| &message.id != id);
            }
            // An Undo confirmation carries no authoritative locator/summary with which to
            // synthesize a previously evicted Inbox or Trash cache row. Leave existing views
            // intact and let the next folder refresh repopulate them; this never marks an
            // unconfirmed local Undo as durable.
            MessageMutation::RestoreArchive { .. } | MessageMutation::RestoreFromTrash { .. } => {}
            _ => {
                for message in &mut view.messages {
                    if &message.id == id {
                        mutation.apply(message);
                    }
                }
            }
        }
    })
}

pub fn persist_reconciled_inbox(
    account_email: &str,
    id: &MessageId,
    state: Option<&ReconciledMessageState>,
) -> io::Result<()> {
    persist_reconciled_message_state(account_email, id, state)
}

/// Compatibility state supplied by the current worker is authoritative for Inbox membership.
/// Attribute updates apply to every cached representation; absence removes only Inbox, not an
/// unrelated All Mail/label cache row.
pub fn persist_reconciled_message_state(
    account_email: &str,
    id: &MessageId,
    state: Option<&ReconciledMessageState>,
) -> io::Result<()> {
    persist_reconciled_inbox_at(&cache_root(), account_email, id, state)
}

fn persist_reconciled_inbox_at(
    cache: &Path,
    account_email: &str,
    id: &MessageId,
    state: Option<&ReconciledMessageState>,
) -> io::Result<()> {
    mutate_stored_mailbox_at(cache, account_email, |view| {
        if state.is_none() && view.folder_id == FolderId::Inbox {
            view.messages.retain(|message| &message.id != id);
        } else if let Some(state) = state {
            for message in &mut view.messages {
                if &message.id == id {
                    message.unread = state.unread;
                    message.starred = state.starred;
                    message.in_inbox = state.in_inbox;
                    message.in_trash = state.in_trash;
                    message.labels = state.labels.clone();
                }
            }
        }
    })
}

fn mutate_stored_mailbox_at(
    cache: &Path,
    account_email: &str,
    mut operation: impl FnMut(&mut StoredFolderView),
) -> io::Result<()> {
    validate_account_email(account_email)?;
    let path = mailbox_path(cache);
    let Some(mut stored) = read_json::<StoredMailbox>(&path)? else {
        return Ok(());
    };
    if !valid_stored_mailbox(&stored) || !stored.account_email.eq_ignore_ascii_case(account_email) {
        return Ok(());
    }
    for view in &mut stored.views {
        operation(view);
    }
    write_private_json(&path, &stored)
}

pub fn load_body(account_email: &str, id: &MessageId) -> io::Result<Option<MessageBody>> {
    load_body_at(&cache_root(), account_email, id, now_unix())
}

pub fn save_body(
    account_email: &str,
    id: &MessageId,
    body: &MessageBody,
) -> io::Result<CacheUsage> {
    save_body_at(
        &cache_root(),
        account_email,
        id,
        body,
        now_unix(),
        BODY_CACHE_BUDGET_BYTES,
    )
}

pub fn clear_bodies() -> io::Result<u64> {
    clear_bodies_at(&cache_root())
}

pub struct AtomicAttachment {
    file: Option<fs::File>,
    temporary: PathBuf,
    destination: PathBuf,
    committed: bool,
}

impl AtomicAttachment {
    pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("attachment output already finished"))?
            .write_all(bytes)
    }

    pub fn finish(mut self) -> io::Result<PathBuf> {
        let mut file = self
            .file
            .take()
            .ok_or_else(|| io::Error::other("attachment output already finished"))?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        fs::rename(&self.temporary, &self.destination)?;
        self.committed = true;
        if let Some(parent) = self.destination.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(self.destination.clone())
    }
}

impl Drop for AtomicAttachment {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.temporary);
        }
    }
}

pub enum OpenAttachmentTarget {
    Cached(PathBuf),
    Download(AtomicAttachment),
}

pub fn prepare_open_attachment(
    account_email: &str,
    id: &MessageId,
    attachment: &Attachment,
) -> io::Result<OpenAttachmentTarget> {
    prepare_open_attachment_at(&cache_root(), account_email, id, attachment)
}

fn prepare_open_attachment_at(
    root: &Path,
    account_email: &str,
    id: &MessageId,
    attachment: &Attachment,
) -> io::Result<OpenAttachmentTarget> {
    validate_account_email(account_email)?;
    if !attachment.is_downloadable() || id.gmail_value().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid attachment descriptor",
        ));
    }
    ensure_private_dir(&root.join("whitford"))?;
    ensure_private_dir(&attachments_path(root))?;
    let destination =
        attachments_path(root).join(attachment_file_name(account_email, id, attachment)?);
    if let Ok(metadata) = fs::symlink_metadata(&destination) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsafe attachment cache entry",
            ));
        }
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o600))?;
        return Ok(OpenAttachmentTarget::Cached(destination));
    }
    Ok(OpenAttachmentTarget::Download(atomic_attachment(
        destination,
    )?))
}

pub fn prepare_save_attachment(destination: &Path) -> io::Result<AtomicAttachment> {
    if !destination.is_absolute()
        || destination
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsafe attachment destination",
        ));
    }
    let name = destination
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing file name"))?;
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing parent"))?;
    let canonical_parent = fs::canonicalize(parent)?;
    if canonical_parent != parent {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "symlinked attachment destination",
        ));
    }
    let destination = canonical_parent.join(name);
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe existing destination",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    atomic_attachment(destination)
}

pub fn cleanup_attachment_partials() -> io::Result<()> {
    let path = attachments_path(&cache_root());
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".whitford-attachment-") && name.ends_with(".part") {
            remove_file_if_exists(&entry.path())?;
        }
    }
    Ok(())
}

pub fn prune_attachment_cache(protected: &Path) -> io::Result<()> {
    prune_attachment_cache_at(&cache_root(), protected, ATTACHMENT_CACHE_BUDGET_BYTES)
}

fn prune_attachment_cache_at(cache: &Path, protected: &Path, budget: u64) -> io::Result<()> {
    let entries = match fs::read_dir(attachments_path(cache)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() {
            remove_file_if_exists(&entry.path())?;
        } else if metadata.is_file() {
            files.push((
                entry.path(),
                metadata.len(),
                metadata.modified().unwrap_or(UNIX_EPOCH),
            ));
        }
    }
    let mut total = files
        .iter()
        .fold(0_u64, |sum, (_, bytes, _)| sum.saturating_add(*bytes));
    files.sort_by(|left, right| left.2.cmp(&right.2).then_with(|| left.0.cmp(&right.0)));
    for (path, bytes, _) in files {
        if total <= budget {
            break;
        }
        if path != protected {
            remove_file_if_exists(&path)?;
            total = total.saturating_sub(bytes);
        }
    }
    Ok(())
}

pub fn usage() -> io::Result<CacheUsage> {
    usage_at(&cache_root())
}

/// Removes every summary/body cache generation, but never preferences or credentials.
pub fn clear_all_mail() -> io::Result<()> {
    clear_all_mail_at(&cache_root())
}

pub fn clear_mailbox() -> io::Result<()> {
    clear_all_mail()
}

fn deserialize_appearance_preference<'de, D>(
    deserializer: D,
) -> Result<AppearancePreference, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(match value.as_str() {
        Some("System") => AppearancePreference::System,
        Some("Light") => AppearancePreference::Light,
        Some("Dark") => AppearancePreference::Dark,
        _ => AppearancePreference::System,
    })
}

fn load_preferences_from(config: &Path) -> Preferences {
    let Ok(Some(mut preferences)) = read_json::<Preferences>(&preferences_path(config)) else {
        return Preferences::default();
    };
    if preferences.version > PREFERENCES_VERSION || !is_valid_limit(preferences.retained_messages) {
        return Preferences::default();
    }
    // Version zero is the retention-only schema, for which serde supplied the
    // appearance default. Normalize it before exposing the value to callers.
    preferences.version = PREFERENCES_VERSION;
    preferences
}

#[cfg(test)]
fn load_limit_from(config: &Path) -> usize {
    load_preferences_from(config).retained_messages
}

fn save_preferences_and_prune_at(
    config: &Path,
    cache: &Path,
    mut preferences: Preferences,
) -> io::Result<()> {
    validate_limit(preferences.retained_messages)?;
    preferences.version = PREFERENCES_VERSION;
    write_private_json(&preferences_path(config), &preferences)?;
    if let Some(mut stored) = read_json::<StoredMailbox>(&mailbox_path(cache))?
        && valid_stored_mailbox(&stored)
    {
        for view in &mut stored.views {
            sort_summaries(&mut view.messages);
            view.messages
                .truncate(preferences.retained_messages.min(MAX_SUMMARIES_PER_FOLDER));
            view.requested_limit = preferences.retained_messages;
        }
        write_private_json(&mailbox_path(cache), &stored)?;
        reconcile_bodies(
            cache,
            &stored.account_email,
            None,
            None,
            BODY_CACHE_BUDGET_BYTES,
        )?;
    }
    Ok(())
}

#[cfg(test)]
fn save_limit_and_prune_at(config: &Path, cache: &Path, limit: usize) -> io::Result<()> {
    let mut preferences = load_preferences_from(config);
    preferences.retained_messages = limit;
    save_preferences_and_prune_at(config, cache, preferences)
}

fn load_latest_from(
    cache: &Path,
    limit: usize,
) -> io::Result<Option<(AccountIdentity, MailboxSnapshot)>> {
    validate_limit(limit)?;
    if account_cleanup_pending(cache)? {
        return Ok(None);
    }
    let stored = match read_json::<StoredMailbox>(&mailbox_path(cache)) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::InvalidData => return Ok(None),
        Err(error) => return Err(error),
    };
    let Some(mut stored) = stored else {
        return Ok(None);
    };
    if !valid_stored_mailbox(&stored) {
        return Ok(None);
    }
    let Some(view) = stored
        .views
        .iter_mut()
        .find(|view| view.folder_id == FolderId::Inbox)
    else {
        return Ok(None);
    };
    sort_summaries(&mut view.messages);
    view.messages.truncate(limit.min(MAX_SUMMARIES_PER_FOLDER));
    Ok(Some((
        AccountIdentity {
            provider: MailProvider::Gmail,
            email: stored.account_email.clone(),
        },
        snapshot_from_stored(&stored.folder_catalog, view),
    )))
}

fn load_folder_from(
    cache: &Path,
    account_email: &str,
    folder_id: &FolderId,
    limit: usize,
    accessed_at: u64,
) -> io::Result<Option<MailboxSnapshot>> {
    validate_limit(limit)?;
    validate_account_email(account_email)?;
    if account_cleanup_pending(cache)? {
        return Ok(None);
    }
    let mut stored = match read_json::<StoredMailbox>(&mailbox_path(cache)) {
        Ok(Some(value)) if valid_stored_mailbox(&value) => value,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => return Ok(None),
        Err(error) => return Err(error),
    };
    if !stored.account_email.eq_ignore_ascii_case(account_email) {
        return Ok(None);
    }
    let Some(index) = stored
        .views
        .iter()
        .position(|view| &view.folder_id == folder_id)
    else {
        return Ok(None);
    };
    stored.views[index].last_accessed_unix = accessed_at;
    let mut view = stored.views[index].clone();
    sort_summaries(&mut view.messages);
    view.messages.truncate(limit.min(MAX_SUMMARIES_PER_FOLDER));
    let snapshot = snapshot_from_stored(&stored.folder_catalog, &view);
    write_private_json(&mailbox_path(cache), &stored)?;
    Ok(Some(snapshot))
}

fn replace_and_save_at(
    cache: &Path,
    account: &AccountIdentity,
    fresh: MailboxSnapshot,
    limit: usize,
) -> io::Result<MailboxSnapshot> {
    replace_folder_and_save_at(cache, account, FolderId::Inbox, fresh, limit, now_unix())
}

fn replace_inbox_and_save_with_notification_gate_at(
    cache: &Path,
    account: &AccountIdentity,
    fresh: MailboxSnapshot,
    limit: usize,
) -> io::Result<BackgroundInboxCommit> {
    validate_limit(limit)?;
    validate_account_email(&account.email)?;
    let previous = match read_json::<StoredMailbox>(&mailbox_path(cache)) {
        Ok(Some(value))
            if valid_stored_mailbox(&value)
                && value.account_email.eq_ignore_ascii_case(&account.email) =>
        {
            Some(value)
        }
        Ok(_) => None,
        Err(error) if error.kind() == io::ErrorKind::InvalidData => None,
        Err(error) => return Err(error),
    };
    let observed = previous
        .as_ref()
        .filter(|stored| stored.inbox_watermark.initialized)
        .map(|stored| {
            stored
                .inbox_watermark
                .observed_ids
                .iter()
                .cloned()
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let new_unread_ids = fresh
        .messages
        .iter()
        .filter(|message| {
            message.folder_id == FolderId::Inbox
                && message.unread
                && !observed.contains(&message.id)
        })
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    let snapshot = replace_and_save_at(cache, account, fresh, limit)?;
    // No prior initialized watermark means the just-written authoritative IDs are
    // a startup/migration baseline, never a source of notifications.
    Ok(BackgroundInboxCommit {
        snapshot,
        new_unread_ids: if previous
            .as_ref()
            .is_some_and(|stored| stored.inbox_watermark.initialized)
        {
            new_unread_ids
        } else {
            Vec::new()
        },
    })
}

fn replace_folder_and_save_at(
    cache: &Path,
    account: &AccountIdentity,
    folder_id: FolderId,
    mut fresh: MailboxSnapshot,
    limit: usize,
    accessed_at: u64,
) -> io::Result<MailboxSnapshot> {
    validate_limit(limit)?;
    if account.email.trim().is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty account"));
    }
    resume_pending_account_cleanup_at(cache)?;
    let previous = match read_json::<StoredMailbox>(&mailbox_path(cache)) {
        Ok(Some(value)) if valid_stored_mailbox(&value) => Some(value),
        Ok(_) => None,
        Err(error) if error.kind() == io::ErrorKind::InvalidData => None,
        Err(error) => return Err(error),
    };
    let account_changed = previous
        .as_ref()
        .is_some_and(|value| !value.account_email.eq_ignore_ascii_case(&account.email));
    let previous_account = previous
        .as_ref()
        .filter(|_| account_changed)
        .map(|stored| stored.account_email.clone());
    sort_summaries(&mut fresh.messages);
    fresh
        .messages
        .retain(|message| message.folder_id == folder_id);
    fresh.messages.truncate(limit.min(MAX_SUMMARIES_PER_FOLDER));
    fresh.metadata.requested_limit = limit;
    fresh.metadata.loaded_count = fresh.messages.len();
    fresh.metadata.fallback_count = fresh
        .messages
        .iter()
        .filter(|message| message.used_fallback)
        .count();
    let view = StoredFolderView {
        folder_id: folder_id.clone(),
        completed_at_unix: fresh
            .metadata
            .completed_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        requested_limit: fresh.metadata.requested_limit,
        skipped_count: fresh.metadata.skipped_count,
        messages: fresh.messages.clone(),
        last_accessed_unix: accessed_at,
    };
    let mut stored = previous
        .filter(|value| value.account_email.eq_ignore_ascii_case(&account.email))
        .unwrap_or_else(|| StoredMailbox {
            version: MAILBOX_VERSION,
            account_email: account.email.clone(),
            folder_catalog: fresh.folder_catalog.clone(),
            views: Vec::new(),
            inbox_watermark: InboxWatermark::default(),
        });
    stored.version = MAILBOX_VERSION;
    stored.folder_catalog = fresh.folder_catalog.clone();
    stored.views.retain(|view| {
        view.folder_id != folder_id && stored.folder_catalog.find(&view.folder_id).is_some()
    });
    stored.views.push(view);
    if folder_id == FolderId::Inbox {
        stored.inbox_watermark = InboxWatermark {
            initialized: true,
            observed_ids: fresh
                .messages
                .iter()
                .map(|message| message.id.clone())
                .collect(),
        };
    }
    stored.views.sort_by(|left, right| {
        right
            .last_accessed_unix
            .cmp(&left.last_accessed_unix)
            .then_with(|| folder_key(&left.folder_id).cmp(&folder_key(&right.folder_id)))
    });
    stored.views.truncate(MAX_CACHED_FOLDER_VIEWS);
    if !valid_stored_mailbox(&stored) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid folder cache snapshot",
        ));
    }
    if let Some(previous_account) = previous_account {
        begin_account_cleanup_at(cache, &previous_account, &account.email)?;
    }
    write_private_json(&mailbox_path(cache), &stored)?;
    // Older locator-derived summaries and bodies cannot be safely promoted to
    // canonical X-GM-MSGID identity. Remove them only after the durable v3 commit.
    let _ = remove_file_if_exists(&legacy_mailbox_path(cache));
    let _ = remove_file_if_exists(&v2_mailbox_path(cache));
    let _ = remove_dir_if_exists(&legacy_bodies_path(cache));
    reconcile_bodies(cache, &account.email, None, None, BODY_CACHE_BUDGET_BYTES)?;
    Ok(fresh)
}

fn prepare_verified_account_at(cache: &Path, account_email: &str) -> io::Result<()> {
    validate_account_email(account_email)?;
    resume_pending_account_cleanup_at(cache)?;
    let previous = match read_json::<StoredMailbox>(&mailbox_path(cache)) {
        Ok(Some(value)) if valid_stored_mailbox(&value) => Some(value),
        Ok(_) => None,
        Err(error) if error.kind() == io::ErrorKind::InvalidData => None,
        Err(error) => return Err(error),
    };
    if let Some(previous) = previous
        .as_ref()
        .filter(|stored| !stored.account_email.eq_ignore_ascii_case(account_email))
    {
        begin_account_cleanup_at(cache, &previous.account_email, account_email)?;
    }
    Ok(())
}

fn begin_account_cleanup_at(
    cache: &Path,
    previous_account_email: &str,
    next_account_email: &str,
) -> io::Result<()> {
    write_private_json(
        &account_cleanup_marker_path(cache),
        &PendingAccountCleanup {
            version: ACCOUNT_CLEANUP_VERSION,
            previous_account_email,
            next_account_email,
        },
    )?;
    sync_mail_cache_dir(cache)?;
    resume_pending_account_cleanup_at(cache)
}

fn resume_pending_account_cleanup_at(cache: &Path) -> io::Result<()> {
    resume_pending_account_cleanup_with(cache, |_| Ok(()))
}

fn resume_pending_account_cleanup_with(
    cache: &Path,
    mut before_stage: impl FnMut(AccountCleanupStage) -> io::Result<()>,
) -> io::Result<()> {
    let marker = account_cleanup_marker_path(cache);
    match fs::symlink_metadata(&marker) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }

    // The durable marker is deliberately independent of the mailbox index. Every stage is
    // idempotent, and the marker is removed only after all account-bound artifacts are gone.
    before_stage(AccountCleanupStage::MailboxIndexes)?;
    for path in [
        mailbox_path(cache),
        v2_mailbox_path(cache),
        legacy_mailbox_path(cache),
    ] {
        remove_file_if_exists(&path)?;
    }
    sync_mail_cache_dir(cache)?;

    before_stage(AccountCleanupStage::BodyCaches)?;
    remove_dir_if_exists(&bodies_path(cache))?;
    remove_dir_if_exists(&legacy_bodies_path(cache))?;
    sync_mail_cache_dir(cache)?;

    before_stage(AccountCleanupStage::AttachmentCache)?;
    remove_dir_if_exists(&attachments_path(cache))?;
    sync_mail_cache_dir(cache)?;

    before_stage(AccountCleanupStage::Marker)?;
    remove_file_if_exists(&marker)?;
    sync_mail_cache_dir(cache)
}

fn load_body_at(
    cache: &Path,
    account_email: &str,
    id: &MessageId,
    accessed_at: u64,
) -> io::Result<Option<MessageBody>> {
    validate_account_email(account_email)?;
    if account_cleanup_pending(cache)? {
        return Ok(None);
    }
    let expected_name = body_file_name(id)?;
    let mut manifest = read_manifest(cache)?;
    reconcile_manifest_entries(cache, &mut manifest, account_email, None)?;
    let Some(index) = manifest.entries.iter().position(|entry| {
        entry.account_email.eq_ignore_ascii_case(account_email)
            && entry.id == *id
            && entry.file_name == expected_name
    }) else {
        write_manifest(cache, &manifest)?;
        return Ok(None);
    };
    let path = bodies_path(cache).join(expected_name);
    let stored = match read_json::<StoredBody>(&path) {
        Ok(Some(value))
            if value.version == BODY_VERSION
                && value.account_email.eq_ignore_ascii_case(account_email)
                && value.id == *id =>
        {
            value
        }
        Ok(_) => {
            manifest.entries.remove(index);
            let _ = remove_file_if_exists(&path);
            write_manifest(cache, &manifest)?;
            return Ok(None);
        }
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            manifest.entries.remove(index);
            let _ = remove_file_if_exists(&path);
            write_manifest(cache, &manifest)?;
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    manifest.entries[index].last_accessed_unix = accessed_at;
    manifest.entries[index].bytes = fs::metadata(path)?.len();
    write_manifest(cache, &manifest)?;
    Ok(Some(stored.body))
}

fn save_body_at(
    cache: &Path,
    account_email: &str,
    id: &MessageId,
    body: &MessageBody,
    accessed_at: u64,
    budget: u64,
) -> io::Result<CacheUsage> {
    validate_account_email(account_email)?;
    let file_name = body_file_name(id)?;
    ensure_bodies_dir(cache)?;
    let path = bodies_path(cache).join(&file_name);
    write_private_json(
        &path,
        &StoredBodyRef {
            version: BODY_VERSION,
            account_email,
            id,
            body,
        },
    )?;
    let bytes = fs::metadata(path)?.len();
    let mut manifest = read_manifest(cache)?;
    manifest.entries.retain(|entry| {
        !(entry.account_email.eq_ignore_ascii_case(account_email) && entry.id == *id)
    });
    manifest.entries.push(ManifestEntry {
        account_email: account_email.to_owned(),
        id: id.clone(),
        file_name,
        bytes,
        last_accessed_unix: accessed_at,
    });
    write_manifest(cache, &manifest)?;
    reconcile_bodies(cache, account_email, None, Some(id), budget)?;
    usage_at(cache)
}

fn clear_bodies_at(cache: &Path) -> io::Result<u64> {
    let before = directory_size(&bodies_path(cache))?
        .saturating_add(directory_size(&legacy_bodies_path(cache))?)
        .saturating_add(directory_size(&attachments_path(cache))?);
    remove_dir_if_exists(&bodies_path(cache))?;
    remove_dir_if_exists(&legacy_bodies_path(cache))?;
    remove_dir_if_exists(&attachments_path(cache))?;
    ensure_bodies_dir(cache)?;
    write_manifest(cache, &empty_manifest())?;
    Ok(before.saturating_sub(directory_size(&bodies_path(cache))?))
}

fn clear_all_mail_at(cache: &Path) -> io::Result<()> {
    let mut first_error = None;
    for path in [
        mailbox_path(cache),
        v2_mailbox_path(cache),
        legacy_mailbox_path(cache),
    ] {
        if let Err(error) = remove_file_if_exists(&path) {
            first_error.get_or_insert(error);
        }
    }
    if let Err(error) = remove_dir_if_exists(&bodies_path(cache)) {
        first_error.get_or_insert(error);
    }
    if let Err(error) = remove_dir_if_exists(&legacy_bodies_path(cache)) {
        first_error.get_or_insert(error);
    }
    if let Err(error) = remove_dir_if_exists(&attachments_path(cache)) {
        first_error.get_or_insert(error);
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    remove_file_if_exists(&account_cleanup_marker_path(cache))?;
    sync_mail_cache_dir(cache)
}

fn usage_at(cache: &Path) -> io::Result<CacheUsage> {
    let summary_bytes = file_size_if_exists(&mailbox_path(cache))?;
    let mut body_bytes = file_size_if_exists(&manifest_path(cache))?;
    let mut body_count = 0;
    if let Some(manifest) = read_json::<BodyManifest>(&manifest_path(cache))?
        && manifest.version == MANIFEST_VERSION
    {
        for entry in manifest.entries {
            if body_file_name(&entry.id).ok().as_deref() == Some(entry.file_name.as_str()) {
                let bytes = file_size_if_exists(&bodies_path(cache).join(entry.file_name))?;
                if bytes > 0 {
                    body_bytes = body_bytes.saturating_add(bytes);
                    body_count += 1;
                }
            }
        }
    }
    body_bytes = body_bytes.saturating_add(directory_size(&attachments_path(cache))?);
    Ok(CacheUsage {
        total_bytes: summary_bytes.saturating_add(body_bytes),
        summary_bytes,
        body_bytes,
        body_count,
        available: true,
    })
}

fn reconcile_bodies(
    cache: &Path,
    account_email: &str,
    retained: Option<&HashSet<MessageId>>,
    protected: Option<&MessageId>,
    budget: u64,
) -> io::Result<()> {
    let mut manifest = read_manifest(cache)?;
    reconcile_manifest_entries(cache, &mut manifest, account_email, retained)?;
    manifest.entries.sort_by(|left, right| {
        left.last_accessed_unix
            .cmp(&right.last_accessed_unix)
            .then_with(|| left.file_name.cmp(&right.file_name))
    });
    let mut total = manifest
        .entries
        .iter()
        .fold(0_u64, |sum, entry| sum.saturating_add(entry.bytes));
    let mut kept = Vec::with_capacity(manifest.entries.len());
    for entry in manifest.entries {
        if total > budget && protected != Some(&entry.id) {
            remove_file_if_exists(&bodies_path(cache).join(&entry.file_name))?;
            total = total.saturating_sub(entry.bytes);
        } else {
            kept.push(entry);
        }
    }
    manifest.entries = kept;
    write_manifest(cache, &manifest)
}

fn reconcile_manifest_entries(
    cache: &Path,
    manifest: &mut BodyManifest,
    account_email: &str,
    retained: Option<&HashSet<MessageId>>,
) -> io::Result<()> {
    ensure_bodies_dir(cache)?;
    let mut seen = HashSet::new();
    manifest.entries.retain_mut(|entry| {
        let valid_name = entry.account_email.eq_ignore_ascii_case(account_email)
            && body_file_name(&entry.id).is_ok_and(|expected| expected == entry.file_name);
        let retained = retained.is_none_or(|ids| ids.contains(&entry.id));
        if !(valid_name && retained && seen.insert(entry.id.clone())) {
            return false;
        }
        match fs::metadata(bodies_path(cache).join(&entry.file_name)) {
            Ok(metadata) if metadata.is_file() => {
                entry.bytes = metadata.len();
                true
            }
            _ => false,
        }
    });
    let known = manifest
        .entries
        .iter()
        .map(|entry| entry.file_name.as_str())
        .collect::<HashSet<_>>();
    for item in fs::read_dir(bodies_path(cache))? {
        let item = item?;
        let name = item.file_name();
        let name = name.to_string_lossy();
        if name == "manifest.json" {
            continue;
        }
        if name.contains(".tmp-") || (!known.contains(name.as_ref()) && name.ends_with(".json")) {
            remove_file_if_exists(&item.path())?;
        }
    }
    Ok(())
}

fn read_manifest(cache: &Path) -> io::Result<BodyManifest> {
    ensure_bodies_dir(cache)?;
    match read_json::<BodyManifest>(&manifest_path(cache)) {
        Ok(Some(value)) if value.version == MANIFEST_VERSION => Ok(value),
        Ok(_) => Ok(empty_manifest()),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => Ok(empty_manifest()),
        Err(error) => Err(error),
    }
}

fn write_manifest(cache: &Path, manifest: &BodyManifest) -> io::Result<()> {
    write_private_json(&manifest_path(cache), manifest)
}

fn empty_manifest() -> BodyManifest {
    BodyManifest {
        version: MANIFEST_VERSION,
        entries: Vec::new(),
    }
}

fn body_file_name(id: &MessageId) -> io::Result<String> {
    let x_gm_msgid = id
        .gmail_value()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid Gmail message ID"))?;
    Ok(format!("gm-{x_gm_msgid}.json"))
}

fn validate_account_email(account_email: &str) -> io::Result<()> {
    (!account_email.trim().is_empty())
        .then_some(())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty account"))
}

fn sort_summaries(messages: &mut [MessageSummary]) {
    messages.sort_by(|left, right| {
        right
            .received_at_unix
            .cmp(&left.received_at_unix)
            .then_with(|| right.id.0.cmp(&left.id.0))
    });
}

fn folder_key(id: &FolderId) -> String {
    match id {
        FolderId::Inbox => "0-inbox".into(),
        FolderId::Starred => "1-starred".into(),
        FolderId::Sent => "2-sent".into(),
        FolderId::AllMail => "3-all".into(),
        FolderId::Trash => "4-trash".into(),
        FolderId::Label(mailbox) => format!("5-{mailbox}"),
    }
}

fn valid_stored_mailbox(stored: &StoredMailbox) -> bool {
    if !(stored.version == MAILBOX_VERSION || stored.version == PREVIOUS_MAILBOX_VERSION)
        || stored.account_email.trim().is_empty()
        || stored.account_email.len() > 320
        || stored.account_email.chars().any(char::is_control)
        || stored.views.len() > MAX_CACHED_FOLDER_VIEWS
        || stored.folder_catalog.folders.len() > crate::model::MAX_FOLDER_CATALOG_ENTRIES
        || stored
            .folder_catalog
            .folders
            .iter()
            .any(|folder| !folder.is_valid())
    {
        return false;
    }
    let mut folders = HashSet::new();
    stored.inbox_watermark.observed_ids.len() <= MAX_SUMMARIES_PER_FOLDER
        && stored
            .inbox_watermark
            .observed_ids
            .iter()
            .all(|id| id.gmail_value().is_some())
        && {
            let mut watermark_ids = HashSet::new();
            stored
                .inbox_watermark
                .observed_ids
                .iter()
                .all(|id| watermark_ids.insert(id.clone()))
        }
        && stored.views.iter().all(|view| {
            let mut ids = HashSet::new();
            is_valid_limit(view.requested_limit)
                && view.messages.len() <= MAX_SUMMARIES_PER_FOLDER
                && folders.insert(view.folder_id.clone())
                && stored.folder_catalog.find(&view.folder_id).is_some()
                && view.messages.iter().all(|message| {
                    message.id.gmail_value().is_some()
                        && ids.insert(message.id.clone())
                        && message.labels.len() <= crate::model::MAX_MESSAGE_LABELS
                        && message.labels.iter().all(|label| {
                            !label.is_empty()
                                && label.len() <= crate::model::MAX_FOLDER_MAILBOX_BYTES
                                && !label.chars().any(char::is_control)
                        })
                        && message.folder_id == view.folder_id
                        && message.locator.folder_id == view.folder_id
                        && message.locator.is_valid()
                        && stored
                            .folder_catalog
                            .find(&view.folder_id)
                            .is_some_and(|folder| folder.mailbox == message.locator.mailbox)
                })
        })
}

fn snapshot_from_stored(catalog: &FolderCatalog, stored: &StoredFolderView) -> MailboxSnapshot {
    MailboxSnapshot {
        folder_catalog: catalog.clone(),
        metadata: SyncMetadata {
            completed_at: UNIX_EPOCH + std::time::Duration::from_secs(stored.completed_at_unix),
            requested_limit: stored.requested_limit,
            loaded_count: stored.messages.len(),
            fallback_count: stored
                .messages
                .iter()
                .filter(|message| message.used_fallback)
                .count(),
            skipped_count: stored.skipped_count,
        },
        messages: stored.messages.clone(),
    }
}

fn config_root() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|path| path.join(".config"))
        })
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn cache_root() -> PathBuf {
    env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|path| path.join(".cache"))
        })
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn preferences_path(config: &Path) -> PathBuf {
    config.join("whitford/preferences.json")
}

fn account_registry_path(config: &Path) -> PathBuf {
    config.join("whitford/accounts-v1.json")
}

fn accounts_cache_root(cache: &Path) -> PathBuf {
    cache.join("whitford/accounts-v1")
}

fn legacy_cache_migration_marker_path(cache: &Path) -> PathBuf {
    cache.join("whitford/legacy-cache-migration-v1.json")
}

fn mailbox_path(cache: &Path) -> PathBuf {
    cache.join("whitford/mailbox-v3.json")
}

fn v2_mailbox_path(cache: &Path) -> PathBuf {
    cache.join("whitford/mailbox-v2.json")
}

fn legacy_mailbox_path(cache: &Path) -> PathBuf {
    cache.join("whitford/mailbox-v1.json")
}

fn bodies_path(cache: &Path) -> PathBuf {
    cache.join("whitford/bodies-v2")
}

fn legacy_bodies_path(cache: &Path) -> PathBuf {
    cache.join("whitford/bodies-v1")
}

fn attachments_path(cache: &Path) -> PathBuf {
    cache.join("whitford/attachments-v1")
}

fn account_cleanup_marker_path(cache: &Path) -> PathBuf {
    cache.join("whitford/account-cleanup-v1.json")
}

fn account_cleanup_pending(cache: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(account_cleanup_marker_path(cache)) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn manifest_path(cache: &Path) -> PathBuf {
    bodies_path(cache).join("manifest.json")
}

fn validate_limit(limit: usize) -> io::Result<()> {
    is_valid_limit(limit)
        .then_some(())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid cache limit"))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<Option<T>> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.len() > MAX_CACHE_JSON_BYTES => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "cache record exceeds size limit",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn ensure_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn ensure_bodies_dir(cache: &Path) -> io::Result<()> {
    ensure_private_dir(&cache.join("whitford"))?;
    ensure_private_dir(&bodies_path(cache))
}

fn sync_mail_cache_dir(cache: &Path) -> io::Result<()> {
    match fs::File::open(cache.join("whitford")) {
        Ok(directory) => directory.sync_all(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn write_private_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing parent"))?;
    ensure_private_dir(parent)?;
    let serial = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_extension(format!("tmp-{}-{serial}", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        serde_json::to_writer(&mut file, value).map_err(io::Error::other)?;
        file.flush()?;
        file.sync_all()?;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn copy_private_file_if_exists(source: &Path, destination: &Path) -> io::Result<()> {
    match fs::symlink_metadata(source) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
            io::Error::new(io::ErrorKind::InvalidData, "unsafe legacy cache file"),
        ),
        Ok(metadata) if metadata.len() > MAX_CACHE_JSON_BYTES => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "legacy cache record exceeds size limit",
        )),
        Ok(_) => {
            let bytes = fs::read(source)?;
            write_private_bytes(destination, &bytes)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn copy_private_dir_if_exists(source: &Path, destination: &Path) -> io::Result<()> {
    let entries = match fs::read_dir(source) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    ensure_private_dir(destination)?;
    for entry in entries {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsafe legacy cache directory entry",
            ));
        }
        let name = entry.file_name();
        copy_private_file_if_exists(&entry.path(), &destination.join(name))?;
    }
    Ok(())
}

fn write_private_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing parent"))?;
    ensure_private_dir(parent)?;
    let serial = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_extension(format!("tmp-{}-{serial}", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn attachment_file_name(
    account_email: &str,
    id: &MessageId,
    attachment: &Attachment,
) -> io::Result<String> {
    let gmail_id = id
        .gmail_value()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid Gmail message ID"))?;
    if !attachment.part.is_valid() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid attachment part",
        ));
    }
    let part = attachment
        .part
        .path
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join("-");
    let extension = Path::new(&attachment.name)
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 10
                && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .map(|value| format!(".{}", value.to_ascii_lowercase()))
        .unwrap_or_default();
    let account_key = account_email
        .bytes()
        .map(|byte| byte.to_ascii_lowercase())
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    Ok(format!(
        "acct-{account_key:016x}-gm-{gmail_id}-part-{part}{extension}"
    ))
}

fn atomic_attachment(destination: PathBuf) -> io::Result<AtomicAttachment> {
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::other("missing attachment parent"))?;
    let serial = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".whitford-attachment-{}-{serial}.part",
        std::process::id()
    ));
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    Ok(AtomicAttachment {
        file: Some(file),
        temporary,
        destination,
        committed: false,
    })
}

fn file_size_if_exists(path: &Path) -> io::Result<u64> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

fn directory_size(path: &Path) -> io::Result<u64> {
    match fs::read_dir(path) {
        Ok(mut items) => items.try_fold(0_u64, |total, item| {
            let item = item?;
            Ok(total.saturating_add(if item.file_type()?.is_file() {
                item.metadata()?.len()
            } else {
                0
            }))
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

fn remove_file_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_dir_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AttachmentState, FolderId, MimePartDescriptor, TransferEncoding};
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);
    const EMAIL: &str = "me@example.com";
    struct TestRoot(PathBuf);
    impl TestRoot {
        fn new() -> Self {
            let serial = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "whitford-cache-test-{}-{serial}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn account_id(value: char) -> AccountId {
        AccountId::new(format!("acct-{}", value.to_string().repeat(32))).unwrap()
    }

    #[test]
    fn registry_persists_opaque_ids_and_rejects_duplicate_gmail_identity() {
        let config = TestRoot::new();
        let record = AccountRecord::new(
            account_id('a'),
            AccountIdentity {
                provider: MailProvider::Gmail,
                email: "Me@Example.com".into(),
            },
        );
        add_account_record_at(&config.0, record.clone()).unwrap();
        assert_eq!(
            load_account_registry_at(&config.0).unwrap().accounts,
            vec![record]
        );
        let duplicate = AccountRecord::new(
            account_id('b'),
            AccountIdentity {
                provider: MailProvider::Gmail,
                email: "me@example.com".into(),
            },
        );
        assert_eq!(
            add_account_record_at(&config.0, duplicate)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn legacy_cache_staging_is_recoverable_and_finalize_is_account_scoped() {
        let root = TestRoot::new();
        let account = account_id('a');
        write_private_json(&mailbox_path(&root.0), &serde_json::json!({"cache": 1})).unwrap();
        ensure_bodies_dir(&root.0).unwrap();
        fs::write(bodies_path(&root.0).join("gm-1.json"), b"body").unwrap();
        ensure_private_dir(&attachments_path(&root.0)).unwrap();
        fs::write(attachments_path(&root.0).join("part"), b"attachment").unwrap();

        let namespace = stage_legacy_cache_migration_at(&root.0, &account).unwrap();
        assert!(mailbox_path(&root.0).exists());
        assert!(namespace.mailbox_path().exists());
        assert_eq!(
            fs::read(namespace.bodies_path().join("gm-1.json")).unwrap(),
            b"body"
        );
        assert_eq!(
            fs::read(namespace.attachments_path().join("part")).unwrap(),
            b"attachment"
        );
        // Idempotent retry after a crash retains both source and copied cache.
        assert_eq!(
            stage_legacy_cache_migration_at(&root.0, &account).unwrap(),
            namespace
        );

        finalize_legacy_cache_migration_at(&root.0, &account).unwrap();
        assert!(!mailbox_path(&root.0).exists());
        assert!(namespace.mailbox_path().exists());
        assert!(clear_account_cache_at(&root.0, &account).is_ok());
        assert!(!namespace.root.exists());
    }

    fn summary(uid: u32) -> MessageSummary {
        MessageSummary {
            id: MessageId::gmail(u64::from(uid)),
            folder_id: FolderId::Inbox,
            locator: crate::model::MessageLocator {
                folder_id: FolderId::Inbox,
                mailbox: "INBOX".into(),
                uid_validity: 7,
                uid,
            },
            sender: format!("Sender {uid}"),
            email: None,
            initials: None,
            subject: format!("Subject {uid}"),
            received_at_unix: Some(i64::from(uid)),
            unread: false,
            starred: false,
            in_inbox: true,
            in_trash: false,
            labels: Vec::new(),
            attachment_state: AttachmentState::Known(Vec::new()),
            used_fallback: false,
        }
    }

    fn snapshot(ids: &[u32]) -> MailboxSnapshot {
        MailboxSnapshot {
            messages: ids.iter().copied().map(summary).collect(),
            folder_catalog: crate::model::FolderCatalog::inbox_only(),
            metadata: SyncMetadata {
                completed_at: UNIX_EPOCH + std::time::Duration::from_secs(100),
                requested_limit: 50,
                loaded_count: ids.len(),
                fallback_count: 0,
                skipped_count: 0,
            },
        }
    }

    fn body(text: &str) -> MessageBody {
        MessageBody {
            text: text.into(),
            html: None,
            attachments: Vec::new(),
            reply_context: crate::model::ReplyContext::default(),
            used_fallback: false,
        }
    }

    fn account() -> AccountIdentity {
        AccountIdentity {
            provider: MailProvider::Gmail,
            email: EMAIL.into(),
        }
    }

    fn received_attachment(name: &str) -> Attachment {
        Attachment {
            name: name.into(),
            media_type: Some("application/pdf".into()),
            octets: Some(12),
            part: MimePartDescriptor {
                path: vec![2, 3],
                encoding: TransferEncoding::Base64,
                encoded_octets: 12,
            },
        }
    }

    #[test]
    fn attachment_cache_uses_private_opaque_atomic_files() {
        let root = TestRoot::new();
        let attachment = received_attachment("../../private report.PDF");
        let OpenAttachmentTarget::Download(mut output) =
            prepare_open_attachment_at(&root.0, EMAIL, &MessageId::gmail(42), &attachment).unwrap()
        else {
            panic!("unexpected cache hit")
        };
        assert!(
            !output
                .destination
                .to_string_lossy()
                .contains("private report")
        );
        assert_eq!(
            fs::metadata(output.temporary.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&output.temporary)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        output.write_all(b"durable bytes").unwrap();
        let path = output.finish().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"durable bytes");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(matches!(
            prepare_open_attachment_at(&root.0, EMAIL, &MessageId::gmail(42), &attachment)
                .unwrap(),
            OpenAttachmentTarget::Cached(cached) if cached == path
        ));
        clear_bodies_at(&root.0).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn cancelled_atomic_save_preserves_destination_and_removes_partial() {
        let root = TestRoot::new();
        let destination = root.0.join("report.pdf");
        fs::write(&destination, b"existing").unwrap();
        let mut output = prepare_save_attachment(&destination).unwrap();
        let temporary = output.temporary.clone();
        output.write_all(b"replacement").unwrap();
        drop(output);
        assert_eq!(fs::read(destination).unwrap(), b"existing");
        assert!(!temporary.exists());
    }

    #[test]
    fn save_as_rejects_symlinked_paths_and_targets() {
        let root = TestRoot::new();
        let real = root.0.join("real");
        fs::create_dir(&real).unwrap();
        let alias = root.0.join("alias");
        symlink(&real, &alias).unwrap();
        assert!(prepare_save_attachment(&alias.join("file.pdf")).is_err());
        let target = real.join("target.pdf");
        let outside = root.0.join("outside.pdf");
        fs::write(&outside, b"outside").unwrap();
        symlink(&outside, &target).unwrap();
        assert!(prepare_save_attachment(&target).is_err());
        assert_eq!(fs::read(outside).unwrap(), b"outside");
    }

    #[test]
    fn attachment_cache_pruning_is_bounded_and_preserves_current_file() {
        let root = TestRoot::new();
        let directory = attachments_path(&root.0);
        ensure_private_dir(&directory).unwrap();
        for name in ["a", "b", "c"] {
            fs::write(directory.join(name), b"12345").unwrap();
        }
        let protected = directory.join("c");
        prune_attachment_cache_at(&root.0, &protected, 8).unwrap();
        assert!(!directory.join("a").exists());
        assert!(!directory.join("b").exists());
        assert!(protected.exists());
    }

    #[test]
    fn retention_choices_are_stable() {
        assert_eq!(RETENTION_OPTIONS, [50, 100, 250, 500]);
        assert_eq!(limit_at(selected_index(250)), Some(250));
        assert!(!is_valid_limit(51));
    }

    #[test]
    fn preferences_missing_file_uses_system_defaults() {
        let config = TestRoot::new();
        assert_eq!(load_preferences_from(&config.0), Preferences::default());
    }

    #[test]
    fn retention_only_preferences_migrate_to_system_appearance() {
        let config = TestRoot::new();
        write_private_json(
            &preferences_path(&config.0),
            &serde_json::json!({ "retained_messages": 250 }),
        )
        .unwrap();

        assert_eq!(
            load_preferences_from(&config.0),
            Preferences::new(250, AppearancePreference::System)
        );
    }

    #[test]
    fn malformed_appearance_does_not_discard_valid_retention() {
        let config = TestRoot::new();
        write_private_json(
            &preferences_path(&config.0),
            &serde_json::json!({
                "version": PREFERENCES_VERSION,
                "retained_messages": 500,
                "appearance": "Solarized"
            }),
        )
        .unwrap();

        assert_eq!(
            load_preferences_from(&config.0),
            Preferences::new(500, AppearancePreference::System)
        );
    }

    #[test]
    fn invalid_preferences_retention_is_rejected() {
        let config = TestRoot::new();
        write_private_json(
            &preferences_path(&config.0),
            &serde_json::json!({
                "version": PREFERENCES_VERSION,
                "retained_messages": 51,
                "appearance": "Dark"
            }),
        )
        .unwrap();
        assert_eq!(load_preferences_from(&config.0), Preferences::default());
    }

    #[test]
    fn complete_preferences_round_trip_in_current_version() {
        let config = TestRoot::new();
        let cache = TestRoot::new();
        let preferences = Preferences::new(250, AppearancePreference::Dark);
        save_preferences_and_prune_at(&config.0, &cache.0, preferences.clone()).unwrap();

        assert_eq!(load_preferences_from(&config.0), preferences);
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(preferences_path(&config.0)).unwrap()).unwrap();
        assert_eq!(stored["version"], PREFERENCES_VERSION);
        assert_eq!(stored["appearance"], "Dark");
    }

    #[test]
    fn preferences_write_error_is_returned_without_modifying_existing_file() {
        let config = TestRoot::new();
        let cache = TestRoot::new();
        let obstructing_path = config.0.join("whitford");
        fs::write(&obstructing_path, b"not a directory").unwrap();

        let result = save_preferences_and_prune_at(
            &config.0,
            &cache.0,
            Preferences::new(250, AppearancePreference::Dark),
        );

        assert!(result.is_err());
        assert_eq!(fs::read(obstructing_path).unwrap(), b"not a directory");
    }

    #[test]
    fn v3_membership_is_authoritative_and_removes_legacy_indexes_after_save() {
        let root = TestRoot::new();
        write_private_json(&legacy_mailbox_path(&root.0), &serde_json::json!({})).unwrap();
        write_private_json(&v2_mailbox_path(&root.0), &serde_json::json!({})).unwrap();
        replace_and_save_at(&root.0, &account(), snapshot(&[1, 2]), 50).unwrap();
        replace_and_save_at(&root.0, &account(), snapshot(&[3]), 50).unwrap();
        let (_, loaded) = load_latest_from(&root.0, 50).unwrap().unwrap();
        assert_eq!(loaded.messages, vec![summary(3)]);
        assert!(!legacy_mailbox_path(&root.0).exists());
        assert!(!v2_mailbox_path(&root.0).exists());
    }

    #[test]
    fn corrupt_summary_is_a_miss() {
        let root = TestRoot::new();
        write_private_json(&mailbox_path(&root.0), &serde_json::json!({"bad": true})).unwrap();
        assert!(load_latest_from(&root.0, 50).unwrap().is_none());
        fs::write(mailbox_path(&root.0), b"bad json").unwrap();
        assert!(load_latest_from(&root.0, 50).unwrap().is_none());
    }

    #[test]
    fn oversized_cache_json_is_rejected_before_allocation() {
        let root = TestRoot::new();
        ensure_private_dir(&root.0.join("whitford")).unwrap();
        let file = fs::File::create(mailbox_path(&root.0)).unwrap();
        file.set_len(MAX_CACHE_JSON_BYTES + 1).unwrap();
        let result = read_json::<StoredMailbox>(&mailbox_path(&root.0));
        assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::InvalidData));
    }

    #[test]
    fn body_round_trip_usage_and_modes() {
        let root = TestRoot::new();
        let id = MessageId::gmail(9);
        let expected = body("");
        let usage = save_body_at(&root.0, EMAIL, &id, &expected, 10, u64::MAX).unwrap();
        assert_eq!(usage.body_count, 1);
        assert_eq!(
            load_body_at(&root.0, EMAIL, &id, 20).unwrap(),
            Some(expected)
        );
        assert_eq!(
            fs::metadata(bodies_path(&root.0))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.0.join("whitford"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(bodies_path(&root.0).join("gm-9.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn previous_body_schema_is_removed_as_a_cache_miss() {
        let root = TestRoot::new();
        let id = MessageId::gmail(9);
        save_body_at(&root.0, EMAIL, &id, &body("old"), 10, u64::MAX).unwrap();
        let path = bodies_path(&root.0).join("gm-9.json");
        let mut stored: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        stored["version"] = serde_json::json!(BODY_VERSION - 1);
        write_private_json(&path, &stored).unwrap();

        assert_eq!(load_body_at(&root.0, EMAIL, &id, 20).unwrap(), None);
        assert!(!path.exists());
    }

    #[test]
    fn invalid_ids_cannot_become_paths() {
        let root = TestRoot::new();
        let id = MessageId("gmail:1:../../secret".into());
        assert_eq!(
            save_body_at(&root.0, EMAIL, &id, &body("x"), 1, 10)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn lru_evicts_oldest_and_keeps_oversize_current() {
        let root = TestRoot::new();
        let first = MessageId::gmail(1);
        let second = MessageId::gmail(2);
        save_body_at(&root.0, EMAIL, &first, &body(&"a".repeat(200)), 1, u64::MAX).unwrap();
        let first_bytes = fs::metadata(bodies_path(&root.0).join("gm-1.json"))
            .unwrap()
            .len();
        save_body_at(
            &root.0,
            EMAIL,
            &second,
            &body(&"b".repeat(200)),
            2,
            first_bytes,
        )
        .unwrap();
        assert!(load_body_at(&root.0, EMAIL, &first, 3).unwrap().is_none());
        assert!(load_body_at(&root.0, EMAIL, &second, 3).unwrap().is_some());
        let third = MessageId::gmail(3);
        save_body_at(&root.0, EMAIL, &third, &body(&"x".repeat(1000)), 4, 1).unwrap();
        assert!(load_body_at(&root.0, EMAIL, &third, 5).unwrap().is_some());
    }

    #[test]
    fn canonical_bodies_survive_folder_membership_and_locator_changes() {
        let root = TestRoot::new();
        let kept = MessageId::gmail(1);
        let orphan = MessageId::gmail(2);
        save_body_at(&root.0, EMAIL, &kept, &body("keep"), 1, u64::MAX).unwrap();
        save_body_at(&root.0, EMAIL, &orphan, &body("reuse"), 2, u64::MAX).unwrap();
        let mut remapped = snapshot(&[1]);
        remapped.messages[0].locator.uid_validity = 99;
        remapped.messages[0].locator.uid = 500;
        replace_and_save_at(&root.0, &account(), remapped, 50).unwrap();
        assert!(load_body_at(&root.0, EMAIL, &kept, 3).unwrap().is_some());
        assert_eq!(
            load_body_at(&root.0, EMAIL, &orphan, 3).unwrap(),
            Some(body("reuse"))
        );
        let loaded = load_latest_from(&root.0, 50).unwrap().unwrap().1;
        assert_eq!(loaded.messages[0].locator.uid_validity, 99);
        assert_eq!(loaded.messages[0].locator.uid, 500);
    }

    #[test]
    fn account_change_drops_even_colliding_body_ids() {
        let root = TestRoot::new();
        replace_and_save_at(&root.0, &account(), snapshot(&[1]), 50).unwrap();
        let id = MessageId::gmail(1);
        save_body_at(&root.0, EMAIL, &id, &body("private"), 1, u64::MAX).unwrap();
        let other = AccountIdentity {
            provider: MailProvider::Gmail,
            email: "other@example.com".into(),
        };
        replace_and_save_at(&root.0, &other, snapshot(&[1]), 50).unwrap();
        assert!(
            load_body_at(&root.0, "other@example.com", &id, 2)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn verified_account_switch_invalidates_old_mail_before_a_failed_new_save() {
        let root = TestRoot::new();
        replace_and_save_at(&root.0, &account(), snapshot(&[1]), 50).unwrap();
        save_body_at(
            &root.0,
            EMAIL,
            &MessageId::gmail(1),
            &body("account a private"),
            1,
            u64::MAX,
        )
        .unwrap();

        prepare_verified_account_at(&root.0, "other@example.com").unwrap();
        let other = AccountIdentity {
            provider: MailProvider::Gmail,
            email: "other@example.com".into(),
        };
        assert!(replace_and_save_at(&root.0, &other, snapshot(&[1]), 42).is_err());

        assert!(load_latest_from(&root.0, 50).unwrap().is_none());
        assert!(
            load_body_at(&root.0, EMAIL, &MessageId::gmail(1), 2)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn interrupted_account_cleanup_retries_after_the_mailbox_index_is_gone() {
        let root = TestRoot::new();
        replace_and_save_at(&root.0, &account(), snapshot(&[1]), 50).unwrap();
        save_body_at(
            &root.0,
            EMAIL,
            &MessageId::gmail(1),
            &body("account a private"),
            1,
            u64::MAX,
        )
        .unwrap();
        ensure_private_dir(&attachments_path(&root.0)).unwrap();
        fs::write(attachments_path(&root.0).join("old-account"), b"private").unwrap();
        write_private_json(
            &legacy_mailbox_path(&root.0),
            &serde_json::json!({"old": true}),
        )
        .unwrap();
        write_private_json(
            &account_cleanup_marker_path(&root.0),
            &PendingAccountCleanup {
                version: ACCOUNT_CLEANUP_VERSION,
                previous_account_email: EMAIL,
                next_account_email: "other@example.com",
            },
        )
        .unwrap();

        // A crash immediately after the durable marker is published must not expose the old
        // mailbox while the next startup is preparing to resume cleanup.
        assert!(load_latest_from(&root.0, 50).unwrap().is_none());
        assert!(
            load_body_at(&root.0, EMAIL, &MessageId::gmail(1), 2)
                .unwrap()
                .is_none()
        );

        let error = resume_pending_account_cleanup_with(&root.0, |stage| {
            if stage == AccountCleanupStage::BodyCaches {
                Err(io::Error::other("simulated process interruption"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "simulated process interruption");
        assert!(!mailbox_path(&root.0).exists());
        assert!(!legacy_mailbox_path(&root.0).exists());
        assert!(bodies_path(&root.0).exists());
        assert!(attachments_path(&root.0).exists());
        assert!(account_cleanup_marker_path(&root.0).exists());

        prepare_verified_account_at(&root.0, "other@example.com").unwrap();
        assert!(!mailbox_path(&root.0).exists());
        assert!(!bodies_path(&root.0).exists());
        assert!(!legacy_bodies_path(&root.0).exists());
        assert!(!attachments_path(&root.0).exists());
        assert!(!account_cleanup_marker_path(&root.0).exists());
    }

    #[test]
    fn new_account_save_finishes_a_crashed_cleanup_before_publishing() {
        let root = TestRoot::new();
        replace_and_save_at(&root.0, &account(), snapshot(&[1]), 50).unwrap();
        ensure_private_dir(&attachments_path(&root.0)).unwrap();
        let old_attachment = attachments_path(&root.0).join("old-account");
        fs::write(&old_attachment, b"private").unwrap();
        write_private_json(
            &account_cleanup_marker_path(&root.0),
            &PendingAccountCleanup {
                version: ACCOUNT_CLEANUP_VERSION,
                previous_account_email: EMAIL,
                next_account_email: "other@example.com",
            },
        )
        .unwrap();

        let error = resume_pending_account_cleanup_with(&root.0, |stage| {
            if stage == AccountCleanupStage::Marker {
                Err(io::Error::other("simulated crash before commit"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "simulated crash before commit");
        assert!(account_cleanup_marker_path(&root.0).exists());
        assert!(!old_attachment.exists());

        let other = AccountIdentity {
            provider: MailProvider::Gmail,
            email: "other@example.com".into(),
        };
        replace_and_save_at(&root.0, &other, snapshot(&[1]), 50).unwrap();

        assert!(!account_cleanup_marker_path(&root.0).exists());
        assert!(!attachments_path(&root.0).exists());
        let stored = read_json::<StoredMailbox>(&mailbox_path(&root.0))
            .unwrap()
            .unwrap();
        assert_eq!(stored.account_email, other.email);
    }

    #[test]
    fn account_tag_rejects_colliding_body_after_interrupted_account_switch() {
        let root = TestRoot::new();
        let id = MessageId::gmail(1);
        save_body_at(&root.0, EMAIL, &id, &body("account a secret"), 1, u64::MAX).unwrap();

        // Simulate a process dying after a new account index was published but before legacy body
        // cleanup. The body record itself remains bound to account A and must not be returned.
        let other = AccountIdentity {
            provider: MailProvider::Gmail,
            email: "other@example.com".into(),
        };
        let fresh = snapshot(&[1]);
        write_private_json(
            &mailbox_path(&root.0),
            &StoredMailbox {
                version: MAILBOX_VERSION,
                account_email: other.email.clone(),
                folder_catalog: fresh.folder_catalog,
                views: vec![StoredFolderView {
                    folder_id: FolderId::Inbox,
                    completed_at_unix: 100,
                    requested_limit: 50,
                    skipped_count: 0,
                    last_accessed_unix: 100,
                    messages: fresh.messages,
                }],
                inbox_watermark: InboxWatermark::default(),
            },
        )
        .unwrap();

        assert!(
            load_body_at(&root.0, &other.email, &id, 2)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn retention_decrease_prunes_summaries_but_keeps_canonical_lru_bodies() {
        let root = TestRoot::new();
        let config = TestRoot::new();
        let ids = (1..=60).collect::<Vec<_>>();
        replace_and_save_at(&root.0, &account(), snapshot(&ids), 100).unwrap();
        let old = MessageId::gmail(1);
        save_body_at(&root.0, EMAIL, &old, &body("old"), 1, u64::MAX).unwrap();
        save_limit_and_prune_at(&config.0, &root.0, 50).unwrap();
        assert_eq!(
            load_latest_from(&root.0, 50)
                .unwrap()
                .unwrap()
                .1
                .metadata
                .requested_limit,
            50
        );
        assert_eq!(
            load_latest_from(&root.0, 50)
                .unwrap()
                .unwrap()
                .1
                .messages
                .len(),
            50
        );
        assert!(load_body_at(&root.0, EMAIL, &old, 2).unwrap().is_some());
    }

    #[test]
    fn clear_bodies_preserves_summary_and_preferences() {
        let root = TestRoot::new();
        let config = TestRoot::new();
        save_limit_and_prune_at(&config.0, &root.0, 250).unwrap();
        replace_and_save_at(&root.0, &account(), snapshot(&[1]), 50).unwrap();
        save_body_at(
            &root.0,
            EMAIL,
            &MessageId::gmail(1),
            &body("cached"),
            1,
            u64::MAX,
        )
        .unwrap();
        assert!(clear_bodies_at(&root.0).unwrap() > 0);
        assert_eq!(clear_bodies_at(&root.0).unwrap(), 0);
        assert!(mailbox_path(&root.0).exists());
        assert_eq!(load_limit_from(&config.0), 250);
    }

    #[test]
    fn clear_all_removes_all_mail_formats() {
        let root = TestRoot::new();
        write_private_json(&legacy_mailbox_path(&root.0), &serde_json::json!({})).unwrap();
        write_private_json(&v2_mailbox_path(&root.0), &serde_json::json!({})).unwrap();
        write_private_json(&mailbox_path(&root.0), &serde_json::json!({})).unwrap();
        fs::create_dir_all(legacy_bodies_path(&root.0)).unwrap();
        fs::write(legacy_bodies_path(&root.0).join("old.json"), b"old").unwrap();
        save_body_at(
            &root.0,
            EMAIL,
            &MessageId::gmail(1),
            &body("x"),
            1,
            u64::MAX,
        )
        .unwrap();
        ensure_private_dir(&attachments_path(&root.0)).unwrap();
        fs::write(attachments_path(&root.0).join("old"), b"old").unwrap();
        write_private_json(
            &account_cleanup_marker_path(&root.0),
            &PendingAccountCleanup {
                version: ACCOUNT_CLEANUP_VERSION,
                previous_account_email: EMAIL,
                next_account_email: "other@example.com",
            },
        )
        .unwrap();
        clear_all_mail_at(&root.0).unwrap();
        assert!(!legacy_mailbox_path(&root.0).exists());
        assert!(!v2_mailbox_path(&root.0).exists());
        assert!(!mailbox_path(&root.0).exists());
        assert!(!bodies_path(&root.0).exists());
        assert!(!legacy_bodies_path(&root.0).exists());
        assert!(!attachments_path(&root.0).exists());
        assert!(!account_cleanup_marker_path(&root.0).exists());
    }

    #[test]
    fn folder_cache_caps_each_view_and_keeps_only_eight_recent_views() {
        let root = TestRoot::new();
        let descriptors = (0..10)
            .map(|index| crate::model::FolderDescriptor {
                id: FolderId::Label(format!("Label {index}")),
                mailbox: format!("Label {index}"),
                display_name: format!("Label {index}"),
                kind: crate::model::FolderKind::Label,
            })
            .chain(crate::model::FolderCatalog::inbox_only().folders)
            .collect::<Vec<_>>();
        let catalog = crate::model::FolderCatalog::bounded(descriptors);
        for index in 0..9_u32 {
            let folder_id = FolderId::Label(format!("Label {index}"));
            let messages = (1..=600_u32)
                .map(|uid| {
                    let mut value = summary(uid);
                    value.id = MessageId::gmail(u64::from(index) * 1_000 + u64::from(uid));
                    value.folder_id = folder_id.clone();
                    value.locator = crate::model::MessageLocator {
                        folder_id: folder_id.clone(),
                        mailbox: format!("Label {index}"),
                        uid_validity: index + 1,
                        uid,
                    };
                    value
                })
                .collect();
            replace_folder_and_save_at(
                &root.0,
                &account(),
                folder_id,
                MailboxSnapshot {
                    messages,
                    metadata: SyncMetadata {
                        completed_at: UNIX_EPOCH,
                        requested_limit: 500,
                        loaded_count: 600,
                        fallback_count: 0,
                        skipped_count: 0,
                    },
                    folder_catalog: catalog.clone(),
                },
                500,
                u64::from(index + 1),
            )
            .unwrap();
        }

        let stored = read_json::<StoredMailbox>(&mailbox_path(&root.0))
            .unwrap()
            .unwrap();
        assert!(valid_stored_mailbox(&stored));
        assert_eq!(stored.views.len(), MAX_CACHED_FOLDER_VIEWS);
        assert!(
            stored
                .views
                .iter()
                .all(|view| view.messages.len() == MAX_SUMMARIES_PER_FOLDER)
        );
        assert!(
            load_folder_from(&root.0, EMAIL, &FolderId::Label("Label 0".into()), 500, 20,)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            load_folder_from(&root.0, EMAIL, &FolderId::Label("Label 8".into()), 500, 20,)
                .unwrap()
                .unwrap()
                .messages
                .len(),
            500
        );

        load_folder_from(&root.0, EMAIL, &FolderId::Label("Label 1".into()), 500, 30)
            .unwrap()
            .unwrap();
        let folder_id = FolderId::Label("Label 9".into());
        let mut value = summary(1);
        value.id = MessageId::gmail(9_001);
        value.folder_id = folder_id.clone();
        value.locator = crate::model::MessageLocator {
            folder_id: folder_id.clone(),
            mailbox: "Label 9".into(),
            uid_validity: 10,
            uid: 1,
        };
        replace_folder_and_save_at(
            &root.0,
            &account(),
            folder_id,
            MailboxSnapshot {
                messages: vec![value],
                metadata: SyncMetadata {
                    completed_at: UNIX_EPOCH,
                    requested_limit: 500,
                    loaded_count: 1,
                    fallback_count: 0,
                    skipped_count: 0,
                },
                folder_catalog: catalog,
            },
            500,
            21,
        )
        .unwrap();
        assert!(
            load_folder_from(&root.0, EMAIL, &FolderId::Label("Label 1".into()), 500, 31,)
                .unwrap()
                .is_some()
        );
        assert!(
            load_folder_from(&root.0, EMAIL, &FolderId::Label("Label 2".into()), 500, 31,)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn only_authoritative_mutations_change_the_summary_cache() {
        let root = TestRoot::new();
        replace_and_save_at(&root.0, &account(), snapshot(&[1, 2]), 50).unwrap();
        persist_confirmed_mutation_at(
            &root.0,
            EMAIL,
            &MessageId::gmail(1),
            &MessageMutation::SetStarred(true),
        )
        .unwrap();
        let loaded = load_folder_from(&root.0, EMAIL, &FolderId::Inbox, 50, 20)
            .unwrap()
            .unwrap();
        assert!(
            loaded
                .messages
                .iter()
                .find(|message| message.id == MessageId::gmail(1))
                .unwrap()
                .starred
        );

        persist_reconciled_inbox_at(&root.0, EMAIL, &MessageId::gmail(1), None).unwrap();
        let loaded = load_folder_from(&root.0, EMAIL, &FolderId::Inbox, 50, 21)
            .unwrap()
            .unwrap();
        assert_eq!(
            loaded
                .messages
                .iter()
                .map(|message| message.id.clone())
                .collect::<Vec<_>>(),
            vec![MessageId::gmail(2)]
        );
    }

    #[test]
    fn confirmed_archive_updates_inbox_without_evicting_all_mail_projection() {
        let root = TestRoot::new();
        let catalog = crate::model::FolderCatalog::bounded(vec![
            crate::model::FolderDescriptor {
                id: FolderId::Inbox,
                mailbox: "INBOX".into(),
                display_name: "Inbox".into(),
                kind: crate::model::FolderKind::Inbox,
            },
            crate::model::FolderDescriptor {
                id: FolderId::AllMail,
                mailbox: "[Gmail]/All Mail".into(),
                display_name: "All Mail".into(),
                kind: crate::model::FolderKind::AllMail,
            },
        ]);
        let mut inbox = snapshot(&[1]);
        inbox.folder_catalog = catalog.clone();
        replace_folder_and_save_at(&root.0, &account(), FolderId::Inbox, inbox, 50, 1).unwrap();

        let mut all_mail = snapshot(&[1]);
        all_mail.folder_catalog = catalog;
        all_mail.messages[0].folder_id = FolderId::AllMail;
        all_mail.messages[0].locator.folder_id = FolderId::AllMail;
        all_mail.messages[0].locator.mailbox = "[Gmail]/All Mail".into();
        replace_folder_and_save_at(&root.0, &account(), FolderId::AllMail, all_mail, 50, 2)
            .unwrap();

        persist_confirmed_mutation_at(
            &root.0,
            EMAIL,
            &MessageId::gmail(1),
            &MessageMutation::Archive,
        )
        .unwrap();
        assert!(
            load_folder_from(&root.0, EMAIL, &FolderId::Inbox, 50, 3)
                .unwrap()
                .unwrap()
                .messages
                .is_empty()
        );
        assert_eq!(
            load_folder_from(&root.0, EMAIL, &FolderId::AllMail, 50, 3)
                .unwrap()
                .unwrap()
                .messages
                .iter()
                .map(|message| message.id.clone())
                .collect::<Vec<_>>(),
            vec![MessageId::gmail(1)]
        );
    }

    #[test]
    fn notification_gate_baselines_then_emits_only_new_unread_ids() {
        let root = TestRoot::new();
        let mut initial = snapshot(&[1, 2]);
        initial.messages[0].unread = true;
        let first =
            replace_inbox_and_save_with_notification_gate_at(&root.0, &account(), initial, 50)
                .unwrap();
        assert!(first.new_unread_ids.is_empty());

        let mut next = snapshot(&[1, 2, 3, 4]);
        next.messages
            .iter_mut()
            .find(|m| m.id == MessageId::gmail(3))
            .unwrap()
            .unread = true;
        // A newly delivered, already-read message must not be announced.
        next.messages
            .iter_mut()
            .find(|m| m.id == MessageId::gmail(4))
            .unwrap()
            .unread = false;
        let commit =
            replace_inbox_and_save_with_notification_gate_at(&root.0, &account(), next, 50)
                .unwrap();
        assert_eq!(commit.new_unread_ids, vec![MessageId::gmail(3)]);

        // A known ID toggling unread is not a new delivery.
        let mut reread = commit.snapshot;
        reread
            .messages
            .iter_mut()
            .find(|m| m.id == MessageId::gmail(1))
            .unwrap()
            .unread = true;
        assert!(
            replace_inbox_and_save_with_notification_gate_at(&root.0, &account(), reread, 50,)
                .unwrap()
                .new_unread_ids
                .is_empty()
        );
    }

    #[test]
    fn v3_missing_watermark_migrates_as_a_silent_baseline() {
        let root = TestRoot::new();
        let fresh = snapshot(&[1]);
        write_private_json(
            &mailbox_path(&root.0),
            &serde_json::json!({
                "version": PREVIOUS_MAILBOX_VERSION,
                "account_email": EMAIL,
                "folder_catalog": fresh.folder_catalog,
                "views": [{
                    "folder_id": "Inbox", "completed_at_unix": 100,
                    "requested_limit": 50, "skipped_count": 0, "last_accessed_unix": 100,
                    "messages": fresh.messages,
                }],
            }),
        )
        .unwrap();
        let mut incoming = snapshot(&[1, 2]);
        incoming
            .messages
            .iter_mut()
            .for_each(|message| message.unread = true);
        assert!(
            replace_inbox_and_save_with_notification_gate_at(&root.0, &account(), incoming, 50,)
                .unwrap()
                .new_unread_ids
                .is_empty()
        );
        let stored = read_json::<StoredMailbox>(&mailbox_path(&root.0))
            .unwrap()
            .unwrap();
        assert_eq!(stored.version, MAILBOX_VERSION);
        assert!(stored.inbox_watermark.initialized);
        assert_eq!(stored.inbox_watermark.observed_ids.len(), 2);
    }
}
