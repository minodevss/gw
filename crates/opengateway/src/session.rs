use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Default)]
pub(crate) struct Sessions {
    pins: Mutex<HashMap<(String, String), Pin>>,
}

#[derive(Debug, Clone)]
struct Pin {
    account: String,
    expires_at: i64,
}

impl Sessions {
    pub(crate) fn resolve<T>(
        &self,
        key: Option<&str>,
        model: &str,
        now: i64,
        ttl: i64,
        choose: impl FnOnce(Option<&str>) -> Option<(String, T)>,
    ) -> Option<T> {
        let Some(key) = key else {
            return choose(None).map(|(_, value)| value);
        };
        let mut pins = self
            .pins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pins.retain(|_, pin| pin.expires_at > now);
        let slot = (key.to_owned(), model.to_owned());
        let pinned = pins.get(&slot).map(|pin| pin.account.clone());
        let (account, value) = choose(pinned.as_deref())?;
        pins.insert(
            slot,
            Pin {
                account,
                expires_at: now + ttl,
            },
        );
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_per_session_and_model() {
        let sessions = Sessions::default();
        let first = sessions.resolve(Some("s"), "opus", 0, 300, |pinned| {
            assert_eq!(pinned, None);
            Some(("a".to_owned(), ()))
        });
        assert_eq!(first, Some(()));
        sessions.resolve(Some("s"), "opus", 10, 300, |pinned| {
            assert_eq!(pinned, Some("a"));
            Some(("a".to_owned(), ()))
        });
        sessions.resolve(Some("s"), "haiku", 10, 300, |pinned| {
            assert_eq!(pinned, None);
            Some(("b".to_owned(), ()))
        });
    }

    #[test]
    fn expires_after_ttl_since_last_use() {
        let sessions = Sessions::default();
        sessions.resolve(Some("s"), "m", 0, 300, |_| Some(("a".to_owned(), ())));
        sessions.resolve(Some("s"), "m", 299, 300, |pinned| {
            assert_eq!(pinned, Some("a"));
            Some(("a".to_owned(), ()))
        });
        sessions.resolve(Some("s"), "m", 600, 300, |pinned| {
            assert_eq!(pinned, None);
            Some(("b".to_owned(), ()))
        });
    }

    #[test]
    fn requests_without_session_key_are_not_pinned() {
        let sessions = Sessions::default();
        sessions.resolve(None, "m", 0, 300, |_| Some(("a".to_owned(), ())));
        sessions.resolve(None, "m", 1, 300, |pinned| {
            assert_eq!(pinned, None);
            Some(("a".to_owned(), ()))
        });
    }
}
