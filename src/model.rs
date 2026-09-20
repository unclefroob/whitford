use serde::{Deserialize, Serialize};
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum FolderId {
    Inbox,
}
impl FolderId {
    pub const INBOX: Self = Self::Inbox;
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct MessageId(pub String);
impl MessageId {
    pub fn gmail(uid_validity: u32, uid: u32) -> Self {
        Self(format!("gmail:{uid_validity}:{uid}"))
    }

    pub fn gmail_parts(&self) -> Option<(u32, u32)> {
        let mut parts = self.0.split(':');
        if parts.next()? != "gmail" {
            return None;
        }
        let uid_validity_text = parts.next()?;
        let uid_text = parts.next()?;
        if !uid_validity_text.bytes().all(|byte| byte.is_ascii_digit())
            || !uid_text.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let uid_validity = uid_validity_text.parse::<u32>().ok()?;
        let uid = uid_text.parse::<u32>().ok()?;
        (parts.next().is_none() && uid_validity != 0 && uid != 0).then_some((uid_validity, uid))
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
    pub used_fallback: bool,
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
pub fn fixture_messages() -> Vec<MessageSummary> {
    [
        (1, "Mara Chen", "Design notes", true, true),
        (2, "Daniel Park", "Q2 roadmap", false, false),
        (3, "Priya Sharma", "Lunch next week?", true, false),
    ]
    .into_iter()
    .map(
        |(uid, sender, subject, unread, attachment)| MessageSummary {
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
    fn gmail_parts_accepts_only_exact_nonzero_numeric_ids() {
        assert_eq!(MessageId::gmail(7, 9).gmail_parts(), Some((7, 9)));
        for invalid in [
            "gmail:0:1",
            "gmail:1:0",
            "gmail:1:2:3",
            "gmail:1",
            "gmail:+1:2",
            "other:1:2",
            "gmail: 1:2",
            "gmail:4294967296:2",
        ] {
            assert_eq!(MessageId(invalid.into()).gmail_parts(), None, "{invalid}");
        }
    }
}
