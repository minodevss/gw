use std::collections::BTreeSet;

use crate::ir::{Effort, Item, Part, Request, Thinking, Tool};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ThinkingKind {
    Adaptive,
    Enabled,
    Disabled,
}

impl ThinkingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Adaptive => "adaptive",
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }
}

impl Thinking {
    pub fn kind(self) -> ThinkingKind {
        match self {
            Self::Adaptive => ThinkingKind::Adaptive,
            Self::Enabled { .. } => ThinkingKind::Enabled,
            Self::Disabled => ThinkingKind::Disabled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Feature {
    Effort(Effort),
    Thinking(ThinkingKind),
    Fast,
    ImageInput,
    DocumentInput,
    StructuredOutput,
    StrictTools,
    CustomTools,
    MidConversationSystem,
    Temperature,
    TopP,
    MaxOutputTokens,
    Stop,
    User,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Features(BTreeSet<Feature>);

impl Features {
    pub fn insert(&mut self, feature: Feature) {
        self.0.insert(feature);
    }

    pub fn contains(&self, feature: Feature) -> bool {
        self.0.contains(&feature)
    }

    pub fn iter(&self) -> impl Iterator<Item = Feature> + '_ {
        self.0.iter().copied()
    }

    pub fn of(request: &Request) -> Self {
        let mut features = Self::default();
        if let Some(effort) = request.effort {
            features.insert(Feature::Effort(effort));
        }
        if let Some(thinking) = request.thinking {
            features.insert(Feature::Thinking(thinking.kind()));
        }
        if request.speed.is_some() {
            features.insert(Feature::Fast);
        }
        if request.structured_output.is_some() {
            features.insert(Feature::StructuredOutput);
        }
        if request.temperature.is_some() {
            features.insert(Feature::Temperature);
        }
        if request.top_p.is_some() {
            features.insert(Feature::TopP);
        }
        if request.max_output_tokens.is_some() {
            features.insert(Feature::MaxOutputTokens);
        }
        if !request.stop.is_empty() {
            features.insert(Feature::Stop);
        }
        if request.user.is_some() {
            features.insert(Feature::User);
        }
        for tool in &request.tools {
            match tool {
                Tool::Function { strict, .. } => {
                    if *strict == Some(true) {
                        features.insert(Feature::StrictTools);
                    }
                }
                Tool::Custom { .. } => features.insert(Feature::CustomTools),
            }
        }
        for part in request
            .system
            .iter()
            .chain(request.items.iter().flat_map(parts))
        {
            features.insert_part(part);
        }
        if request
            .items
            .iter()
            .any(|item| matches!(item, Item::System { .. }))
        {
            features.insert(Feature::MidConversationSystem);
        }
        features
    }

    fn insert_part(&mut self, part: &Part) {
        match part {
            Part::Text(_) => {}
            Part::Image(_) => self.insert(Feature::ImageInput),
            Part::Document(_) => self.insert(Feature::DocumentInput),
        }
    }
}

fn parts(item: &Item) -> &[Part] {
    match item {
        Item::Message { parts, .. } | Item::System { parts } => parts,
        Item::ToolResult(result) => &result.parts,
        Item::Reasoning(_) | Item::ToolCall(_) => &[],
    }
}
