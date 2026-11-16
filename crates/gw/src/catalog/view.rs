use std::collections::BTreeMap;

use proto::Api;
use serde_json::{Value, json};

use super::{Capabilities, Catalog};
use crate::account::Account;

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ModelView {
    pub(crate) id: String,
    pub(crate) accounts: Vec<String>,
    pub(crate) context: Option<u64>,
    pub(crate) max_output: Option<u32>,
    pub(crate) efforts: Option<Vec<String>>,
    pub(crate) fast: bool,
    pub(crate) image: bool,
    pub(crate) responses_everywhere: bool,
    pub(crate) instructions: Option<String>,
}

pub(crate) fn models(catalog: &Catalog, accounts: &[Account]) -> Vec<ModelView> {
    let mut grouped: BTreeMap<&str, Vec<(&Account, &Capabilities)>> = BTreeMap::new();
    for (account_name, model, caps) in catalog.models() {
        if let Some(account) = accounts.iter().find(|a| a.name == account_name) {
            grouped.entry(model).or_default().push((account, caps));
        }
    }
    grouped
        .into_iter()
        .map(|(id, serving)| ModelView {
            id: id.to_owned(),
            accounts: serving.iter().map(|(a, _)| a.name.clone()).collect(),
            context: common(&serving, |c| c.context).and_then(|v| v.into_iter().min()),
            max_output: common(&serving, |c| c.max_output).and_then(|v| v.into_iter().min()),
            efforts: common(&serving, |c| c.efforts.clone()).map(intersect),
            fast: serving.iter().all(|(_, c)| c.fast == Some(true)),
            image: serving.iter().all(|(_, c)| c.image == Some(true)),
            responses_everywhere: serving
                .iter()
                .all(|(a, _)| a.kind.apis().contains(&Api::Responses)),
            instructions: serving.iter().find_map(|(_, c)| c.instructions.clone()),
        })
        .collect()
}

fn common<T>(
    serving: &[(&Account, &Capabilities)],
    field: impl Fn(&Capabilities) -> Option<T>,
) -> Option<Vec<T>> {
    serving.iter().map(|(_, caps)| field(caps)).collect()
}

fn intersect(lists: Vec<Vec<String>>) -> Vec<String> {
    let mut lists = lists.into_iter();
    let first = lists.next().unwrap_or_default();
    lists.fold(first, |acc, next| {
        acc.into_iter().filter(|v| next.contains(v)).collect()
    })
}

// Codex rejects the whole catalog when a model lacks an instructions template, and the template is
// Codex's system prompt for that model, so only models whose upstream supplied one are listed.
// https://github.com/openai/codex/blob/main/codex-rs/protocol/src/openai_models.rs
pub(crate) fn codex(models: &[ModelView]) -> Value {
    let models: Vec<Value> = models
        .iter()
        .filter_map(|m| m.instructions.as_ref().map(|instructions| (m, instructions)))
        .map(|(m, instructions)| {
            let levels: Vec<Value> = m
                .efforts
                .iter()
                .flatten()
                .map(|effort| json!({ "effort": effort, "description": effort }))
                .collect();
            let tiers: Vec<Value> = if m.fast {
                vec![json!({ "id": "priority", "name": "Fast", "description": "priority processing" })]
            } else {
                Vec::new()
            };
            let mut modalities = vec!["text"];
            if m.image {
                modalities.push("image");
            }
            json!({
                "slug": m.id,
                "display_name": m.id,
                "description": null,
                "supported_reasoning_levels": levels,
                "shell_type": "unified_exec",
                "visibility": "list",
                "supported_in_api": true,
                "priority": 0,
                "service_tiers": tiers,
                "availability_nux": null,
                "upgrade": null,
                "support_verbosity": false,
                "default_verbosity": null,
                "apply_patch_tool_type": if m.responses_everywhere { json!("freeform") } else { Value::Null },
                "truncation_policy": { "mode": "bytes", "limit": 10_000 },
                "context_window": m.context,
                "experimental_supported_tools": [],
                "input_modalities": modalities,
                "model_messages": { "instructions_template": instructions },
            })
        })
        .collect();
    json!({ "models": models })
}

pub(crate) fn anthropic(models: &[ModelView]) -> Value {
    let data: Vec<Value> = models
        .iter()
        .map(|m| json!({ "type": "model", "id": m.id, "display_name": m.id, "created_at": "1970-01-01T00:00:00Z" }))
        .collect();
    json!({
        "data": data,
        "has_more": false,
        "first_id": models.first().map(|m| m.id.as_str()),
        "last_id": models.last().map(|m| m.id.as_str()),
    })
}

pub(crate) fn openai(models: &[ModelView]) -> Value {
    let data: Vec<Value> = models
        .iter()
        .map(|m| {
            let mut modalities = vec!["text"];
            if m.image {
                modalities.push("image");
            }
            json!({
                "id": m.id,
                "object": "model",
                "created": 0,
                "owned_by": "gw",
                "context_length": m.context,
                "max_output_tokens": m.max_output,
                "supported_reasoning_levels": m.efforts,
                "input_modalities": modalities,
            })
        })
        .collect();
    json!({ "object": "list", "data": data })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{Secret, State};
    use crate::provider::{Kind, Method, Provider};

    fn account(name: &str, provider: Provider, method: Method) -> Account {
        Account {
            name: name.to_owned(),
            kind: Kind::new(provider, method).unwrap(),
            secret: Secret::Key { key: String::new() },
            state: State::Active,
            usage: None,
            usage_at: None,
            created_at: 0,
        }
    }

    #[test]
    fn shows_only_capabilities_known_for_every_account() {
        let accounts = vec![
            account("codex", Provider::OpenAi, Method::Codex),
            account("key", Provider::OpenAi, Method::Key),
        ];
        let mut catalog = Catalog::default();
        catalog.insert(
            "codex",
            "gpt",
            Capabilities {
                context: Some(400_000),
                efforts: Some(vec!["low".to_owned(), "xhigh".to_owned()]),
                ..Capabilities::default()
            },
        );
        catalog.insert("key", "gpt", Capabilities::default());
        let views = models(&catalog, &accounts);
        let gpt = views.first().unwrap();
        assert_eq!(gpt.context, None);
        assert_eq!(gpt.efforts, None);
        assert!(gpt.responses_everywhere);
    }

    #[test]
    fn codex_view_lists_only_models_with_instructions() {
        let without = ModelView {
            id: "claude-opus".to_owned(),
            ..ModelView::default()
        };
        assert_eq!(
            codex(&[without])["models"].as_array().map(Vec::len),
            Some(0)
        );
    }

    #[test]
    fn codex_view_contains_required_fields() {
        let view = ModelView {
            id: "gpt".to_owned(),
            efforts: Some(vec!["high".to_owned()]),
            instructions: Some("You are Codex.".to_owned()),
            ..ModelView::default()
        };
        let value = codex(&[view]);
        let model = &value["models"][0];
        for field in [
            "slug",
            "display_name",
            "supported_reasoning_levels",
            "shell_type",
            "visibility",
            "supported_in_api",
            "priority",
            "support_verbosity",
            "truncation_policy",
            "experimental_supported_tools",
        ] {
            assert!(model.get(field).is_some(), "{field}");
        }
        assert_eq!(
            model["model_messages"]["instructions_template"],
            "You are Codex."
        );
        assert_eq!(model["apply_patch_tool_type"], Value::Null);
    }
}
