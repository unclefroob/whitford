use crate::composer::{ComposeDraft, DraftAttachment};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    io::{self, Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const VERSION: u8 = 1;
const MAX_DRAFT_JSON_BYTES: u64 = 3 * 1024 * 1024;
const MAX_STAGED_FILE_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_STAGED_TOTAL_BYTES: u64 = 18 * 1024 * 1024;
pub const MAX_STAGED_FILES: usize = 25;
const MAX_DRAFTS: usize = 100;
const MAX_SIGNATURE_BYTES: u64 = 256 * 1024;
static NEXT_FILE: AtomicU64 = AtomicU64::new(1);

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredDraft {
    version: u8,
    draft: ComposeDraft,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SignaturePreference {
    pub html: String,
    pub enabled: bool,
}

pub fn load_signature(account_email: &str) -> io::Result<SignaturePreference> {
    load_signature_at(&drafts_root()?, account_email)
}

fn load_signature_at(root: &Path, account_email: &str) -> io::Result<SignaturePreference> {
    let path = account_root(root, account_email).join("signature.json");
    Ok(read_bounded_limit(&path, MAX_SIGNATURE_BYTES)?.unwrap_or_default())
}

pub fn save_signature(account_email: &str, preference: &SignaturePreference) -> io::Result<()> {
    save_signature_at(&drafts_root()?, account_email, preference)
}

fn save_signature_at(
    root: &Path,
    account_email: &str,
    preference: &SignaturePreference,
) -> io::Result<()> {
    let account = account_root(root, account_email);
    ensure_private_dir(&account)?;
    let bytes = serde_json::to_vec(preference)?;
    if bytes.len() as u64 > MAX_SIGNATURE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "signature is too large",
        ));
    }
    atomic_write(&account.join("signature.json"), &bytes)
}

pub fn save(account_email: &str, draft: &ComposeDraft) -> io::Result<()> {
    save_at(&drafts_root()?, account_email, draft)
}

pub fn load_all(account_email: &str) -> io::Result<Vec<ComposeDraft>> {
    load_all_at(&drafts_root()?, account_email)
}

pub fn stage_file(
    account_email: &str,
    draft_id: &str,
    source: &Path,
    display_name: &str,
    media_type: &str,
) -> io::Result<DraftAttachment> {
    stage_file_at(
        &drafts_root()?,
        account_email,
        draft_id,
        source,
        display_name,
        media_type,
    )
}

pub fn delete(account_email: &str, draft_id: &str) -> io::Result<()> {
    validate_id(draft_id)?;
    let path = account_root(&drafts_root()?, account_email).join(draft_id);
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Removes every local draft artifact for one account, including staged
/// attachments and the account signature. Missing account data is already
/// clean and therefore succeeds.
pub fn purge_account(account_email: &str) -> io::Result<()> {
    purge_account_at(&drafts_root()?, account_email)
}

fn purge_account_at(root: &Path, account_email: &str) -> io::Result<()> {
    if account_email.trim().is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty account"));
    }
    let account = account_root(root, account_email);
    match fs::remove_dir_all(account) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub fn staged_path(account_email: &str, draft_id: &str, staged_file: &str) -> io::Result<PathBuf> {
    validate_id(draft_id)?;
    validate_leaf(staged_file)?;
    Ok(account_root(&drafts_root()?, account_email)
        .join(draft_id)
        .join("files")
        .join(staged_file))
}

fn drafts_root() -> io::Result<PathBuf> {
    let root = env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .filter(|path| path.is_absolute())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cache directory unavailable"))?;
    Ok(root.join("whitford/drafts-v1"))
}

fn save_at(root: &Path, account_email: &str, draft: &ComposeDraft) -> io::Result<()> {
    validate_id(&draft.id)?;
    if !draft.account_email.eq_ignore_ascii_case(account_email) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "draft account mismatch",
        ));
    }
    let account = account_root(root, account_email);
    ensure_private_dir(&account)?;
    if !account.join(&draft.id).exists() && draft_count(&account)? >= MAX_DRAFTS {
        return Err(io::Error::new(
            io::ErrorKind::StorageFull,
            "too many drafts",
        ));
    }
    let directory = account.join(&draft.id);
    ensure_private_dir(&directory)?;
    ensure_private_dir(&directory.join("files"))?;
    validate_attachment_refs(draft)?;
    let bytes = serde_json::to_vec(&StoredDraft {
        version: VERSION,
        draft: draft.clone(),
    })?;
    if bytes.len() as u64 > MAX_DRAFT_JSON_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "draft is too large",
        ));
    }
    atomic_write(&directory.join("draft.json"), &bytes)?;
    Ok(())
}

fn load_all_at(root: &Path, account_email: &str) -> io::Result<Vec<ComposeDraft>> {
    let account = account_root(root, account_email);
    let entries = match fs::read_dir(&account) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut drafts = Vec::new();
    for entry in entries.take(MAX_DRAFTS + 1) {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if validate_id(&id).is_err() {
            continue;
        }
        let Some(stored) = read_bounded::<StoredDraft>(&entry.path().join("draft.json"))? else {
            continue;
        };
        if stored.version == VERSION
            && stored.draft.id == id
            && stored
                .draft
                .account_email
                .eq_ignore_ascii_case(account_email)
            && validate_attachment_refs(&stored.draft).is_ok()
        {
            let modified = entry
                .path()
                .join("draft.json")
                .metadata()
                .and_then(|value| value.modified())
                .unwrap_or(UNIX_EPOCH);
            drafts.push((modified, stored.draft));
        }
    }
    if drafts.len() > MAX_DRAFTS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "too many drafts",
        ));
    }
    drafts.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)));
    Ok(drafts.into_iter().map(|(_, draft)| draft).collect())
}

fn stage_file_at(
    root: &Path,
    account_email: &str,
    draft_id: &str,
    source: &Path,
    display_name: &str,
    media_type: &str,
) -> io::Result<DraftAttachment> {
    validate_id(draft_id)?;
    let mut input = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(source)?;
    let metadata = input.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "attachment must be a regular file",
        ));
    }
    if metadata.len() > MAX_STAGED_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "attachment is too large",
        ));
    }
    let files = account_root(root, account_email)
        .join(draft_id)
        .join("files");
    ensure_private_dir(&files)?;
    let mut count = 0_usize;
    let mut total = 0_u64;
    for entry in fs::read_dir(&files)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_file() {
            count = count.saturating_add(1);
            total = total.saturating_add(metadata.len());
        }
    }
    if count >= MAX_STAGED_FILES || total.saturating_add(metadata.len()) > MAX_STAGED_TOTAL_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::StorageFull,
            "attachment total is too large",
        ));
    }
    let id = unique_id();
    let destination = files.join(&id);
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&destination)?;
    let copied = io::copy(
        &mut Read::by_ref(&mut input).take(MAX_STAGED_FILE_BYTES + 1),
        &mut output,
    )?;
    if copied > MAX_STAGED_FILE_BYTES || copied != metadata.len() {
        drop(output);
        let _ = fs::remove_file(&destination);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "attachment changed or is too large",
        ));
    }
    output.sync_all()?;
    Ok(DraftAttachment {
        id: id.clone(),
        display_name: bounded_label(display_name, "attachment"),
        media_type: bounded_label(media_type, "application/octet-stream"),
        staged_file: id,
        bytes: copied,
    })
}

fn validate_attachment_refs(draft: &ComposeDraft) -> io::Result<()> {
    let mut count = 0_usize;
    let mut total = 0_u64;
    for attachment in draft
        .attachments
        .iter()
        .chain(draft.inline_images.iter().map(|inline| &inline.attachment))
    {
        count = count.saturating_add(1);
        total = total.saturating_add(attachment.bytes);
        validate_id(&attachment.id)?;
        validate_leaf(&attachment.staged_file)?;
        if attachment.bytes > MAX_STAGED_FILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "attachment is too large",
            ));
        }
    }
    if count > MAX_STAGED_FILES || total > MAX_STAGED_TOTAL_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "attachment total is too large",
        ));
    }
    Ok(())
}

pub fn remove_staged(account_email: &str, draft_id: &str, staged_file: &str) -> io::Result<()> {
    let path = staged_path(account_email, draft_id, staged_file)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn validate_id(value: &str) -> io::Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid draft identifier",
        ))
    } else {
        Ok(())
    }
}

fn validate_leaf(value: &str) -> io::Result<()> {
    let path = Path::new(value);
    if value.is_empty()
        || value.len() > 255
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid staged file",
        ))
    } else {
        Ok(())
    }
}

fn ensure_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing parent"))?;
    ensure_private_dir(parent)?;
    let temporary = parent.join(format!(".draft-{}.tmp", unique_id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn read_bounded<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<Option<T>> {
    read_bounded_limit(path, MAX_DRAFT_JSON_BYTES)
}

fn read_bounded_limit<T: for<'de> Deserialize<'de>>(
    path: &Path,
    limit: u64,
) -> io::Result<Option<T>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "draft is not a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "draft is too large",
        ));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(io::Error::other)
}

fn draft_count(account: &Path) -> io::Result<usize> {
    Ok(fs::read_dir(account)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .take(MAX_DRAFTS + 1)
        .count())
}

fn account_root(root: &Path, account_email: &str) -> PathBuf {
    root.join(format!(
        "{:016x}",
        stable_hash(account_email.trim().to_ascii_lowercase().as_bytes())
    ))
}

fn stable_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn unique_id() -> String {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
    format!("{time:x}-{sequence:x}")
}

fn bounded_label(value: &str, fallback: &str) -> String {
    let value: String = value
        .chars()
        .filter(|character| !character.is_control())
        .take(255)
        .collect();
    let value = value.trim();
    if value.is_empty() {
        fallback.into()
    } else {
        value.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{composer::ComposeKind, model::MessageId};

    fn temp_root() -> PathBuf {
        let root = env::temp_dir().join(format!("whitford-drafts-test-{}", unique_id()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn draft() -> ComposeDraft {
        ComposeDraft {
            id: "draft-1".into(),
            account_email: "me@example.com".into(),
            kind: ComposeKind::Forward {
                original: MessageId::gmail(1),
            },
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "Hi".into(),
            html: "<p>Hello</p>".into(),
            text: "Hello".into(),
            attachments: Vec::new(),
            inline_images: Vec::new(),
            thread: None,
            dirty_revision: 1,
        }
    }

    #[test]
    fn saves_and_loads_private_draft_atomically() {
        let root = temp_root();
        save_at(&root, "me@example.com", &draft()).unwrap();
        let loaded = load_all_at(&root, "me@example.com").unwrap();
        assert_eq!(loaded, vec![draft()]);
        let account = account_root(&root, "me@example.com");
        assert_eq!(
            fs::metadata(account.join("draft-1/draft.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saves_multiple_drafts_without_deleting_siblings() {
        let root = temp_root();
        let first = draft();
        let mut second = draft();
        second.id = "draft-2".into();
        second.subject = "Another message".into();

        save_at(&root, "me@example.com", &first).unwrap();
        save_at(&root, "me@example.com", &second).unwrap();

        let loaded = load_all_at(&root, "me@example.com").unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(loaded.iter().any(|value| value.id == first.id));
        assert!(loaded.iter().any(|value| value.id == second.id));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn account_purge_removes_drafts_staged_files_and_signature_only_for_that_account() {
        let root = temp_root();
        let first = draft();
        save_at(&root, "me@example.com", &first).unwrap();
        save_signature_at(
            &root,
            "me@example.com",
            &SignaturePreference {
                html: "<b>Private</b>".into(),
                enabled: true,
            },
        )
        .unwrap();
        let source = root.join("source.txt");
        fs::write(&source, b"secret attachment").unwrap();
        stage_file_at(
            &root,
            "me@example.com",
            &first.id,
            &source,
            "secret.txt",
            "text/plain",
        )
        .unwrap();

        let mut other = draft();
        other.account_email = "other@example.com".into();
        save_at(&root, "other@example.com", &other).unwrap();

        purge_account_at(&root, "me@example.com").unwrap();

        assert!(load_all_at(&root, "me@example.com").unwrap().is_empty());
        assert_eq!(
            load_signature_at(&root, "me@example.com").unwrap(),
            SignaturePreference::default()
        );
        assert_eq!(
            load_all_at(&root, "other@example.com").unwrap(),
            vec![other]
        );
        purge_account_at(&root, "me@example.com").unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stages_a_copy_and_rejects_traversal() {
        let root = temp_root();
        let source = root.join("source.txt");
        fs::write(&source, b"hello").unwrap();
        let attachment = stage_file_at(
            &root,
            "me@example.com",
            "draft-1",
            &source,
            "hello.txt",
            "text/plain",
        )
        .unwrap();
        assert_eq!(attachment.bytes, 5);
        assert!(validate_leaf("../escape").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn staging_rejects_symlinks_and_aggregate_overflow() {
        use std::os::unix::fs::symlink;
        let root = temp_root();
        let source = root.join("source.bin");
        let file = fs::File::create(&source).unwrap();
        file.set_len(MAX_STAGED_TOTAL_BYTES + 1).unwrap();
        assert!(
            stage_file_at(
                &root,
                "me@example.com",
                "draft-1",
                &source,
                "source.bin",
                "application/octet-stream"
            )
            .is_err()
        );
        file.set_len(4).unwrap();
        let link = root.join("link.bin");
        symlink(&source, &link).unwrap();
        assert!(
            stage_file_at(
                &root,
                "me@example.com",
                "draft-1",
                &link,
                "link.bin",
                "application/octet-stream"
            )
            .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn signature_preference_is_private_and_account_scoped() {
        let root = temp_root();
        let preference = SignaturePreference {
            html: "<b>Ryan</b>".into(),
            enabled: true,
        };
        save_signature_at(&root, "me@example.com", &preference).unwrap();
        assert_eq!(
            load_signature_at(&root, "me@example.com").unwrap(),
            preference
        );
        assert_eq!(
            load_signature_at(&root, "other@example.com").unwrap(),
            SignaturePreference::default()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
