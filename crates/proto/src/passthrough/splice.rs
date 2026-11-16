use std::ops::Range;

use bytes::{Bytes, BytesMut};

use crate::codec::DecodeError;

const KEY: &str = "model";

pub fn splice_model(body: &[u8], model: &str) -> Result<Bytes, DecodeError> {
    let span = Scanner::new(body).find_member(KEY)?;
    let replacement = serde_json::to_string(model)?;
    let (Some(head), Some(tail)) = (body.get(..span.start), body.get(span.end..)) else {
        return Err(malformed("value span out of range"));
    };
    let mut out = BytesMut::with_capacity(body.len() + replacement.len());
    out.extend_from_slice(head);
    out.extend_from_slice(replacement.as_bytes());
    out.extend_from_slice(tail);
    Ok(out.freeze())
}

fn malformed(reason: &str) -> DecodeError {
    DecodeError::Json(reason.to_owned())
}

struct Scanner<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Scanner<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn byte(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.byte(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), DecodeError> {
        if self.byte() == Some(expected) {
            self.pos += 1;
            Ok(())
        } else {
            Err(malformed("unexpected token in top-level object"))
        }
    }

    fn skip_string(&mut self) -> Result<Range<usize>, DecodeError> {
        let start = self.pos;
        self.expect(b'"')?;
        loop {
            match self.byte() {
                None => return Err(malformed("unterminated string")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(start..self.pos);
                }
                Some(b'\\') => self.pos += 2,
                Some(_) => self.pos += 1,
            }
        }
    }

    fn skip_value(&mut self) -> Result<Range<usize>, DecodeError> {
        self.skip_whitespace();
        let start = self.pos;
        match self.byte() {
            None => return Err(malformed("missing value")),
            Some(b'"') => {
                self.skip_string()?;
            }
            Some(b'{' | b'[') => self.skip_container()?,
            Some(_) => {
                while !matches!(
                    self.byte(),
                    None | Some(b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
                ) {
                    self.pos += 1;
                }
                if self.pos == start {
                    return Err(malformed("missing value"));
                }
            }
        }
        Ok(start..self.pos)
    }

    fn skip_container(&mut self) -> Result<(), DecodeError> {
        let mut depth = 0usize;
        loop {
            match self.byte() {
                None => return Err(malformed("unterminated container")),
                Some(b'"') => {
                    self.skip_string()?;
                    continue;
                }
                Some(b'{' | b'[') => depth += 1,
                Some(b'}' | b']') => depth = depth.saturating_sub(1),
                Some(_) => {}
            }
            self.pos += 1;
            if depth == 0 {
                return Ok(());
            }
        }
    }

    fn find_member(&mut self, key: &str) -> Result<Range<usize>, DecodeError> {
        self.skip_whitespace();
        self.expect(b'{')?;
        loop {
            self.skip_whitespace();
            let name = self.skip_string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            let value = self.skip_value()?;
            if self.is_key(&name, key) {
                return Ok(value);
            }
            self.skip_whitespace();
            match self.byte() {
                Some(b',') => self.pos += 1,
                Some(b'}') => return Err(malformed("missing top-level model")),
                _ => return Err(malformed("unexpected token in top-level object")),
            }
        }
    }

    fn is_key(&self, span: &Range<usize>, key: &str) -> bool {
        self.bytes
            .get(span.clone())
            .and_then(|raw| serde_json::from_slice::<String>(raw).ok())
            .is_some_and(|name| name == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn splice(body: &str, model: &str) -> Result<String, DecodeError> {
        splice_model(body.as_bytes(), model).map(|bytes| String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[test]
    fn replaces_only_the_value_span() {
        assert_eq!(
            splice(
                " {\"a\" : [1, {\"model\":\"x\"}] ,\n \"model\" :  \"old\" , \"z\":1}",
                "new"
            )
            .unwrap(),
            " {\"a\" : [1, {\"model\":\"x\"}] ,\n \"model\" :  \"new\" , \"z\":1}"
        );
    }

    #[test]
    fn matches_escaped_key_and_skips_escaped_quotes() {
        assert_eq!(
            splice(r#"{"s":"a\"}\\","model":"o"}"#, "n").unwrap(),
            r#"{"s":"a\"}\\","model":"n"}"#
        );
    }

    #[test]
    fn rejects_missing_model() {
        assert!(splice(r#"{"a":{"model":"x"}}"#, "n").is_err());
        assert!(splice("[]", "n").is_err());
        assert!(splice(r#"{"a":"unterminated"#, "n").is_err());
    }
}
