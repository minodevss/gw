use crate::store::LogRow;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Filter {
    terms: Vec<(Field, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Account,
    Session,
    Status,
    Model,
    Key,
    Reason,
}

impl Filter {
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let terms = text
            .split_whitespace()
            .map(|term| {
                let (field, value) = term
                    .split_once(':')
                    .ok_or_else(|| format!("expected field:value, got {term}"))?;
                let field = match field {
                    "account" => Field::Account,
                    "session" => Field::Session,
                    "status" => Field::Status,
                    "model" => Field::Model,
                    "key" => Field::Key,
                    "reason" => Field::Reason,
                    other => return Err(format!("unknown field {other}")),
                };
                Ok((field, value.to_owned()))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { terms })
    }

    pub(crate) fn matches(&self, row: &LogRow) -> bool {
        self.terms.iter().all(|(field, value)| {
            let haystack = match field {
                Field::Account => row.account.clone().unwrap_or_default(),
                Field::Session => row.session.clone().unwrap_or_default(),
                Field::Status => row.status.to_string(),
                Field::Model => row.model.clone(),
                Field::Key => row.key.clone().unwrap_or_default(),
                Field::Reason => row.reason.clone(),
            };
            haystack.contains(value.as_str())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_all_terms_as_substrings() {
        let filter = Filter::parse("account:oauth-1 status:429").unwrap();
        let row = LogRow {
            account: Some("anthropic-oauth-1".to_owned()),
            status: 429,
            ..LogRow::default()
        };
        assert!(filter.matches(&row));
        assert!(!filter.matches(&LogRow { status: 200, ..row }));
    }

    #[test]
    fn rejects_unknown_fields() {
        assert!(Filter::parse("colour:red").is_err());
    }
}
