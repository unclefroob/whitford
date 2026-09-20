use serde::{Deserialize, Serialize};
use std::time::SystemTime;

pub const MAX_FOLDER_MAILBOX_BYTES: usize = 1_024;
pub const MAX_FOLDER_CATALOG_ENTRIES: usize = 256;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MailProvider {
    Gmail,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountIdentity {
    pub provider: MailProvider,
    pub email: String,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Attachment {
    pub name: String,
    pub media_type: Option<String>,
    pub octets: Option<u64>,
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
    pub attachment_state: AttachmentState,
    pub used_fallback: bool,
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
            attachment_state: AttachmentState::Known(if attachment {
                vec![Attachment {
                    name: "notes.pdf".into(),
                    media_type: Some("application/pdf".into()),
                    octets: Some(10),
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
}
