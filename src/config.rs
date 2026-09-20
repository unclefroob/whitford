use serde::Deserialize;
use std::{
    env, fmt, fs,
    io::Read,
    path::{Path, PathBuf},
};
use url::Url;

const MAX_CONFIG_BYTES: u64 = 65_536;
const EXPECTED_PROJECT: &str = "whitford-email";
const MAX_FIELD_BYTES: usize = 4096;

#[derive(Clone)]
pub struct OAuthClientConfig {
    pub(crate) client_id: String,
    pub(crate) client_secret: Option<String>,
    pub(crate) auth_url: Url,
    pub(crate) token_url: Url,
}
impl fmt::Debug for OAuthClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OAuthClientConfig([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigError {
    DirectoryUnavailable,
    Missing,
    Unreadable,
    TooLarge,
    Invalid,
    WrongProject,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigLoadError {
    pub kind: ConfigError,
    pub path: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientFile {
    installed: Installed,
}
#[derive(Deserialize)]
struct Installed {
    client_id: String,
    project_id: String,
    auth_uri: String,
    token_uri: String,
    client_secret: Option<String>,
    redirect_uris: Vec<String>,
}

pub fn resolve_config_path(
    xdg: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf, ConfigError> {
    let root = match xdg {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(_) => return Err(ConfigError::DirectoryUnavailable),
        None => home
            .filter(|path| path.is_absolute())
            .map(|path| path.join(".config"))
            .ok_or(ConfigError::DirectoryUnavailable)?,
    };
    Ok(root.join("whitford/google-oauth.json"))
}

pub fn default_config_path() -> Result<PathBuf, ConfigError> {
    let xdg = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    let home = env::var_os("HOME").map(PathBuf::from);
    resolve_config_path(xdg.as_deref(), home.as_deref())
}

pub fn load() -> Result<OAuthClientConfig, ConfigLoadError> {
    let path = default_config_path().map_err(|kind| ConfigLoadError { kind, path: None })?;
    let mut file = fs::File::open(&path).map_err(|error| {
        let kind = if error.kind() == std::io::ErrorKind::NotFound {
            ConfigError::Missing
        } else {
            ConfigError::Unreadable
        };
        ConfigLoadError {
            kind,
            path: Some(path.clone()),
        }
    })?;
    if !file
        .metadata()
        .map_err(|_| ConfigLoadError {
            kind: ConfigError::Unreadable,
            path: Some(path.clone()),
        })?
        .is_file()
    {
        return Err(ConfigLoadError {
            kind: ConfigError::Unreadable,
            path: Some(path),
        });
    }
    let bytes = read_bounded(&mut file).map_err(|kind| ConfigLoadError {
        kind,
        path: Some(path.clone()),
    })?;
    parse(&bytes).map_err(|kind| ConfigLoadError {
        kind,
        path: Some(path),
    })
}

fn read_bounded(reader: &mut impl Read) -> Result<Vec<u8>, ConfigError> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ConfigError::Unreadable)?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        Err(ConfigError::TooLarge)
    } else {
        Ok(bytes)
    }
}

pub fn config_location_hint() -> (Option<String>, bool) {
    let is_flatpak = env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists();
    (
        default_config_path()
            .ok()
            .map(|path| path.to_string_lossy().into_owned()),
        is_flatpak,
    )
}

pub fn parse(bytes: &[u8]) -> Result<OAuthClientConfig, ConfigError> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(if bytes.is_empty() {
            ConfigError::Invalid
        } else {
            ConfigError::TooLarge
        });
    }
    let parsed: ClientFile = serde_json::from_slice(bytes).map_err(|_| ConfigError::Invalid)?;
    let installed = parsed.installed;
    if installed.project_id != EXPECTED_PROJECT {
        return Err(ConfigError::WrongProject);
    }
    if installed.client_id.trim().is_empty()
        || installed.client_id.len() > MAX_FIELD_BYTES
        || !installed.client_id.ends_with(".apps.googleusercontent.com")
    {
        return Err(ConfigError::Invalid);
    }
    if !installed
        .redirect_uris
        .iter()
        .any(|uri| uri == "http://localhost" || uri.starts_with("http://127.0.0.1"))
    {
        return Err(ConfigError::Invalid);
    }
    let auth_url = exact_url(
        &installed.auth_uri,
        &[
            "https://accounts.google.com/o/oauth2/auth",
            "https://accounts.google.com/o/oauth2/v2/auth",
        ],
    )?;
    let token_url = exact_url(
        &installed.token_uri,
        &["https://oauth2.googleapis.com/token"],
    )?;
    if installed
        .client_secret
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.len() > MAX_FIELD_BYTES)
    {
        return Err(ConfigError::Invalid);
    }
    Ok(OAuthClientConfig {
        client_id: installed.client_id,
        client_secret: installed.client_secret,
        auth_url,
        token_url,
    })
}

fn exact_url(value: &str, allowed: &[&str]) -> Result<Url, ConfigError> {
    let url = Url::parse(value).map_err(|_| ConfigError::Invalid)?;
    if !allowed.iter().any(|allowed| url.as_str() == *allowed)
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ConfigError::Invalid);
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn valid() -> Vec<u8> {
        br#"{"installed":{"client_id":"x.apps.googleusercontent.com","project_id":"whitford-email","auth_uri":"https://accounts.google.com/o/oauth2/v2/auth","token_uri":"https://oauth2.googleapis.com/token","redirect_uris":["http://localhost"]}}"#.to_vec()
    }
    #[test]
    fn paths_follow_xdg_and_home_rules() {
        assert_eq!(
            resolve_config_path(Some(Path::new("/cfg")), None).unwrap(),
            PathBuf::from("/cfg/whitford/google-oauth.json")
        );
        assert_eq!(
            resolve_config_path(None, Some(Path::new("/home/a"))).unwrap(),
            PathBuf::from("/home/a/.config/whitford/google-oauth.json")
        );
        assert_eq!(
            resolve_config_path(Some(Path::new("relative")), None),
            Err(ConfigError::DirectoryUnavailable)
        );
    }
    #[test]
    fn valid_installed_client_is_accepted_and_debug_is_redacted() {
        let config = parse(&valid()).unwrap();
        assert!(format!("{config:?}").contains("REDACTED"));
    }
    #[test]
    fn wrong_project_and_web_clients_fail_closed() {
        let wrong = String::from_utf8(valid())
            .unwrap()
            .replace("whitford-email", "other");
        assert_eq!(
            parse(wrong.as_bytes()).unwrap_err(),
            ConfigError::WrongProject
        );
        assert_eq!(parse(br#"{"web":{}}"#).unwrap_err(), ConfigError::Invalid);
    }
    #[test]
    fn unsafe_endpoint_and_oversize_are_rejected() {
        let unsafe_json = String::from_utf8(valid()).unwrap().replace(
            "https://oauth2.googleapis.com/token",
            "http://oauth2.googleapis.com/token",
        );
        assert_eq!(
            parse(unsafe_json.as_bytes()).unwrap_err(),
            ConfigError::Invalid
        );
        assert_eq!(
            parse(&vec![b'x'; 65_537]).unwrap_err(),
            ConfigError::TooLarge
        );
    }

    #[test]
    fn bounded_reader_accepts_exact_limit_and_rejects_one_extra_byte() {
        let mut exact_bytes = valid();
        exact_bytes.resize(MAX_CONFIG_BYTES as usize, b' ');
        let mut exact = std::io::Cursor::new(exact_bytes);
        let bounded = read_bounded(&mut exact).unwrap();
        assert_eq!(bounded.len(), MAX_CONFIG_BYTES as usize);
        assert!(parse(&bounded).is_ok());

        let mut oversized_bytes = valid();
        oversized_bytes.resize(MAX_CONFIG_BYTES as usize + 1, b' ');
        let mut oversized = std::io::Cursor::new(oversized_bytes);
        assert_eq!(read_bounded(&mut oversized), Err(ConfigError::TooLarge));
    }
}
