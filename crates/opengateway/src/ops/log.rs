use crate::store::LogRow;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Filter {
    terms: Vec<(Field, String)>,
    words: Vec<String>,
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
    pub(crate) fn parse_loose(text: &str) -> Self {
        let mut filter = Self::default();
        for term in text.split_whitespace() {
            match term.split_once(':').and_then(|(field, value)| {
                Field::from_name(field).map(|field| (field, value.to_lowercase()))
            }) {
                Some(pair) => filter.terms.push(pair),
                None => filter.words.push(term.to_lowercase()),
            }
        }
        filter
    }

    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let terms = text
            .split_whitespace()
            .map(|term| {
                let (field, value) = term
                    .split_once(':')
                    .ok_or_else(|| format!("expected field:value, got {term}"))?;
                let field =
                    Field::from_name(field).ok_or_else(|| format!("unknown field {field}"))?;
                Ok((field, value.to_owned()))
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            terms,
            words: Vec::new(),
        })
    }

    pub(crate) fn matches(&self, row: &LogRow) -> bool {
        let fields = self.terms.iter().all(|(field, value)| {
            let haystack = match field {
                Field::Account => row.account.clone().unwrap_or_default(),
                Field::Session => row.session.clone().unwrap_or_default(),
                Field::Status => row.status.to_string(),
                Field::Model => row.model.clone(),
                Field::Key => row.key.clone().unwrap_or_default(),
                Field::Reason => row.reason.clone(),
            };
            haystack.to_lowercase().contains(&value.to_lowercase())
        });
        fields && (self.words.is_empty() || self.matches_words(row))
    }

    fn matches_words(&self, row: &LogRow) -> bool {
        let haystack = [
            row.key.as_deref(),
            row.session.as_deref(),
            Some(row.api.as_str()),
            Some(row.model.as_str()),
            row.account.as_deref(),
            Some(row.reason.as_str()),
            row.error.as_deref(),
            row.note.as_deref(),
            Some(row.status.to_string().as_str()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
        self.words
            .iter()
            .all(|word| haystack.contains(word.as_str()))
    }
}

impl Field {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "account" => Some(Self::Account),
            "session" => Some(Self::Session),
            "status" => Some(Self::Status),
            "model" => Some(Self::Model),
            "key" => Some(Self::Key),
            "reason" => Some(Self::Reason),
            _ => None,
        }
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
    fn loose_filter_searches_every_column() {
        let row = LogRow {
            account: Some("claude-max-1".to_owned()),
            error: Some("Hit the 5 hour limit".to_owned()),
            status: 429,
            ..LogRow::default()
        };
        assert!(Filter::parse_loose("HOUR").matches(&row));
        assert!(Filter::parse_loose("account:max hour").matches(&row));
        assert!(!Filter::parse_loose("nothing").matches(&row));
        assert!(Filter::parse_loose("http://x").matches(&LogRow {
            note: Some("http://x".to_owned()),
            ..LogRow::default()
        }));
    }

    #[test]
    fn rejects_unknown_fields() {
        assert!(Filter::parse("colour:red").is_err());
    }
}
