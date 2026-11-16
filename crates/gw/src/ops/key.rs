use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

use crate::store::{self, Store};

const PREFIX: &str = "gw_";

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("random source unavailable")]
    Random,
    #[error("key {0} already exists")]
    Exists(String),
    #[error(transparent)]
    Store(#[from] store::Error),
}

pub(crate) struct Issued {
    pub(crate) key: String,
    pub(crate) snippets: Vec<Snippet>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Snippet {
    pub(crate) client: &'static str,
    pub(crate) text: String,
}

pub(crate) fn create(store: &Store, name: &str, base_url: &str, now: i64) -> Result<Issued, Error> {
    if store.keys()?.iter().any(|k| k.name == name) {
        return Err(Error::Exists(name.to_owned()));
    }
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| Error::Random)?;
    let key = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    store.insert_key(name, &hash(&key), now)?;
    let snippets = snippets(base_url, &key);
    Ok(Issued { key, snippets })
}

pub(crate) fn hash(key: &str) -> String {
    Sha256::digest(key.as_bytes())
        .iter()
        .flat_map(|byte| [byte >> 4, byte & 0x0f])
        .filter_map(|nibble| char::from_digit(u32::from(nibble), 16))
        .collect()
}

pub(crate) fn snippets(base_url: &str, key: &str) -> Vec<Snippet> {
    vec![
        Snippet {
            client: "Claude Code",
            text: format!(
                "~/.claude/settings.json\n{{\n  \"env\": {{\n    \"ANTHROPIC_BASE_URL\": \"{base_url}\",\n    \"ANTHROPIC_AUTH_TOKEN\": \"{key}\",\n    \"CLAUDE_CODE_PROMPT_CACHE_TTL\": \"1h\"\n  }}\n}}\n"
            ),
        },
        Snippet {
            client: "Codex",
            text: format!(
                "~/.codex/config.toml\nmodel_provider = \"gw\"\n\n[model_providers.gw]\nname = \"gw\"\nbase_url = \"{base_url}/v1\"\nwire_api = \"responses\"\nenv_key = \"GW_API_KEY\"\nmodel_catalog_url = \"{base_url}/codex/models\"\n\n~/.codex/no-web-search.config.toml  (for non-OpenAI models: codex -p no-web-search -m <model>)\nmodel_provider = \"gw\"\nweb_search = \"disabled\"\n\n[features]\nmulti_agent = false\n\nshell\nexport GW_API_KEY={key}\n"
            ),
        },
        Snippet {
            client: "opencode",
            text: format!(
                "opencode.json\n{{\n  \"provider\": {{\n    \"gw\": {{\n      \"package\": \"@opencode/ai/providers/openai-compatible\",\n      \"settings\": {{ \"baseURL\": \"{base_url}/v1\", \"apiKey\": \"{key}\" }}\n    }}\n  }}\n}}\n"
            ),
        },
        Snippet {
            client: "pi",
            text: format!(
                "models.yml\nproviders:\n  gw:\n    baseUrl: {base_url}/v1\n    api: openai-responses\n    apiKey: {key}\n"
            ),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_hex_sha256() {
        assert_eq!(
            hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn snippets_embed_url_and_key() {
        let all = snippets("http://100.64.0.2:8787", "gw_x");
        assert!(all.iter().all(|s| s.text.contains("gw_x")));
        assert!(
            all.iter()
                .all(|s| s.text.contains("http://100.64.0.2:8787"))
        );
    }
}
