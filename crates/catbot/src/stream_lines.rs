//! Newline-delimited records from HTTP chunks. Only records crossing a chunk boundary need copying.
use std::borrow::Cow;

#[derive(Default)]
pub(crate) struct StreamLines {
    partial: Vec<u8>,
}

impl StreamLines {
    /// Keep an unterminated suffix for the next chunk. Callers ignore it if the stream ends.
    pub(crate) fn push<'a>(&'a mut self, chunk: &'a [u8]) -> impl Iterator<Item = Cow<'a, [u8]>> + 'a {
        chunk.split_inclusive(|&byte| byte == b'\n').filter_map(|part| {
            if part.ends_with(b"\n") {
                if self.partial.is_empty() {
                    return Some(Cow::Borrowed(part));
                }
                self.partial.extend_from_slice(part);
                return Some(Cow::Owned(std::mem::take(&mut self.partial)));
            }
            self.partial.extend_from_slice(part);
            None
        })
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
        }
    }
}
