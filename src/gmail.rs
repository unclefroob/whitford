use crate::model::{Attachment, AttachmentState};
use async_imap::imap_proto::types::{BodyContentCommon, BodyContentSinglePart, BodyStructure};
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
pub const SUMMARY_FETCH_QUERY: &str = "(UID FLAGS INTERNALDATE RFC822.SIZE BODYSTRUCTURE BODY.PEEK[HEADER.FIELDS (FROM SUBJECT DATE MESSAGE-ID)])";
/// Full-message fallback used only for one deliberately opened UID. Summary sync never uses it.
pub const FULL_MESSAGE_BODY_FALLBACK_QUERY: &str = "(UID BODY.PEEK[])";
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_ATTACHMENTS: usize = 20;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MessageFlags {
    pub seen: bool,
    pub flagged: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawMessageSummary {
    pub uid_validity: u32,
    pub uid: u32,
    pub flags: MessageFlags,
    pub internal_date_unix: Option<i64>,
    pub rfc822_size: Option<u32>,
    pub header: Vec<u8>,
    pub attachment_state: AttachmentState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawMessageBody {
    pub uid_validity: u32,
    pub uid: u32,
    pub raw: Vec<u8>,
}

#[derive(Debug)]
pub struct InboxFetch {
    pub records: Vec<RawMessageSummary>,
    pub skipped_count: usize,
}

struct SummaryCandidate {
    uid: Option<u32>,
    flags: MessageFlags,
    internal_date_unix: Option<i64>,
    size: Option<u32>,
    header: Option<Vec<u8>>,
    attachment_state: AttachmentState,
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
    })
}

fn collect_attachments(structure: &BodyStructure<'_>, output: &mut Vec<Attachment>) {
    if output.len() >= MAX_ATTACHMENTS {
        return;
    }
    match structure {
        BodyStructure::Basic { common, other, .. } | BodyStructure::Text { common, other, .. } => {
            if let Some(value) = attachment_from_leaf(common, other) {
                output.push(value);
            }
        }
        BodyStructure::Message {
            common,
            other,
            body,
            ..
        } => {
            if let Some(value) = attachment_from_leaf(common, other) {
                output.push(value);
            } else {
                collect_attachments(body, output);
            }
        }
        BodyStructure::Multipart { bodies, .. } => {
            for body in bodies {
                collect_attachments(body, output);
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
    collect_attachments(structure, &mut attachments);
    AttachmentState::Known(attachments)
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
        })
        .collect();
    let _ = session.logout().await;
    let mut result = classify_summaries(uid_validity, &requested, candidates);
    result.skipped_count = result.skipped_count.saturating_add(discovery_skipped);
    Ok(result)
}

fn classify_summaries(
    uid_validity: u32,
    requested: &BTreeSet<u32>,
    candidates: Vec<SummaryCandidate>,
) -> InboxFetch {
    let mut records = Vec::new();
    let mut received = BTreeSet::new();
    let mut skipped_count = 0;
    for candidate in candidates {
        let Some(uid) = candidate
            .uid
            .filter(|uid| requested.contains(uid) && *uid != 0)
        else {
            skipped_count += 1;
            continue;
        };
        if !received.insert(uid) {
            skipped_count += 1;
            continue;
        }
        let Some(header) = candidate.header else {
            skipped_count += 1;
            continue;
        };
        records.push(RawMessageSummary {
            uid_validity,
            uid,
            flags: candidate.flags,
            internal_date_unix: candidate.internal_date_unix,
            rfc822_size: candidate.size,
            header,
            attachment_state: candidate.attachment_state,
        });
    }
    skipped_count += requested.len().saturating_sub(received.len());
    records.sort_by_key(|message| std::cmp::Reverse(message.uid));
    InboxFetch {
        records,
        skipped_count,
    }
}

pub async fn fetch_body(
    email: &str,
    access_token: &str,
    uid_validity: u32,
    uid: u32,
) -> Result<RawMessageBody, GmailError> {
    if uid_validity == 0 || uid == 0 {
        return Err(GmailError::Protocol);
    }
    timeout(
        Duration::from_secs(30),
        fetch_body_inner(email, access_token, uid_validity, uid),
    )
    .await
    .map_err(|_| GmailError::TimedOut)?
}

async fn fetch_body_inner(
    email: &str,
    access_token: &str,
    uid_validity: u32,
    uid: u32,
) -> Result<RawMessageBody, GmailError> {
    let client = tls_client().await?;
    let auth = Xoauth2(xoauth2_response(email, access_token)?);
    let mut session = client
        .authenticate("XOAUTH2", auth)
        .await
        .map_err(|_| GmailError::AuthenticationFailed)?;
    let mailbox = session
        .examine("INBOX")
        .await
        .map_err(|_| GmailError::InboxUnavailable)?;
    if mailbox.uid_validity != Some(uid_validity) {
        let _ = session.logout().await;
        return Err(GmailError::MailboxChanged);
    }
    let result = async {
        let fetches: Vec<_> = session
            .uid_fetch(uid.to_string(), FULL_MESSAGE_BODY_FALLBACK_QUERY)
            .await
            .map_err(|_| GmailError::Protocol)?
            .try_collect()
            .await
            .map_err(|_| GmailError::Protocol)?;
        match fetches.as_slice() {
            [] => Err(GmailError::MessageMissing),
            [fetch] if fetch.uid == Some(uid) => fetch
                .body()
                .map(|raw| RawMessageBody {
                    uid_validity,
                    uid,
                    raw: raw.to_vec(),
                })
                .ok_or(GmailError::Protocol),
            _ => Err(GmailError::Protocol),
        }
    }
    .await;
    let _ = session.logout().await;
    result
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
    fn summary_and_body_queries_are_separated_and_read_only() {
        assert_eq!(
            SUMMARY_FETCH_QUERY,
            "(UID FLAGS INTERNALDATE RFC822.SIZE BODYSTRUCTURE BODY.PEEK[HEADER.FIELDS (FROM SUBJECT DATE MESSAGE-ID)])"
        );
        assert!(!SUMMARY_FETCH_QUERY.contains("BODY.PEEK[]"));
        assert!(!SUMMARY_FETCH_QUERY.contains("BODY[TEXT]"));
        assert_eq!(FULL_MESSAGE_BODY_FALLBACK_QUERY, "(UID BODY.PEEK[])");
        assert!(!FULL_MESSAGE_BODY_FALLBACK_QUERY.contains("BODY[]"));
        for query in [SUMMARY_FETCH_QUERY, FULL_MESSAGE_BODY_FALLBACK_QUERY] {
            let upper = query.to_ascii_uppercase();
            for forbidden in [
                " STORE ", "SELECT ", "EXPUNGE", "APPEND", " COPY ", " MOVE ",
            ] {
                assert!(!upper.contains(forbidden), "{query}");
            }
        }
    }
    #[test]
    fn summary_classification_rejects_bad_rows() {
        let requested = BTreeSet::from([1, 2]);
        let candidate = |uid, header| SummaryCandidate {
            uid,
            flags: MessageFlags::default(),
            internal_date_unix: None,
            size: None,
            header,
            attachment_state: AttachmentState::Unknown,
        };
        let result = classify_summaries(
            7,
            &requested,
            vec![
                candidate(Some(1), Some(b"From: A\r\n\r\n".to_vec())),
                candidate(Some(1), Some(b"duplicate".to_vec())),
                candidate(Some(9), Some(b"unrequested".to_vec())),
                candidate(Some(2), None),
                candidate(None, Some(b"missing uid".to_vec())),
            ],
        );
        assert_eq!(result.records.len(), 1);
        assert_eq!(result.skipped_count, 4);
    }
}
