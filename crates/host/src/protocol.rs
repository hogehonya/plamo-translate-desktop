//! Chrome's native messaging framing and the messages exchanged with the extension.
//!
//! Each message is UTF-8 JSON preceded by its length as a 32-bit integer in native byte order.
//! Messages to the browser may be at most 1 MiB; streamed text is sent in small pieces.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// Largest message accepted from the extension. A text this long is far beyond anything worth
/// translating in one request.
const MAX_REQUEST: usize = 64 << 20;

/// A message from the extension.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Translate `text`. `target` is a language code or `auto` (Japanese ⇄ English); `source`
    /// is a language code or absent to detect it; `style` is `polite` or `plain` for Japanese.
    Translate {
        id: u64,
        text: String,
        #[serde(default)]
        target: Option<String>,
        #[serde(default)]
        source: Option<String>,
        #[serde(default)]
        style: Option<String>,
    },
    /// Stop the running translation.
    Cancel,
    /// Report the model and its state.
    Status,
}

/// A message to the extension.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Status {
        /// `starting`, `loading`, `ready`, `no_model`, or `error`.
        state: &'static str,
        model: Option<String>,
        device: Option<String>,
        message: Option<String>,
        version: &'static str,
    },
    /// Translation `id` started into `target`.
    Started { id: u64, target: String },
    /// Chunk `index` (from 0) of `count` of a long text is starting.
    Progress { id: u64, index: usize, count: usize },
    /// Text to append to the translation.
    Text { id: u64, text: String },
    /// Keep only the first `length` UTF-16 code units of the translation: output that ran away
    /// is being translated again.
    Rewind { id: u64, length: usize },
    /// Translation `id` ended. `stopped` if it was cancelled or cut short.
    Done {
        id: u64,
        tokens: usize,
        cut: usize,
        stopped: bool,
    },
    /// `code` is one of `no_model`, `model_missing`, `load_failed`, `runtime_failed`,
    /// `bad_request`, or `failed`; `message` has the details in English.
    Error {
        id: Option<u64>,
        code: &'static str,
        message: String,
    },
}

/// Reads one message, or `None` at the end of the input (the browser closed the port).
pub fn read_message(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0; 4];
    match input.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_ne_bytes(len) as usize;
    if len > MAX_REQUEST {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("message of {len} bytes is too large"),
        ));
    }
    let mut message = vec![0; len];
    input.read_exact(&mut message)?;
    Ok(Some(message))
}

/// Writes one message and flushes it.
pub fn write_message(output: &mut impl Write, response: &Response) -> io::Result<()> {
    let json = serde_json::to_vec(response)?;
    output.write_all(&(json.len() as u32).to_ne_bytes())?;
    output.write_all(&json)?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_framed_messages() {
        let mut buffer = Vec::new();
        write_message(
            &mut buffer,
            &Response::Text {
                id: 7,
                text: "こんにちは".into(),
            },
        )
        .unwrap();
        let mut input = buffer.as_slice();
        let message = read_message(&mut input).unwrap().unwrap();
        assert_eq!(
            String::from_utf8(message).unwrap(),
            r#"{"type":"text","id":7,"text":"こんにちは"}"#
        );
        assert_eq!(read_message(&mut input).unwrap(), None);
    }

    #[test]
    fn parses_requests() {
        let request: Request =
            serde_json::from_str(r#"{"type":"translate","id":1,"text":"Hello","target":"auto"}"#)
                .unwrap();
        assert_eq!(
            request,
            Request::Translate {
                id: 1,
                text: "Hello".into(),
                target: Some("auto".into()),
                source: None,
                style: None,
            }
        );
        let cancel: Request = serde_json::from_str(r#"{"type":"cancel"}"#).unwrap();
        assert_eq!(cancel, Request::Cancel);
    }
}
