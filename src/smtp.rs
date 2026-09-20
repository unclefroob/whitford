use crate::{
    composer::{self, Recipient, ThreadHeaders},
    model::{ReplyAddress, ReplyContext},
};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Attachment, Mailbox, MultiPart, SinglePart, header},
    transport::smtp::authentication::{Credentials, Mechanism},
};
use std::{collections::HashSet, fmt, time::Duration};
use tokio::time::timeout;

pub const SMTP_HOST: &str = "smtp.gmail.com";
pub const MAX_REPLY_BYTES: usize = 1024 * 1024;
const MAX_RECIPIENTS: usize = 100;
pub const MAX_RAW_PART_BYTES: usize = 18 * 1024 * 1024;
const SEND_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplyKind {
    Reply,
    ReplyAll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SmtpError {
    MissingRecipient,
    InvalidAddress,
    EmptyBody,
    BodyTooLarge,
    TooManyRecipients,
    Build,
    Authentication,
    Rejected,
    DeliveryUncertain,
}

impl fmt::Display for SmtpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingRecipient => "the original message has no reply address",
            Self::InvalidAddress => "an email address is invalid",
            Self::EmptyBody => "the reply is empty",
            Self::BodyTooLarge => "the reply is too large",
            Self::TooManyRecipients => "the reply has too many recipients",
            Self::Build => "the reply could not be constructed",
            Self::Authentication => "Gmail authorization is required",
            Self::Rejected => "Gmail rejected the reply",
            Self::DeliveryUncertain => "the reply's delivery status is uncertain",
        })
    }
}

impl std::error::Error for SmtpError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubmissionPart {
    pub display_name: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InlineSubmissionPart {
    pub part: SubmissionPart,
    pub content_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MailSubmission {
    pub account_email: String,
    pub to: Vec<Recipient>,
    pub cc: Vec<Recipient>,
    pub bcc: Vec<Recipient>,
    pub subject: String,
    pub html: String,
    pub attachments: Vec<SubmissionPart>,
    pub inline_images: Vec<InlineSubmissionPart>,
    pub thread: Option<ThreadHeaders>,
}

/// Builds a bounded MIME message. Body sanitization is deliberately repeated
/// here because persisted drafts and the WebView are both untrusted inputs.
pub fn build_message(submission: &MailSubmission) -> Result<Message, SmtpError> {
    let all_recipients = submission
        .to
        .iter()
        .chain(&submission.cc)
        .chain(&submission.bcc)
        .cloned()
        .collect::<Vec<_>>();
    if submission.to.is_empty() && submission.cc.is_empty() && submission.bcc.is_empty() {
        return Err(SmtpError::MissingRecipient);
    }
    if all_recipients.len() > MAX_RECIPIENTS {
        return Err(SmtpError::TooManyRecipients);
    }
    composer::validate_recipients(&all_recipients).map_err(|error| match error {
        composer::ComposeError::TooManyRecipients => SmtpError::TooManyRecipients,
        _ => SmtpError::InvalidAddress,
    })?;
    let html = composer::sanitize_html(&submission.html);
    let text = composer::html_to_plain(&html);
    if text.trim().is_empty() && submission.inline_images.is_empty() {
        return Err(SmtpError::EmptyBody);
    }
    let raw_bytes = text
        .len()
        .checked_add(html.len())
        .and_then(|total| {
            submission
                .attachments
                .iter()
                .chain(submission.inline_images.iter().map(|inline| &inline.part))
                .try_fold(total, |value, part| value.checked_add(part.bytes.len()))
        })
        .ok_or(SmtpError::BodyTooLarge)?;
    if raw_bytes > MAX_RAW_PART_BYTES {
        return Err(SmtpError::BodyTooLarge);
    }

    let from = mailbox_recipient(&Recipient {
        name: None,
        email: submission.account_email.clone(),
    })?;
    let mut builder = Message::builder()
        .from(from)
        .subject(clean_subject(&submission.subject))
        .date_now()
        .message_id(None);
    for recipient in &submission.to {
        builder = builder.to(mailbox_recipient(recipient)?);
    }
    for recipient in &submission.cc {
        builder = builder.cc(mailbox_recipient(recipient)?);
    }
    // Lettre derives the SMTP envelope before dropping Bcc from serialized headers.
    for recipient in &submission.bcc {
        builder = builder.bcc(mailbox_recipient(recipient)?);
    }
    if let Some(thread) = &submission.thread
        && let Some(parent) = thread.in_reply_to.as_deref().and_then(canonical_message_id)
    {
        builder = builder.header(header::InReplyTo::from(parent.clone()));
        let references = generic_references(thread, &parent);
        if !references.is_empty() {
            builder = builder.header(header::References::from(references));
        }
    }

    let html_part = SinglePart::html(html);
    let rich_part = if submission.inline_images.is_empty() {
        MultiPart::alternative()
            .singlepart(SinglePart::plain(text))
            .singlepart(html_part)
    } else {
        let mut related = MultiPart::related().singlepart(html_part);
        for inline in &submission.inline_images {
            let content_type = content_type(&inline.part.media_type)?;
            related = related.singlepart(
                Attachment::new_inline_with_name(
                    clean_content_id(&inline.content_id)?,
                    clean_file_name(&inline.part.display_name),
                )
                .body(inline.part.bytes.clone(), content_type),
            );
        }
        MultiPart::alternative()
            .singlepart(SinglePart::plain(text))
            .multipart(related)
    };
    let mime = if submission.attachments.is_empty() {
        rich_part
    } else {
        let mut mixed = MultiPart::mixed().multipart(rich_part);
        for attachment in &submission.attachments {
            mixed = mixed.singlepart(
                Attachment::new(clean_file_name(&attachment.display_name)).body(
                    attachment.bytes.clone(),
                    content_type(&attachment.media_type)?,
                ),
            );
        }
        mixed
    };
    builder.multipart(mime).map_err(|_| SmtpError::Build)
}

/// Builds a plain-text RFC 5322 reply without performing network I/O.
pub fn build_reply(
    account_email: &str,
    original_subject: &str,
    context: &ReplyContext,
    kind: ReplyKind,
    body: &str,
) -> Result<Message, SmtpError> {
    validate_reply(account_email, context, kind, body)?;
    let from = mailbox(&ReplyAddress {
        name: None,
        email: account_email.to_owned(),
    })?;
    let (to, cc) = recipients(account_email, context, kind)?;
    let mut builder = Message::builder()
        .from(from)
        .subject(reply_subject(original_subject))
        .date_now()
        .message_id(None);
    for recipient in to {
        builder = builder.to(mailbox(&recipient)?);
    }
    for recipient in cc {
        builder = builder.cc(mailbox(&recipient)?);
    }
    if let Some(parent) = context.message_id.as_deref().and_then(canonical_message_id) {
        builder = builder.header(header::InReplyTo::from(parent.clone()));
        let references = references(context, &parent);
        if !references.is_empty() {
            builder = builder.header(header::References::from(references));
        }
    }
    builder.body(body.to_owned()).map_err(|_| SmtpError::Build)
}

/// Performs the inexpensive checks suitable for the UI thread. MIME encoding is
/// intentionally left to `build_reply` in the mail worker.
pub fn validate_reply(
    account_email: &str,
    context: &ReplyContext,
    kind: ReplyKind,
    body: &str,
) -> Result<(), SmtpError> {
    if body.trim().is_empty() {
        return Err(SmtpError::EmptyBody);
    }
    if body.len() > MAX_REPLY_BYTES {
        return Err(SmtpError::BodyTooLarge);
    }
    mailbox(&ReplyAddress {
        name: None,
        email: account_email.to_owned(),
    })?;
    let (to, cc) = recipients(account_email, context, kind)?;
    for address in to.iter().chain(&cc) {
        mailbox(address)?;
    }
    Ok(())
}

/// Sends a prepared message through Gmail using an OAuth access token.
///
/// Network failures are deliberately reported as uncertain: SMTP may have
/// accepted DATA before the connection was lost, so callers must not retry
/// automatically.
pub async fn send_message(
    account_email: &str,
    access_token: &str,
    message: Message,
) -> Result<(), SmtpError> {
    if account_email.trim().is_empty() || access_token.trim().is_empty() {
        return Err(SmtpError::Authentication);
    }
    let transport = AsyncSmtpTransport::<Tokio1Executor>::relay(SMTP_HOST)
        .map_err(|_| SmtpError::Build)?
        .credentials(Credentials::new(
            account_email.to_owned(),
            access_token.to_owned(),
        ))
        .authentication(vec![Mechanism::Xoauth2])
        .build();
    match timeout(SEND_TIMEOUT, transport.send(message)).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) if error.status().is_some_and(|code| u16::from(code) == 535) => {
            Err(SmtpError::Authentication)
        }
        Ok(Err(error)) if error.is_permanent() || error.is_transient() => Err(SmtpError::Rejected),
        Ok(Err(_)) | Err(_) => Err(SmtpError::DeliveryUncertain),
    }
}

fn recipients(
    account_email: &str,
    context: &ReplyContext,
    kind: ReplyKind,
) -> Result<(Vec<ReplyAddress>, Vec<ReplyAddress>), SmtpError> {
    let primary = if context.reply_to.is_empty() {
        &context.from
    } else {
        &context.reply_to
    };
    if primary.is_empty() {
        return Err(SmtpError::MissingRecipient);
    }
    let mut seen = HashSet::from([account_email.trim().to_ascii_lowercase()]);
    let mut to = Vec::new();
    let mut cc = Vec::new();
    match kind {
        // Reply is deliberately a single-recipient feature. This must stay in
        // lockstep with the one fixed recipient shown by the composer.
        ReplyKind::Reply => {
            if let Some(address) = primary.first() {
                append_unique(std::slice::from_ref(address), &mut to, &mut seen);
            }
        }
        ReplyKind::ReplyAll => append_unique(primary, &mut to, &mut seen),
    }
    if kind == ReplyKind::ReplyAll {
        append_unique(&context.to, &mut to, &mut seen);
        append_unique(&context.cc, &mut cc, &mut seen);
    }
    if to.is_empty() {
        return Err(SmtpError::MissingRecipient);
    }
    if to.len() + cc.len() > MAX_RECIPIENTS {
        return Err(SmtpError::TooManyRecipients);
    }
    Ok((to, cc))
}

fn append_unique(
    source: &[ReplyAddress],
    target: &mut Vec<ReplyAddress>,
    seen: &mut HashSet<String>,
) {
    for address in source {
        let key = address.email.trim().to_ascii_lowercase();
        if !key.is_empty() && seen.insert(key) {
            target.push(address.clone());
        }
    }
}

fn mailbox(address: &ReplyAddress) -> Result<Mailbox, SmtpError> {
    let email = address
        .email
        .trim()
        .parse()
        .map_err(|_| SmtpError::InvalidAddress)?;
    Ok(Mailbox::new(address.name.clone(), email))
}

fn mailbox_recipient(address: &Recipient) -> Result<Mailbox, SmtpError> {
    let email = address
        .email
        .trim()
        .parse()
        .map_err(|_| SmtpError::InvalidAddress)?;
    let name = address.name.as_ref().map(|name| {
        name.chars()
            .filter(|character| !character.is_control())
            .take(256)
            .collect()
    });
    Ok(Mailbox::new(name, email))
}

fn content_type(value: &str) -> Result<header::ContentType, SmtpError> {
    value.parse().map_err(|_| SmtpError::Build)
}

fn clean_file_name(value: &str) -> String {
    let value: String = value
        .chars()
        .filter(|character| !character.is_control() && !matches!(character, '/' | '\\'))
        .take(255)
        .collect();
    let value = value.trim();
    if value.is_empty() {
        "attachment".into()
    } else {
        value.into()
    }
}

fn clean_content_id(value: &str) -> Result<String, SmtpError> {
    let value = value.trim().trim_start_matches('<').trim_end_matches('>');
    (!value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'@' | b'.' | b'-' | b'_')))
    .then(|| value.to_owned())
    .ok_or(SmtpError::Build)
}

fn clean_subject(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(composer::MAX_SUBJECT_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn generic_references(thread: &ThreadHeaders, parent: &str) -> String {
    let mut ids: Vec<_> = thread
        .references
        .iter()
        .filter_map(|value| canonical_message_id(value))
        .collect();
    if ids.last().is_none_or(|last| last != parent) {
        ids.push(parent.to_owned());
    }
    while ids.join(" ").len() > 900 && ids.len() > 1 {
        ids.remove(0);
    }
    ids.join(" ")
}

fn reply_subject(subject: &str) -> String {
    let subject = subject
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(512)
        .collect::<String>();
    let subject = subject.trim();
    if subject
        .get(..3)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("re:"))
    {
        subject.to_owned()
    } else if subject.is_empty() {
        "Re: (No subject)".into()
    } else {
        format!("Re: {subject}")
    }
}

fn references(context: &ReplyContext, parent: &str) -> String {
    let mut ids: Vec<_> = context
        .references
        .iter()
        .filter_map(|value| canonical_message_id(value))
        .collect();
    if ids.last().is_none_or(|last| last != parent) {
        ids.push(parent.to_owned());
    }
    while ids.join(" ").len() > 900 && ids.len() > 1 {
        ids.remove(0);
    }
    ids.join(" ")
}

fn canonical_message_id(value: &str) -> Option<String> {
    let value = value.trim();
    let value = value
        .strip_prefix('<')
        .and_then(|value| value.strip_suffix('>'))
        .unwrap_or(value);
    (!value.is_empty()
        && value.len() <= 994
        && value.contains('@')
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        && !value.contains(['<', '>']))
    .then(|| format!("<{value}>"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{gmail::RawMessageBody, message::map_body};

    fn address(name: &str, email: &str) -> ReplyAddress {
        ReplyAddress {
            name: Some(name.into()),
            email: email.into(),
        }
    }

    fn context() -> ReplyContext {
        ReplyContext {
            from: vec![address("Sender", "sender@example.com")],
            reply_to: vec![],
            to: vec![
                address("Me", "me@example.com"),
                address("Other", "other@example.com"),
            ],
            cc: vec![
                address("Duplicate", "OTHER@example.com"),
                address("Third", "third@example.com"),
            ],
            message_id: Some("<parent@example.com>".into()),
            references: vec!["<root@example.com>".into()],
            sent_at_unix: None,
        }
    }

    fn recipient(email: &str) -> Recipient {
        Recipient {
            name: None,
            email: email.into(),
        }
    }

    #[test]
    fn generic_message_builds_full_mime_tree_without_bcc_header() {
        let message = build_message(&MailSubmission {
            account_email: "me@example.com".into(),
            to: vec![recipient("to@example.com")],
            cc: Vec::new(),
            bcc: vec![recipient("secret@example.com")],
            subject: "Topic".into(),
            html: r#"<p>Hello<img src="cid:image-1"></p>"#.into(),
            attachments: vec![SubmissionPart {
                display_name: "notes.txt".into(),
                media_type: "text/plain".into(),
                bytes: b"notes".to_vec(),
            }],
            inline_images: vec![InlineSubmissionPart {
                part: SubmissionPart {
                    display_name: "image.png".into(),
                    media_type: "image/png".into(),
                    bytes: vec![1, 2, 3],
                },
                content_id: "image-1".into(),
            }],
            thread: Some(ThreadHeaders {
                in_reply_to: Some("parent@example.com".into()),
                references: Vec::new(),
            }),
        })
        .unwrap();
        let formatted = String::from_utf8_lossy(&message.formatted()).into_owned();
        assert!(formatted.contains("multipart/mixed"));
        assert!(formatted.contains("multipart/alternative"));
        assert!(formatted.contains("multipart/related"));
        assert!(formatted.contains("Content-ID: <image-1>"));
        assert!(formatted.contains("In-Reply-To: <parent@example.com>"));
        assert!(!formatted.contains("Bcc:"));
        assert!(!formatted.contains("secret@example.com"));
    }

    #[test]
    fn reply_builds_safe_threaded_plain_text_message() {
        let formatted = String::from_utf8(
            build_reply(
                "me@example.com",
                "Status",
                &context(),
                ReplyKind::Reply,
                "Thanks!",
            )
            .unwrap()
            .formatted(),
        )
        .unwrap();
        assert!(formatted.contains("To: Sender <sender@example.com>"));
        assert!(formatted.contains("Subject: Re: Status"));
        assert!(formatted.contains("In-Reply-To: <parent@example.com>"));
        assert!(formatted.contains("References: <root@example.com> <parent@example.com>"));
        assert!(formatted.ends_with("Thanks!"));
    }

    #[test]
    fn raw_multi_value_references_survive_parsing_and_outgoing_threading() {
        let original = map_body(RawMessageBody {
            id: crate::model::MessageId::gmail(99),
            locator: crate::model::MessageLocator {
                folder_id: crate::model::FolderId::Inbox,
                mailbox: "INBOX".into(),
                uid_validity: 7,
                uid: 9,
            },
            raw: b"From: Sender <sender@example.com>\r\n\
Message-ID: <parent@example.com>\r\n\
References: <root@example.com> <middle@example.com>\r\n\
Subject: Thread update\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\nOriginal"
                .to_vec(),
            header: Vec::new(),
            plain: None,
            html: None,
            attachments: Vec::new(),
        });

        assert_eq!(
            original.reply_context.references,
            ["<root@example.com>", "<middle@example.com>"]
        );
        let formatted = String::from_utf8(
            build_reply(
                "me@example.com",
                "Thread update",
                &original.reply_context,
                ReplyKind::Reply,
                "Reply",
            )
            .unwrap()
            .formatted(),
        )
        .unwrap();
        assert!(formatted.contains("In-Reply-To: <parent@example.com>"));
        assert!(
            formatted.contains(
                "References: <root@example.com> <middle@example.com> <parent@example.com>"
            )
        );
    }

    #[test]
    fn reply_all_deduplicates_and_excludes_the_account() {
        let formatted = String::from_utf8(
            build_reply(
                "ME@example.com",
                "re: Status",
                &context(),
                ReplyKind::ReplyAll,
                "Done",
            )
            .unwrap()
            .formatted(),
        )
        .unwrap();
        assert!(formatted.contains("sender@example.com"));
        assert!(formatted.contains("other@example.com"));
        assert!(formatted.contains("third@example.com"));
        assert!(!formatted.contains("To: Me <me@example.com>"));
        assert_eq!(formatted.matches("OTHER@example.com").count(), 0);
        assert!(formatted.contains("Subject: re: Status"));
    }

    #[test]
    fn reply_sends_only_the_one_recipient_shown_by_the_composer() {
        let mut context = context();
        context.reply_to = vec![
            address("Shown", "shown@example.com"),
            address("Hidden", "hidden@example.com"),
        ];
        let formatted = String::from_utf8(
            build_reply(
                "me@example.com",
                "Status",
                &context,
                ReplyKind::Reply,
                "Done",
            )
            .unwrap()
            .formatted(),
        )
        .unwrap();
        assert!(formatted.contains("To: Shown <shown@example.com>"));
        assert!(!formatted.contains("hidden@example.com"));
    }

    #[test]
    fn missing_parent_omits_threading_headers() {
        let mut context = context();
        context.message_id = None;
        let formatted = String::from_utf8(
            build_reply("me@example.com", "", &context, ReplyKind::Reply, "Hello")
                .unwrap()
                .formatted(),
        )
        .unwrap();
        assert!(!formatted.contains("In-Reply-To:"));
        assert!(!formatted.contains("References:"));
        assert!(formatted.contains("Subject: Re: (No subject)"));
    }

    #[test]
    fn rejects_empty_oversize_and_header_injection() {
        assert_eq!(
            build_reply("me@example.com", "x", &context(), ReplyKind::Reply, "  ").unwrap_err(),
            SmtpError::EmptyBody
        );
        assert_eq!(
            build_reply(
                "me@example.com",
                "x",
                &context(),
                ReplyKind::Reply,
                &"x".repeat(MAX_REPLY_BYTES + 1),
            )
            .unwrap_err(),
            SmtpError::BodyTooLarge
        );
        let mut malicious = context();
        malicious.from[0].email = "bad@example.com\r\nBcc: victim@example.com".into();
        assert_eq!(
            build_reply("me@example.com", "x", &malicious, ReplyKind::Reply, "hello").unwrap_err(),
            SmtpError::InvalidAddress
        );

        let mut malicious = context();
        malicious.message_id = Some("<ok@example.com>\r\nBcc:victim@example.com".into());
        let formatted = String::from_utf8(
            build_reply(
                "me@example.com",
                "Hello\r\nBcc: victim@example.com",
                &malicious,
                ReplyKind::Reply,
                "hello",
            )
            .unwrap()
            .formatted(),
        )
        .unwrap();
        assert!(!formatted.contains("\r\nBcc:"));
        assert!(!formatted.contains("In-Reply-To:"));
    }
}
