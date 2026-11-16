use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

use crate::ir::Origin;

const PREFIX: &str = "gw1:";

#[derive(Serialize, Deserialize)]
#[serde(tag = "o", rename_all = "snake_case")]
enum Sealed {
    Anthropic {
        s: String,
    },
    AnthropicRedacted {
        d: String,
    },
    OpenAi {
        i: Option<String>,
        e: Option<String>,
    },
    Plain,
}

pub fn seal(origin: &Origin) -> String {
    let sealed = match origin.clone() {
        Origin::Anthropic { signature } => Sealed::Anthropic { s: signature },
        Origin::AnthropicRedacted { data } => Sealed::AnthropicRedacted { d: data },
        Origin::OpenAi { id, encrypted } => Sealed::OpenAi {
            i: id,
            e: encrypted,
        },
        Origin::Plain => Sealed::Plain,
    };
    let json = serde_json::to_vec(&sealed).unwrap_or_default();
    format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(json))
}

pub fn open(value: &str) -> Option<Origin> {
    let encoded = value.strip_prefix(PREFIX)?;
    let json = URL_SAFE_NO_PAD.decode(encoded).ok()?;
    Some(match serde_json::from_slice(&json).ok()? {
        Sealed::Anthropic { s } => Origin::Anthropic { signature: s },
        Sealed::AnthropicRedacted { d } => Origin::AnthropicRedacted { data: d },
        Sealed::OpenAi { i, e } => Origin::OpenAi {
            id: i,
            encrypted: e,
        },
        Sealed::Plain => Origin::Plain,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_origin() {
        let origins = [
            Origin::Anthropic {
                signature: "sig".to_owned(),
            },
            Origin::AnthropicRedacted {
                data: "red".to_owned(),
            },
            Origin::OpenAi {
                id: Some("rs_1".to_owned()),
                encrypted: Some("enc".to_owned()),
            },
            Origin::Plain,
        ];
        for origin in origins {
            assert_eq!(open(&seal(&origin)), Some(origin));
        }
    }

    #[test]
    fn foreign_values_do_not_open() {
        assert_eq!(open("EqQBCkgIBhABGAIiQL"), None);
        assert_eq!(open("gw1:not-base64!"), None);
    }
}
