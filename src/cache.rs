use crate::model::{AccountIdentity, MailProvider, MailboxSnapshot, Message, SyncMetadata};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    env, fs,
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

pub const RETENTION_OPTIONS: [usize; 4] = [50, 100, 250, 500];
pub const DEFAULT_RETENTION: usize = 100;
const CACHE_VERSION: u8 = 1;

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
    messages: Vec<Message>,
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
    read_json::<Preferences>(&preferences_path())
        .ok()
        .flatten()
        .map(|preferences| preferences.retained_messages)
        .filter(|limit| is_valid_limit(*limit))
        .unwrap_or(DEFAULT_RETENTION)
}

pub fn save_limit_and_prune(limit: usize) -> io::Result<()> {
    if !is_valid_limit(limit) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid cache limit",
        ));
    }
    write_private_json(
        &preferences_path(),
        &Preferences {
            retained_messages: limit,
        },
    )?;
    if let Some(mut stored) = read_json::<StoredMailbox>(&mailbox_path())? {
        stored.messages.truncate(limit);
        write_private_json(&mailbox_path(), &stored)?;
    }
    Ok(())
}

pub fn load_latest(limit: usize) -> io::Result<Option<(AccountIdentity, MailboxSnapshot)>> {
    let Some(mut stored) = read_json::<StoredMailbox>(&mailbox_path())? else {
        return Ok(None);
    };
    if stored.version != CACHE_VERSION || stored.account_email.trim().is_empty() {
        return Ok(None);
    }
    stored.messages.truncate(limit);
    let snapshot = snapshot_from_stored(&stored);
    Ok(Some((
        AccountIdentity {
            provider: MailProvider::Gmail,
            email: stored.account_email,
        },
        snapshot,
    )))
}

pub fn merge_and_save(
    account: &AccountIdentity,
    fresh: MailboxSnapshot,
    limit: usize,
) -> io::Result<MailboxSnapshot> {
    let mut by_id = HashMap::new();
    for message in fresh.messages {
        by_id.insert(message.id.clone(), message);
    }
    if let Some(stored) = read_json::<StoredMailbox>(&mailbox_path())?
        && stored.version == CACHE_VERSION
        && stored.account_email.eq_ignore_ascii_case(&account.email)
    {
        for message in stored.messages {
            by_id.entry(message.id.clone()).or_insert(message);
        }
    }
    let mut messages = by_id.into_values().collect::<Vec<_>>();
    messages.sort_by(|left, right| {
        right
            .received_at_unix
            .cmp(&left.received_at_unix)
            .then_with(|| right.id.0.cmp(&left.id.0))
    });
    messages.truncate(limit);
    let fallback_count = messages
        .iter()
        .filter(|message| message.used_fallback)
        .count();
    let snapshot = MailboxSnapshot {
        metadata: SyncMetadata {
            completed_at: fresh.metadata.completed_at,
            requested_limit: fresh.metadata.requested_limit,
            loaded_count: messages.len(),
            fallback_count,
            skipped_count: fresh.metadata.skipped_count,
        },
        messages,
    };
    let stored = StoredMailbox {
        version: CACHE_VERSION,
        account_email: account.email.clone(),
        completed_at_unix: snapshot
            .metadata
            .completed_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        requested_limit: snapshot.metadata.requested_limit,
        skipped_count: snapshot.metadata.skipped_count,
        messages: snapshot.messages.clone(),
    };
    write_private_json(&mailbox_path(), &stored)?;
    Ok(snapshot)
}

pub fn clear_mailbox() -> io::Result<()> {
    match fs::remove_file(mailbox_path()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
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

fn preferences_path() -> PathBuf {
    config_root().join("whitford/preferences.json")
}

fn mailbox_path() -> PathBuf {
    cache_root().join("whitford/mailbox-v1.json")
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

fn write_private_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing parent"))?;
    fs::create_dir_all(parent)?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    serde_json::to_writer(&mut file, value).map_err(io::Error::other)?;
    file.flush()?;
    file.sync_all()?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_choices_are_stable() {
        assert_eq!(RETENTION_OPTIONS, [50, 100, 250, 500]);
        assert_eq!(limit_at(selected_index(250)), Some(250));
        assert!(!is_valid_limit(51));
    }
}
