use std::time::SystemTime;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FolderId {
    Inbox,
}
impl FolderId {
    pub const INBOX: Self = Self::Inbox;
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MessageId(pub String);
impl MessageId {
    pub fn gmail(uid_validity: u32, uid: u32) -> Self {
        Self(format!("gmail:{uid_validity}:{uid}"))
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
    pub name: &'static str,
    pub icon: &'static str,
}
pub const INBOX_FOLDER: Folder = Folder {
    id: FolderId::Inbox,
    name: "Inbox",
    icon: "mail-unread-symbolic",
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attachment {
    pub name: String,
    pub media_type: Option<String>,
    pub octets: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    pub id: MessageId,
    pub folder_id: FolderId,
    pub sender: String,
    pub email: Option<String>,
    pub initials: Option<String>,
    pub subject: String,
    pub preview: Option<String>,
    pub received_at_unix: Option<i64>,
    pub body: String,
    pub unread: bool,
    pub starred: bool,
    pub attachments: Vec<Attachment>,
    pub truncated: bool,
    pub used_fallback: bool,
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
    pub messages: Vec<Message>,
    pub metadata: SyncMetadata,
}
impl MailboxSnapshot {
    pub fn empty(completed_at: SystemTime) -> Self {
        Self {
            messages: Vec::new(),
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
pub fn fixture_messages() -> Vec<Message> {
    [
        (1, "Mara Chen", "Design notes", true, true),
        (2, "Daniel Park", "Q2 roadmap", false, false),
        (3, "Priya Sharma", "Lunch next week?", true, false),
    ]
    .into_iter()
    .map(|(uid, sender, subject, unread, attachment)| Message {
        id: MessageId::gmail(1, uid),
        folder_id: FolderId::Inbox,
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
        preview: Some("A bounded preview".into()),
        received_at_unix: Some(1_700_000_000 + i64::from(uid)),
        body: "A safe plain-text body.".into(),
        unread,
        starred: false,
        attachments: if attachment {
            vec![Attachment {
                name: "notes.pdf".into(),
                media_type: Some("application/pdf".into()),
                octets: Some(10),
            }]
        } else {
            Vec::new()
        },
        truncated: false,
        used_fallback: false,
    })
    .collect()
}
