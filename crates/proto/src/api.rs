#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Api {
    Messages,
    Responses,
    Chat,
}

impl Api {
    pub fn path(self) -> &'static str {
        match self {
            Self::Messages => "/v1/messages",
            Self::Responses => "/v1/responses",
            Self::Chat => "/v1/chat/completions",
        }
    }
}
