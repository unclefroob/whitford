use serde::{Deserialize, Serialize};
use std::{fmt, time::SystemTime};

pub const MAX_FOLDER_MAILBOX_BYTES: usize = 1_024;
pub const MAX_FOLDER_CATALOG_ENTRIES: usize = 256;
pub const MAX_MESSAGE_LABELS: usize = 256;
pub const MAX_ACCOUNTS: usize = 32;

/// An opaque, durable identifier for an account.  It is deliberately unrelated
/// to the email address: account IDs are used in filenames, cache keys and
/// Secret Service attributes, where leaking an identity is unnecessary.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct AccountId(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountIdError {
    Invalid,
    RandomUnavailable,
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AccountId {
    const PREFIX: &str = "acct-";
    const ENCODED_BYTES: usize = 32;

    /// Parses the canonical, filesystem-safe representation.
    pub fn new(value: impl Into<String>) -> Result<Self, AccountIdError> {
        let value = value.into();
        let suffix = value
            .strip_prefix(Self::PREFIX)
            .ok_or(AccountIdError::Invalid)?;
        if suffix.len() != Self::ENCODED_BYTES
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(AccountIdError::Invalid);
        }
        Ok(Self(value))
    }

    /// Creates a random ID suitable for persistence. The encoded value is
    /// opaque and contains no account identity.
    pub fn generate() -> Result<Self, AccountIdError> {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| AccountIdError::RandomUnavailable)?;
        let mut value = String::from(Self::PREFIX);
        for byte in bytes {
            use std::fmt::Write;
            let _ = write!(value, "{byte:02x}");
        }
        Self::new(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AccountRecord {
    pub id: AccountId,
    pub identity: AccountIdentity,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AccountRegistry {
    pub accounts: Vec<AccountRecord>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountRegistryError {
    InvalidAccount,
    DuplicateId,
    DuplicateIdentity,
    TooManyAccounts,
}

impl AccountRegistry {
    pub fn add(&mut self, record: AccountRecord) -> Result<(), AccountRegistryError> {
        if !record.identity.is_valid() {
            return Err(AccountRegistryError::InvalidAccount);
        }
        if self.accounts.len() >= MAX_ACCOUNTS {
            return Err(AccountRegistryError::TooManyAccounts);
        }
        if self
            .accounts
            .iter()
            .any(|existing| existing.id == record.id)
        {
            return Err(AccountRegistryError::DuplicateId);
        }
        if self.accounts.iter().any(|existing| {
            existing.identity.provider == record.identity.provider
                && existing.identity.normalized_email() == record.identity.normalized_email()
        }) {
            return Err(AccountRegistryError::DuplicateIdentity);
        }
        self.accounts.push(record);
        self.accounts.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(())
    }

    pub fn get(&self, id: &AccountId) -> Option<&AccountRecord> {
        self.accounts.iter().find(|record| &record.id == id)
    }

    pub fn identity_exists(&self, identity: &AccountIdentity) -> bool {
        self.accounts.iter().any(|record| {
            record.identity.provider == identity.provider
                && record.identity.normalized_email() == identity.normalized_email()
        })
    }

    pub fn is_valid(&self) -> bool {
        self.accounts.len() <= MAX_ACCOUNTS
            && self
                .accounts
                .iter()
                .all(|record| record.identity.is_valid())
            && self.accounts.iter().enumerate().all(|(index, record)| {
                self.accounts[..index].iter().all(|other| {
                    other.id != record.id
                        && !(other.identity.provider == record.identity.provider
                            && other.identity.normalized_email()
                                == record.identity.normalized_email())
                })
            })
    }
}

impl AccountRecord {
    pub fn new(id: AccountId, identity: AccountIdentity) -> Self {
        Self { id, identity }
    }
}

/// Message identity at a storage or event boundary. Gmail message IDs are only
/// globally unique within one mailbox, so they must never cross this boundary
/// without their owning account ID.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct AccountMessageId {
    pub account_id: AccountId,
    pub message_id: MessageId,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct AccountFolderId {
    pub account_id: AccountId,
    pub folder_id: FolderId,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum FolderId {
    Inbox,
    Sent,
    AllMail,
    Trash,
    Starred,
    Label(String),
}
impl FolderId {
    pub const INBOX: Self = Self::Inbox;
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct MessageId(pub String);
impl MessageId {
    /// Canonical Gmail identity. Unlike an IMAP UID, X-GM-MSGID remains
    /// stable when a message appears in another mailbox.
    pub fn gmail(x_gm_msgid: u64) -> Self {
        Self(format!("gmail-msg:{x_gm_msgid}"))
    }

    pub fn gmail_value(&self) -> Option<u64> {
        let value = self.0.strip_prefix("gmail-msg:")?;
        (!value.is_empty()
            && value.bytes().all(|byte| byte.is_ascii_digit())
            && !value.starts_with('0'))
        .then(|| value.parse::<u64>().ok())
        .flatten()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum MailProvider {
    Gmail,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AccountIdentity {
    pub provider: MailProvider,
    pub email: String,
}

impl AccountIdentity {
    /// A comparison key only. Display and persisted values retain the exact
    /// address returned by the provider, while duplicate detection follows
    /// Gmail's case-insensitive identity semantics.
    pub fn normalized_email(&self) -> String {
        self.email.trim().to_ascii_lowercase()
    }

    pub fn is_valid(&self) -> bool {
        !self.email.trim().is_empty()
            && self.email.len() <= 320
            && !self.email.chars().any(char::is_control)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Folder {
    pub id: FolderId,
    pub name: String,
    pub icon: &'static str,
}

pub fn inbox_folder() -> Folder {
    Folder {
        id: FolderId::Inbox,
        name: "Inbox".into(),
        icon: "mail-unread-symbolic",
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum FolderKind {
    Inbox,
    Sent,
    AllMail,
    Trash,
    Starred,
    Label,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FolderDescriptor {
    pub id: FolderId,
    pub mailbox: String,
    pub display_name: String,
    pub kind: FolderKind,
}

impl FolderDescriptor {
    pub fn is_valid(&self) -> bool {
        !self.mailbox.trim().is_empty()
            && self.mailbox.len() <= MAX_FOLDER_MAILBOX_BYTES
            && !self.mailbox.chars().any(char::is_control)
            && !self.display_name.trim().is_empty()
            && self.display_name.len() <= MAX_FOLDER_MAILBOX_BYTES
            && !self.display_name.chars().any(char::is_control)
            && match (&self.id, self.kind) {
                (FolderId::Inbox, FolderKind::Inbox)
                | (FolderId::Sent, FolderKind::Sent)
                | (FolderId::AllMail, FolderKind::AllMail)
                | (FolderId::Trash, FolderKind::Trash)
                | (FolderId::Starred, FolderKind::Starred) => true,
                (FolderId::Label(id), FolderKind::Label) => id == &self.mailbox,
                _ => false,
            }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FolderCatalog {
    pub folders: Vec<FolderDescriptor>,
}

impl FolderCatalog {
    pub fn inbox_only() -> Self {
        Self {
            folders: vec![FolderDescriptor {
                id: FolderId::Inbox,
                mailbox: "INBOX".into(),
                display_name: "Inbox".into(),
                kind: FolderKind::Inbox,
            }],
        }
    }

    pub fn bounded(mut folders: Vec<FolderDescriptor>) -> Self {
        folders.retain(FolderDescriptor::is_valid);
        folders.sort_by(|left, right| {
            folder_rank(left.kind)
                .cmp(&folder_rank(right.kind))
                .then_with(|| left.display_name.cmp(&right.display_name))
                .then_with(|| left.mailbox.cmp(&right.mailbox))
        });
        folders.dedup_by(|left, right| left.id == right.id);
        folders.truncate(MAX_FOLDER_CATALOG_ENTRIES);
        Self { folders }
    }

    pub fn find(&self, id: &FolderId) -> Option<&FolderDescriptor> {
        self.folders.iter().find(|folder| &folder.id == id)
    }
}

fn folder_rank(kind: FolderKind) -> u8 {
    match kind {
        FolderKind::Inbox => 0,
        FolderKind::Starred => 1,
        FolderKind::Sent => 2,
        FolderKind::AllMail => 3,
        FolderKind::Trash => 4,
        FolderKind::Label => 5,
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MessageLocator {
    pub folder_id: FolderId,
    pub mailbox: String,
    pub uid_validity: u32,
    pub uid: u32,
}

impl MessageLocator {
    pub fn is_valid(&self) -> bool {
        self.uid_validity != 0
            && self.uid != 0
            && !self.mailbox.trim().is_empty()
            && self.mailbox.len() <= MAX_FOLDER_MAILBOX_BYTES
            && !self.mailbox.chars().any(char::is_control)
            && match &self.folder_id {
                FolderId::Inbox => self.mailbox.eq_ignore_ascii_case("INBOX"),
                FolderId::Label(id) => id == &self.mailbox,
                _ => true,
            }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum TransferEncoding {
    SevenBit,
    EightBit,
    Binary,
    Base64,
    QuotedPrintable,
    #[default]
    Unsupported,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct MimePartDescriptor {
    pub path: Vec<u32>,
    pub encoding: TransferEncoding,
    pub encoded_octets: u64,
}

impl MimePartDescriptor {
    pub fn is_valid(&self) -> bool {
        !self.path.is_empty()
            && self.path.len() <= 32
            && self.path.iter().all(|value| *value > 0)
            && self.encoding != TransferEncoding::Unsupported
    }

    pub fn section(&self) -> String {
        self.path
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(".")
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Attachment {
    pub name: String,
    pub media_type: Option<String>,
    pub octets: Option<u64>,
    #[serde(default)]
    pub part: MimePartDescriptor,
}

impl Attachment {
    pub fn is_downloadable(&self) -> bool {
        self.part.is_valid()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum AttachmentState {
    Known(Vec<Attachment>),
    Unknown,
}

impl AttachmentState {
    pub fn has_attachments(&self) -> bool {
        match self {
            Self::Known(attachments) => !attachments.is_empty(),
            Self::Unknown => true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MessageSummary {
    pub id: MessageId,
    pub folder_id: FolderId,
    pub locator: MessageLocator,
    pub sender: String,
    pub email: Option<String>,
    pub initials: Option<String>,
    pub subject: String,
    pub received_at_unix: Option<i64>,
    pub unread: bool,
    pub starred: bool,
    /// Gmail system-label membership, retained separately from user labels.
    /// These flags are authoritative when a message was fetched or reconciled
    /// through a virtual folder such as All Mail or Starred.
    #[serde(default)]
    pub in_inbox: bool,
    #[serde(default)]
    pub in_trash: bool,
    #[serde(default)]
    pub labels: Vec<String>,
    pub attachment_state: AttachmentState,
    pub used_fallback: bool,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum MutationDimension {
    Read,
    Starred,
    Inbox,
    Label(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MessageMutation {
    SetRead(bool),
    SetStarred(bool),
    Archive,
    MoveToTrash {
        mailbox: String,
    },
    /// The compensating operation for Archive. `inbox_mailbox` is a
    /// catalog-derived guard, not an IMAP path to synthesize.
    RestoreArchive {
        inbox_mailbox: String,
    },
    /// The compensating operation for moving a message to Trash. Gmail labels
    /// are restored before `\\Trash` is removed so the message cannot vanish
    /// from every visible IMAP mailbox mid-operation.
    RestoreFromTrash {
        inbox_mailbox: String,
        trash_mailbox: String,
        /// Captured before the forward trash action. A Sent/All Mail message
        /// must not acquire Inbox membership merely because it is undone.
        restore_inbox: bool,
        labels: Vec<String>,
    },
    SetLabel {
        mailbox: String,
        applied: bool,
    },
}

impl MessageMutation {
    pub fn dimension(&self) -> MutationDimension {
        match self {
            Self::SetRead(_) => MutationDimension::Read,
            Self::SetStarred(_) => MutationDimension::Starred,
            Self::Archive
            | Self::MoveToTrash { .. }
            | Self::RestoreArchive { .. }
            | Self::RestoreFromTrash { .. } => MutationDimension::Inbox,
            Self::SetLabel { mailbox, .. } => MutationDimension::Label(mailbox.clone()),
        }
    }

    pub fn apply(&self, message: &mut MessageSummary) {
        match self {
            Self::SetRead(read) => message.unread = !read,
            Self::SetStarred(starred) => message.starred = *starred,
            Self::SetLabel { mailbox, applied } => {
                message.labels.retain(|label| label != mailbox);
                if *applied {
                    message.labels.push(mailbox.clone());
                    message.labels.sort();
                    message.labels.dedup();
                    message.labels.truncate(MAX_MESSAGE_LABELS);
                }
            }
            Self::Archive
            | Self::MoveToTrash { .. }
            | Self::RestoreArchive { .. }
            | Self::RestoreFromTrash { .. } => {}
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciledMessageState {
    pub unread: bool,
    pub starred: bool,
    pub in_inbox: bool,
    pub in_trash: bool,
    pub labels: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MessageBody {
    pub text: String,
    pub html: Option<String>,
    pub attachments: Vec<Attachment>,
    pub reply_context: ReplyContext,
    pub used_fallback: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReplyContext {
    pub from: Vec<ReplyAddress>,
    pub reply_to: Vec<ReplyAddress>,
    pub to: Vec<ReplyAddress>,
    pub cc: Vec<ReplyAddress>,
    pub message_id: Option<String>,
    pub references: Vec<String>,
    pub sent_at_unix: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReplyAddress {
    pub name: Option<String>,
    pub email: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CacheUsage {
    pub total_bytes: u64,
    pub summary_bytes: u64,
    pub body_bytes: u64,
    pub body_count: usize,
    pub available: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncMetadata {
    pub completed_at: SystemTime,
    pub requested_limit: usize,
    pub loaded_count: usize,
    pub fallback_count: usize,
    pub skipped_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MailboxSnapshot {
    pub messages: Vec<MessageSummary>,
    pub metadata: SyncMetadata,
    pub folder_catalog: FolderCatalog,
}
impl MailboxSnapshot {
    pub fn empty(completed_at: SystemTime) -> Self {
        Self {
            messages: Vec::new(),
            folder_catalog: FolderCatalog::inbox_only(),
            metadata: SyncMetadata {
                completed_at,
                requested_limit: 50,
                loaded_count: 0,
                fallback_count: 0,
                skipped_count: 0,
            },
        }
    }
}

#[cfg(test)]
pub fn fixture_messages() -> Vec<MessageSummary> {
    [
        (1, "Mara Chen", "Design notes", true, true),
        (2, "Daniel Park", "Q2 roadmap", false, false),
        (3, "Priya Sharma", "Lunch next week?", true, false),
    ]
    .into_iter()
    .map(
        |(uid, sender, subject, unread, attachment)| MessageSummary {
            id: MessageId::gmail(u64::from(uid)),
            folder_id: FolderId::Inbox,
            locator: MessageLocator {
                folder_id: FolderId::Inbox,
                mailbox: "INBOX".into(),
                uid_validity: 1,
                uid,
            },
            sender: sender.into(),
            email: Some(format!(
                "{}@example.com",
                sender
                    .split_whitespace()
                    .next()
                    .unwrap_or("user")
                    .to_lowercase()
            )),
            initials: Some(
                sender
                    .split_whitespace()
                    .filter_map(|part| part.chars().next())
                    .take(2)
                    .collect(),
            ),
            subject: subject.into(),
            received_at_unix: Some(1_700_000_000 + i64::from(uid)),
            unread,
            starred: false,
            in_inbox: true,
            in_trash: false,
            labels: Vec::new(),
            attachment_state: AttachmentState::Known(if attachment {
                vec![Attachment {
                    name: "notes.pdf".into(),
                    media_type: Some("application/pdf".into()),
                    octets: Some(10),
                    part: MimePartDescriptor::default(),
                }]
            } else {
                Vec::new()
            }),
            used_fallback: false,
        },
    )
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gmail_ids_and_locators_are_strict() {
        let id = MessageId::gmail(1_278_455_344_230_334_865);
        assert_eq!(id.gmail_value(), Some(1_278_455_344_230_334_865));
        for invalid in [
            "gmail-msg:0",
            "gmail-msg:01",
            "gmail-msg:+1",
            "gmail-msg:18446744073709551616",
            "gmail:1:2",
        ] {
            assert_eq!(MessageId(invalid.into()).gmail_value(), None, "{invalid}");
        }
        assert!(
            MessageLocator {
                folder_id: FolderId::Inbox,
                mailbox: "INBOX".into(),
                uid_validity: 7,
                uid: 9,
            }
            .is_valid()
        );
    }

    #[test]
    fn folder_catalog_deduplicates_and_enforces_its_bound() {
        let mut folders = FolderCatalog::inbox_only().folders;
        folders.extend(
            (0..MAX_FOLDER_CATALOG_ENTRIES + 20).map(|index| FolderDescriptor {
                id: FolderId::Label(format!("Label {index}")),
                mailbox: format!("Label {index}"),
                display_name: format!("Label {index}"),
                kind: FolderKind::Label,
            }),
        );
        folders.push(FolderDescriptor {
            id: FolderId::Inbox,
            mailbox: "INBOX".into(),
            display_name: "Duplicate".into(),
            kind: FolderKind::Inbox,
        });
        let catalog = FolderCatalog::bounded(folders);
        assert_eq!(catalog.folders.len(), MAX_FOLDER_CATALOG_ENTRIES);
        assert_eq!(
            catalog
                .folders
                .iter()
                .filter(|folder| folder.id == FolderId::Inbox)
                .count(),
            1
        );
    }

    #[test]
    fn restore_mutations_share_the_serialized_inbox_dimension() {
        let archive = MessageMutation::RestoreArchive {
            inbox_mailbox: "INBOX".into(),
        };
        let trash = MessageMutation::RestoreFromTrash {
            inbox_mailbox: "INBOX".into(),
            trash_mailbox: "Trash".into(),
            restore_inbox: true,
            labels: vec!["Receipts".into()],
        };
        assert_eq!(archive.dimension(), MutationDimension::Inbox);
        assert_eq!(trash.dimension(), MutationDimension::Inbox);
    }

    #[test]
    fn account_ids_are_opaque_and_registry_rejects_duplicate_identity() {
        let id = AccountId::new("acct-0123456789abcdef0123456789abcdef").unwrap();
        assert_eq!(id.as_str(), "acct-0123456789abcdef0123456789abcdef");
        assert!(AccountId::new("me@example.com").is_err());
        assert!(AccountId::new("acct-0123456789ABCDEF0123456789abcdef").is_err());

        let mut registry = AccountRegistry::default();
        registry
            .add(AccountRecord::new(
                id.clone(),
                AccountIdentity {
                    provider: MailProvider::Gmail,
                    email: "Me@Example.com".into(),
                },
            ))
            .unwrap();
        assert_eq!(
            registry
                .add(AccountRecord::new(
                    AccountId::new("acct-fedcba9876543210fedcba9876543210").unwrap(),
                    AccountIdentity {
                        provider: MailProvider::Gmail,
                        email: " me@example.com ".into(),
                    },
                ))
                .unwrap_err(),
            AccountRegistryError::DuplicateIdentity
        );
        assert!(registry.get(&id).is_some());
    }
}
