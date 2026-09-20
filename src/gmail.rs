use crate::model::{
    Attachment, AttachmentState, FolderCatalog, FolderDescriptor, FolderId, FolderKind, MessageId,
    MessageLocator, MessageMutation, MimePartDescriptor, ReconciledMessageState, TransferEncoding,
};
use async_imap::{
    imap_proto::types::{
        BodyContentCommon, BodyContentSinglePart, BodyStructure, ContentEncoding, SectionPath,
    },
    types::NameAttribute,
};
use futures_util::TryStreamExt;
use std::collections::BTreeSet;
use tokio::{
    net::TcpStream,
    time::{Duration, timeout},
};
use tokio_rustls::{
    TlsConnector,
    rustls::{ClientConfig, RootCertStore, pki_types::ServerName},
};

pub const IMAP_HOST: &str = "imap.gmail.com";
pub const IMAP_PORT: u16 = 993;
pub const RETENTION_OPTIONS: [usize; 4] = [50, 100, 250, 500];
pub const SUMMARY_FETCH_QUERY: &str = "(UID X-GM-MSGID X-GM-LABELS FLAGS INTERNALDATE RFC822.SIZE BODYSTRUCTURE BODY.PEEK[HEADER.FIELDS (FROM SUBJECT DATE MESSAGE-ID)])";
pub const BODY_PLAN_QUERY: &str = "(UID X-GM-MSGID BODYSTRUCTURE BODY.PEEK[HEADER.FIELDS (FROM REPLY-TO TO CC DATE MESSAGE-ID REFERENCES)])";
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_ATTACHMENTS: usize = 20;
pub const ATTACHMENT_CHUNK_BYTES: usize = 1024 * 1024;
pub const MAX_ATTACHMENT_BYTES: u64 = 100 * 1024 * 1024;
const MAX_ATTACHMENT_ENCODED_BYTES: u64 = 150 * 1024 * 1024;
pub const MAX_SEARCH_QUERY_BYTES: usize = 2 * 1024;
pub const MAX_SEARCH_RESULTS: usize = 500;
pub const SEARCH_SUMMARY_BATCH: usize = 100;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MessageFlags {
    pub seen: bool,
    pub flagged: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawMessageSummary {
    pub id: MessageId,
    pub locator: MessageLocator,
    pub flags: MessageFlags,
    pub internal_date_unix: Option<i64>,
    pub rfc822_size: Option<u32>,
    pub header: Vec<u8>,
    pub attachment_state: AttachmentState,
    pub labels: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawMessageBody {
    pub id: MessageId,
    pub locator: MessageLocator,
    /// Retained for parsing fixtures and migration tests only. Production body
    /// loads use exact MIME sections and leave this empty.
    pub raw: Vec<u8>,
    pub header: Vec<u8>,
    pub plain: Option<RawTextPart>,
    pub html: Option<RawTextPart>,
    pub attachments: Vec<Attachment>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawTextPart {
    pub descriptor: MimePartDescriptor,
    pub media_type: String,
    pub charset: Option<String>,
    pub raw: Vec<u8>,
}

#[derive(Debug)]
pub struct InboxFetch {
    pub records: Vec<RawMessageSummary>,
    pub skipped_count: usize,
    pub folder_catalog: FolderCatalog,
}

#[derive(Debug)]
pub struct SearchFetch {
    pub records: Vec<RawMessageSummary>,
    pub skipped_count: usize,
    pub truncated: bool,
}

struct SummaryCandidate {
    uid: Option<u32>,
    x_gm_msgid: Option<u64>,
    flags: MessageFlags,
    internal_date_unix: Option<i64>,
    size: Option<u32>,
    header: Option<Vec<u8>>,
    attachment_state: AttachmentState,
    labels: Vec<String>,
}

struct ListedMailbox {
    mailbox: String,
    display_name: String,
    kind: Option<FolderKind>,
    selectable: bool,
    ignored_special_use: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GmailError {
    Offline,
    TlsFailed,
    AuthenticationFailed,
    InboxUnavailable,
    MailboxChanged,
    MessageMissing,
    Protocol,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachmentFetchError {
    Gmail(GmailError),
    TooLarge,
    InvalidEncoding,
    WriteFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MutationResult {
    Confirmed,
    Reconciled(Option<ReconciledMessageState>),
    DefiniteFailure(GmailError),
    Uncertain,
}

#[derive(Debug)]
enum MutationAttemptError {
    Definite(GmailError),
    Uncertain,
}

fn user_labels<'a>(labels: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut labels = labels
        .into_iter()
        .filter(|label| !label.starts_with('\\'))
        .filter(|label| !label.is_empty() && label.len() <= crate::model::MAX_FOLDER_MAILBOX_BYTES)
        .take(crate::model::MAX_MESSAGE_LABELS)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    labels.sort();
    labels.dedup();
    labels
}

fn quote_label(label: &str) -> Option<String> {
    (!label.is_empty()
        && label.len() <= crate::model::MAX_FOLDER_MAILBOX_BYTES
        && !label.chars().any(char::is_control))
    .then(|| format!("\"{}\"", label.replace('\\', "\\\\").replace('"', "\\\"")))
}

fn mutation_store_query(mutation: &MessageMutation) -> Option<String> {
    match mutation {
        MessageMutation::SetRead(true) => Some("+FLAGS.SILENT (\\Seen)".into()),
        MessageMutation::SetRead(false) => Some("-FLAGS.SILENT (\\Seen)".into()),
        MessageMutation::SetStarred(true) => Some("+FLAGS.SILENT (\\Flagged)".into()),
        MessageMutation::SetStarred(false) => Some("-FLAGS.SILENT (\\Flagged)".into()),
        MessageMutation::Archive => Some("-X-GM-LABELS.SILENT (\\Inbox)".into()),
        MessageMutation::SetLabel { mailbox, applied } => Some(format!(
            "{}X-GM-LABELS.SILENT ({})",
            if *applied { "+" } else { "-" },
            quote_label(mailbox)?
        )),
        MessageMutation::MoveToTrash { .. } => None,
    }
}

pub fn valid_retention_limit(limit: usize) -> bool {
    RETENTION_OPTIONS.contains(&limit)
}

pub fn xoauth2_response(email: &str, access_token: &str) -> Result<Vec<u8>, GmailError> {
    if email.trim().is_empty()
        || access_token.trim().is_empty()
        || email.bytes().any(|byte| byte <= 0x20 || byte == 0x7f)
        || access_token
            .bytes()
            .any(|byte| byte <= 0x20 || byte == 0x7f)
    {
        return Err(GmailError::AuthenticationFailed);
    }
    Ok(format!("user={email}\x01auth=Bearer {access_token}\x01\x01").into_bytes())
}

pub fn select_newest_uids<I: IntoIterator<Item = u32>>(uids: I, limit: usize) -> Vec<u32> {
    if !valid_retention_limit(limit) {
        return Vec::new();
    }
    let sorted = uids
        .into_iter()
        .filter(|uid| *uid != 0)
        .collect::<BTreeSet<_>>();
    let skip = sorted.len().saturating_sub(limit);
    sorted.into_iter().skip(skip).collect()
}

pub fn uid_sequence_set(uids: &[u32]) -> Option<String> {
    (!uids.is_empty() && uids.iter().all(|uid| *uid != 0)).then(|| {
        uids.iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    })
}

pub fn validate_search_query(query: &str) -> Option<&str> {
    let query = query.trim();
    (!query.is_empty()
        && query.len() <= MAX_SEARCH_QUERY_BYTES
        && !query.chars().any(char::is_control))
    .then_some(query)
}

fn quote_search_query(query: &str) -> Option<String> {
    validate_search_query(query)
        .map(|query| format!("\"{}\"", query.replace('\\', "\\\\").replace('"', "\\\"")))
}

pub fn newest_search_uids<I: IntoIterator<Item = u32>>(uids: I) -> (Vec<u32>, bool) {
    let sorted = uids
        .into_iter()
        .filter(|uid| *uid != 0)
        .collect::<BTreeSet<_>>();
    let truncated = sorted.len() > MAX_SEARCH_RESULTS;
    let skip = sorted.len().saturating_sub(MAX_SEARCH_RESULTS);
    (sorted.into_iter().skip(skip).collect(), truncated)
}

pub fn search_uid_batches(uids: &[u32]) -> impl Iterator<Item = &[u32]> {
    uids.chunks(SEARCH_SUMMARY_BATCH)
}

pub fn newest_sequence_set(message_count: u32, limit: usize) -> Option<String> {
    if message_count == 0 || !valid_retention_limit(limit) {
        return None;
    }
    let bounded = u32::try_from(limit).ok()?;
    let first = message_count
        .saturating_sub(bounded.saturating_sub(1))
        .max(1);
    Some(format!("{first}:{message_count}"))
}

fn discovered_uids<I: IntoIterator<Item = Option<u32>>>(
    expected: usize,
    limit: usize,
    values: I,
) -> (Vec<u32>, usize) {
    let unique = values
        .into_iter()
        .flatten()
        .filter(|uid| *uid > 0)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let expected = expected.min(limit);
    let first = unique.len().saturating_sub(expected);
    let uids = unique[first..].to_vec();
    let skipped = expected
        .saturating_sub(uids.len())
        .saturating_add(unique.len().saturating_sub(expected));
    (uids, skipped)
}

fn bounded(value: &str, max_chars: usize, max_bytes: usize) -> String {
    value
        .chars()
        .take(max_chars)
        .scan(0_usize, |bytes, character| {
            let next = *bytes + character.len_utf8();
            (next <= max_bytes).then(|| {
                *bytes = next;
                character
            })
        })
        .collect()
}

fn parameter<'a>(
    params: &'a Option<Vec<(std::borrow::Cow<'a, str>, std::borrow::Cow<'a, str>)>>,
    key: &str,
) -> Option<&'a str> {
    params
        .as_ref()?
        .iter()
        .find_map(|(name, value)| name.eq_ignore_ascii_case(key).then_some(value.as_ref()))
}

fn attachment_from_leaf(
    common: &BodyContentCommon<'_>,
    other: &BodyContentSinglePart<'_>,
    path: &[u32],
) -> Option<Attachment> {
    let disposition_name = common
        .disposition
        .as_ref()
        .and_then(|value| parameter(&value.params, "filename"));
    let type_name = parameter(&common.ty.params, "name");
    let explicitly_attached = common
        .disposition
        .as_ref()
        .is_some_and(|value| value.ty.eq_ignore_ascii_case("attachment"));
    if !explicitly_attached
        && disposition_name.is_none()
        && type_name.is_none()
        && common.ty.ty.eq_ignore_ascii_case("text")
    {
        return None;
    }
    let name = disposition_name
        .or(type_name)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Unnamed attachment");
    Some(Attachment {
        name: bounded(name, 255, 1_024),
        media_type: Some(format!(
            "{}/{}",
            bounded(common.ty.ty.as_ref(), 64, 128).to_ascii_lowercase(),
            bounded(common.ty.subtype.as_ref(), 64, 128).to_ascii_lowercase()
        )),
        octets: Some(u64::from(other.octets)),
        part: MimePartDescriptor {
            path: path.to_vec(),
            encoding: transfer_encoding(&other.transfer_encoding),
            encoded_octets: u64::from(other.octets),
        },
    })
}

fn transfer_encoding(value: &ContentEncoding<'_>) -> TransferEncoding {
    match value {
        ContentEncoding::SevenBit => TransferEncoding::SevenBit,
        ContentEncoding::EightBit => TransferEncoding::EightBit,
        ContentEncoding::Binary => TransferEncoding::Binary,
        ContentEncoding::Base64 => TransferEncoding::Base64,
        ContentEncoding::QuotedPrintable => TransferEncoding::QuotedPrintable,
        ContentEncoding::Other(_) => TransferEncoding::Unsupported,
    }
}

fn collect_attachments(
    structure: &BodyStructure<'_>,
    path: &mut Vec<u32>,
    output: &mut Vec<Attachment>,
) {
    if output.len() >= MAX_ATTACHMENTS {
        return;
    }
    match structure {
        BodyStructure::Basic { common, other, .. } | BodyStructure::Text { common, other, .. } => {
            if let Some(value) = attachment_from_leaf(common, other, path) {
                output.push(value);
            }
        }
        BodyStructure::Message {
            common,
            other,
            body,
            ..
        } => {
            if let Some(value) = attachment_from_leaf(common, other, path) {
                output.push(value);
            } else {
                path.push(1);
                collect_attachments(body, path, output);
                path.pop();
            }
        }
        BodyStructure::Multipart { bodies, .. } => {
            for (index, body) in bodies.iter().enumerate() {
                let Ok(index) = u32::try_from(index + 1) else {
                    break;
                };
                path.push(index);
                collect_attachments(body, path, output);
                path.pop();
                if output.len() >= MAX_ATTACHMENTS {
                    break;
                }
            }
        }
    }
}

fn attachment_state(structure: Option<&BodyStructure<'_>>) -> AttachmentState {
    let Some(structure) = structure else {
        return AttachmentState::Unknown;
    };
    let mut attachments = Vec::new();
    let mut path = if matches!(structure, BodyStructure::Multipart { .. }) {
        Vec::new()
    } else {
        vec![1]
    };
    collect_attachments(structure, &mut path, &mut attachments);
    AttachmentState::Known(attachments)
}

#[derive(Clone)]
struct TextPlan {
    descriptor: MimePartDescriptor,
    media_type: String,
    charset: Option<String>,
}

#[derive(Default)]
struct BodyPlan {
    plain: Option<TextPlan>,
    html: Option<TextPlan>,
    attachments: Vec<Attachment>,
}

fn collect_body_plan(structure: &BodyStructure<'_>, path: &mut Vec<u32>, plan: &mut BodyPlan) {
    if plan.attachments.len() >= MAX_ATTACHMENTS && plan.plain.is_some() && plan.html.is_some() {
        return;
    }
    match structure {
        BodyStructure::Basic { common, other, .. } | BodyStructure::Text { common, other, .. } => {
            if let Some(attachment) = attachment_from_leaf(common, other, path) {
                if plan.attachments.len() < MAX_ATTACHMENTS {
                    plan.attachments.push(attachment);
                }
                return;
            }
            if !common.ty.ty.eq_ignore_ascii_case("text") {
                return;
            }
            let descriptor = MimePartDescriptor {
                path: path.clone(),
                encoding: transfer_encoding(&other.transfer_encoding),
                encoded_octets: u64::from(other.octets),
            };
            if !descriptor.is_valid() {
                return;
            }
            let value = TextPlan {
                descriptor,
                media_type: format!(
                    "text/{}",
                    bounded(common.ty.subtype.as_ref(), 64, 128).to_ascii_lowercase()
                ),
                charset: parameter(&common.ty.params, "charset")
                    .map(|value| bounded(value, 64, 128)),
            };
            if common.ty.subtype.eq_ignore_ascii_case("html") {
                plan.html.get_or_insert(value);
            } else if common.ty.subtype.eq_ignore_ascii_case("plain") {
                plan.plain.get_or_insert(value);
            }
        }
        BodyStructure::Message {
            common,
            other,
            body,
            ..
        } => {
            if let Some(attachment) = attachment_from_leaf(common, other, path) {
                if plan.attachments.len() < MAX_ATTACHMENTS {
                    plan.attachments.push(attachment);
                }
            } else {
                path.push(1);
                collect_body_plan(body, path, plan);
                path.pop();
            }
        }
        BodyStructure::Multipart { bodies, .. } => {
            for (index, body) in bodies.iter().enumerate() {
                let Ok(index) = u32::try_from(index + 1) else {
                    break;
                };
                path.push(index);
                collect_body_plan(body, path, plan);
                path.pop();
            }
        }
    }
}

fn body_plan(structure: &BodyStructure<'_>) -> BodyPlan {
    let mut plan = BodyPlan::default();
    let mut path = if matches!(structure, BodyStructure::Multipart { .. }) {
        Vec::new()
    } else {
        vec![1]
    };
    collect_body_plan(structure, &mut path, &mut plan);
    plan
}

fn folder_catalog(entries: impl IntoIterator<Item = ListedMailbox>) -> FolderCatalog {
    let mut folders: Vec<_> = entries
        .into_iter()
        .filter(|entry| entry.selectable && !entry.ignored_special_use)
        .map(|entry| {
            let kind = entry.kind.unwrap_or_else(|| {
                if entry.mailbox.eq_ignore_ascii_case("INBOX") {
                    FolderKind::Inbox
                } else {
                    FolderKind::Label
                }
            });
            let id = match kind {
                FolderKind::Inbox => FolderId::Inbox,
                FolderKind::Sent => FolderId::Sent,
                FolderKind::AllMail => FolderId::AllMail,
                FolderKind::Trash => FolderId::Trash,
                FolderKind::Starred => FolderId::Starred,
                FolderKind::Label => FolderId::Label(entry.mailbox.clone()),
            };
            FolderDescriptor {
                id,
                mailbox: entry.mailbox,
                display_name: entry.display_name,
                kind,
            }
        })
        .collect();
    if !folders.iter().any(|folder| folder.id == FolderId::Inbox) {
        folders.push(FolderDescriptor {
            id: FolderId::Inbox,
            mailbox: "INBOX".into(),
            display_name: "Inbox".into(),
            kind: FolderKind::Inbox,
        });
    }
    FolderCatalog::bounded(folders)
}

fn list_entry(name: &async_imap::types::Name) -> ListedMailbox {
    let attributes = name.attributes();
    let kind = if name.name().eq_ignore_ascii_case("INBOX") {
        Some(FolderKind::Inbox)
    } else if attributes.contains(&NameAttribute::Sent) {
        Some(FolderKind::Sent)
    } else if attributes.contains(&NameAttribute::All) {
        Some(FolderKind::AllMail)
    } else if attributes.contains(&NameAttribute::Trash) {
        Some(FolderKind::Trash)
    } else if attributes.contains(&NameAttribute::Flagged) {
        Some(FolderKind::Starred)
    } else {
        None
    };
    let ignored_special_use = attributes.iter().any(|attribute| match attribute {
        NameAttribute::Archive | NameAttribute::Drafts | NameAttribute::Junk => true,
        NameAttribute::Extension(value) => value.eq_ignore_ascii_case("\\Important"),
        _ => false,
    });
    let display_name = match kind {
        Some(FolderKind::Inbox) => "Inbox".into(),
        Some(FolderKind::Sent) => "Sent".into(),
        Some(FolderKind::AllMail) => "All Mail".into(),
        Some(FolderKind::Trash) => "Trash".into(),
        Some(FolderKind::Starred) => "Starred".into(),
        _ => name.name().to_owned(),
    };
    ListedMailbox {
        mailbox: name.name().to_owned(),
        display_name,
        kind,
        selectable: !attributes.contains(&NameAttribute::NoSelect),
        ignored_special_use,
    }
}

struct Xoauth2(Vec<u8>);
impl async_imap::Authenticator for Xoauth2 {
    type Response = Vec<u8>;
    fn process(&mut self, challenge: &[u8]) -> Self::Response {
        if challenge.is_empty() {
            self.0.clone()
        } else {
            Vec::new()
        }
    }
}

async fn tls_client()
-> Result<async_imap::Client<tokio_rustls::client::TlsStream<TcpStream>>, GmailError> {
    let tcp = TcpStream::connect((IMAP_HOST, IMAP_PORT))
        .await
        .map_err(|_| GmailError::Offline)?;
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = ClientConfig::builder_with_provider(
        tokio_rustls::rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .map_err(|_| GmailError::TlsFailed)?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let server_name = ServerName::try_from(IMAP_HOST).map_err(|_| GmailError::TlsFailed)?;
    let tls = TlsConnector::from(std::sync::Arc::new(config))
        .connect(server_name, tcp)
        .await
        .map_err(|_| GmailError::TlsFailed)?;
    let mut client = async_imap::Client::new(tls);
    client
        .read_response()
        .await
        .map_err(|_| GmailError::Protocol)?
        .ok_or(GmailError::Protocol)?;
    Ok(client)
}

pub async fn fetch_inbox(
    email: &str,
    access_token: &str,
    limit: usize,
) -> Result<InboxFetch, GmailError> {
    if !valid_retention_limit(limit) {
        return Err(GmailError::Protocol);
    }
    timeout(
        Duration::from_secs(30),
        fetch_inbox_inner(email, access_token, limit),
    )
    .await
    .map_err(|_| GmailError::TimedOut)?
}

pub async fn fetch_folder(
    email: &str,
    access_token: &str,
    folder: FolderDescriptor,
    limit: usize,
) -> Result<InboxFetch, GmailError> {
    if !valid_retention_limit(limit) || !folder.is_valid() {
        return Err(GmailError::Protocol);
    }
    timeout(
        Duration::from_secs(30),
        fetch_folder_inner(email, access_token, folder, limit),
    )
    .await
    .map_err(|_| GmailError::TimedOut)?
}

pub async fn search_all_mail(
    email: &str,
    access_token: &str,
    folder: FolderDescriptor,
    query: &str,
) -> Result<SearchFetch, GmailError> {
    if folder.id != FolderId::AllMail
        || !folder.is_valid()
        || validate_search_query(query).is_none()
    {
        return Err(GmailError::Protocol);
    }
    timeout(
        Duration::from_secs(30),
        search_all_mail_inner(email, access_token, folder, query),
    )
    .await
    .map_err(|_| GmailError::TimedOut)?
}

async fn search_all_mail_inner(
    email: &str,
    access_token: &str,
    folder: FolderDescriptor,
    query: &str,
) -> Result<SearchFetch, GmailError> {
    let mut session = authenticated_session(email, access_token).await?;
    let mailbox = session
        .examine(&folder.mailbox)
        .await
        .map_err(|_| GmailError::InboxUnavailable)?;
    let uid_validity = mailbox.uid_validity.ok_or(GmailError::Protocol)?;
    let quoted = quote_search_query(query).ok_or(GmailError::Protocol)?;
    let found = session
        .uid_search(format!("X-GM-RAW {quoted}"))
        .await
        .map_err(|_| GmailError::Protocol)?;
    let (uids, truncated) = newest_search_uids(found);
    let requested = uids.iter().copied().collect::<BTreeSet<_>>();
    let mut candidates = Vec::with_capacity(uids.len());
    for batch in search_uid_batches(&uids) {
        let sequence = uid_sequence_set(batch).ok_or(GmailError::Protocol)?;
        let fetches: Vec<_> = session
            .uid_fetch(sequence, SUMMARY_FETCH_QUERY)
            .await
            .map_err(|_| GmailError::Protocol)?
            .try_collect()
            .await
            .map_err(|_| GmailError::Protocol)?;
        candidates.extend(fetches.into_iter().map(|fetch| {
            SummaryCandidate {
                uid: fetch.uid,
                x_gm_msgid: fetch.gmail_msg_id().copied(),
                flags: MessageFlags {
                    seen: fetch
                        .flags()
                        .any(|flag| matches!(flag, async_imap::types::Flag::Seen)),
                    flagged: fetch
                        .flags()
                        .any(|flag| matches!(flag, async_imap::types::Flag::Flagged)),
                },
                internal_date_unix: fetch.internal_date().map(|date| date.timestamp()),
                size: fetch.size,
                header: fetch
                    .header()
                    .map(|value| value[..value.len().min(MAX_HEADER_BYTES)].to_vec()),
                attachment_state: attachment_state(fetch.bodystructure()),
                labels: user_labels(
                    fetch
                        .gmail_labels()
                        .into_iter()
                        .flatten()
                        .map(|label| label.as_ref()),
                ),
            }
        }));
    }
    let _ = session.logout().await;
    let result = classify_summaries(folder, uid_validity, &requested, candidates);
    Ok(SearchFetch {
        records: result.records,
        skipped_count: result.skipped_count,
        truncated,
    })
}

async fn fetch_folder_inner(
    email: &str,
    access_token: &str,
    folder: FolderDescriptor,
    limit: usize,
) -> Result<InboxFetch, GmailError> {
    let mut session = authenticated_session(email, access_token).await?;
    let mailbox = session
        .examine(&folder.mailbox)
        .await
        .map_err(|_| GmailError::InboxUnavailable)?;
    let uid_validity = mailbox.uid_validity.ok_or(GmailError::Protocol)?;
    let Some(sequence_numbers) = newest_sequence_set(mailbox.exists, limit) else {
        let _ = session.logout().await;
        return Ok(InboxFetch {
            records: Vec::new(),
            skipped_count: 0,
            folder_catalog: FolderCatalog::bounded(vec![folder]),
        });
    };
    let expected =
        usize::try_from(mailbox.exists.min(limit as u32)).map_err(|_| GmailError::Protocol)?;
    let uid_fetches: Vec<_> = session
        .fetch(&sequence_numbers, "(UID)")
        .await
        .map_err(|_| GmailError::Protocol)?
        .try_collect()
        .await
        .map_err(|_| GmailError::Protocol)?;
    let (uids, discovery_skipped) = discovered_uids(
        expected,
        limit,
        uid_fetches.into_iter().map(|fetch| fetch.uid),
    );
    let Some(sequence) = uid_sequence_set(&uids) else {
        let _ = session.logout().await;
        return Ok(InboxFetch {
            records: Vec::new(),
            skipped_count: discovery_skipped,
            folder_catalog: FolderCatalog::bounded(vec![folder]),
        });
    };
    let fetches: Vec<_> = session
        .uid_fetch(sequence, SUMMARY_FETCH_QUERY)
        .await
        .map_err(|_| GmailError::Protocol)?
        .try_collect()
        .await
        .map_err(|_| GmailError::Protocol)?;
    let requested = uids.into_iter().collect::<BTreeSet<_>>();
    let candidates = fetches
        .into_iter()
        .map(|fetch| SummaryCandidate {
            uid: fetch.uid,
            x_gm_msgid: fetch.gmail_msg_id().copied(),
            flags: MessageFlags {
                seen: fetch
                    .flags()
                    .any(|flag| matches!(flag, async_imap::types::Flag::Seen)),
                flagged: fetch
                    .flags()
                    .any(|flag| matches!(flag, async_imap::types::Flag::Flagged)),
            },
            internal_date_unix: fetch.internal_date().map(|date| date.timestamp()),
            size: fetch.size,
            header: fetch
                .header()
                .map(|value| value[..value.len().min(MAX_HEADER_BYTES)].to_vec()),
            attachment_state: attachment_state(fetch.bodystructure()),
            labels: user_labels(
                fetch
                    .gmail_labels()
                    .into_iter()
                    .flatten()
                    .map(|label| label.as_ref()),
            ),
        })
        .collect();
    let _ = session.logout().await;
    let mut result = classify_summaries(folder, uid_validity, &requested, candidates);
    result.skipped_count = result.skipped_count.saturating_add(discovery_skipped);
    Ok(result)
}

async fn fetch_inbox_inner(
    email: &str,
    access_token: &str,
    limit: usize,
) -> Result<InboxFetch, GmailError> {
    let client = tls_client().await?;
    let auth = Xoauth2(xoauth2_response(email, access_token)?);
    let mut session = client
        .authenticate("XOAUTH2", auth)
        .await
        .map_err(|_| GmailError::AuthenticationFailed)?;
    let listed = {
        let mut stream = session
            .list(None, Some("*"))
            .await
            .map_err(|_| GmailError::Protocol)?;
        let mut listed = Vec::with_capacity(crate::model::MAX_FOLDER_CATALOG_ENTRIES);
        let mut special_count = 0_usize;
        while let Some(name) = stream.try_next().await.map_err(|_| GmailError::Protocol)? {
            let entry = list_entry(&name);
            let retain_special = entry.kind.is_some() && special_count < 16;
            if retain_special {
                special_count += 1;
            }
            if retain_special || listed.len() < crate::model::MAX_FOLDER_CATALOG_ENTRIES {
                listed.push(entry);
            }
        }
        listed
    };
    let folder_catalog = folder_catalog(listed);
    let mailbox = session
        .examine("INBOX")
        .await
        .map_err(|_| GmailError::InboxUnavailable)?;
    let uid_validity = mailbox.uid_validity.ok_or(GmailError::Protocol)?;
    let Some(sequence_numbers) = newest_sequence_set(mailbox.exists, limit) else {
        let _ = session.logout().await;
        return Ok(InboxFetch {
            records: Vec::new(),
            skipped_count: 0,
            folder_catalog,
        });
    };
    let expected =
        usize::try_from(mailbox.exists.min(limit as u32)).map_err(|_| GmailError::Protocol)?;
    let uid_fetches: Vec<_> = session
        .fetch(&sequence_numbers, "(UID)")
        .await
        .map_err(|_| GmailError::Protocol)?
        .try_collect()
        .await
        .map_err(|_| GmailError::Protocol)?;
    let (uids, discovery_skipped) = discovered_uids(
        expected,
        limit,
        uid_fetches.into_iter().map(|fetch| fetch.uid),
    );
    let Some(sequence) = uid_sequence_set(&uids) else {
        let _ = session.logout().await;
        return Ok(InboxFetch {
            records: Vec::new(),
            skipped_count: discovery_skipped,
            folder_catalog,
        });
    };
    let fetches: Vec<_> = session
        .uid_fetch(sequence, SUMMARY_FETCH_QUERY)
        .await
        .map_err(|_| GmailError::Protocol)?
        .try_collect()
        .await
        .map_err(|_| GmailError::Protocol)?;
    let requested: BTreeSet<_> = uids.into_iter().collect();
    let candidates = fetches
        .into_iter()
        .map(|fetch| SummaryCandidate {
            uid: fetch.uid,
            x_gm_msgid: fetch.gmail_msg_id().copied(),
            flags: MessageFlags {
                seen: fetch
                    .flags()
                    .any(|flag| matches!(flag, async_imap::types::Flag::Seen)),
                flagged: fetch
                    .flags()
                    .any(|flag| matches!(flag, async_imap::types::Flag::Flagged)),
            },
            internal_date_unix: fetch.internal_date().map(|date| date.timestamp()),
            size: fetch.size,
            header: fetch
                .header()
                .map(|value| value[..value.len().min(MAX_HEADER_BYTES)].to_vec()),
            attachment_state: attachment_state(fetch.bodystructure()),
            labels: user_labels(
                fetch
                    .gmail_labels()
                    .into_iter()
                    .flatten()
                    .map(|label| label.as_ref()),
            ),
        })
        .collect();
    let _ = session.logout().await;
    let mut result = classify_summaries(
        FolderDescriptor {
            id: FolderId::Inbox,
            mailbox: "INBOX".into(),
            display_name: "Inbox".into(),
            kind: FolderKind::Inbox,
        },
        uid_validity,
        &requested,
        candidates,
    );
    result.skipped_count = result.skipped_count.saturating_add(discovery_skipped);
    result.folder_catalog = folder_catalog;
    Ok(result)
}

fn classify_summaries(
    folder: FolderDescriptor,
    uid_validity: u32,
    requested: &BTreeSet<u32>,
    candidates: Vec<SummaryCandidate>,
) -> InboxFetch {
    let mut records = Vec::new();
    let mut received_uids = BTreeSet::new();
    let mut received_ids = BTreeSet::new();
    let mut skipped_count = 0;
    for candidate in candidates {
        let Some(uid) = candidate
            .uid
            .filter(|uid| requested.contains(uid) && *uid != 0)
        else {
            skipped_count += 1;
            continue;
        };
        if !received_uids.insert(uid) {
            skipped_count += 1;
            continue;
        }
        let Some(x_gm_msgid) = candidate.x_gm_msgid.filter(|value| *value != 0) else {
            skipped_count += 1;
            continue;
        };
        if !received_ids.insert(x_gm_msgid) {
            skipped_count += 1;
            continue;
        }
        let Some(header) = candidate.header else {
            skipped_count += 1;
            continue;
        };
        records.push(RawMessageSummary {
            id: MessageId::gmail(x_gm_msgid),
            locator: MessageLocator {
                folder_id: folder.id.clone(),
                mailbox: folder.mailbox.clone(),
                uid_validity,
                uid,
            },
            flags: candidate.flags,
            internal_date_unix: candidate.internal_date_unix,
            rfc822_size: candidate.size,
            header,
            attachment_state: candidate.attachment_state,
            labels: candidate.labels,
        });
    }
    skipped_count += requested.len().saturating_sub(received_uids.len());
    records.sort_by_key(|message| std::cmp::Reverse(message.locator.uid));
    InboxFetch {
        records,
        skipped_count,
        folder_catalog: FolderCatalog::bounded(vec![folder]),
    }
}

pub async fn mutate_inbox(
    email: &str,
    access_token: &str,
    id: &MessageId,
    locator: &MessageLocator,
    mutation: &MessageMutation,
) -> MutationResult {
    if id.gmail_value().is_none() || !locator.is_valid() || locator.folder_id != FolderId::Inbox {
        return MutationResult::DefiniteFailure(GmailError::Protocol);
    }
    let attempted = timeout(
        Duration::from_secs(30),
        mutate_inbox_inner(email, access_token, id, locator, mutation),
    )
    .await;
    match attempted {
        Ok(Ok(())) => MutationResult::Confirmed,
        Ok(Err(MutationAttemptError::Definite(error))) => MutationResult::DefiniteFailure(error),
        Ok(Err(MutationAttemptError::Uncertain)) | Err(_) => {
            match timeout(
                Duration::from_secs(20),
                reconcile_inbox_inner(email, access_token, id),
            )
            .await
            {
                Ok(Ok(state)) => MutationResult::Reconciled(state),
                _ => MutationResult::Uncertain,
            }
        }
    }
}

async fn authenticated_session(
    email: &str,
    access_token: &str,
) -> Result<async_imap::Session<tokio_rustls::client::TlsStream<TcpStream>>, GmailError> {
    let client = tls_client().await?;
    client
        .authenticate("XOAUTH2", Xoauth2(xoauth2_response(email, access_token)?))
        .await
        .map_err(|_| GmailError::AuthenticationFailed)
}

async fn mutate_inbox_inner(
    email: &str,
    access_token: &str,
    id: &MessageId,
    locator: &MessageLocator,
    mutation: &MessageMutation,
) -> Result<(), MutationAttemptError> {
    let mut session = authenticated_session(email, access_token)
        .await
        .map_err(MutationAttemptError::Definite)?;
    let mailbox = session
        .select(&locator.mailbox)
        .await
        .map_err(|_| MutationAttemptError::Definite(GmailError::InboxUnavailable))?;
    if mailbox.uid_validity != Some(locator.uid_validity) {
        return Err(MutationAttemptError::Definite(GmailError::MailboxChanged));
    }
    let identities: Vec<_> = session
        .uid_fetch(locator.uid.to_string(), "(UID X-GM-MSGID)")
        .await
        .map_err(|_| MutationAttemptError::Definite(GmailError::Protocol))?
        .try_collect()
        .await
        .map_err(|_| MutationAttemptError::Definite(GmailError::Protocol))?;
    if identities.len() != 1
        || identities[0].uid != Some(locator.uid)
        || identities[0].gmail_msg_id().copied() != id.gmail_value()
    {
        return Err(MutationAttemptError::Definite(GmailError::MessageMissing));
    }
    let result = match mutation {
        MessageMutation::MoveToTrash { mailbox } => {
            if quote_label(mailbox).is_none() {
                return Err(MutationAttemptError::Definite(GmailError::Protocol));
            }
            session.uid_mv(locator.uid.to_string(), mailbox).await
        }
        _ => {
            let query = mutation_store_query(mutation)
                .ok_or(MutationAttemptError::Definite(GmailError::Protocol))?;
            match session.uid_store(locator.uid.to_string(), query).await {
                Ok(stream) => stream.try_collect::<Vec<_>>().await.map(|_| ()),
                Err(error) => Err(error),
            }
        }
    };
    match result {
        Ok(()) => {
            let _ = session.logout().await;
            Ok(())
        }
        Err(async_imap::error::Error::No(_) | async_imap::error::Error::Bad(_))
            if !matches!(mutation, MessageMutation::MoveToTrash { .. }) =>
        {
            Err(MutationAttemptError::Definite(GmailError::Protocol))
        }
        Err(async_imap::error::Error::Validate(_)) => {
            Err(MutationAttemptError::Definite(GmailError::Protocol))
        }
        Err(_) => Err(MutationAttemptError::Uncertain),
    }
}

async fn reconcile_inbox_inner(
    email: &str,
    access_token: &str,
    id: &MessageId,
) -> Result<Option<ReconciledMessageState>, GmailError> {
    let mut session = authenticated_session(email, access_token).await?;
    session
        .examine("INBOX")
        .await
        .map_err(|_| GmailError::InboxUnavailable)?;
    let ids = session
        .uid_search(format!(
            "X-GM-MSGID {}",
            id.gmail_value().ok_or(GmailError::Protocol)?
        ))
        .await
        .map_err(|_| GmailError::Protocol)?;
    let Some(uid) = ids.into_iter().next() else {
        let _ = session.logout().await;
        return Ok(None);
    };
    let values: Vec<_> = session
        .uid_fetch(uid.to_string(), "(UID X-GM-MSGID FLAGS X-GM-LABELS)")
        .await
        .map_err(|_| GmailError::Protocol)?
        .try_collect()
        .await
        .map_err(|_| GmailError::Protocol)?;
    let state = values
        .first()
        .filter(|fetch| fetch.gmail_msg_id().copied() == id.gmail_value())
        .map(|fetch| ReconciledMessageState {
            unread: !fetch
                .flags()
                .any(|flag| matches!(flag, async_imap::types::Flag::Seen)),
            starred: fetch
                .flags()
                .any(|flag| matches!(flag, async_imap::types::Flag::Flagged)),
            labels: user_labels(
                fetch
                    .gmail_labels()
                    .into_iter()
                    .flatten()
                    .map(|label| label.as_ref()),
            ),
        });
    let _ = session.logout().await;
    state.ok_or(GmailError::Protocol).map(Some)
}

pub async fn fetch_body(
    email: &str,
    access_token: &str,
    id: &MessageId,
    locator: &MessageLocator,
) -> Result<RawMessageBody, GmailError> {
    if id.gmail_value().is_none() || !locator.is_valid() {
        return Err(GmailError::Protocol);
    }
    timeout(
        Duration::from_secs(30),
        fetch_body_inner(email, access_token, id, locator),
    )
    .await
    .map_err(|_| GmailError::TimedOut)?
}

fn body_identity_matches(
    expected_id: &MessageId,
    locator: &MessageLocator,
    fetched_uid: Option<u32>,
    fetched_x_gm_msgid: Option<u64>,
) -> bool {
    fetched_uid == Some(locator.uid) && fetched_x_gm_msgid == expected_id.gmail_value()
}

async fn fetch_body_inner(
    email: &str,
    access_token: &str,
    id: &MessageId,
    locator: &MessageLocator,
) -> Result<RawMessageBody, GmailError> {
    let client = tls_client().await?;
    let auth = Xoauth2(xoauth2_response(email, access_token)?);
    let mut session = client
        .authenticate("XOAUTH2", auth)
        .await
        .map_err(|_| GmailError::AuthenticationFailed)?;
    let mailbox = session
        .examine(&locator.mailbox)
        .await
        .map_err(|_| GmailError::InboxUnavailable)?;
    if mailbox.uid_validity != Some(locator.uid_validity) {
        let _ = session.logout().await;
        return Err(GmailError::MailboxChanged);
    }
    let result = async {
        let fetches: Vec<_> = session
            .uid_fetch(locator.uid.to_string(), BODY_PLAN_QUERY)
            .await
            .map_err(|_| GmailError::Protocol)?
            .try_collect()
            .await
            .map_err(|_| GmailError::Protocol)?;
        let fetch = match fetches.as_slice() {
            [] => return Err(GmailError::MessageMissing),
            [fetch]
                if body_identity_matches(id, locator, fetch.uid, fetch.gmail_msg_id().copied()) =>
            {
                fetch
            }
            _ => return Err(GmailError::Protocol),
        };
        let header = fetch.header().ok_or(GmailError::Protocol)?;
        let header = header[..header.len().min(MAX_HEADER_BYTES)].to_vec();
        let plan = fetch
            .bodystructure()
            .map(body_plan)
            .ok_or(GmailError::Protocol)?;
        let BodyPlan {
            plain,
            html,
            attachments,
        } = plan;
        // HTML is the rendered representation when present; its plain-text
        // alternative would be redundant transfer and memory. Fetch plain
        // only when there is no HTML body.
        let plain = html.is_none().then_some(plain).flatten();
        let sections = [plain.as_ref(), html.as_ref()]
            .into_iter()
            .flatten()
            .map(|part| format!("BODY.PEEK[{}]", part.descriptor.section()))
            .collect::<Vec<_>>();
        if sections.is_empty() {
            return Ok(RawMessageBody {
                id: id.clone(),
                locator: locator.clone(),
                raw: Vec::new(),
                header,
                plain: None,
                html: None,
                attachments,
            });
        }
        let query = format!("(UID X-GM-MSGID {})", sections.join(" "));
        let body_fetches: Vec<_> = session
            .uid_fetch(locator.uid.to_string(), query)
            .await
            .map_err(|_| GmailError::Protocol)?
            .try_collect()
            .await
            .map_err(|_| GmailError::Protocol)?;
        let body_fetch = match body_fetches.as_slice() {
            [fetch]
                if body_identity_matches(id, locator, fetch.uid, fetch.gmail_msg_id().copied()) =>
            {
                fetch
            }
            [] => return Err(GmailError::MessageMissing),
            _ => return Err(GmailError::Protocol),
        };
        let extract = |part: Option<TextPlan>| -> Result<Option<RawTextPart>, GmailError> {
            part.map(|part| {
                let path = SectionPath::Part(part.descriptor.path.clone(), None);
                let raw = body_fetch.section(&path).ok_or(GmailError::Protocol)?;
                Ok(RawTextPart {
                    descriptor: part.descriptor,
                    media_type: part.media_type,
                    charset: part.charset,
                    raw: raw.to_vec(),
                })
            })
            .transpose()
        };
        Ok(RawMessageBody {
            id: id.clone(),
            locator: locator.clone(),
            raw: Vec::new(),
            header,
            plain: extract(plain)?,
            html: extract(html)?,
            attachments,
        })
    }
    .await;
    let _ = session.logout().await;
    result
}

struct AttachmentDecoder {
    encoding: TransferEncoding,
    pending: Vec<u8>,
    decoded: u64,
    base64_ended: bool,
}

impl AttachmentDecoder {
    fn new(encoding: TransferEncoding) -> Result<Self, AttachmentFetchError> {
        if encoding == TransferEncoding::Unsupported {
            return Err(AttachmentFetchError::InvalidEncoding);
        }
        Ok(Self {
            encoding,
            pending: Vec::with_capacity(4),
            decoded: 0,
            base64_ended: false,
        })
    }

    fn push(&mut self, input: &[u8], final_chunk: bool) -> Result<Vec<u8>, AttachmentFetchError> {
        let output = match self.encoding {
            TransferEncoding::SevenBit | TransferEncoding::EightBit | TransferEncoding::Binary => {
                input.to_vec()
            }
            TransferEncoding::Base64 => self.push_base64(input, final_chunk)?,
            TransferEncoding::QuotedPrintable => self.push_quoted_printable(input, final_chunk)?,
            TransferEncoding::Unsupported => {
                return Err(AttachmentFetchError::InvalidEncoding);
            }
        };
        self.decoded = self
            .decoded
            .checked_add(output.len() as u64)
            .ok_or(AttachmentFetchError::TooLarge)?;
        if self.decoded > MAX_ATTACHMENT_BYTES {
            return Err(AttachmentFetchError::TooLarge);
        }
        Ok(output)
    }

    fn push_base64(
        &mut self,
        input: &[u8],
        final_chunk: bool,
    ) -> Result<Vec<u8>, AttachmentFetchError> {
        let mut output = Vec::with_capacity(input.len().saturating_mul(3) / 4 + 3);
        for byte in input.iter().copied() {
            if byte.is_ascii_whitespace() {
                continue;
            }
            if self.base64_ended {
                return Err(AttachmentFetchError::InvalidEncoding);
            }
            self.pending.push(byte);
            if self.pending.len() == 4 {
                decode_base64_group(&self.pending, &mut output)?;
                self.base64_ended = self.pending[2] == b'=' || self.pending[3] == b'=';
                self.pending.clear();
            }
        }
        if final_chunk && !self.pending.is_empty() {
            if self.pending.len() == 1 {
                return Err(AttachmentFetchError::InvalidEncoding);
            }
            while self.pending.len() < 4 {
                self.pending.push(b'=');
            }
            decode_base64_group(&self.pending, &mut output)?;
            self.base64_ended = true;
            self.pending.clear();
        }
        Ok(output)
    }

    fn push_quoted_printable(
        &mut self,
        input: &[u8],
        final_chunk: bool,
    ) -> Result<Vec<u8>, AttachmentFetchError> {
        let mut output = Vec::with_capacity(input.len());
        for byte in input.iter().copied() {
            match self.pending.as_slice() {
                [] if byte == b'=' => self.pending.push(byte),
                [] => output.push(byte),
                [b'='] if byte == b'\n' => self.pending.clear(),
                [b'='] if byte == b'\r' || hex_value(byte).is_some() => self.pending.push(byte),
                [b'=', b'\r'] if byte == b'\n' => self.pending.clear(),
                [b'=', high] if hex_value(*high).is_some() && hex_value(byte).is_some() => {
                    output.push(
                        hex_value(*high).unwrap_or_default() * 16
                            + hex_value(byte).unwrap_or_default(),
                    );
                    self.pending.clear();
                }
                _ => return Err(AttachmentFetchError::InvalidEncoding),
            }
        }
        if final_chunk && !self.pending.is_empty() {
            return Err(AttachmentFetchError::InvalidEncoding);
        }
        Ok(output)
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_base64_group(group: &[u8], output: &mut Vec<u8>) -> Result<(), AttachmentFetchError> {
    if group.len() != 4 || group[0] == b'=' || group[1] == b'=' {
        return Err(AttachmentFetchError::InvalidEncoding);
    }
    let value = |byte| match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        b'=' => Some(0),
        _ => None,
    };
    if group[2] == b'=' && group[3] != b'=' {
        return Err(AttachmentFetchError::InvalidEncoding);
    }
    let a = value(group[0]).ok_or(AttachmentFetchError::InvalidEncoding)?;
    let b = value(group[1]).ok_or(AttachmentFetchError::InvalidEncoding)?;
    let c = value(group[2]).ok_or(AttachmentFetchError::InvalidEncoding)?;
    let d = value(group[3]).ok_or(AttachmentFetchError::InvalidEncoding)?;
    output.push((a << 2) | (b >> 4));
    if group[2] != b'=' {
        output.push((b << 4) | (c >> 2));
    }
    if group[3] != b'=' {
        output.push((c << 6) | d);
    }
    Ok(())
}

fn attachment_chunk_query(section: &str, offset: u64, count: u64) -> String {
    format!("(UID X-GM-MSGID BODY.PEEK[{section}]<{offset}.{count}>)")
}

pub async fn fetch_attachment(
    email: &str,
    access_token: &str,
    id: &MessageId,
    locator: &MessageLocator,
    attachment: &Attachment,
    mut write: impl FnMut(&[u8]) -> std::io::Result<()>,
    mut progress: impl FnMut(u64, u64),
) -> Result<u64, AttachmentFetchError> {
    if id.gmail_value().is_none() || !locator.is_valid() || !attachment.is_downloadable() {
        return Err(AttachmentFetchError::Gmail(GmailError::Protocol));
    }
    if attachment.part.encoded_octets > MAX_ATTACHMENT_ENCODED_BYTES {
        return Err(AttachmentFetchError::TooLarge);
    }
    timeout(
        Duration::from_secs(300),
        fetch_attachment_inner(
            email,
            access_token,
            id,
            locator,
            attachment,
            &mut write,
            &mut progress,
        ),
    )
    .await
    .map_err(|_| AttachmentFetchError::Gmail(GmailError::TimedOut))?
}

async fn fetch_attachment_inner(
    email: &str,
    access_token: &str,
    id: &MessageId,
    locator: &MessageLocator,
    attachment: &Attachment,
    write: &mut impl FnMut(&[u8]) -> std::io::Result<()>,
    progress: &mut impl FnMut(u64, u64),
) -> Result<u64, AttachmentFetchError> {
    let mut session = authenticated_session(email, access_token)
        .await
        .map_err(AttachmentFetchError::Gmail)?;
    let mailbox = session
        .examine(&locator.mailbox)
        .await
        .map_err(|_| AttachmentFetchError::Gmail(GmailError::InboxUnavailable))?;
    if mailbox.uid_validity != Some(locator.uid_validity) {
        let _ = session.logout().await;
        return Err(AttachmentFetchError::Gmail(GmailError::MailboxChanged));
    }
    let total = attachment.part.encoded_octets;
    let path = SectionPath::Part(attachment.part.path.clone(), None);
    let section = attachment.part.section();
    let mut decoder = AttachmentDecoder::new(attachment.part.encoding)?;
    let mut offset = 0_u64;
    if total == 0 {
        let identities: Vec<_> = session
            .uid_fetch(locator.uid.to_string(), "(UID X-GM-MSGID)")
            .await
            .map_err(|_| AttachmentFetchError::Gmail(GmailError::Protocol))?
            .try_collect()
            .await
            .map_err(|_| AttachmentFetchError::Gmail(GmailError::Protocol))?;
        if !matches!(identities.as_slice(), [fetch] if body_identity_matches(id, locator, fetch.uid, fetch.gmail_msg_id().copied()))
        {
            let _ = session.logout().await;
            return Err(AttachmentFetchError::Gmail(GmailError::MessageMissing));
        }
    }
    while offset < total {
        let count = (total - offset).min(ATTACHMENT_CHUNK_BYTES as u64);
        let query = attachment_chunk_query(&section, offset, count);
        let fetches: Vec<_> = session
            .uid_fetch(locator.uid.to_string(), query)
            .await
            .map_err(|_| AttachmentFetchError::Gmail(GmailError::Protocol))?
            .try_collect()
            .await
            .map_err(|_| AttachmentFetchError::Gmail(GmailError::Protocol))?;
        let fetch = match fetches.as_slice() {
            [fetch]
                if body_identity_matches(id, locator, fetch.uid, fetch.gmail_msg_id().copied()) =>
            {
                fetch
            }
            [] => {
                let _ = session.logout().await;
                return Err(AttachmentFetchError::Gmail(GmailError::MessageMissing));
            }
            _ => {
                let _ = session.logout().await;
                return Err(AttachmentFetchError::Gmail(GmailError::Protocol));
            }
        };
        let encoded = fetch
            .section(&path)
            .ok_or(AttachmentFetchError::Gmail(GmailError::Protocol))?;
        if encoded.is_empty() || encoded.len() as u64 > count {
            let _ = session.logout().await;
            return Err(AttachmentFetchError::Gmail(GmailError::Protocol));
        }
        offset = offset.saturating_add(encoded.len() as u64);
        let final_chunk = offset >= total;
        let decoded = decoder.push(encoded, final_chunk)?;
        write(&decoded).map_err(|_| AttachmentFetchError::WriteFailed)?;
        progress(offset.min(total), total);
    }
    let _ = session.logout().await;
    Ok(decoder.decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xoauth2_framing_is_exact() {
        assert_eq!(
            xoauth2_response("me@example.com", "token").unwrap(),
            b"user=me@example.com\x01auth=Bearer token\x01\x01"
        );
        assert!(xoauth2_response("bad\nemail", "token").is_err());
    }
    #[test]
    fn retention_drives_uid_bounds() {
        for limit in RETENTION_OPTIONS {
            let result = select_newest_uids(1..=600, limit);
            assert_eq!(result.len(), limit);
            assert_eq!(result.first(), Some(&(601 - limit as u32)));
            assert_eq!(result.last(), Some(&600));
            assert_eq!(
                newest_sequence_set(600, limit),
                Some(format!("{}:600", 601 - limit))
            );
        }
        assert!(select_newest_uids(1..=50, 42).is_empty());
        assert_eq!(newest_sequence_set(50, 42), None);
    }
    #[test]
    fn uid_discovery_counts_races() {
        let (uids, skipped) = discovered_uids(4, 50, [Some(9), Some(8), Some(8), None]);
        assert_eq!(uids, vec![8, 9]);
        assert_eq!(skipped, 2);
        assert_eq!(uid_sequence_set(&[8, 9]).as_deref(), Some("8,9"));
        assert_eq!(uid_sequence_set(&[0]), None);
    }
    #[test]
    fn gmail_search_queries_are_bounded_and_safely_quoted() {
        assert_eq!(validate_search_query("  from:alice  "), Some("from:alice"));
        assert_eq!(
            quote_search_query("from:\"Alice\" \\ team").as_deref(),
            Some("\"from:\\\"Alice\\\" \\\\ team\"")
        );
        assert!(validate_search_query("").is_none());
        assert!(validate_search_query("line\nbreak").is_none());
        assert!(validate_search_query(&"x".repeat(MAX_SEARCH_QUERY_BYTES)).is_some());
        assert!(validate_search_query(&"x".repeat(MAX_SEARCH_QUERY_BYTES + 1)).is_none());
    }
    #[test]
    fn gmail_search_keeps_newest_five_hundred_in_bounded_batches() {
        let (uids, truncated) = newest_search_uids((1..=620).chain([620, 0]));
        assert!(truncated);
        assert_eq!(uids.len(), MAX_SEARCH_RESULTS);
        assert_eq!(uids.first(), Some(&121));
        assert_eq!(uids.last(), Some(&620));
        let batches = search_uid_batches(&uids)
            .map(<[u32]>::len)
            .collect::<Vec<_>>();
        assert_eq!(batches, vec![100, 100, 100, 100, 100]);

        let (short, truncated) = newest_search_uids([9, 4, 9, 0]);
        assert_eq!(short, vec![4, 9]);
        assert!(!truncated);
    }
    #[test]
    fn special_use_discovery_is_bounded_stable_and_skips_non_mail_views() {
        let entry = |mailbox: &str, kind, selectable, ignored_special_use| ListedMailbox {
            mailbox: mailbox.into(),
            display_name: mailbox.into(),
            kind,
            selectable,
            ignored_special_use,
        };
        let catalog = folder_catalog([
            entry("[Gmail]/Sent Mail", Some(FolderKind::Sent), true, false),
            entry("Projects/Rust", None, true, false),
            entry("[Gmail]/Drafts", None, true, true),
            entry("Container", None, false, false),
            entry("INBOX", Some(FolderKind::Inbox), true, false),
        ]);
        assert_eq!(
            catalog
                .folders
                .iter()
                .map(|folder| folder.id.clone())
                .collect::<Vec<_>>(),
            vec![
                FolderId::Inbox,
                FolderId::Sent,
                FolderId::Label("Projects/Rust".into())
            ]
        );
        assert_eq!(
            catalog
                .find(&FolderId::Sent)
                .map(|folder| folder.mailbox.as_str()),
            Some("[Gmail]/Sent Mail")
        );
    }
    #[test]
    fn summary_and_body_queries_are_separated_and_read_only() {
        assert_eq!(
            SUMMARY_FETCH_QUERY,
            "(UID X-GM-MSGID X-GM-LABELS FLAGS INTERNALDATE RFC822.SIZE BODYSTRUCTURE BODY.PEEK[HEADER.FIELDS (FROM SUBJECT DATE MESSAGE-ID)])"
        );
        assert!(!SUMMARY_FETCH_QUERY.contains("BODY.PEEK[]"));
        assert!(!SUMMARY_FETCH_QUERY.contains("BODY[TEXT]"));
        assert!(BODY_PLAN_QUERY.contains("BODYSTRUCTURE"));
        assert!(BODY_PLAN_QUERY.contains("BODY.PEEK[HEADER.FIELDS"));
        assert!(!BODY_PLAN_QUERY.contains("BODY.PEEK[]"));
        for query in [SUMMARY_FETCH_QUERY, BODY_PLAN_QUERY] {
            let upper = query.to_ascii_uppercase();
            for forbidden in [
                " STORE ", "SELECT ", "EXPUNGE", "APPEND", " COPY ", " MOVE ",
            ] {
                assert!(!upper.contains(forbidden), "{query}");
            }
        }
    }
    #[test]
    fn gmail_mutation_commands_are_exact_and_labels_are_quoted() {
        assert_eq!(
            mutation_store_query(&MessageMutation::SetRead(true)).as_deref(),
            Some("+FLAGS.SILENT (\\Seen)")
        );
        assert_eq!(
            mutation_store_query(&MessageMutation::SetStarred(false)).as_deref(),
            Some("-FLAGS.SILENT (\\Flagged)")
        );
        assert_eq!(
            mutation_store_query(&MessageMutation::Archive).as_deref(),
            Some("-X-GM-LABELS.SILENT (\\Inbox)")
        );
        assert_eq!(
            mutation_store_query(&MessageMutation::SetLabel {
                mailbox: "Project \\\"A".into(),
                applied: true
            })
            .as_deref(),
            Some("+X-GM-LABELS.SILENT (\"Project \\\\\\\"A\")")
        );
        assert!(
            mutation_store_query(&MessageMutation::SetLabel {
                mailbox: "bad\nlabel".into(),
                applied: true
            })
            .is_none()
        );
        assert!(
            mutation_store_query(&MessageMutation::MoveToTrash {
                mailbox: "Trash".into()
            })
            .is_none()
        );
    }

    #[test]
    fn only_user_labels_reach_the_local_model() {
        assert_eq!(
            user_labels(["\\Inbox", "Work", "Work", "Family"]),
            vec!["Family", "Work"]
        );
    }
    #[test]
    fn body_identity_requires_both_the_locator_uid_and_canonical_id() {
        let id = MessageId::gmail(42);
        let locator = MessageLocator {
            folder_id: FolderId::Inbox,
            mailbox: "INBOX".into(),
            uid_validity: 7,
            uid: 9,
        };
        assert!(body_identity_matches(&id, &locator, Some(9), Some(42)));
        assert!(!body_identity_matches(&id, &locator, Some(8), Some(42)));
        assert!(!body_identity_matches(&id, &locator, Some(9), Some(41)));
        assert!(!body_identity_matches(&id, &locator, Some(9), None));
    }
    #[test]
    fn summary_classification_rejects_bad_rows() {
        let requested = BTreeSet::from([1, 2, 3]);
        let candidate = |uid, header| SummaryCandidate {
            uid,
            x_gm_msgid: uid.map(u64::from),
            flags: MessageFlags::default(),
            internal_date_unix: None,
            size: None,
            header,
            attachment_state: AttachmentState::Unknown,
            labels: Vec::new(),
        };
        let mut missing_canonical = candidate(Some(3), Some(b"valid header".to_vec()));
        missing_canonical.x_gm_msgid = None;
        let result = classify_summaries(
            FolderDescriptor {
                id: FolderId::Inbox,
                mailbox: "INBOX".into(),
                display_name: "Inbox".into(),
                kind: FolderKind::Inbox,
            },
            7,
            &requested,
            vec![
                candidate(Some(1), Some(b"From: A\r\n\r\n".to_vec())),
                candidate(Some(1), Some(b"duplicate".to_vec())),
                candidate(Some(9), Some(b"unrequested".to_vec())),
                candidate(Some(2), None),
                candidate(None, Some(b"missing uid".to_vec())),
                missing_canonical,
            ],
        );
        assert_eq!(result.records.len(), 1);
        assert_eq!(result.skipped_count, 5);
        assert_eq!(result.records[0].id, MessageId::gmail(1));
        assert_eq!(result.records[0].locator.uid_validity, 7);
        assert_eq!(result.records[0].locator.uid, 1);
    }

    #[test]
    fn attachment_decoder_handles_chunk_boundaries_without_whole_payload_buffers() {
        let mut base64 = AttachmentDecoder::new(TransferEncoding::Base64).unwrap();
        let mut decoded = Vec::new();
        for (chunk, final_chunk) in [(&b"SGV"[..], false), (&b"sbG8g\r\nV29ybGQ="[..], true)] {
            decoded.extend(base64.push(chunk, final_chunk).unwrap());
        }
        assert_eq!(decoded, b"Hello World");

        let mut quoted = AttachmentDecoder::new(TransferEncoding::QuotedPrintable).unwrap();
        let mut decoded = quoted.push(b"hello=2", false).unwrap();
        decoded.extend(quoted.push(b"0world=\r", false).unwrap());
        decoded.extend(quoted.push(b"\nnext", true).unwrap());
        assert_eq!(decoded, b"hello worldnext");
    }

    #[test]
    fn attachment_decoder_rejects_invalid_or_oversized_output() {
        let mut invalid = AttachmentDecoder::new(TransferEncoding::Base64).unwrap();
        assert_eq!(
            invalid.push(b"%%%", true),
            Err(AttachmentFetchError::InvalidEncoding)
        );
        let mut oversized = AttachmentDecoder::new(TransferEncoding::Binary).unwrap();
        oversized.decoded = MAX_ATTACHMENT_BYTES;
        assert_eq!(
            oversized.push(b"x", true),
            Err(AttachmentFetchError::TooLarge)
        );
    }

    #[test]
    fn attachment_queries_are_exact_bounded_read_only_sections() {
        let descriptor = MimePartDescriptor {
            path: vec![2, 1],
            encoding: TransferEncoding::Base64,
            encoded_octets: 2_000_000,
        };
        assert_eq!(descriptor.section(), "2.1");
        let query = attachment_chunk_query(
            &descriptor.section(),
            ATTACHMENT_CHUNK_BYTES as u64,
            ATTACHMENT_CHUNK_BYTES as u64,
        );
        assert_eq!(query, "(UID X-GM-MSGID BODY.PEEK[2.1]<1048576.1048576>)");
        assert!(!query.contains("BODY[]"));
        assert!(!query.contains(" STORE "));
    }

    #[test]
    fn bodystructure_plan_persists_exact_text_and_attachment_descriptors() {
        use async_imap::imap_proto::types::{ContentDisposition, ContentType};
        use std::borrow::Cow;

        let common = |kind: &'static str, subtype: &'static str| BodyContentCommon {
            ty: ContentType {
                ty: Cow::Borrowed(kind),
                subtype: Cow::Borrowed(subtype),
                params: None,
            },
            disposition: None,
            language: None,
            location: None,
        };
        let single = |encoding, octets| BodyContentSinglePart {
            id: None,
            md5: None,
            description: None,
            transfer_encoding: encoding,
            octets,
        };
        let plain = BodyStructure::Text {
            common: common("text", "plain"),
            other: single(ContentEncoding::QuotedPrintable, 40),
            lines: 2,
            extension: None,
        };
        let mut attachment_common = common("application", "pdf");
        attachment_common.disposition = Some(ContentDisposition {
            ty: Cow::Borrowed("attachment"),
            params: Some(vec![(
                Cow::Borrowed("filename"),
                Cow::Borrowed("report.pdf"),
            )]),
        });
        let attachment = BodyStructure::Basic {
            common: attachment_common,
            other: single(ContentEncoding::Base64, 1_024),
            extension: None,
        };
        let structure = BodyStructure::Multipart {
            common: common("multipart", "mixed"),
            bodies: vec![plain, attachment],
            extension: None,
        };
        let plan = body_plan(&structure);
        assert_eq!(plan.plain.unwrap().descriptor.path, vec![1]);
        assert_eq!(plan.attachments.len(), 1);
        assert_eq!(plan.attachments[0].name, "report.pdf");
        assert_eq!(plan.attachments[0].part.path, vec![2]);
        assert_eq!(plan.attachments[0].part.encoding, TransferEncoding::Base64);
        assert_eq!(plan.attachments[0].part.encoded_octets, 1_024);
    }
}
