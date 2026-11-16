use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RawJson(Box<RawValue>);

impl RawJson {
    pub fn parse(text: String) -> Result<Self, serde_json::Error> {
        RawValue::from_string(text).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.get()
    }
}

impl PartialEq for RawJson {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_original_bytes() {
        let text = r#"{"b":1,  "a":[2.50]}"#;
        let raw = RawJson::parse(text.to_owned()).unwrap();
        assert_eq!(raw.as_str(), text);
        assert_eq!(serde_json::to_string(&raw).unwrap(), text);
    }
}
