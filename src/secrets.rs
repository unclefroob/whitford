use secret_service::{EncryptionType, SecretService};
use std::{collections::HashMap, fmt};
use zeroize::Zeroizing;

const LABEL: &str = "Whitford Gmail authorization";
const CONTENT_TYPE: &str = "text/plain; charset=utf-8";
const MAX_SECRET_BYTES: usize = 8192;

pub struct RefreshToken(Zeroizing<String>);
impl RefreshToken {
    pub fn new(value: String) -> Result<Self, SecretError> {
        if value.trim().is_empty() || value.len() > MAX_SECRET_BYTES {
            Err(SecretError::Invalid)
        } else {
            Ok(Self(Zeroizing::new(value)))
        }
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for RefreshToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RefreshToken([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretError {
    Unavailable,
    Locked,
    Ambiguous,
    Invalid,
    ReadFailed,
    SaveFailed,
    DeleteFailed,
}

fn attributes() -> HashMap<&'static str, &'static str> {
    HashMap::from([
        ("xdg:schema", "dev.whitford.OAuthRefreshToken"),
        ("application", "dev.whitford.Whitford"),
        ("provider", "gmail"),
        ("kind", "oauth-refresh-token"),
        ("schema-version", "1"),
    ])
}

pub async fn load() -> Result<Option<RefreshToken>, SecretError> {
    let service = SecretService::connect(EncryptionType::Dh)
        .await
        .map_err(|_| SecretError::Unavailable)?;
    let collection = service
        .get_default_collection()
        .await
        .map_err(|_| SecretError::Unavailable)?;
    unlock_collection(&collection).await?;
    let items = collection
        .search_items(attributes())
        .await
        .map_err(|_| SecretError::ReadFailed)?;
    unlock_items(&items).await?;
    match items.len() {
        0 => Ok(None),
        1 => {
            let bytes = items[0]
                .get_secret()
                .await
                .map_err(|_| SecretError::ReadFailed)?;
            classify_secret_values(vec![bytes])
        }
        _ => Err(SecretError::Ambiguous),
    }
}

pub async fn replace(token: &RefreshToken) -> Result<(), SecretError> {
    let service = SecretService::connect(EncryptionType::Dh)
        .await
        .map_err(|_| SecretError::Unavailable)?;
    let collection = service
        .get_default_collection()
        .await
        .map_err(|_| SecretError::Unavailable)?;
    unlock_collection(&collection).await?;
    collection
        .create_item(
            LABEL,
            attributes(),
            token.expose().as_bytes(),
            true,
            CONTENT_TYPE,
        )
        .await
        .map_err(|_| SecretError::SaveFailed)?;
    Ok(())
}

pub async fn delete() -> Result<(), SecretError> {
    let service = SecretService::connect(EncryptionType::Dh)
        .await
        .map_err(|_| SecretError::Unavailable)?;
    let collection = service
        .get_default_collection()
        .await
        .map_err(|_| SecretError::Unavailable)?;
    unlock_collection(&collection).await?;
    let items = collection
        .search_items(attributes())
        .await
        .map_err(|_| SecretError::DeleteFailed)?;
    unlock_items(&items)
        .await
        .map_err(|_| SecretError::DeleteFailed)?;
    for item in items {
        item.delete().await.map_err(|_| SecretError::DeleteFailed)?;
    }
    Ok(())
}

async fn unlock_collection(collection: &secret_service::Collection<'_>) -> Result<(), SecretError> {
    if collection
        .is_locked()
        .await
        .map_err(|_| SecretError::Locked)?
    {
        collection.unlock().await.map_err(|_| SecretError::Locked)?;
    }
    Ok(())
}

async fn unlock_items(items: &[secret_service::Item<'_>]) -> Result<(), SecretError> {
    for item in items {
        if item.is_locked().await.map_err(|_| SecretError::Locked)? {
            item.unlock().await.map_err(|_| SecretError::Locked)?;
        }
    }
    Ok(())
}

fn classify_secret_values(values: Vec<Vec<u8>>) -> Result<Option<RefreshToken>, SecretError> {
    match values.len() {
        0 => Ok(None),
        1 => {
            let value = String::from_utf8(values.into_iter().next().unwrap_or_default())
                .map_err(|_| SecretError::Invalid)?;
            RefreshToken::new(value).map(Some)
        }
        _ => Err(SecretError::Ambiguous),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct FakeStore {
        values: Vec<Vec<u8>>,
        locked: bool,
        fail_replace: bool,
        fail_delete: bool,
    }
    impl FakeStore {
        fn load(&self) -> Result<Option<RefreshToken>, SecretError> {
            if self.locked {
                Err(SecretError::Locked)
            } else {
                classify_secret_values(self.values.clone())
            }
        }
        fn replace(&mut self, token: &RefreshToken) -> Result<(), SecretError> {
            if self.fail_replace {
                Err(SecretError::SaveFailed)
            } else {
                self.values = vec![token.expose().as_bytes().to_vec()];
                Ok(())
            }
        }
        fn delete(&mut self) -> Result<(), SecretError> {
            if self.fail_delete {
                Err(SecretError::DeleteFailed)
            } else {
                self.values.clear();
                Ok(())
            }
        }
    }
    #[test]
    fn token_is_validated_and_redacted() {
        assert!(RefreshToken::new(String::new()).is_err());
        let token = RefreshToken::new("canary-secret".into()).unwrap();
        let debug = format!("{token:?}");
        assert!(!debug.contains("canary"));
        assert!(debug.contains("REDACTED"));
    }
    #[test]
    fn attributes_are_fixed_and_contain_no_identity() {
        let attrs = attributes();
        assert_eq!(attrs.len(), 5);
        assert_eq!(attrs["provider"], "gmail");
    }
    #[test]
    fn secret_contract_classifies_zero_one_multiple_and_boundaries() {
        assert!(classify_secret_values(vec![]).unwrap().is_none());
        assert_eq!(
            classify_secret_values(vec![b"ok".to_vec()])
                .unwrap()
                .unwrap()
                .expose(),
            "ok"
        );
        assert_eq!(
            classify_secret_values(vec![b"a".to_vec(), b"b".to_vec()]).unwrap_err(),
            SecretError::Ambiguous
        );
        assert_eq!(
            classify_secret_values(vec![vec![0xff]]).unwrap_err(),
            SecretError::Invalid
        );
        assert_eq!(
            classify_secret_values(vec![vec![b'x'; MAX_SECRET_BYTES + 1]]).unwrap_err(),
            SecretError::Invalid
        );
    }
    #[test]
    fn fake_store_contract_covers_lock_replace_delete_and_failures() {
        let token = RefreshToken::new("new".into()).unwrap();
        let mut store = FakeStore {
            values: vec![],
            locked: true,
            fail_replace: false,
            fail_delete: false,
        };
        assert_eq!(store.load().unwrap_err(), SecretError::Locked);
        store.locked = false;
        store.replace(&token).unwrap();
        assert_eq!(store.load().unwrap().unwrap().expose(), "new");
        store.delete().unwrap();
        assert!(store.load().unwrap().is_none());
        store.fail_replace = true;
        assert_eq!(store.replace(&token).unwrap_err(), SecretError::SaveFailed);
        store.fail_delete = true;
        assert_eq!(store.delete().unwrap_err(), SecretError::DeleteFailed);
    }
}
