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
    /// Translate `segments` (such as the paragraphs of a page) in order, with the earlier ones
    /// as history; each translation comes back as a `segment` response. Other fields as for
    /// `translate`.
    TranslateSegments {
        id: u64,
        segments: Vec<String>,
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
    /// List the downloaded models and the compute devices.
    Options,
    /// Use this model (a catalog id) and device (`auto`, `cpu`, or a device name) instead of the
    /// app's selection; absent fields follow the app.
    Configure {
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        device: Option<String>,
    },
}

/// A downloaded model the extension can choose.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ModelChoice {
    pub id: String,
    /// Such as `2B · Q4_K_M`.
    pub label: String,
    pub bytes: u64,
}

/// A compute device the extension can choose.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct DeviceChoice {
    /// Such as `Vulkan0`.
    pub name: String,
    pub description: String,
    /// `gpu`, `integrated_gpu`, or `other`.
    pub kind: &'static str,
    pub memory_free: u64,
}

/// A message to the extension.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Status {
        /// `starting`, `loading`, `ready`, `no_model`, or `error`.
        state: &'static str,
        /// The model in use, such as `2B · Q4_K_M`.
        model: Option<String>,
        /// The device in use, such as `Vulkan0` or `CPU`.
        device: Option<String>,
        message: Option<String>,
        version: &'static str,
    },
    /// What `configure` can choose from, and the current choices.
    Options {
        models: Vec<ModelChoice>,
        devices: Vec<DeviceChoice>,
        /// The app's selection (a label) and device setting (`auto`, `cpu`, or a name).
        app_model: Option<String>,
        app_device: String,
        /// The extension's choices, if any (as in `configure`).
        model: Option<String>,
        device: Option<String>,
    },
    /// Translation `id` started into `target`.
    Started { id: u64, target: String },
    /// Chunk `index` (from 0) of `count` of a long text is starting.
    Progress { id: u64, index: usize, count: usize },
    /// Text to append to the translation.
    Text { id: u64, text: String },
    /// The complete translation of segment `index` of a `translate_segments` request.
    Segment { id: u64, index: usize, text: String },
    /// Keep only the first `length` UTF-16 code units of the translation: output that ran away
    /// is being translated again.
    Rewind { id: u64, length: usize },
    /// Translation `id` ended. `stopped` if it was cancelled or cut short; `cancelled` if it
    /// was cancelled (by `cancel`, a newer request, or a change of model), so that trying again
    /// can succeed.
    Done {
        id: u64,
        tokens: usize,
        cut: usize,
        stopped: bool,
        cancelled: bool,
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
        let configure: Request =
            serde_json::from_str(r#"{"type":"configure","device":"cpu"}"#).unwrap();
        assert_eq!(
            configure,
            Request::Configure {
                model: None,
                device: Some("cpu".into()),
            }
        );
    }
}
