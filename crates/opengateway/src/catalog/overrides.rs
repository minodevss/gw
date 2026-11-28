use std::collections::BTreeMap;

use serde::Deserialize;

use super::Capabilities;
use crate::provider::Provider;

#[derive(Debug, Default, Deserialize)]
#[serde(transparent)]
pub(crate) struct Overrides(BTreeMap<String, BTreeMap<String, Patch>>);

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Patch {
    context: Option<u64>,
    max_output: Option<u32>,
    efforts: Option<Vec<String>>,
    thinking: Option<Vec<String>>,
    fast: Option<bool>,
    image: Option<bool>,
    document: Option<bool>,
    structured_output: Option<bool>,
    unsupported: Option<Vec<String>>,
}

impl Overrides {
    pub(crate) fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    pub(crate) fn apply(
        &self,
        provider: Provider,
        account: &str,
        models: &mut Vec<(String, Capabilities)>,
    ) {
        let scopes = [provider.as_str(), account];
        let patches = scopes
            .iter()
            .filter_map(|scope| self.0.get(*scope))
            .flatten();
        for (model, patch) in patches {
            let caps = if let Some((_, caps)) = models.iter_mut().find(|(id, _)| id == model) {
                caps
            } else {
                models.push((model.clone(), Capabilities::default()));
                match models.last_mut() {
                    Some((_, caps)) => caps,
                    None => continue,
                }
            };
            patch.apply(caps);
        }
    }
}

impl Patch {
    fn apply(&self, caps: &mut Capabilities) {
        let mut touched = Vec::new();
        set(&mut caps.context, self.context, "context", &mut touched);
        set(
            &mut caps.max_output,
            self.max_output,
            "max_output",
            &mut touched,
        );
        set(
            &mut caps.efforts,
            self.efforts.clone(),
            "efforts",
            &mut touched,
        );
        set(
            &mut caps.thinking,
            self.thinking.clone(),
            "thinking",
            &mut touched,
        );
        set(&mut caps.fast, self.fast, "fast", &mut touched);
        set(&mut caps.image, self.image, "image", &mut touched);
        set(&mut caps.document, self.document, "document", &mut touched);
        set(
            &mut caps.structured_output,
            self.structured_output,
            "structured_output",
            &mut touched,
        );
        if let Some(unsupported) = &self.unsupported {
            caps.unsupported.clone_from(unsupported);
            touched.push("unsupported");
        }
        for name in touched {
            if !caps.overridden.iter().any(|n| n == name) {
                caps.overridden.push(name.to_owned());
            }
        }
    }
}

fn set<T>(
    slot: &mut Option<T>,
    value: Option<T>,
    name: &'static str,
    touched: &mut Vec<&'static str>,
) {
    if let Some(value) = value {
        *slot = Some(value);
        touched.push(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_missing_models_and_marks_overridden_fields() {
        let overrides = Overrides::parse(
            r#"
            [zai."glm-5.3"]
            context = 200000
            efforts = ["low", "high"]
            "#,
        )
        .unwrap();
        let mut models = Vec::new();
        overrides.apply(Provider::Zai, "zai-plan-1", &mut models);
        let (id, caps) = models.first().unwrap();
        assert_eq!(id, "glm-5.3");
        assert_eq!(caps.context, Some(200_000));
        assert_eq!(caps.overridden, ["context", "efforts"]);
    }

    #[test]
    fn override_wins_over_upstream_value() {
        let overrides = Overrides::parse("[openai.\"gpt\"]\ncontext = 1\n").unwrap();
        let mut models = vec![(
            "gpt".to_owned(),
            Capabilities {
                context: Some(400_000),
                ..Capabilities::default()
            },
        )];
        overrides.apply(Provider::OpenAi, "openai-key-1", &mut models);
        assert_eq!(models.first().unwrap().1.context, Some(1));
    }

    #[test]
    fn account_scope_applies_only_to_that_account_and_wins_over_provider() {
        let overrides =
            Overrides::parse("[custom.\"m\"]\ncontext = 1\n[local-a.\"m\"]\ncontext = 2\n")
                .unwrap();
        let mut a = Vec::new();
        overrides.apply(Provider::Custom, "local-a", &mut a);
        assert_eq!(a.first().unwrap().1.context, Some(2));
        let mut b = Vec::new();
        overrides.apply(Provider::Custom, "local-b", &mut b);
        assert_eq!(b.first().unwrap().1.context, Some(1));
    }

    #[test]
    fn rejects_unknown_fields() {
        assert!(Overrides::parse("[zai.\"m\"]\ncontxt = 1\n").is_err());
    }
}
