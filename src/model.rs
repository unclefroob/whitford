use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FolderId(pub &'static str);

impl FolderId {
    pub const INBOX: Self = Self("inbox");
    pub const STARRED: Self = Self("starred");
    pub const DRAFTS: Self = Self("drafts");
    pub const SENT: Self = Self("sent");
    pub const ARCHIVE: Self = Self("archive");
    pub const TRASH: Self = Self("trash");
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MessageId(pub &'static str);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FolderKind {
    Mailbox,
    Starred,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Folder {
    pub id: FolderId,
    pub name: &'static str,
    pub icon: &'static str,
    pub kind: FolderKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attachment {
    pub name: &'static str,
    pub details: Option<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    pub id: MessageId,
    pub folder_id: FolderId,
    pub sender: &'static str,
    pub email: &'static str,
    pub initials: Option<&'static str>,
    pub subject: &'static str,
    pub preview: Option<&'static str>,
    pub timestamp: Option<&'static str>,
    pub body: &'static str,
    pub unread: bool,
    pub starred: bool,
    pub attachments: Vec<Attachment>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureSet {
    pub folders: Vec<Folder>,
    pub messages: Vec<Message>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixtureError {
    DuplicateFolderId,
    DuplicateMessageId,
    UnknownFolder,
}

impl FixtureSet {
    pub fn validate(&self) -> Result<(), FixtureError> {
        let mut folder_ids = HashSet::new();
        for folder in &self.folders {
            if !folder_ids.insert(folder.id) {
                return Err(FixtureError::DuplicateFolderId);
            }
        }
        let mut message_ids = HashSet::new();
        for message in &self.messages {
            if !message_ids.insert(message.id) {
                return Err(FixtureError::DuplicateMessageId);
            }
            if !folder_ids.contains(&message.folder_id) {
                return Err(FixtureError::UnknownFolder);
            }
        }
        Ok(())
    }
}

pub fn fixtures() -> FixtureSet {
    let folders = vec![
        folder(
            FolderId::INBOX,
            "Inbox",
            "mail-unread-symbolic",
            FolderKind::Mailbox,
        ),
        folder(
            FolderId::STARRED,
            "Starred",
            "starred-symbolic",
            FolderKind::Starred,
        ),
        folder(
            FolderId::DRAFTS,
            "Drafts",
            "document-new-symbolic",
            FolderKind::Mailbox,
        ),
        folder(
            FolderId::SENT,
            "Sent",
            "mail-send-symbolic",
            FolderKind::Mailbox,
        ),
        folder(
            FolderId::ARCHIVE,
            "Archive",
            "mail-archive-symbolic",
            FolderKind::Mailbox,
        ),
        folder(
            FolderId::TRASH,
            "Trash",
            "user-trash-symbolic",
            FolderKind::Mailbox,
        ),
    ];
    let messages = vec![
        message(
            "mara",
            "Mara Chen",
            "mara@studio.dev",
            "MC",
            "Design notes for Monday",
            Some("Here are the notes from our design sync. I’ve included a summary…"),
            Some("2 minutes ago"),
            "Hi Alex,\n\nHere are the notes from our design sync. I’ve included a summary of the key decisions, some open questions, and the next steps.\n\nOverall, the direction feels solid and I’m excited about where this is going. Let me know if I missed anything or if you have other thoughts before we share with the wider team.\n\nBest,\nMara",
            true,
            false,
            vec![Attachment {
                name: "design-notes-monday.pdf",
                details: Some("748 KB • PDF"),
            }],
        ),
        message(
            "daniel",
            "Daniel Park",
            "daniel@northfield.dev",
            "DP",
            "Re: Q2 roadmap",
            Some("This looks great! A few small suggestions…"),
            Some("1 hour ago"),
            "Hi Alex,\n\nThis looks great. I left a few small suggestions on the Q2 roadmap, but the milestones and sequencing feel right to me.\n\nDaniel",
            false,
            false,
            vec![],
        ),
        message(
            "priya",
            "Priya Sharma",
            "priya@northfield.dev",
            "PS",
            "Lunch next week?",
            Some("Are you free for lunch sometime next week?…"),
            Some("3 hours ago"),
            "Hi Alex,\n\nAre you free for lunch sometime next week? Tuesday or Thursday both work for me.\n\nPriya",
            true,
            false,
            vec![],
        ),
        message(
            "team",
            "Team Updates",
            "updates@northfield.dev",
            "TU",
            "Sprint 16 retrospective",
            Some("Thanks everyone for a solid sprint. Here’s a…"),
            Some("Yesterday"),
            "Thanks everyone for a solid sprint. The retrospective notes and action items are ready for review.",
            false,
            false,
            vec![],
        ),
        message(
            "noah",
            "Noah Kim",
            "noah@northfield.dev",
            "NK",
            "Photos from the meetup",
            Some("Sharing a few photos from last night’s meetup…"),
            Some("Yesterday"),
            "Sharing a few photos from last night’s meetup. Thanks for making it such a good evening.",
            false,
            false,
            vec![Attachment {
                name: "meetup-photos.zip",
                details: None,
            }],
        ),
        message(
            "elena",
            "Elena Rossi",
            "elena@homes.example",
            "ER",
            "Re: Apartment availability",
            Some("Good news — the apartment is still available…"),
            Some("Apr 26"),
            "Good news — the apartment is still available. Let me know if you would like to arrange another viewing.",
            false,
            true,
            vec![],
        ),
        without_initials(message(
            "kai",
            "Kai Tan",
            "kai@trails.example",
            "KT",
            "Weekend hiking plan",
            Some("Trails are looking good this weekend. Want to…"),
            Some("Apr 26"),
            "Trails are looking good this weekend. Want to meet at the north entrance at eight?",
            false,
            false,
            vec![],
        )),
        message(
            "lena",
            "Lena Müller",
            "lena@museum.example",
            "LM",
            "Re: Berlin recommendations and an intentionally long subject line",
            None,
            None,
            "Definitely check out the museum and the café around the corner. Both are quiet in the morning.",
            false,
            true,
            vec![],
        ),
        in_folder(
            message(
                "draft",
                "Alex Morgan",
                "alex@northfield.dev",
                "AM",
                "Project launch note",
                Some("A few thoughts before we share this…"),
                Some("Draft"),
                "A few thoughts before we share this with the team.",
                false,
                false,
                vec![],
            ),
            FolderId::DRAFTS,
        ),
        in_folder(
            message(
                "sent",
                "Jordan Lee",
                "jordan@northfield.dev",
                "JL",
                "Re: Studio booking",
                Some("Thanks — I’ve confirmed the room for Friday."),
                Some("Apr 24"),
                "Thanks — I’ve confirmed the room for Friday.",
                false,
                false,
                vec![],
            ),
            FolderId::SENT,
        ),
        in_folder(
            message(
                "archived",
                "Research Weekly",
                "digest@research.example",
                "RW",
                "Interaction design reading list",
                Some("Five thoughtful links for your weekend…"),
                Some("Apr 18"),
                "Five thoughtful links for your weekend reading list.",
                false,
                false,
                vec![],
            ),
            FolderId::ARCHIVE,
        ),
    ];
    FixtureSet { folders, messages }
}

fn folder(id: FolderId, name: &'static str, icon: &'static str, kind: FolderKind) -> Folder {
    Folder {
        id,
        name,
        icon,
        kind,
    }
}

#[allow(clippy::too_many_arguments)]
fn message(
    id: &'static str,
    sender: &'static str,
    email: &'static str,
    initials: &'static str,
    subject: &'static str,
    preview: Option<&'static str>,
    timestamp: Option<&'static str>,
    body: &'static str,
    unread: bool,
    starred: bool,
    attachments: Vec<Attachment>,
) -> Message {
    Message {
        id: MessageId(id),
        folder_id: FolderId::INBOX,
        sender,
        email,
        initials: Some(initials),
        subject,
        preview,
        timestamp,
        body,
        unread,
        starred,
        attachments,
    }
}

fn in_folder(mut message: Message, folder_id: FolderId) -> Message {
    message.folder_id = folder_id;
    message
}

fn without_initials(mut message: Message) -> Message {
    message.initials = None;
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_are_deterministic_and_cover_visual_cases() {
        let first = fixtures();
        let second = fixtures();
        assert_eq!(first, second);
        assert!(first.messages.iter().any(|message| message.unread));
        assert!(first.messages.iter().any(|message| message.starred));
        assert!(
            first
                .messages
                .iter()
                .any(|message| !message.attachments.is_empty())
        );
        assert!(
            first
                .messages
                .iter()
                .any(|message| message.preview.is_none())
        );
        assert!(
            first
                .messages
                .iter()
                .any(|message| message.subject.len() > 40)
        );
        assert!(
            first
                .messages
                .iter()
                .any(|message| message.initials.is_none())
        );
        assert!(
            first
                .messages
                .iter()
                .any(|message| message.timestamp.is_none())
        );
        assert!(
            first
                .messages
                .iter()
                .flat_map(|message| &message.attachments)
                .any(|attachment| attachment.details.is_none())
        );
        assert!(first.folders.iter().any(|folder| {
            !first
                .messages
                .iter()
                .any(|message| message.folder_id == folder.id)
        }));
    }

    #[test]
    fn fixture_validation_rejects_duplicate_ids_and_unknown_folders() {
        let fixtures = fixtures();
        assert!(fixtures.validate().is_ok());
        let mut duplicate = fixtures.clone();
        duplicate.messages.push(duplicate.messages[0].clone());
        assert_eq!(duplicate.validate(), Err(FixtureError::DuplicateMessageId));
        let mut duplicate_folder = fixtures.clone();
        duplicate_folder
            .folders
            .push(duplicate_folder.folders[0].clone());
        assert_eq!(
            duplicate_folder.validate(),
            Err(FixtureError::DuplicateFolderId)
        );
        let mut orphan = fixtures;
        orphan.messages[0].folder_id = FolderId("missing");
        assert_eq!(orphan.validate(), Err(FixtureError::UnknownFolder));
    }
}
