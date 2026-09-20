use crate::model::{MessageId, ReplyAddress, ReplyContext};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, collections::HashSet};

pub const MAX_RECIPIENTS: usize = 100;
pub const MAX_SUBJECT_CHARS: usize = 512;
pub const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ComposeKind {
    Reply { original: MessageId },
    ReplyAll { original: MessageId },
    Forward { original: MessageId },
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct Recipient {
    pub name: Option<String>,
    pub email: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ThreadHeaders {
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct DraftAttachment {
    pub id: String,
    pub display_name: String,
    pub media_type: String,
    /// File name relative to this draft's private `files` directory.
    pub staged_file: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct DraftInlineImage {
    pub attachment: DraftAttachment,
    pub content_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ComposeDraft {
    pub id: String,
    pub account_email: String,
    pub kind: ComposeKind,
    pub to: Vec<Recipient>,
    pub cc: Vec<Recipient>,
    pub bcc: Vec<Recipient>,
    pub subject: String,
    pub html: String,
    pub text: String,
    pub attachments: Vec<DraftAttachment>,
    pub inline_images: Vec<DraftInlineImage>,
    pub thread: Option<ThreadHeaders>,
    pub dirty_revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComposeError {
    MissingRecipient,
    TooManyRecipients,
    InvalidAddress,
    BodyTooLarge,
}

pub fn new_reply(
    id: String,
    account_email: &str,
    original: MessageId,
    subject: &str,
    context: &ReplyContext,
    original_html: &str,
    signature_html: &str,
) -> Result<ComposeDraft, ComposeError> {
    let primary = primary_recipients(context);
    let Some(recipient) = primary.first() else {
        return Err(ComposeError::MissingRecipient);
    };
    draft(
        id,
        account_email,
        ComposeKind::Reply { original },
        vec![from_reply_address(recipient)],
        Vec::new(),
        reply_subject(subject),
        context,
        original_html,
        signature_html,
    )
}

pub fn new_reply_all(
    id: String,
    account_email: &str,
    original: MessageId,
    subject: &str,
    context: &ReplyContext,
    original_html: &str,
    signature_html: &str,
) -> Result<ComposeDraft, ComposeError> {
    let mut seen = HashSet::from([account_email.trim().to_ascii_lowercase()]);
    let mut to = Vec::new();
    let mut cc = Vec::new();
    append_unique(primary_recipients(context), &mut to, &mut seen);
    append_unique(&context.to, &mut to, &mut seen);
    append_unique(&context.cc, &mut cc, &mut seen);
    if to.is_empty() {
        return Err(ComposeError::MissingRecipient);
    }
    if to.len() + cc.len() > MAX_RECIPIENTS {
        return Err(ComposeError::TooManyRecipients);
    }
    draft(
        id,
        account_email,
        ComposeKind::ReplyAll { original },
        to,
        cc,
        reply_subject(subject),
        context,
        original_html,
        signature_html,
    )
}

pub fn new_forward(
    id: String,
    account_email: &str,
    original: MessageId,
    subject: &str,
    context: &ReplyContext,
    original_html: &str,
    signature_html: &str,
) -> Result<ComposeDraft, ComposeError> {
    let quote = forwarded_quote(context, original_html);
    let html = initial_html(signature_html, &quote);
    validate_html(&html)?;
    Ok(ComposeDraft {
        id,
        account_email: account_email.to_owned(),
        kind: ComposeKind::Forward { original },
        to: Vec::new(),
        cc: Vec::new(),
        bcc: Vec::new(),
        subject: forward_subject(subject),
        text: html_to_plain(&html),
        html,
        attachments: Vec::new(),
        inline_images: Vec::new(),
        thread: None,
        dirty_revision: 0,
    })
}

#[allow(clippy::too_many_arguments)]
fn draft(
    id: String,
    account_email: &str,
    kind: ComposeKind,
    to: Vec<Recipient>,
    cc: Vec<Recipient>,
    subject: String,
    context: &ReplyContext,
    original_html: &str,
    signature_html: &str,
) -> Result<ComposeDraft, ComposeError> {
    let quote = reply_quote(context, original_html);
    let html = initial_html(signature_html, &quote);
    validate_html(&html)?;
    Ok(ComposeDraft {
        id,
        account_email: account_email.to_owned(),
        kind,
        to,
        cc,
        bcc: Vec::new(),
        subject,
        text: html_to_plain(&html),
        html,
        attachments: Vec::new(),
        inline_images: Vec::new(),
        thread: Some(thread_headers(context)),
        dirty_revision: 0,
    })
}

pub fn sanitize_html(input: &str) -> String {
    ammonia::Builder::default()
        .tags(
            [
                "a",
                "b",
                "blockquote",
                "br",
                "caption",
                "center",
                "col",
                "colgroup",
                "div",
                "em",
                "font",
                "h1",
                "h2",
                "h3",
                "h4",
                "h5",
                "h6",
                "hr",
                "i",
                "img",
                "li",
                "ol",
                "p",
                "pre",
                "s",
                "small",
                "span",
                "strong",
                "sub",
                "sup",
                "table",
                "tbody",
                "td",
                "tfoot",
                "th",
                "thead",
                "tr",
                "u",
                "ul",
            ]
            .into_iter()
            .collect(),
        )
        .add_tag_attributes("a", ["href", "title"])
        .add_tag_attributes("img", ["src", "alt", "title", "width", "height", "border"])
        .add_generic_attributes(&[
            "align",
            "bgcolor",
            "border",
            "cellpadding",
            "cellspacing",
            "class",
            "dir",
            "height",
            "style",
            "valign",
            "width",
        ])
        .attribute_filter(|_, attribute, value| {
            if attribute == "style" {
                sanitize_email_style(value).map(Cow::Owned)
            } else {
                Some(Cow::Borrowed(value))
            }
        })
        .url_schemes(["http", "https", "mailto", "cid"].into_iter().collect())
        .link_rel(None)
        .clean(input)
        .to_string()
}

fn sanitize_email_style(value: &str) -> Option<String> {
    const SAFE_PROPERTIES: &[&str] = &[
        "background",
        "background-color",
        "border",
        "border-bottom",
        "border-collapse",
        "border-color",
        "border-left",
        "border-radius",
        "border-right",
        "border-spacing",
        "border-style",
        "border-top",
        "border-width",
        "color",
        "direction",
        "display",
        "font",
        "font-family",
        "font-size",
        "font-style",
        "font-weight",
        "height",
        "letter-spacing",
        "line-height",
        "margin",
        "margin-bottom",
        "margin-left",
        "margin-right",
        "margin-top",
        "max-height",
        "max-width",
        "min-height",
        "min-width",
        "padding",
        "padding-bottom",
        "padding-left",
        "padding-right",
        "padding-top",
        "table-layout",
        "text-align",
        "text-decoration",
        "text-indent",
        "text-transform",
        "vertical-align",
        "white-space",
        "width",
        "word-break",
        "word-spacing",
        "word-wrap",
    ];
    let declarations = value
        .split(';')
        .filter_map(|declaration| {
            let (property, value) = declaration.split_once(':')?;
            let property = property.trim().to_ascii_lowercase();
            let value = value.trim();
            let unsafe_value = value.to_ascii_lowercase();
            if !SAFE_PROPERTIES.contains(&property.as_str())
                || unsafe_value.contains("url(")
                || unsafe_value.contains("expression(")
                || unsafe_value.contains("javascript:")
                || value.chars().any(char::is_control)
            {
                return None;
            }
            Some(format!("{property}:{value}"))
        })
        .collect::<Vec<_>>()
        .join(";");
    (!declarations.is_empty()).then_some(declarations)
}

pub fn html_to_plain(input: &str) -> String {
    mail_parser::decoders::html::html_to_text(&sanitize_html(input))
        .trim()
        .to_owned()
}

pub fn plain_to_html(input: &str) -> String {
    let escaped = escape_html(input);
    escaped
        .split('\n')
        .map(|line| format!("<div>{}</div>", if line.is_empty() { "<br>" } else { line }))
        .collect()
}

pub fn validate_recipients(recipients: &[Recipient]) -> Result<(), ComposeError> {
    if recipients.len() > MAX_RECIPIENTS {
        return Err(ComposeError::TooManyRecipients);
    }
    for recipient in recipients {
        let value = recipient.email.trim();
        if value.is_empty()
            || value.len() > 320
            || value.chars().any(char::is_control)
            || !value.contains('@')
        {
            return Err(ComposeError::InvalidAddress);
        }
    }
    Ok(())
}

fn validate_html(html: &str) -> Result<(), ComposeError> {
    (html.len() <= MAX_HTML_BYTES)
        .then_some(())
        .ok_or(ComposeError::BodyTooLarge)
}

fn primary_recipients(context: &ReplyContext) -> &[ReplyAddress] {
    if context.reply_to.is_empty() {
        &context.from
    } else {
        &context.reply_to
    }
}

fn append_unique(source: &[ReplyAddress], target: &mut Vec<Recipient>, seen: &mut HashSet<String>) {
    for address in source {
        let key = address.email.trim().to_ascii_lowercase();
        if !key.is_empty() && seen.insert(key) {
            target.push(from_reply_address(address));
        }
    }
}

fn from_reply_address(value: &ReplyAddress) -> Recipient {
    Recipient {
        name: value.name.clone(),
        email: value.email.trim().to_owned(),
    }
}

fn initial_html(signature: &str, quote: &str) -> String {
    let signature = sanitize_html(signature);
    let signature = if signature.trim().is_empty() {
        String::new()
    } else {
        format!(r#"<div data-whitford-signature="true">{signature}</div>"#)
    };
    format!("<div><br></div>{signature}{quote}")
}

fn reply_quote(context: &ReplyContext, original_html: &str) -> String {
    let sender = context
        .from
        .first()
        .map(display_address)
        .unwrap_or_else(|| "Unknown sender".into());
    format!(
        r#"<div class="whitford-quote-intro">On the original message, {} wrote:</div><blockquote class="whitford-quote">{}</blockquote>"#,
        escape_html(&sender),
        sanitize_html(original_html)
    )
}

fn forwarded_quote(context: &ReplyContext, original_html: &str) -> String {
    let sender = context
        .from
        .first()
        .map(display_address)
        .unwrap_or_else(|| "Unknown sender".into());
    format!(
        r#"<div class="whitford-forward"><p>---------- Forwarded message ---------</p><p>From: {}</p>{}</div>"#,
        escape_html(&sender),
        sanitize_html(original_html)
    )
}

fn display_address(address: &ReplyAddress) -> String {
    address
        .name
        .as_ref()
        .filter(|v| !v.trim().is_empty())
        .map_or_else(
            || address.email.clone(),
            |name| format!("{name} <{}>", address.email),
        )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn clean_subject(subject: &str) -> String {
    subject
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_SUBJECT_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

pub fn reply_subject(subject: &str) -> String {
    let value = clean_subject(subject);
    if value
        .get(..3)
        .is_some_and(|v| v.eq_ignore_ascii_case("re:"))
    {
        value
    } else if value.is_empty() {
        "Re: (No subject)".into()
    } else {
        format!("Re: {value}")
    }
}

pub fn forward_subject(subject: &str) -> String {
    let value = clean_subject(subject);
    if value
        .get(..4)
        .is_some_and(|v| v.eq_ignore_ascii_case("fwd:"))
    {
        value
    } else if value.is_empty() {
        "Fwd: (No subject)".into()
    } else {
        format!("Fwd: {value}")
    }
}

fn thread_headers(context: &ReplyContext) -> ThreadHeaders {
    ThreadHeaders {
        in_reply_to: context.message_id.clone(),
        references: context.references.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(email: &str) -> ReplyAddress {
        ReplyAddress {
            name: None,
            email: email.into(),
        }
    }

    #[test]
    fn reply_all_deduplicates_and_excludes_self() {
        let context = ReplyContext {
            from: vec![address("sender@example.com")],
            to: vec![address("me@example.com"), address("OTHER@example.com")],
            cc: vec![address("other@example.com")],
            ..Default::default()
        };
        let draft = new_reply_all(
            "d".into(),
            "ME@example.com",
            MessageId::gmail(1),
            "Hi",
            &context,
            "<p>old</p>",
            "",
        )
        .unwrap();
        assert_eq!(draft.to.len(), 2);
        assert!(draft.cc.is_empty());
    }

    #[test]
    fn sanitizer_removes_active_content() {
        let clean = sanitize_html(
            r#"<script>x()</script><p onclick="x()"><a href="javascript:x()">Hi</a><img src="cid:image"></p>"#,
        );
        assert!(!clean.contains("script"));
        assert!(!clean.contains("onclick"));
        assert!(!clean.contains("javascript"));
        assert!(clean.contains("cid:image"));
    }

    #[test]
    fn sanitizer_preserves_safe_email_layout_and_inline_formatting() {
        let clean = sanitize_html(
            r#"<table width="600" cellpadding="8" style="background-color:#fff;position:fixed;background-image:url(https://tracker.test/p)"><tr><td align="center" style="font-size:18px;color:#123456">Hello</td></tr></table>"#,
        );
        assert!(clean.contains("<table"));
        assert!(clean.contains("width=\"600\""));
        assert!(clean.contains("cellpadding=\"8\""));
        assert!(clean.contains("background-color:#fff"));
        assert!(clean.contains("font-size:18px;color:#123456"));
        assert!(!clean.contains("position:fixed"));
        assert!(!clean.contains("tracker.test"));
    }

    #[test]
    fn forward_has_no_thread_headers_and_plain_alternative() {
        let draft = new_forward(
            "d".into(),
            "me@example.com",
            MessageId::gmail(2),
            "Topic",
            &ReplyContext::default(),
            "<p>Hello <b>there</b></p>",
            "",
        )
        .unwrap();
        assert_eq!(draft.subject, "Fwd: Topic");
        assert!(draft.thread.is_none());
        assert!(draft.text.contains("Hello there"));
    }
}
