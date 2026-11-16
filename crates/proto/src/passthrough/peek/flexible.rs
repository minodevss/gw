use std::fmt;
use std::marker::PhantomData;

use serde::de::{Deserialize, Deserializer, SeqAccess, Visitor};

#[derive(Debug)]
pub(super) enum Flexible<T> {
    Text,
    List(Vec<T>),
}

impl<T> Flexible<T> {
    pub(super) fn items(&self) -> &[T] {
        match self {
            Self::Text => &[],
            Self::List(items) => items,
        }
    }

    pub(super) fn is_present(&self) -> bool {
        match self {
            Self::Text => true,
            Self::List(items) => !items.is_empty(),
        }
    }
}

pub(super) fn items<T>(value: Option<&Flexible<T>>) -> &[T] {
    value.map_or(&[], Flexible::items)
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Flexible<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(FlexibleVisitor(PhantomData))
    }
}

struct FlexibleVisitor<T>(PhantomData<T>);

impl<'de, T: Deserialize<'de>> Visitor<'de> for FlexibleVisitor<T> {
    type Value = Flexible<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string or an array")
    }

    fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
        Ok(Flexible::Text)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Flexible::List(items))
    }
}
