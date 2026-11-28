const MAX_LEN: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum NameError {
    #[error("enter a name")]
    Empty,
    #[error("use at most {MAX_LEN} characters")]
    TooLong,
    #[error("use letters, digits, - _ and .")]
    BadCharacter,
    #[error("that name is already used")]
    Taken,
}

pub(crate) fn check(name: &str, taken: &[String]) -> Result<(), NameError> {
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name.chars().count() > MAX_LEN {
        return Err(NameError::TooLong);
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(NameError::BadCharacter);
    }
    if taken.iter().any(|existing| existing == name) {
        return Err(NameError::Taken);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_names() {
        assert_eq!(check("claude-max_1.a", &[]), Ok(()));
    }

    #[test]
    fn rejects_empty_spaces_long_and_taken() {
        assert_eq!(check("", &[]), Err(NameError::Empty));
        assert_eq!(check("a b", &[]), Err(NameError::BadCharacter));
        assert_eq!(check(&"a".repeat(41), &[]), Err(NameError::TooLong));
        assert_eq!(check("a", &["a".to_owned()]), Err(NameError::Taken));
    }
}
