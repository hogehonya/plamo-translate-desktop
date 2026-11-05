/// Incremental UTF-8 decoder for token pieces.
///
/// A multi-byte character may be split across tokens. Bytes are buffered until they form
/// complete characters; invalid sequences are replaced with U+FFFD.
#[derive(Debug, Default)]
pub(crate) struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    /// Appends `bytes` and returns the text that is now complete.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(s) => {
                    out.push_str(s);
                    self.pending.clear();
                    return out;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    // The prefix up to `valid_up_to` is valid UTF-8, so this never falls back.
                    out.push_str(std::str::from_utf8(&self.pending[..valid]).unwrap_or_default());
                    match e.error_len() {
                        Some(len) => {
                            out.push(char::REPLACEMENT_CHARACTER);
                            self.pending.drain(..valid + len);
                        }
                        None => {
                            // Incomplete sequence at the end: keep it for the next push.
                            self.pending.drain(..valid);
                            return out;
                        }
                    }
                }
            }
        }
    }

    /// Flushes any incomplete trailing sequence as U+FFFD.
    pub(crate) fn finish(&mut self) -> String {
        if self.pending.is_empty() {
            String::new()
        } else {
            self.pending.clear();
            char::REPLACEMENT_CHARACTER.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_split_characters() {
        let bytes = "日本語".as_bytes();
        let mut s = Utf8Stream::default();
        let mut out = String::new();
        for b in bytes {
            out.push_str(&s.push(std::slice::from_ref(b)));
        }
        out.push_str(&s.finish());
        assert_eq!(out, "日本語");
    }

    #[test]
    fn replaces_invalid_bytes() {
        let mut s = Utf8Stream::default();
        assert_eq!(s.push(b"a\xffb"), "a\u{fffd}b");
        assert_eq!(s.push(b"\xe6\x97"), "");
        assert_eq!(s.finish(), "\u{fffd}");
    }
}
