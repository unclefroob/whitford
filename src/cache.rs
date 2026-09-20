use crate::model::{
    AccountIdentity, CacheUsage, MailProvider, MailboxSnapshot, MessageBody, MessageId,
    MessageSummary, SyncMetadata,
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
const MAILBOX_VERSION: u8 = 2;
const BODY_VERSION: u8 = 3;
const MANIFEST_VERSION: u8 = 2;
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

#[derive(Deserialize, Serialize)]
struct Preferences {
    retained_messages: usize,
}

#[derive(Deserialize, Serialize)]
struct StoredMailbox {
    version: u8,
    account_email: String,
    completed_at_unix: u64,
    requested_limit: usize,
    skipped_count: usize,
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
    load_limit_from(&config_root())
}

pub fn save_limit_and_prune(limit: usize) -> io::Result<()> {
    save_limit_and_prune_at(&config_root(), &cache_root(), limit)
}

pub fn load_latest(limit: usize) -> io::Result<Option<(AccountIdentity, MailboxSnapshot)>> {
    load_latest_from(&cache_root(), limit)
}

/// Saves the server's authoritative newest-N membership. Old absent rows are not merged.
pub fn replace_and_save(
    account: &AccountIdentity,
    fresh: MailboxSnapshot,
    limit: usize,
) -> io::Result<MailboxSnapshot> {
    replace_and_save_at(&cache_root(), account, fresh, limit)
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

pub fn usage() -> io::Result<CacheUsage> {
    usage_at(&cache_root())
}

/// Removes v1/v2 summaries and bodies, but never preferences or credentials.
pub fn clear_all_mail() -> io::Result<()> {
    clear_all_mail_at(&cache_root())
}

pub fn clear_mailbox() -> io::Result<()> {
    clear_all_mail()
}

fn load_limit_from(config: &Path) -> usize {
    read_json::<Preferences>(&preferences_path(config))
        .ok()
        .flatten()
        .map(|value| value.retained_messages)
        .filter(|value| is_valid_limit(*value))
        .unwrap_or(DEFAULT_RETENTION)
}

fn save_limit_and_prune_at(config: &Path, cache: &Path, limit: usize) -> io::Result<()> {
    validate_limit(limit)?;
    write_private_json(
        &preferences_path(config),
        &Preferences {
            retained_messages: limit,
        },
    )?;
    if let Some(mut stored) = read_json::<StoredMailbox>(&mailbox_path(cache))?
        && stored.version == MAILBOX_VERSION
    {
        sort_summaries(&mut stored.messages);
        stored.messages.truncate(limit);
        stored.requested_limit = limit;
        write_private_json(&mailbox_path(cache), &stored)?;
        let retained = stored
            .messages
            .iter()
            .map(|message| message.id.clone())
            .collect();
        reconcile_bodies(
            cache,
            &stored.account_email,
            Some(&retained),
            None,
            BODY_CACHE_BUDGET_BYTES,
        )?;
    }
    Ok(())
}

fn load_latest_from(
    cache: &Path,
    limit: usize,
) -> io::Result<Option<(AccountIdentity, MailboxSnapshot)>> {
    validate_limit(limit)?;
    let stored = match read_json::<StoredMailbox>(&mailbox_path(cache)) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::InvalidData => return Ok(None),
        Err(error) => return Err(error),
    };
    let Some(mut stored) = stored else {
        return Ok(None);
    };
    if stored.version != MAILBOX_VERSION || stored.account_email.trim().is_empty() {
        return Ok(None);
    }
    sort_summaries(&mut stored.messages);
    stored.messages.truncate(limit);
    Ok(Some((
        AccountIdentity {
            provider: MailProvider::Gmail,
            email: stored.account_email.clone(),
        },
        snapshot_from_stored(&stored),
    )))
}

fn replace_and_save_at(
    cache: &Path,
    account: &AccountIdentity,
    mut fresh: MailboxSnapshot,
    limit: usize,
) -> io::Result<MailboxSnapshot> {
    validate_limit(limit)?;
    if account.email.trim().is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty account"));
    }
    let account_changed = match read_json::<StoredMailbox>(&mailbox_path(cache)) {
        Ok(stored) => {
            stored.is_some_and(|value| !value.account_email.eq_ignore_ascii_case(&account.email))
        }
        Err(error) if error.kind() == io::ErrorKind::InvalidData => true,
        Err(error) => return Err(error),
    };
    sort_summaries(&mut fresh.messages);
    fresh.messages.truncate(limit);
    fresh.metadata.loaded_count = fresh.messages.len();
    fresh.metadata.fallback_count = fresh
        .messages
        .iter()
        .filter(|message| message.used_fallback)
        .count();
    let stored = StoredMailbox {
        version: MAILBOX_VERSION,
        account_email: account.email.clone(),
        completed_at_unix: fresh
            .metadata
            .completed_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        requested_limit: fresh.metadata.requested_limit,
        skipped_count: fresh.metadata.skipped_count,
        messages: fresh.messages.clone(),
    };
    // Remove the previous account's bodies before publishing the new account index. A crash can
    // lose reusable cache data, but can never pair one account's index with another's bodies.
    if account_changed {
        remove_dir_if_exists(&bodies_path(cache))?;
    }
    write_private_json(&mailbox_path(cache), &stored)?;
    // v1 is ignored on load and removed only after the durable v2 commit.
    let _ = remove_file_if_exists(&legacy_mailbox_path(cache));
    let retained = fresh
        .messages
        .iter()
        .map(|message| message.id.clone())
        .collect();
    reconcile_bodies(
        cache,
        &account.email,
        Some(&retained),
        None,
        BODY_CACHE_BUDGET_BYTES,
    )?;
    Ok(fresh)
}

fn load_body_at(
    cache: &Path,
    account_email: &str,
    id: &MessageId,
    accessed_at: u64,
) -> io::Result<Option<MessageBody>> {
    validate_account_email(account_email)?;
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
    let retained = authoritative_ids(cache, account_email)?;
    reconcile_bodies(cache, account_email, retained.as_ref(), Some(id), budget)?;
    usage_at(cache)
}

fn authoritative_ids(cache: &Path, account_email: &str) -> io::Result<Option<HashSet<MessageId>>> {
    match read_json::<StoredMailbox>(&mailbox_path(cache)) {
        Ok(Some(stored))
            if stored.version == MAILBOX_VERSION
                && stored.account_email.eq_ignore_ascii_case(account_email) =>
        {
            Ok(Some(
                stored
                    .messages
                    .into_iter()
                    .map(|message| message.id)
                    .collect(),
            ))
        }
        Ok(Some(_)) => Ok(Some(HashSet::new())),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => Ok(Some(HashSet::new())),
        Ok(None) => Ok(None),
        Err(error) => Err(error),
    }
}

fn clear_bodies_at(cache: &Path) -> io::Result<u64> {
    let before = directory_size(&bodies_path(cache))?;
    remove_dir_if_exists(&bodies_path(cache))?;
    ensure_bodies_dir(cache)?;
    write_manifest(cache, &empty_manifest())?;
    Ok(before.saturating_sub(directory_size(&bodies_path(cache))?))
}

fn clear_all_mail_at(cache: &Path) -> io::Result<()> {
    let mut first_error = None;
    for path in [mailbox_path(cache), legacy_mailbox_path(cache)] {
        if let Err(error) = remove_file_if_exists(&path) {
            first_error.get_or_insert(error);
        }
    }
    if let Err(error) = remove_dir_if_exists(&bodies_path(cache)) {
        first_error.get_or_insert(error);
    }
    first_error.map_or(Ok(()), Err)
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
    let (uid_validity, uid) = id
        .gmail_parts()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid Gmail message ID"))?;
    Ok(format!("{uid_validity}-{uid}.json"))
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

fn snapshot_from_stored(stored: &StoredMailbox) -> MailboxSnapshot {
    MailboxSnapshot {
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

fn mailbox_path(cache: &Path) -> PathBuf {
    cache.join("whitford/mailbox-v2.json")
}

fn legacy_mailbox_path(cache: &Path) -> PathBuf {
    cache.join("whitford/mailbox-v1.json")
}

fn bodies_path(cache: &Path) -> PathBuf {
    cache.join("whitford/bodies-v1")
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
    use crate::model::{AttachmentState, FolderId};
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

    fn summary(uid: u32) -> MessageSummary {
        MessageSummary {
            id: MessageId::gmail(7, uid),
            folder_id: FolderId::Inbox,
            sender: format!("Sender {uid}"),
            email: None,
            initials: None,
            subject: format!("Subject {uid}"),
            received_at_unix: Some(i64::from(uid)),
            unread: false,
            starred: false,
            attachment_state: AttachmentState::Known(Vec::new()),
            used_fallback: false,
        }
    }

    fn snapshot(ids: &[u32]) -> MailboxSnapshot {
        MailboxSnapshot {
            messages: ids.iter().copied().map(summary).collect(),
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

    #[test]
    fn retention_choices_are_stable() {
        assert_eq!(RETENTION_OPTIONS, [50, 100, 250, 500]);
        assert_eq!(limit_at(selected_index(250)), Some(250));
        assert!(!is_valid_limit(51));
    }

    #[test]
    fn v2_membership_is_authoritative_and_migrates_v1_after_save() {
        let root = TestRoot::new();
        write_private_json(&legacy_mailbox_path(&root.0), &serde_json::json!({})).unwrap();
        replace_and_save_at(&root.0, &account(), snapshot(&[1, 2]), 50).unwrap();
        replace_and_save_at(&root.0, &account(), snapshot(&[3]), 50).unwrap();
        let (_, loaded) = load_latest_from(&root.0, 50).unwrap().unwrap();
        assert_eq!(loaded.messages, vec![summary(3)]);
        assert!(!legacy_mailbox_path(&root.0).exists());
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
    fn body_round_trip_usage_and_modes() {
        let root = TestRoot::new();
        let id = MessageId::gmail(7, 9);
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
            fs::metadata(bodies_path(&root.0).join("7-9.json"))
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
        let id = MessageId::gmail(7, 9);
        save_body_at(&root.0, EMAIL, &id, &body("old"), 10, u64::MAX).unwrap();
        let path = bodies_path(&root.0).join("7-9.json");
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
        let first = MessageId::gmail(7, 1);
        let second = MessageId::gmail(7, 2);
        save_body_at(&root.0, EMAIL, &first, &body(&"a".repeat(200)), 1, u64::MAX).unwrap();
        let first_bytes = fs::metadata(bodies_path(&root.0).join("7-1.json"))
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
        let third = MessageId::gmail(7, 3);
        save_body_at(&root.0, EMAIL, &third, &body(&"x".repeat(1000)), 4, 1).unwrap();
        assert!(load_body_at(&root.0, EMAIL, &third, 5).unwrap().is_some());
    }

    #[test]
    fn authoritative_summaries_prune_orphaned_bodies() {
        let root = TestRoot::new();
        let kept = MessageId::gmail(7, 1);
        let orphan = MessageId::gmail(7, 2);
        save_body_at(&root.0, EMAIL, &kept, &body("keep"), 1, u64::MAX).unwrap();
        save_body_at(&root.0, EMAIL, &orphan, &body("drop"), 2, u64::MAX).unwrap();
        replace_and_save_at(&root.0, &account(), snapshot(&[1]), 50).unwrap();
        assert!(load_body_at(&root.0, EMAIL, &kept, 3).unwrap().is_some());
        assert!(load_body_at(&root.0, EMAIL, &orphan, 3).unwrap().is_none());
    }

    #[test]
    fn account_change_drops_even_colliding_body_ids() {
        let root = TestRoot::new();
        replace_and_save_at(&root.0, &account(), snapshot(&[1]), 50).unwrap();
        let id = MessageId::gmail(7, 1);
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
    fn account_tag_rejects_colliding_body_after_interrupted_account_switch() {
        let root = TestRoot::new();
        let id = MessageId::gmail(7, 1);
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
                completed_at_unix: 100,
                requested_limit: 50,
                skipped_count: 0,
                messages: fresh.messages,
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
    fn retention_decrease_prunes_summaries_and_body_files() {
        let root = TestRoot::new();
        let config = TestRoot::new();
        let ids = (1..=60).collect::<Vec<_>>();
        replace_and_save_at(&root.0, &account(), snapshot(&ids), 100).unwrap();
        let old = MessageId::gmail(7, 1);
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
        assert!(load_body_at(&root.0, EMAIL, &old, 2).unwrap().is_none());
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
            &MessageId::gmail(7, 1),
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
        write_private_json(&mailbox_path(&root.0), &serde_json::json!({})).unwrap();
        save_body_at(
            &root.0,
            EMAIL,
            &MessageId::gmail(1, 1),
            &body("x"),
            1,
            u64::MAX,
        )
        .unwrap();
        clear_all_mail_at(&root.0).unwrap();
        assert!(!legacy_mailbox_path(&root.0).exists());
        assert!(!mailbox_path(&root.0).exists());
        assert!(!bodies_path(&root.0).exists());
    }
}
