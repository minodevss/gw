use crate::raw::RawJson;

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub model: String,
    pub system: Vec<Part>,
    pub items: Vec<Item>,
    pub tools: Vec<Tool>,
    pub tool_choice: Option<ToolChoice>,
    pub parallel_tool_calls: Option<bool>,
    pub max_output_tokens: Option<u32>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub stop: Vec<String>,
    pub effort: Option<Effort>,
    pub thinking: Option<Thinking>,
    pub reasoning_summary: bool,
    pub speed: Option<Speed>,
    pub structured_output: Option<StructuredOutput>,
    pub stream: bool,
    pub cache_key: Option<String>,
    pub user: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructuredOutput {
    pub name: Option<String>,
    pub schema: RawJson,
    pub strict: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Message { role: Role, parts: Vec<Part> },
    System { parts: Vec<Part> },
    Reasoning(Reasoning),
    ToolCall(ToolCall),
    ToolResult(ToolResult),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text(String),
    Image(Media),
    Document(Document),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Media {
    Base64 { media_type: String, data: String },
    Url(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub media: Media,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reasoning {
    pub text: String,
    pub summary: Vec<String>,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Anthropic {
        signature: String,
    },
    AnthropicRedacted {
        data: String,
    },
    OpenAi {
        id: Option<String>,
        encrypted: Option<String>,
    },
    Plain,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub kind: ToolKind,
    pub arguments: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Function,
    Custom,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub call_id: String,
    pub kind: ToolKind,
    pub parts: Vec<Part>,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Tool {
    Function {
        name: String,
        description: Option<String>,
        parameters: RawJson,
        strict: Option<bool>,
    },
    Custom {
        name: String,
        description: Option<String>,
        format: Option<RawJson>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolChoice {
    Auto,
    None,
    Required,
    Named(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Effort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "none" => Self::None,
            "minimal" => Self::Minimal,
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            "xhigh" => Self::Xhigh,
            "max" => Self::Max,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thinking {
    Adaptive,
    Enabled { budget_tokens: u32 },
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speed {
    Fast,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub id: String,
    pub model: String,
    pub output: Vec<Item>,
    pub stop: StopReason,
    pub usage: Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    ToolUse,
    StopSequence,
    Refusal,
    ContentFilter,
    ContextWindow,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_uncached: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    pub reasoning: Option<u64>,
}

impl Usage {
    pub fn input_total(&self) -> u64 {
        self.input_uncached + self.cache_read + self.cache_write
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Start { id: String, model: String },
    BlockStart { index: u32, block: Block },
    TextDelta { index: u32, text: String },
    ReasoningDelta { index: u32, text: String },
    ReasoningSummaryDelta { index: u32, text: String },
    ReasoningOrigin { index: u32, origin: Origin },
    ToolArgumentsDelta { index: u32, json: String },
    BlockStop { index: u32 },
    Usage(Usage),
    Stop(StopReason),
    Error(crate::error::ErrorBody),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Text,
    Reasoning,
    ToolCall {
        id: String,
        name: String,
        kind: ToolKind,
    },
}
