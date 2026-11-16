use std::fmt;

use crate::provider::{Kind, Scope, Usage};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Account {
    pub(crate) name: String,
    pub(crate) kind: Kind,
    pub(crate) secret: Secret,
    pub(crate) state: State,
    pub(crate) usage: Option<Usage>,
    pub(crate) usage_at: Option<i64>,
    pub(crate) created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum State {
    Active,
    Blocked { scope: Scope, reset_at: i64 },
    Inactive { reason: String },
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Secret {
    Oauth {
        access: String,
        refresh: String,
        expires_at: i64,
        account_id: Option<String>,
        client_id: Option<String>,
    },
    Token {
        token: String,
        expires_at: Option<i64>,
    },
    Key {
        key: String,
    },
    Custom {
        url: String,
        api: String,
        key: Option<String>,
    },
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Oauth { .. } => "Secret::Oauth(..)",
            Self::Token { .. } => "Secret::Token(..)",
            Self::Key { .. } => "Secret::Key(..)",
            Self::Custom { .. } => "Secret::Custom(..)",
        })
    }
}

impl Secret {
    pub(crate) fn bearer(&self) -> &str {
        match self {
            Self::Oauth { access, .. } => access,
            Self::Token { token, .. } => token,
            Self::Key { key } => key,
            Self::Custom { key, .. } => key.as_deref().unwrap_or_default(),
        }
    }

    pub(crate) fn account_id(&self) -> Option<&str> {
        match self {
            Self::Oauth { account_id, .. } => account_id.as_deref(),
            Self::Token { .. } | Self::Key { .. } | Self::Custom { .. } => None,
        }
    }

    pub(crate) fn expires_at(&self) -> Option<i64> {
        match self {
            Self::Oauth { expires_at, .. } => Some(*expires_at),
            Self::Token { expires_at, .. } => *expires_at,
            Self::Key { .. } | Self::Custom { .. } => None,
        }
    }
}

impl Account {
    pub(crate) fn is_usable_for(&self, model: &str, now: i64) -> bool {
        match &self.state {
            State::Active => true,
            State::Blocked { scope, reset_at } => *reset_at <= now || !scope.covers(model),
            State::Inactive { .. } => false,
        }
    }

    pub(crate) fn last_reset(&self) -> Option<i64> {
        self.usage.as_ref().and_then(Usage::earliest_reset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Method, Provider};

    fn account(state: State) -> Account {
        Account {
            name: "a".to_owned(),
            kind: Kind::new(Provider::Anthropic, Method::Oauth).unwrap(),
            secret: Secret::Key {
                key: "k".to_owned(),
            },
            state,
            usage: None,
            usage_at: None,
            created_at: 0,
        }
    }

    #[test]
    fn blocked_family_only_covers_that_family() {
        let a = account(State::Blocked {
            scope: Scope::Family("opus".to_owned()),
            reset_at: 100,
        });
        assert!(!a.is_usable_for("claude-opus-5-5", 50));
        assert!(a.is_usable_for("claude-sonnet-5-5", 50));
        assert!(a.is_usable_for("claude-opus-5-5", 100));
    }

    #[test]
    fn debug_hides_secret_values() {
        let secret = Secret::Key {
            key: "sk-secret".to_owned(),
        };
        assert!(!format!("{secret:?}").contains("sk-secret"));
    }
}
