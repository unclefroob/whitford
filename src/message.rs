use crate::{
    gmail::RawFetchedMessage,
    model::{Attachment, FolderId, Message, MessageId},
};
use mail_parser::{MessageParser, MimeHeaders, PartType};
use unicode_segmentation::UnicodeSegmentation;

const BODY_BYTES: usize = 32 * 1024;
const BODY_GRAPHEMES: usize = 16_384;

pub fn map_message(raw: RawFetchedMessage) -> Message {
    let parsed = MessageParser::default().parse(&raw.raw);
    let mut used_fallback = parsed.is_none();
    let (sender, email, subject, body, received_at, attachments) = if let Some(parsed) = parsed {
        let from = parsed.from().and_then(|addresses| addresses.first());
        let sender = from
            .and_then(|address| address.name.as_deref())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("Unknown sender");
        let email = from
            .and_then(|address| address.address.as_deref())
            .filter(|value| !value.trim().is_empty())
            .map(|value| cap(value, 320, 320));
        let subject = parsed
            .subject()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                used_fallback = true;
                "(No subject)"
            });
        let body = parsed
            .body_html(0)
            .map(|html| html_to_readable_text(&html))
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                parsed
                    .body_text(0)
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| value.into_owned())
            })
            .unwrap_or_else(|| {
                used_fallback = true;
                "No readable message body.".into()
            });
        let attachments = parsed
            .attachments()
            .take(20)
            .map(|part| {
                let name = part
                    .attachment_name()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("Unnamed attachment");
                let media_type = part.content_type().map(|kind| {
                    format!(
                        "{}/{}",
                        kind.ctype(),
                        kind.subtype().unwrap_or("octet-stream")
                    )
                });
                let octets = match &part.body {
                    PartType::Binary(value) | PartType::InlineBinary(value) => {
                        Some(value.len() as u64)
                    }
                    _ => None,
                };
                Attachment {
                    name: cap(name, 255, 255),
                    media_type,
                    octets,
                }
            })
            .collect();
        (
            cap(sender, 160, 160),
            email,
            cap(subject, 512, 512),
            cap(&normalize(&body), BODY_GRAPHEMES, BODY_BYTES),
            parsed.date().map(|date| date.to_timestamp()),
            attachments,
        )
    } else {
        (
            "Unknown sender".into(),
            None,
            "(No subject)".into(),
            "No readable message body.".into(),
            raw.internal_date_unix,
            Vec::new(),
        )
    };
    let initials = initials(&sender);
    let preview_text = cap(
        body.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .as_str(),
        280,
        1_120,
    );
    Message {
        id: MessageId::gmail(raw.uid_validity, raw.uid),
        folder_id: FolderId::Inbox,
        sender,
        email,
        initials,
        subject,
        preview: (!preview_text.is_empty()).then_some(preview_text),
        received_at_unix: received_at.or(raw.internal_date_unix),
        body,
        unread: !raw.flags.seen,
        starred: raw.flags.flagged,
        attachments,
        truncated: raw.truncated,
        used_fallback,
    }
}

pub fn map_messages(mut records: Vec<RawFetchedMessage>) -> (Vec<Message>, usize) {
    records.sort_by_key(|record| std::cmp::Reverse(record.uid));
    let messages: Vec<_> = records.into_iter().map(map_message).collect();
    let fallbacks = messages
        .iter()
        .filter(|message| message.used_fallback)
        .count();
    (messages, fallbacks)
}

fn remove_dangerous_blocks(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        let tag = [b"script".as_slice(), b"style".as_slice()]
            .into_iter()
            .find(|tag| starts_tag(bytes, cursor, tag));
        if let Some(tag) = tag {
            let closing = [b"</".as_slice(), tag].concat();
            let Some(close_start) = find_ascii_case(bytes, cursor + tag.len() + 1, &closing) else {
                break;
            };
            let Some(close_end) = bytes[close_start..].iter().position(|byte| *byte == b'>') else {
                break;
            };
            cursor = close_start + close_end + 1;
            continue;
        }
        let character = input[cursor..].chars().next().unwrap_or_default();
        output.push(character);
        cursor += character.len_utf8();
    }
    output
}

fn html_to_readable_text(input: &str) -> String {
    mail_parser::decoders::html::html_to_text(&remove_dangerous_blocks(input))
}

fn starts_tag(bytes: &[u8], offset: usize, tag: &[u8]) -> bool {
    bytes.get(offset) == Some(&b'<')
        && bytes
            .get(offset + 1..offset + 1 + tag.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(tag))
        && bytes
            .get(offset + 1 + tag.len())
            .is_some_and(|next| next.is_ascii_whitespace() || *next == b'>')
}

fn find_ascii_case(haystack: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    (start..=haystack.len().saturating_sub(needle.len()))
        .find(|offset| haystack[*offset..*offset + needle.len()].eq_ignore_ascii_case(needle))
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .filter_map(|character| match character {
            '\0' => None,
            '\n' | '\t' => Some(character),
            character if character.is_control() => Some(' '),
            _ => Some(character),
        })
        .collect::<String>()
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

fn cap(value: &str, max_graphemes: usize, max_bytes: usize) -> String {
    let normalized = normalize(value);
    let mut output = String::new();
    for grapheme in normalized.graphemes(true).take(max_graphemes) {
        if output.len() + grapheme.len() > max_bytes {
            break;
        }
        output.push_str(grapheme);
    }
    output
}

fn initials(sender: &str) -> Option<String> {
    let value: String = sender
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gmail::MessageFlags;
    fn raw(bytes: &[u8]) -> RawFetchedMessage {
        RawFetchedMessage {
            uid_validity: 7,
            uid: 9,
            flags: MessageFlags {
                seen: false,
                flagged: true,
            },
            internal_date_unix: None,
            rfc822_size: Some(bytes.len() as u32),
            raw: bytes.to_vec(),
            truncated: false,
        }
    }
    #[test]
    fn maps_decoded_plain_mail_and_stable_uid() {
        let message = map_message(raw(b"From: =?UTF-8?Q?Mara_Chen?= <mara@example.com>\r\nSubject: Hello\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nSafe body"));
        assert_eq!(message.id, MessageId::gmail(7, 9));
        assert_eq!(message.sender, "Mara Chen");
        assert_eq!(message.body, "Safe body");
        assert!(message.unread && message.starred);
    }
    #[test]
    fn html_is_converted_and_scripts_do_not_render_as_markup() {
        let message = map_message(raw(b"From: A <a@b.test>\r\nContent-Type: text/html\r\n\r\n<p>Hello <b>world</b></p><script>bad()</script>"));
        assert!(!message.body.contains("<b>"));
        assert!(!message.body.contains("<script>"));
        assert!(!message.body.contains("bad()"));
    }
    #[test]
    fn multipart_prefers_readable_html_without_tracking_destinations() {
        let message = map_message(raw(
            br#"From: e&s <offers@example.com>
Subject: Sale
MIME-Version: 1.0
Content-Type: multipart/alternative; boundary=offer

--offer
Content-Type: text/plain; charset=utf-8

Kitchen & Cooking ( https://email.example.com/c/a-very-long-tracking-destination ) | Bathroom ( https://email.example.com/c/another-tracking-destination )
--offer
Content-Type: text/html; charset=utf-8

<html><body><p>Refresh essentials.</p><p><a href="https://email.example.com/c/a-very-long-tracking-destination">Kitchen &amp; Cooking</a> | <a href="https://email.example.com/c/another-tracking-destination">Bathroom</a></p></body></html>
--offer--
"#,
        ));

        assert!(message.body.contains("Refresh essentials."));
        assert!(message.body.contains("Kitchen & Cooking | Bathroom"));
        assert!(!message.body.contains("email.example.com"));
        assert!(!message.body.contains("tracking-destination"));
    }
    #[test]
    fn malformed_and_missing_fields_have_bounded_fallbacks() {
        let message = map_message(raw(&[0xff, 0, 1]));
        assert_eq!(message.sender, "Unknown sender");
        assert_eq!(message.subject, "(No subject)");
        assert!(message.used_fallback);
        let huge = "🙂".repeat(20_000);
        assert!(cap(&huge, 16_384, BODY_BYTES).len() <= BODY_BYTES);
    }
    #[test]
    fn unicode_headers_rtl_and_grapheme_caps_are_safe() {
        let message = map_message(raw(b"From: =?UTF-8?B?5YWo5a2X?= <a@example.com>\r\nSubject: =?UTF-8?B?2YXYsdit2KjYpw==?=\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nhello"));
        assert_eq!(message.sender, "全字");
        assert_eq!(message.subject, "مرحبا");
        let family = "👨‍👩‍👧‍👦".repeat(100);
        let capped = cap(&family, 3, 1024);
        assert_eq!(capped.graphemes(true).count(), 3);
        assert!(std::str::from_utf8(capped.as_bytes()).is_ok());
    }
    #[test]
    fn multipart_prefers_plain_and_limits_attachment_metadata() {
        let mut mime = String::from(
            "From: A <a@example.com>\r\nSubject: Parts\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain\r\n\r\nplain preferred\r\n",
        );
        for index in 0..21 {
            mime.push_str(&format!("--x\r\nContent-Type: application/octet-stream; name=\"{index}.bin\"\r\nContent-Disposition: attachment; filename=\"{index}.bin\"\r\n\r\ndata\r\n"));
        }
        mime.push_str("--x--\r\n");
        let message = map_message(raw(mime.as_bytes()));
        assert!(message.body.contains("plain preferred"));
        assert_eq!(message.attachments.len(), 20);
    }
    #[test]
    fn repeated_dangerous_blocks_are_removed_in_one_bounded_pass() {
        let html = format!(
            "safe{}tail",
            "<ScRiPt>x()</ScRiPt><style>bad</style>".repeat(2000)
        );
        let cleaned = remove_dangerous_blocks(&html);
        assert_eq!(cleaned, "safetail");
    }
}
