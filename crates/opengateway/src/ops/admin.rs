use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::store::{self, Store};

const META_KEY: &str = "admin_token";
const PREFIX: &str = "gwa_";

pub(crate) fn token(store: &Store, rotate: bool) -> store::Result<String> {
    if !rotate && let Some(token) = current(store)? {
        return Ok(token);
    }
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| store::Error::Invalid(format!("random source failed: {error}")))?;
    let token = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    store.set_meta(META_KEY, &token)?;
    Ok(token)
}

pub(crate) fn current(store: &Store) -> store::Result<Option<String>> {
    store.meta(META_KEY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_created_once_and_replaced_on_rotate() {
        let dir = std::env::temp_dir().join(format!("opengateway-admin-{}", std::process::id()));
        let store = Store::open(&crate::home::database(&dir).unwrap()).unwrap();
        assert_eq!(current(&store).unwrap(), None);
        let first = token(&store, false).unwrap();
        assert!(first.starts_with(PREFIX));
        assert_eq!(token(&store, false).unwrap(), first);
        let rotated = token(&store, true).unwrap();
        assert_ne!(rotated, first);
        assert_eq!(current(&store).unwrap(), Some(rotated));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
