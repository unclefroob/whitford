use crate::config::OAuthClientConfig;
use futures_util::StreamExt;
use oauth2::{
    AuthUrl, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl,
    Scope, TokenUrl, basic::BasicClient,
};
use serde::Deserialize;
use std::{
    fmt,
    net::SocketAddr,
    time::{Duration, SystemTime},
};
use url::Url;
use zeroize::Zeroizing;

pub const CALLBACK_PATH: &str = "/oauth/callback";
pub const CALLBACK_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_PROVIDER_RESPONSE: usize = 65_536;

pub struct SecretString(Zeroizing<String>);
impl SecretString {
    pub fn new(value: String) -> Result<Self, OAuthError> {
        if value.trim().is_empty() {
            Err(OAuthError::InvalidToken)
        } else {
            Ok(Self(Zeroizing::new(value)))
        }
    }
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString([REDACTED])")
    }
}

pub struct AuthorizationUrl(Zeroizing<String>);
impl AuthorizationUrl {
    pub(crate) fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for AuthorizationUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthorizationUrl([REDACTED])")
    }
}

pub struct AuthorizationRequest {
    pub url: AuthorizationUrl,
    pub(crate) csrf_state: SecretString,
    pub(crate) pkce_verifier: PkceCodeVerifier,
    pub redirect_uri: String,
    pub deadline: SystemTime,
}
impl fmt::Debug for AuthorizationRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthorizationRequest([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OAuthError {
    InvalidConfiguration,
    CallbackInvalid,
    CallbackWrongPath,
    AuthorizationDenied,
    AuthorizationTimedOut,
    StateMismatch,
    InvalidToken,
    IdentityInvalid,
    Network,
    ProviderUnavailable,
    RateLimited,
    AuthorizationExpired,
    Protocol,
}

pub struct TokenGrant {
    pub access_token: SecretString,
    pub refresh_token: Option<SecretString>,
}
pub fn require_initial_refresh_token(grant: &TokenGrant) -> Result<&SecretString, OAuthError> {
    grant.refresh_token.as_ref().ok_or(OAuthError::InvalidToken)
}
impl fmt::Debug for TokenGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TokenGrant([REDACTED])")
    }
}

#[derive(Deserialize)]
struct TokenPayload {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    error: Option<String>,
}
#[derive(Deserialize)]
struct UserInfo {
    sub: Option<String>,
    email: Option<String>,
    email_verified: Option<bool>,
}

pub fn authorization_request(
    config: &OAuthClientConfig,
    port: u16,
    now: SystemTime,
) -> Result<AuthorizationRequest, OAuthError> {
    let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
    let client = BasicClient::new(ClientId::new(config.client_id.clone()))
        .set_auth_uri(AuthUrl::from_url(config.auth_url.clone()))
        .set_token_uri(TokenUrl::from_url(config.token_url.clone()))
        .set_redirect_uri(
            RedirectUrl::new(redirect_uri.clone()).map_err(|_| OAuthError::InvalidConfiguration)?,
        );
    let client = if let Some(secret) = &config.client_secret {
        client.set_client_secret(ClientSecret::new(secret.clone()))
    } else {
        client
    };
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, csrf) = client
        .authorize_url(CsrfToken::new_random)
        .set_pkce_challenge(challenge)
        .add_scope(Scope::new("https://mail.google.com/".into()))
        .add_scope(Scope::new("openid".into()))
        .add_scope(Scope::new("email".into()))
        .add_extra_param("access_type", "offline")
        .add_extra_param("prompt", "consent")
        .url();
    Ok(AuthorizationRequest {
        url: AuthorizationUrl::new(url.to_string()),
        csrf_state: SecretString::new(csrf.secret().to_owned())?,
        pkce_verifier: verifier,
        redirect_uri,
        deadline: now + CALLBACK_TIMEOUT,
    })
}

pub fn http_client() -> Result<reqwest::Client, OAuthError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .use_rustls_tls()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| OAuthError::Network)
}

pub async fn exchange_code(
    config: &OAuthClientConfig,
    request: &AuthorizationRequest,
    code: &str,
    client: &reqwest::Client,
) -> Result<TokenGrant, OAuthError> {
    if code.trim().is_empty() {
        return Err(OAuthError::Protocol);
    }
    let mut form = vec![
        ("client_id", config.client_id.as_str()),
        ("code", code),
        ("code_verifier", request.pkce_verifier.secret()),
        ("grant_type", "authorization_code"),
        ("redirect_uri", request.redirect_uri.as_str()),
    ];
    if let Some(secret) = &config.client_secret {
        form.push(("client_secret", secret.as_str()));
    }
    token_request(config, &form, client).await
}

pub async fn refresh(
    config: &OAuthClientConfig,
    refresh_token: &str,
    client: &reqwest::Client,
) -> Result<TokenGrant, OAuthError> {
    let mut form = vec![
        ("client_id", config.client_id.as_str()),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    if let Some(secret) = &config.client_secret {
        form.push(("client_secret", secret.as_str()));
    }
    token_request(config, &form, client).await
}

async fn token_request(
    config: &OAuthClientConfig,
    form: &[(&str, &str)],
    client: &reqwest::Client,
) -> Result<TokenGrant, OAuthError> {
    let response = client
        .post(config.token_url.clone())
        .form(form)
        .send()
        .await
        .map_err(|_| OAuthError::Network)?;
    let status = response.status().as_u16();
    let body = read_bounded_body(response).await?;
    parse_token_payload(status, &body)
}

fn parse_token_payload(status: u16, body: &[u8]) -> Result<TokenGrant, OAuthError> {
    let payload: TokenPayload = serde_json::from_slice(body).map_err(|_| OAuthError::Protocol)?;
    if !(200..300).contains(&status) {
        return Err(match payload.error.as_deref() {
            Some("invalid_grant") => OAuthError::AuthorizationExpired,
            _ if status == 429 => OAuthError::RateLimited,
            _ if status >= 500 => OAuthError::ProviderUnavailable,
            _ => OAuthError::Protocol,
        });
    }
    if payload.expires_in == Some(0) {
        return Err(OAuthError::InvalidToken);
    }
    Ok(TokenGrant {
        access_token: SecretString::new(payload.access_token.ok_or(OAuthError::InvalidToken)?)?,
        refresh_token: payload.refresh_token.map(SecretString::new).transpose()?,
    })
}

pub async fn fetch_identity(
    access_token: &SecretString,
    client: &reqwest::Client,
) -> Result<crate::model::AccountIdentity, OAuthError> {
    let response = client
        .get("https://openidconnect.googleapis.com/v1/userinfo")
        .bearer_auth(access_token.expose())
        .send()
        .await
        .map_err(|_| OAuthError::Network)?;
    let status = response.status().as_u16();
    let body = read_bounded_body(response).await?;
    if !(200..300).contains(&status) {
        return Err(if status >= 500 {
            OAuthError::ProviderUnavailable
        } else {
            OAuthError::IdentityInvalid
        });
    }
    parse_identity_payload(&body)
}

fn parse_identity_payload(body: &[u8]) -> Result<crate::model::AccountIdentity, OAuthError> {
    let info: UserInfo = serde_json::from_slice(body).map_err(|_| OAuthError::Protocol)?;
    let email = info
        .email
        .filter(|email| !email.trim().is_empty())
        .ok_or(OAuthError::IdentityInvalid)?;
    if info.sub.as_deref().is_none_or(str::is_empty) || info.email_verified != Some(true) {
        return Err(OAuthError::IdentityInvalid);
    }
    Ok(crate::model::AccountIdentity {
        provider: crate::model::MailProvider::Gmail,
        email,
    })
}

async fn read_bounded_body(response: reqwest::Response) -> Result<Vec<u8>, OAuthError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_PROVIDER_RESPONSE as u64)
    {
        return Err(OAuthError::Protocol);
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| OAuthError::Network)?;
        append_bounded(&mut body, &chunk)?;
    }
    Ok(body)
}

fn append_bounded(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), OAuthError> {
    if body.len().saturating_add(chunk.len()) > MAX_PROVIDER_RESPONSE {
        return Err(OAuthError::Protocol);
    }
    body.extend_from_slice(chunk);
    Ok(())
}

pub fn parse_callback_request(
    request: &[u8],
    peer: SocketAddr,
    expected_host: &str,
    expected_state: &str,
) -> Result<String, OAuthError> {
    if request.len() > 8192 || !peer.ip().is_loopback() {
        return Err(OAuthError::CallbackInvalid);
    }
    let text = std::str::from_utf8(request).map_err(|_| OAuthError::CallbackInvalid)?;
    let mut lines = text.split("\r\n");
    let first = lines.next().ok_or(OAuthError::CallbackInvalid)?;
    let mut parts = first.split_whitespace();
    if parts.next() != Some("GET") {
        return Err(OAuthError::CallbackInvalid);
    }
    let target = parts.next().ok_or(OAuthError::CallbackInvalid)?;
    if parts.next() != Some("HTTP/1.1") || parts.next().is_some() {
        return Err(OAuthError::CallbackInvalid);
    }
    let hosts: Vec<_> = lines
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("host"))
        .map(|(_, value)| value.trim())
        .collect();
    let [host] = hosts.as_slice() else {
        return Err(OAuthError::CallbackInvalid);
    };
    if *host != expected_host {
        return Err(OAuthError::CallbackInvalid);
    }
    let url = Url::parse(&format!("http://{expected_host}{target}"))
        .map_err(|_| OAuthError::CallbackInvalid)?;
    if url.path() != CALLBACK_PATH {
        return Err(OAuthError::CallbackWrongPath);
    }
    let mut code = Vec::new();
    let mut state = Vec::new();
    let mut error = Vec::new();
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => code.push(value.into_owned()),
            "state" => state.push(value.into_owned()),
            "error" => error.push(value.into_owned()),
            _ => {}
        }
    }
    if state.len() != 1 || state[0].is_empty() || state[0] != expected_state {
        return Err(if state.len() == 1 {
            OAuthError::StateMismatch
        } else {
            OAuthError::CallbackInvalid
        });
    }
    if !error.is_empty() && !code.is_empty() {
        return Err(OAuthError::CallbackInvalid);
    }
    if error.len() == 1 && !error[0].is_empty() {
        return Err(OAuthError::AuthorizationDenied);
    }
    if error.len() > 1 || error.first().is_some_and(String::is_empty) {
        return Err(OAuthError::CallbackInvalid);
    }
    if code.len() != 1 || code[0].is_empty() {
        return Err(OAuthError::CallbackInvalid);
    }
    Ok(code.remove(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    fn config() -> OAuthClientConfig {
        config::parse(br#"{"installed":{"client_id":"x.apps.googleusercontent.com","project_id":"whitford-email","auth_uri":"https://accounts.google.com/o/oauth2/v2/auth","token_uri":"https://oauth2.googleapis.com/token","redirect_uris":["http://localhost"]}}"#).unwrap()
    }
    #[test]
    fn request_has_exact_scopes_pkce_and_offline_consent() {
        let request = authorization_request(&config(), 4321, SystemTime::UNIX_EPOCH).unwrap();
        let url = Url::parse(request.url.expose()).unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(url.host_str(), Some("accounts.google.com"));
        assert_eq!(
            query.get("redirect_uri").unwrap(),
            "http://127.0.0.1:4321/oauth/callback"
        );
        assert_eq!(query.get("access_type").unwrap(), "offline");
        assert_eq!(query.get("prompt").unwrap(), "consent");
        assert_eq!(query.get("code_challenge_method").unwrap(), "S256");
    }
    #[test]
    fn callback_checks_peer_host_path_and_state() {
        let raw =
            b"GET /oauth/callback?code=abc&state=state HTTP/1.1\r\nHost: 127.0.0.1:4321\r\n\r\n";
        assert_eq!(
            parse_callback_request(
                raw,
                "127.0.0.1:99".parse().unwrap(),
                "127.0.0.1:4321",
                "state"
            )
            .unwrap(),
            "abc"
        );
        assert_eq!(
            parse_callback_request(
                raw,
                "127.0.0.1:99".parse().unwrap(),
                "127.0.0.1:4321",
                "other"
            ),
            Err(OAuthError::StateMismatch)
        );
    }
    #[test]
    fn duplicate_and_denied_callbacks_fail() {
        let duplicate =
            b"GET /oauth/callback?code=a&code=b&state=s HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n";
        assert_eq!(
            parse_callback_request(
                duplicate,
                "127.0.0.1:2".parse().unwrap(),
                "127.0.0.1:1",
                "s"
            ),
            Err(OAuthError::CallbackInvalid)
        );
        let contradictory = b"GET /oauth/callback?code=a&error=denied&state=s HTTP/1.1\r\nhost: 127.0.0.1:1\r\n\r\n";
        assert_eq!(
            parse_callback_request(
                contradictory,
                "127.0.0.1:2".parse().unwrap(),
                "127.0.0.1:1",
                "s"
            ),
            Err(OAuthError::CallbackInvalid)
        );
    }

    #[test]
    fn token_payload_matrix_is_strict_and_redacted() {
        let grant = parse_token_payload(
            200,
            br#"{"access_token":"access","refresh_token":"refresh","expires_in":3600}"#,
        )
        .unwrap();
        assert_eq!(grant.access_token.expose(), "access");
        assert!(grant.refresh_token.is_some());
        let missing_refresh =
            parse_token_payload(200, br#"{"access_token":"access","expires_in":3600}"#).unwrap();
        assert_eq!(
            require_initial_refresh_token(&missing_refresh).unwrap_err(),
            OAuthError::InvalidToken
        );
        for body in [
            b"{}".as_slice(),
            br#"{"access_token":""}"#,
            br#"{"access_token":"access","expires_in":0}"#,
            b"not-json",
        ] {
            assert!(parse_token_payload(200, body).is_err());
        }
        assert_eq!(
            parse_token_payload(400, br#"{"error":"invalid_grant"}"#).unwrap_err(),
            OAuthError::AuthorizationExpired
        );
        assert_eq!(
            parse_token_payload(429, br#"{"error":"slow_down"}"#).unwrap_err(),
            OAuthError::RateLimited
        );
        assert_eq!(
            parse_token_payload(503, br#"{"error":"unavailable"}"#).unwrap_err(),
            OAuthError::ProviderUnavailable
        );
    }

    #[test]
    fn identity_payload_requires_verified_nonempty_subject_and_email() {
        let account = parse_identity_payload(
            br#"{"sub":"stable","email":"person@example.com","email_verified":true}"#,
        )
        .unwrap();
        assert_eq!(account.email, "person@example.com");
        for body in [
            br#"{"sub":"stable","email":"person@example.com","email_verified":false}"#.as_slice(),
            br#"{"sub":"","email":"person@example.com","email_verified":true}"#,
            br#"{"sub":"stable","email_verified":true}"#,
            b"bad-json",
        ] {
            assert!(parse_identity_payload(body).is_err());
        }
    }

    #[test]
    fn chunk_accumulation_rejects_limit_plus_one_even_without_content_length() {
        let mut body = Vec::new();
        append_bounded(&mut body, &vec![b'x'; MAX_PROVIDER_RESPONSE]).unwrap();
        assert_eq!(append_bounded(&mut body, b"x"), Err(OAuthError::Protocol));
    }

    #[test]
    fn callback_validation_table_rejects_untrusted_shapes() {
        let peer: SocketAddr = "127.0.0.1:2".parse().unwrap();
        let cases: &[&[u8]] = &[
            b"POST /oauth/callback?code=a&state=s HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n",
            b"GET /oauth/callback?code=a&state=s HTTP/1.0\r\nHost: 127.0.0.1:1\r\n\r\n",
            b"GET /oauth/callback?code=a&state=s HTTP/1.1\r\nHost: evil.test\r\n\r\n",
            b"GET /oauth/callback?code=a HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n",
            b"GET /oauth/callback?code=&state=s HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n",
            b"GET /oauth/callback?code=a&state= HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n",
            b"GET /oauth/callback?code=a&state=s&state=s HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n",
            b"GET /oauth/callback?state=s HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n",
        ];
        for request in cases {
            assert!(parse_callback_request(request, peer, "127.0.0.1:1", "s").is_err());
        }
        let valid = b"GET /oauth/callback?code=a&state=s HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n";
        assert!(
            parse_callback_request(valid, "192.0.2.1:2".parse().unwrap(), "127.0.0.1:1", "s")
                .is_err()
        );
        let wrong_path = b"GET /other?code=a&state=s HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n";
        assert_eq!(
            parse_callback_request(wrong_path, peer, "127.0.0.1:1", "s"),
            Err(OAuthError::CallbackWrongPath)
        );
    }
}
