use bytes::{Bytes, BytesMut};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Debug, Default)]
pub struct Parser {
    buffer: BytesMut,
}

impl Parser {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<Frame> {
        self.buffer.extend_from_slice(chunk);
        let mut frames = Vec::new();
        while let Some((end, separator)) = find_boundary(&self.buffer) {
            let raw = self.buffer.split_to(end + separator);
            if let Some(frame) = parse_frame(raw.get(..end).unwrap_or_default()) {
                frames.push(frame);
            }
        }
        frames
    }
}

pub fn frame(event: Option<&str>, data: &str) -> Bytes {
    let mut out = String::new();
    if let Some(event) = event {
        out.push_str("event: ");
        out.push_str(event);
        out.push('\n');
    }
    for line in data.split('\n') {
        out.push_str("data: ");
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
    Bytes::from(out)
}

fn find_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2));
    let crlf = buffer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

fn parse_frame(raw: &[u8]) -> Option<Frame> {
    let text = String::from_utf8_lossy(raw);
    let mut event = None;
    let mut data: Option<String> = None;
    for line in text.lines() {
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => event = Some(value.to_owned()),
            "data" => match &mut data {
                Some(existing) => {
                    existing.push('\n');
                    existing.push_str(value);
                }
                None => data = Some(value.to_owned()),
            },
            _ => {}
        }
    }
    data.map(|data| Frame { event, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frames_split_across_chunks() {
        let mut parser = Parser::default();
        assert!(parser.push(b"event: ping\nda").is_empty());
        let frames = parser.push(b"ta: {}\n\ndata: [DONE]\n\n");
        assert_eq!(
            frames,
            vec![
                Frame {
                    event: Some("ping".to_owned()),
                    data: "{}".to_owned()
                },
                Frame {
                    event: None,
                    data: "[DONE]".to_owned()
                },
            ]
        );
    }

    #[test]
    fn ignores_comment_only_frames() {
        let mut parser = Parser::default();
        assert!(parser.push(b": keep-alive\n\n").is_empty());
    }

    #[test]
    fn frame_round_trips_through_parser() {
        let mut parser = Parser::default();
        let frames = parser.push(&frame(Some("message_stop"), "{\"a\":1}"));
        assert_eq!(frames.first().map(|f| f.data.as_str()), Some("{\"a\":1}"));
    }
}
