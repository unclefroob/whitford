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
pub const MESSAGE_LIMIT: usize = 50;
pub const FETCH_QUERY: &str = "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[])";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MessageFlags {
    pub seen: bool,
    pub flagged: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawFetchedMessage {
    pub uid_validity: u32,
    pub uid: u32,
    pub flags: MessageFlags,
    pub internal_date_unix: Option<i64>,
    pub rfc822_size: Option<u32>,
    pub raw: Vec<u8>,
}

#[derive(Debug)]
pub struct InboxFetch {
    pub records: Vec<RawFetchedMessage>,
    pub skipped_count: usize,
}
struct FetchCandidate {
    uid: Option<u32>,
    flags: MessageFlags,
    internal_date_unix: Option<i64>,
    size: Option<u32>,
    body: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GmailError {
    Offline,
    TlsFailed,
    AuthenticationFailed,
    InboxUnavailable,
    Protocol,
    TimedOut,
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

pub fn select_newest_uids<I: IntoIterator<Item = u32>>(uids: I) -> Vec<u32> {
    let sorted = uids.into_iter().collect::<BTreeSet<_>>();
    let skip = sorted.len().saturating_sub(MESSAGE_LIMIT);
    sorted.into_iter().skip(skip).collect()
}

pub fn uid_sequence_set(uids: &[u32]) -> Option<String> {
    (!uids.is_empty()).then(|| {
        uids.iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    })
}

pub fn newest_sequence_set(message_count: u32) -> Option<String> {
    (message_count > 0).then(|| {
        let first = message_count
            .saturating_sub(MESSAGE_LIMIT as u32 - 1)
            .max(1);
        format!("{first}:{message_count}")
    })
}

fn discovered_uids<I: IntoIterator<Item = Option<u32>>>(
    expected: usize,
    values: I,
) -> (Vec<u32>, usize) {
    let unique = values
        .into_iter()
        .flatten()
        .filter(|uid| *uid > 0)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let first = unique.len().saturating_sub(expected.min(MESSAGE_LIMIT));
    let uids = unique[first..].to_vec();
    let skipped = expected
        .saturating_sub(uids.len())
        .saturating_add(unique.len().saturating_sub(expected));
    (uids, skipped)
}

#[cfg(test)]
fn read_only_command_plan(sequence: &str, uids: &str) -> Vec<String> {
    vec![
        "AUTHENTICATE XOAUTH2".into(),
        "EXAMINE INBOX".into(),
        format!("FETCH {sequence} (UID)"),
        format!("UID FETCH {uids} {FETCH_QUERY}"),
        "LOGOUT".into(),
    ]
}

pub fn build_raw_message(
    uid_validity: u32,
    uid: u32,
    flags: MessageFlags,
    rfc822_size: Option<u32>,
    raw: &[u8],
) -> Result<RawFetchedMessage, GmailError> {
    if uid == 0 {
        return Err(GmailError::Protocol);
    }
    Ok(RawFetchedMessage {
        uid_validity,
        uid,
        flags,
        internal_date_unix: None,
        rfc822_size,
        raw: raw.to_vec(),
    })
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

pub async fn fetch_inbox(email: &str, access_token: &str) -> Result<InboxFetch, GmailError> {
    timeout(
        Duration::from_secs(30),
        fetch_inbox_inner(email, access_token),
    )
    .await
    .map_err(|_| GmailError::TimedOut)?
}

async fn fetch_inbox_inner(email: &str, access_token: &str) -> Result<InboxFetch, GmailError> {
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
    let Some(sequence_numbers) = newest_sequence_set(mailbox.exists) else {
        let _ = session.logout().await;
        return Ok(InboxFetch {
            records: Vec::new(),
            skipped_count: 0,
        });
    };
    let expected = usize::try_from(mailbox.exists.min(MESSAGE_LIMIT as u32))
        .map_err(|_| GmailError::Protocol)?;
    let uid_fetches: Vec<_> = session
        .fetch(&sequence_numbers, "(UID)")
        .await
        .map_err(|_| GmailError::Protocol)?
        .try_collect()
        .await
        .map_err(|_| GmailError::Protocol)?;
    let (uids, discovery_skipped) =
        discovered_uids(expected, uid_fetches.into_iter().map(|fetch| fetch.uid));
    let Some(sequence) = uid_sequence_set(&uids) else {
        let _ = session.logout().await;
        return Ok(InboxFetch {
            records: Vec::new(),
            skipped_count: 0,
        });
    };
    let fetches: Vec<_> = session
        .uid_fetch(sequence, FETCH_QUERY)
        .await
        .map_err(|_| GmailError::Protocol)?
        .try_collect()
        .await
        .map_err(|_| GmailError::Protocol)?;
    let requested: BTreeSet<_> = uids.into_iter().collect();
    let candidates = fetches
        .into_iter()
        .map(|fetch| FetchCandidate {
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
            body: fetch.body().map(ToOwned::to_owned),
        })
        .collect();
    let _ = session.logout().await;
    let mut result = classify_fetches(uid_validity, &requested, candidates);
    result.skipped_count = result.skipped_count.saturating_add(discovery_skipped);
    Ok(result)
}

fn classify_fetches(
    uid_validity: u32,
    requested: &BTreeSet<u32>,
    candidates: Vec<FetchCandidate>,
) -> InboxFetch {
    let mut records = Vec::new();
    let mut received = BTreeSet::new();
    let mut skipped_count = 0;
    for candidate in candidates {
        let Some(uid) = candidate.uid.filter(|uid| requested.contains(uid)) else {
            skipped_count += 1;
            continue;
        };
        if !received.insert(uid) {
            skipped_count += 1;
            continue;
        }
        let Some(body) = candidate.body else {
            skipped_count += 1;
            continue;
        };
        let Ok(mut mapped) =
            build_raw_message(uid_validity, uid, candidate.flags, candidate.size, &body)
        else {
            skipped_count += 1;
            continue;
        };
        mapped.internal_date_unix = candidate.internal_date_unix;
        records.push(mapped);
    }
    skipped_count += requested.len().saturating_sub(received.len());
    records.sort_by_key(|message| std::cmp::Reverse(message.uid));
    InboxFetch {
        records,
        skipped_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xoauth2_has_control_a_framing_and_is_not_base64_encoded() {
        assert_eq!(
            xoauth2_response("me@example.com", "token").unwrap(),
            b"user=me@example.com\x01auth=Bearer token\x01\x01"
        );
        assert!(xoauth2_response("bad\nemail", "token").is_err());
    }
    #[test]
    fn newest_uids_are_sorted_deduplicated_and_bounded() {
        let result = select_newest_uids((1..=60).rev().chain([60, 59]));
        assert_eq!(result.len(), 50);
        assert_eq!(result.first(), Some(&11));
        assert_eq!(result.last(), Some(&60));
        assert_eq!(uid_sequence_set(&result).unwrap().split(',').count(), 50);
    }
    #[test]
    fn newest_sequence_fetch_is_bounded_and_uid_discovery_counts_races() {
        assert_eq!(newest_sequence_set(0), None);
        assert_eq!(newest_sequence_set(1).as_deref(), Some("1:1"));
        assert_eq!(newest_sequence_set(50).as_deref(), Some("1:50"));
        assert_eq!(newest_sequence_set(51).as_deref(), Some("2:51"));
        let (uids, skipped) = discovered_uids(4, [Some(9), Some(8), Some(8), None]);
        assert_eq!(uids, vec![8, 9]);
        assert_eq!(skipped, 2);
        let (uids, skipped) = discovered_uids(2, [Some(1), Some(2), Some(3)]);
        assert_eq!(uids, vec![2, 3]);
        assert_eq!(skipped, 1);
    }
    #[test]
    fn no_uids_skips_fetch_and_zero_uid_is_rejected() {
        assert_eq!(uid_sequence_set(&[]), None);
        assert!(build_raw_message(1, 0, MessageFlags::default(), None, b"x").is_err());
    }
    #[test]
    fn uid_boundaries_and_read_only_command_plan_are_exact() {
        for count in [0, 1, 49, 50, 51] {
            assert_eq!(select_newest_uids(1..=count).len(), count.min(50) as usize);
        }
        assert!(build_raw_message(1, 1, MessageFlags::default(), None, b"complete").is_ok());
        let plan = read_only_command_plan("51:100", "101,102");
        let transcript = plan.join("\n");
        for required in [
            "AUTHENTICATE XOAUTH2",
            "EXAMINE INBOX",
            "FETCH 51:100 (UID)",
            "UID FETCH 101,102",
            "BODY.PEEK",
            "LOGOUT",
        ] {
            assert!(transcript.contains(required));
        }
        for forbidden in [
            "SELECT ",
            "UID SEARCH",
            " STORE ",
            " COPY ",
            " MOVE ",
            "EXPUNGE",
            "APPEND",
        ] {
            assert!(!transcript.contains(forbidden));
        }
        assert!(FETCH_QUERY.contains("BODY.PEEK[]"));
        assert!(!FETCH_QUERY.contains("<0."));
    }
    #[test]
    fn fetch_classification_counts_unrequested_duplicate_and_bodyless() {
        let requested = BTreeSet::from([1, 2]);
        let candidate = |uid, body: Option<Vec<u8>>| FetchCandidate {
            uid,
            flags: MessageFlags::default(),
            internal_date_unix: None,
            size: None,
            body,
        };
        let result = classify_fetches(
            7,
            &requested,
            vec![
                candidate(Some(1), Some(b"ok".to_vec())),
                candidate(Some(1), Some(b"duplicate".to_vec())),
                candidate(Some(9), Some(b"unrequested".to_vec())),
                candidate(Some(2), None),
                candidate(None, Some(b"missing uid".to_vec())),
            ],
        );
        assert_eq!(result.records.len(), 1);
        assert_eq!(result.skipped_count, 4);
        let raced = classify_fetches(
            7,
            &BTreeSet::from([1, 2]),
            vec![candidate(Some(1), Some(b"ok".to_vec()))],
        );
        assert_eq!(raced.skipped_count, 1);
    }
}
