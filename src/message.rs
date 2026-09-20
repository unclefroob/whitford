use crate::{
    gmail::{RawMessageBody, RawMessageSummary},
    model::{
        Attachment, FolderId, MessageBody, MessageId, MessageSummary, ReplyAddress, ReplyContext,
    },
};
use mail_parser::{Address, HeaderValue, MessageParser, MimeHeaders, PartType};
use unicode_segmentation::UnicodeSegmentation;

pub fn map_summary(raw: RawMessageSummary) -> MessageSummary {
    let parsed = MessageParser::default().parse_headers(&raw.header);
    let mut used_fallback = parsed.is_none();
    let (sender, email, subject, received_at) = if let Some(parsed) = parsed {
        let from = parsed.from().and_then(|addresses| addresses.first());
        let email = from
            .and_then(|address| address.address.as_deref())
            .filter(|value| !value.trim().is_empty())
            .map(|value| cap(value, 320, 320));
        let sender = from
            .and_then(|address| address.name.as_deref())
            .filter(|value| !value.trim().is_empty())
            .map(|value| cap(value, 160, 640))
            .or_else(|| email.clone())
            .unwrap_or_else(|| {
                used_fallback = true;
                "Unknown sender".into()
            });
        let subject = parsed
            .subject()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                used_fallback = true;
                "(No subject)"
            });
        (
            sender,
            email,
            cap(subject, 512, 2_048),
            parsed.date().map(|date| date.to_timestamp()),
        )
    } else {
        ("Unknown sender".into(), None, "(No subject)".into(), None)
    };
    let initials = initials(&sender);
    MessageSummary {
        id: MessageId::gmail(raw.uid_validity, raw.uid),
        folder_id: FolderId::Inbox,
        sender,
        email,
        initials,
        subject,
        received_at_unix: received_at.or(raw.internal_date_unix),
        unread: !raw.flags.seen,
        starred: raw.flags.flagged,
        attachment_state: raw.attachment_state,
        used_fallback,
    }
}

pub fn map_summaries(mut records: Vec<RawMessageSummary>) -> (Vec<MessageSummary>, usize) {
    records.sort_by_key(|record| std::cmp::Reverse(record.uid));
    let messages: Vec<_> = records.into_iter().map(map_summary).collect();
    let fallbacks = messages
        .iter()
        .filter(|message| message.used_fallback)
        .count();
    (messages, fallbacks)
}

pub fn map_body(raw: RawMessageBody) -> MessageBody {
    let parsed = MessageParser::default().parse(&raw.raw);
    let mut used_fallback = parsed.is_none();
    let (text, html, attachments, reply_context) = if let Some(parsed) = parsed {
        let html = parsed.html_part(0).and_then(|part| match &part.body {
            PartType::Html(value) if !value.trim().is_empty() => Some(value.clone().into_owned()),
            _ => None,
        });
        let html_text = html.as_deref().map(html_to_readable_text);
        let text = html_text
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
                    name: cap(name, 255, 1_024),
                    media_type,
                    octets,
                }
            })
            .collect();
        let reply_context = ReplyContext {
            from: addresses(parsed.from()),
            reply_to: addresses(parsed.reply_to()),
            to: addresses(parsed.to()),
            cc: addresses(parsed.cc()),
            message_id: bounded_message_id(parsed.message_id()),
            references: message_ids(parsed.references()),
            sent_at_unix: parsed.date().map(|date| date.to_timestamp()),
        };
        (normalize(&text), html, attachments, reply_context)
    } else {
        (
            "No readable message body.".into(),
            None,
            Vec::new(),
            ReplyContext::default(),
        )
    };
    MessageBody {
        text,
        html,
        attachments,
        reply_context,
        used_fallback,
    }
}

const MAX_REPLY_ADDRESSES_PER_HEADER: usize = 100;
const MAX_REFERENCE_IDS: usize = 50;

fn addresses(value: Option<&Address<'_>>) -> Vec<ReplyAddress> {
    value
        .into_iter()
        .flat_map(Address::iter)
        .filter_map(|address| {
            let email = address.address.as_deref()?.trim();
            if email.is_empty()
                || email.len() > 320
                || !email.contains('@')
                || email.chars().any(char::is_control)
            {
                return None;
            }
            let name = address
                .name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(|name| cap(name, 160, 640));
            Some(ReplyAddress {
                name,
                email: email.to_owned(),
            })
        })
        .take(MAX_REPLY_ADDRESSES_PER_HEADER)
        .collect()
}

fn bounded_message_id(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .map(|value| {
            value
                .strip_prefix('<')
                .and_then(|value| value.strip_suffix('>'))
                .unwrap_or(value)
        })
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 994
                && value.contains('@')
                && !value
                    .chars()
                    .any(|character| character.is_control() || character.is_whitespace())
                && !value.contains(['<', '>'])
        })
        .map(|value| format!("<{value}>"))
}

fn message_ids(value: &HeaderValue<'_>) -> Vec<String> {
    let values: Box<dyn Iterator<Item = &str> + '_> = match value {
        HeaderValue::Text(value) => Box::new(std::iter::once(value.as_ref())),
        HeaderValue::TextList(values) => Box::new(values.iter().map(AsRef::as_ref)),
        _ => Box::new(std::iter::empty()),
    };
    values
        .filter_map(|value| bounded_message_id(Some(value)))
        .take(MAX_REFERENCE_IDS)
        .collect()
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
    use crate::{gmail::MessageFlags, model::AttachmentState};

    fn summary(bytes: &[u8]) -> RawMessageSummary {
        RawMessageSummary {
            uid_validity: 7,
            uid: 9,
            flags: MessageFlags {
                seen: false,
                flagged: true,
            },
            internal_date_unix: Some(123),
            rfc822_size: Some(90_000),
            header: bytes.to_vec(),
            attachment_state: AttachmentState::Known(Vec::new()),
        }
    }

    fn body(bytes: &[u8]) -> RawMessageBody {
        RawMessageBody {
            uid_validity: 7,
            uid: 9,
            raw: bytes.to_vec(),
        }
    }

    #[test]
    fn summary_maps_only_bounded_headers_and_stable_uid() {
        let message = map_summary(summary(b"From: =?UTF-8?Q?Mara_Chen?= <mara@example.com>\r\nSubject: Hello\r\nDate: Tue, 1 Jan 2019 00:00:00 +0000\r\n\r\nignored body"));
        assert_eq!(message.id, MessageId::gmail(7, 9));
        assert_eq!(message.sender, "Mara Chen");
        assert_eq!(message.subject, "Hello");
        assert!(message.unread && message.starred);
        let huge = format!("Subject: {}\r\n\r\n", "🙂".repeat(20_000));
        assert!(map_summary(summary(huge.as_bytes())).subject.len() <= 2_048);
    }

    #[test]
    fn body_preserves_complete_plain_and_html_content() {
        let suffix = "x".repeat(100_000);
        let raw = format!(
            "From: A <a@b.test>\r\nContent-Type: text/html\r\n\r\n<p>Hello</p><p>{suffix}</p>"
        );
        let message = map_body(body(raw.as_bytes()));
        assert!(message.text.contains(&suffix));
        assert!(
            message
                .html
                .as_deref()
                .is_some_and(|html| html.contains(&suffix))
        );
    }

    #[test]
    fn body_keeps_bounded_reply_and_thread_metadata() {
        let message = map_body(body(
            b"From: Sender <sender@example.com>\r\n\
Reply-To: Team <reply@example.com>\r\n\
To: Me <me@example.com>, Other <other@example.com>\r\n\
Cc: Third <third@example.com>\r\n\
Date: Tue, 1 Jan 2019 00:00:00 +0000\r\n\
Message-ID: <parent@example.com>\r\n\
References: <root@example.com> <middle@example.com>\r\n\
Content-Type: text/plain\r\n\r\nHello",
        ));
        let context = message.reply_context;
        assert_eq!(context.from[0].email, "sender@example.com");
        assert_eq!(context.reply_to[0].email, "reply@example.com");
        assert_eq!(context.to.len(), 2);
        assert_eq!(context.cc[0].email, "third@example.com");
        assert_eq!(context.message_id.as_deref(), Some("<parent@example.com>"));
        assert_eq!(
            context.references,
            ["<root@example.com>", "<middle@example.com>"]
        );
        assert_eq!(context.sent_at_unix, Some(1_546_300_800));
    }

    #[test]
    fn html_text_removes_script_and_tracking_destinations() {
        let message = map_body(body(br#"From: e&s <offers@example.com>
Content-Type: text/html; charset=utf-8

<p><a href="https://email.example.com/tracking">Kitchen &amp; Cooking</a></p><script>bad()</script>"#));
        assert!(message.text.contains("Kitchen & Cooking"));
        assert!(!message.text.contains("tracking"));
        assert!(!message.text.contains("bad()"));
        assert!(message.html.is_some());
    }

    #[test]
    fn malformed_values_have_safe_fallbacks() {
        let message = map_summary(summary(&[0xff, 0, 1]));
        assert_eq!(message.sender, "Unknown sender");
        assert_eq!(message.subject, "(No subject)");
        assert!(message.used_fallback);
        let body = map_body(body(&[0xff, 0, 1]));
        assert_eq!(body.text, "No readable message body.");
        assert!(body.used_fallback);
    }

    #[test]
    fn email_address_is_a_complete_sender_when_display_name_is_absent() {
        let message = map_summary(summary(
            b"From: no-reply@example.com\r\nSubject: Maintenance complete\r\n\r\n",
        ));
        assert_eq!(message.sender, "no-reply@example.com");
        assert_eq!(message.email.as_deref(), Some("no-reply@example.com"));
        assert!(!message.used_fallback);
    }

    #[test]
    fn full_body_attachment_metadata_is_bounded() {
        let mut mime = String::from(
            "From: A <a@example.com>\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain\r\n\r\nplain\r\n",
        );
        for index in 0..21 {
            mime.push_str(&format!("--x\r\nContent-Type: application/octet-stream; name=\"{index}.bin\"\r\nContent-Disposition: attachment; filename=\"{index}.bin\"\r\n\r\ndata\r\n"));
        }
        mime.push_str("--x--\r\n");
        assert_eq!(map_body(body(mime.as_bytes())).attachments.len(), 20);
    }

    #[test]
    fn repeated_dangerous_blocks_are_removed_in_one_pass() {
        let html = format!(
            "safe{}tail",
            "<ScRiPt>x()</ScRiPt><style>bad</style>".repeat(2000)
        );
        assert_eq!(remove_dangerous_blocks(&html), "safetail");
    }
}
