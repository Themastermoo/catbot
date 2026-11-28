//! Newline-delimited records from HTTP chunks. Only records crossing a chunk boundary need copying.
use std::borrow::Cow;

/// Cap on a single buffered line so a malformed or newline-less stream cannot OOM the server.
const MAX_PARTIAL: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct StreamLines {
    partial: Vec<u8>,
    oversize: bool,
}

impl StreamLines {
    /// Keep an unterminated suffix for the next chunk. Call [`Self::finish`] at EOF to flush any
    /// final record that lacked a trailing newline.
    pub(crate) fn push<'a>(&'a mut self, chunk: &'a [u8]) -> impl Iterator<Item = Cow<'a, [u8]>> + 'a {
        chunk.split_inclusive(|&byte| byte == b'\n').filter_map(|part| {
            if part.ends_with(b"\n") {
                if self.oversize {
                    self.oversize = false;
                    self.partial.clear();
                    return None;
                }
                if self.partial.is_empty() {
                    if part.len() > MAX_PARTIAL {
                        return None;
                    }
                    return Some(Cow::Borrowed(part));
                }
                if self.partial.len().saturating_add(part.len()) > MAX_PARTIAL {
                    self.partial.clear();
                    return None;
                }
                self.partial.extend_from_slice(part);
                return Some(Cow::Owned(std::mem::take(&mut self.partial)));
            }
            if !self.oversize {
                if self.partial.len().saturating_add(part.len()) > MAX_PARTIAL {
                    self.partial.clear();
                    self.oversize = true;
                } else {
                    self.partial.extend_from_slice(part);
                }
            }
            None
        })
    }

    /// Flushes any trailing unterminated record when the stream reaches EOF.
    pub(crate) fn finish(&mut self) -> Option<Vec<u8>> {
        if self.oversize {
            self.oversize = false;
            self.partial.clear();
            return None;
        }
        if self.partial.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.partial))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_survive_every_chunk_size() {
        let data = "one\r\n\n猫\ntail".as_bytes();
        for size in 1..=data.len() {
            let mut reader = StreamLines::default();
            let mut records = Vec::new();
            for chunk in data.chunks(size) {
                records.extend(reader.push(chunk).map(Cow::into_owned));
            }
            assert_eq!(records, [b"one\r\n".as_slice(), b"\n", "猫\n".as_bytes()]);
            assert_eq!(reader.finish().as_deref(), Some(b"tail".as_slice()));
            assert_eq!(reader.finish(), None);
        }
    }

    #[test]
    fn oversize_line_is_dropped_without_breaking_following_lines() {
        let mut reader = StreamLines::default();
        let big = vec![b'a'; MAX_PARTIAL + 16];
        assert!(reader.push(&big).next().is_none());
        let records: Vec<Vec<u8>> = reader.push(b"tail\nnext\n").map(Cow::into_owned).collect();
        assert_eq!(records, vec![b"next\n".to_vec()]);
    }
}
