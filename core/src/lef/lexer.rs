//! Streaming LEF/DEF tokenizer shared by both native backends.
//!
//! The hot paths scan the read buffer as slices (memchr for record ends, a
//! byte-class table for word ends) instead of one byte per call; at hundreds
//! of MB per design the per-byte calls were the dominant cost of every DEF
//! query.

use flate2::read::MultiGzDecoder;
use memchr::{memchr, memchr3};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

const LEXER_BUF_SIZE: usize = 1024 * 1024;

pub struct Token {
    pub text: String,
    pub offset: u64,
    quoted: bool,
}

impl Token {
    pub(super) fn is_keyword(&self, keyword: &str) -> bool {
        !self.quoted && self.text.eq_ignore_ascii_case(keyword)
    }
}

pub fn open_input(path: &Path) -> io::Result<Box<dyn Read + Send>> {
    let file = File::open(path)?;
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gz"))
    {
        Ok(Box::new(MultiGzDecoder::new(file)))
    } else {
        Ok(Box::new(file))
    }
}

/// Bytes that end a word: ASCII whitespace, `(`, `)`, `;`, `+` and `#`.
const WORD_END: [bool; 256] = {
    let mut table = [false; 256];
    let ends = b" \t\n\r\x0c();+#";
    let mut index = 0;
    while index < ends.len() {
        table[ends[index] as usize] = true;
        index += 1;
    }
    table
};

pub struct Lexer<R: Read> {
    reader: R,
    buffer: Vec<u8>,
    pos: usize,
    end: usize,
    /// File offset of `buffer[0]`.
    base: u64,
}

impl<R: Read> Lexer<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: vec![0; LEXER_BUF_SIZE],
            pos: 0,
            end: 0,
            base: 0,
        }
    }

    pub fn next_token(&mut self) -> io::Result<Option<Token>> {
        loop {
            let (byte, offset) = match self.next_byte()? {
                Some(item) => item,
                None => return Ok(None),
            };
            if byte == b'#' {
                self.skip_comment()?;
                continue;
            }
            if byte.is_ascii_whitespace() {
                continue;
            }
            if matches!(byte, b'+' | b'-') {
                if let Some((next, _)) = self.next_byte()? {
                    self.unread();
                    if next.is_ascii_digit() || next == b'.' {
                        return self.read_word(byte, offset);
                    }
                }
                return Ok(Some(Token {
                    text: (byte as char).to_string(),
                    offset,
                    quoted: false,
                }));
            }
            if matches!(byte, b'(' | b')' | b';') {
                return Ok(Some(Token {
                    text: (byte as char).to_string(),
                    offset,
                    quoted: false,
                }));
            }
            if byte == b'"' {
                return self.read_quoted(offset);
            }
            return self.read_word(byte, offset);
        }
    }

    fn read_word(&mut self, first: u8, offset: u64) -> io::Result<Option<Token>> {
        let mut bytes = Vec::with_capacity(16);
        bytes.push(first);
        loop {
            if self.pos >= self.end && !self.refill()? {
                break;
            }
            let chunk = &self.buffer[self.pos..self.end];
            let length = chunk
                .iter()
                .position(|&byte| WORD_END[byte as usize])
                .unwrap_or(chunk.len());
            bytes.extend_from_slice(&chunk[..length]);
            self.pos += length;
            if self.pos < self.end {
                // The delimiter stays unread, except a comment, which ends
                // the word and is consumed with it.
                if self.buffer[self.pos] == b'#' {
                    self.pos += 1;
                    self.skip_comment()?;
                }
                break;
            }
        }
        Ok(Some(Token {
            text: into_text(bytes),
            offset,
            quoted: false,
        }))
    }

    fn read_quoted(&mut self, offset: u64) -> io::Result<Option<Token>> {
        let mut bytes = Vec::new();
        loop {
            if self.pos >= self.end && !self.refill()? {
                break;
            }
            let chunk = &self.buffer[self.pos..self.end];
            match memchr(b'"', chunk) {
                Some(index) => {
                    bytes.extend_from_slice(&chunk[..index]);
                    self.pos += index + 1;
                    break;
                }
                None => {
                    bytes.extend_from_slice(chunk);
                    self.pos = self.end;
                }
            }
        }
        Ok(Some(Token {
            text: into_text(bytes),
            offset,
            quoted: true,
        }))
    }

    fn skip_comment(&mut self) -> io::Result<()> {
        loop {
            if self.pos >= self.end && !self.refill()? {
                return Ok(());
            }
            match memchr(b'\n', &self.buffer[self.pos..self.end]) {
                Some(index) => {
                    self.pos += index + 1;
                    return Ok(());
                }
                None => self.pos = self.end,
            }
        }
    }

    fn next_byte(&mut self) -> io::Result<Option<(u8, u64)>> {
        if self.pos >= self.end && !self.refill()? {
            return Ok(None);
        }
        let byte = self.buffer[self.pos];
        let offset = self.base + self.pos as u64;
        self.pos += 1;
        Ok(Some((byte, offset)))
    }

    /// Step back over the byte `next_byte` just returned. Always valid: a
    /// refill happens only when the buffer is exhausted, so the byte just
    /// read is still at `pos - 1`.
    fn unread(&mut self) {
        self.pos -= 1;
    }

    /// Read the next chunk. Returns `false` at end of input.
    fn refill(&mut self) -> io::Result<bool> {
        self.base += self.end as u64;
        self.pos = 0;
        self.end = 0;
        loop {
            match self.reader.read(&mut self.buffer) {
                Ok(count) => {
                    self.end = count;
                    return Ok(count > 0);
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            }
        }
    }

    /// Append the rest of a `;`-terminated record to `out`, without the `;`.
    /// Quoted text is copied verbatim; a `#` comment becomes one space.
    pub fn read_until_semicolon(&mut self, out: &mut Vec<u8>) -> io::Result<()> {
        self.scan_record(Some(out), false).map(|_| ())
    }

    /// Consume the rest of a `;`-terminated record without copying it.
    pub fn skip_until_semicolon(&mut self) -> io::Result<()> {
        self.scan_record(None, false).map(|_| ())
    }

    /// Like `read_until_semicolon`, but stop after the first `+` outside
    /// quotes and outside parentheses (`( inst pin + SYNTHESIZED )` is part of
    /// the connection list). Returns `true` when it stopped at a `+`: the
    /// record continues and the caller must consume the rest.
    pub fn read_until_plus_or_semicolon(&mut self, out: &mut Vec<u8>) -> io::Result<bool> {
        self.scan_record(Some(out), true)
    }

    fn scan_record(
        &mut self,
        mut out: Option<&mut Vec<u8>>,
        stop_at_plus: bool,
    ) -> io::Result<bool> {
        let mut in_quote = false;
        let mut in_comment = false;
        let mut depth = 0usize;
        loop {
            if self.pos >= self.end && !self.refill()? {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "unterminated component record",
                ));
            }
            let chunk = &self.buffer[self.pos..self.end];
            if in_comment {
                match memchr(b'\n', chunk) {
                    Some(index) => {
                        self.pos += index + 1;
                        in_comment = false;
                        if let Some(out) = out.as_deref_mut() {
                            out.push(b' ');
                        }
                    }
                    None => self.pos = self.end,
                }
                continue;
            }
            if in_quote {
                let (length, closed) = match memchr(b'"', chunk) {
                    Some(index) => (index + 1, true),
                    None => (chunk.len(), false),
                };
                if let Some(out) = out.as_deref_mut() {
                    out.extend_from_slice(&chunk[..length]);
                }
                self.pos += length;
                in_quote = !closed;
                continue;
            }
            let hit = if stop_at_plus {
                // Hop to the next `+` or the next other delimiter, whichever
                // comes first; parentheses are counted only in the skipped
                // bytes, when a `+` needs a depth.
                let plus = memchr(b'+', chunk);
                let limit = plus.unwrap_or(chunk.len());
                memchr3(b';', b'"', b'#', &chunk[..limit]).or(plus)
            } else {
                memchr3(b';', b'"', b'#', chunk)
            };
            let skipped = &chunk[..hit.unwrap_or(chunk.len())];
            if stop_at_plus {
                let opens = memchr::memchr_iter(b'(', skipped).count();
                let closes = memchr::memchr_iter(b')', skipped).count();
                depth = (depth + opens).saturating_sub(closes);
            }
            match hit {
                Some(index) => {
                    let byte = chunk[index];
                    if let Some(out) = out.as_deref_mut() {
                        out.extend_from_slice(&chunk[..index]);
                        if byte == b'"' {
                            out.push(b'"');
                        }
                    }
                    self.pos += index + 1;
                    match byte {
                        b';' => return Ok(false),
                        b'+' if depth == 0 => return Ok(true),
                        b'+' => {
                            if let Some(out) = out.as_deref_mut() {
                                out.push(b'+');
                            }
                        }
                        b'"' => in_quote = true,
                        _ => in_comment = true,
                    }
                }
                None => {
                    if let Some(out) = out.as_deref_mut() {
                        out.extend_from_slice(chunk);
                    }
                    self.pos = self.end;
                }
            }
        }
    }
}

fn into_text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes)
        .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reader that returns at most `step` bytes per call, so tokens and
    /// records straddle buffer refills.
    struct Trickle<'a> {
        data: &'a [u8],
        step: usize,
    }

    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let count = self.step.min(self.data.len()).min(buf.len());
            buf[..count].copy_from_slice(&self.data[..count]);
            self.data = &self.data[count..];
            Ok(count)
        }
    }

    const SAMPLE: &[u8] = b"UNITS DISTANCE MICRONS 1000 ; # c ; omment\n\
        - u1 BUF + PLACED ( -10 +20 ) N + PROPERTY p \"a;b # c\" # x ; y\n ;\n\
        A+B ( x#y\n) \"q\"";

    fn tokens(step: usize) -> Vec<(String, u64)> {
        let mut lexer = Lexer::new(Trickle { data: SAMPLE, step });
        let mut out = Vec::new();
        while let Some(token) = lexer.next_token().unwrap() {
            out.push((token.text, token.offset));
        }
        out
    }

    #[test]
    fn tokens_do_not_depend_on_read_size() {
        let whole = tokens(SAMPLE.len());
        for step in 1..8 {
            assert_eq!(tokens(step), whole, "step {step}");
        }
        let texts: Vec<&str> = whole.iter().map(|(text, _)| text.as_str()).collect();
        assert_eq!(&texts[..5], ["UNITS", "DISTANCE", "MICRONS", "1000", ";"]);
        assert!(texts.contains(&"-10") && texts.contains(&"+20"));
        assert!(texts.contains(&"a;b # c"));
        assert_eq!(
            &texts[texts.len() - 7..],
            ["A", "+", "B", "(", "x", ")", "q"]
        );
    }

    #[test]
    fn token_offsets_are_file_offsets() {
        for (text, offset) in tokens(3) {
            let start = offset as usize;
            let expected = if SAMPLE[start] == b'"' {
                format!("\"{text}")
            } else {
                text.clone()
            };
            assert!(
                SAMPLE[start..].starts_with(expected.as_bytes()),
                "{text} at {offset}"
            );
        }
    }

    #[test]
    fn records_do_not_depend_on_read_size() {
        let data: &[u8] =
            b"- u1 BUF + PROPERTY p \"a;b # c\" # x ; y\n + PLACED ( 1 2 ) N ;- u2 X ;";
        for step in [1, 2, 3, 5, 7, data.len()] {
            let mut lexer = Lexer::new(Trickle { data, step });
            let mut first = Vec::new();
            lexer.read_until_semicolon(&mut first).unwrap();
            assert_eq!(
                first, b"- u1 BUF + PROPERTY p \"a;b # c\"   + PLACED ( 1 2 ) N ",
                "step {step}"
            );
            lexer.skip_until_semicolon().unwrap();
            assert!(lexer.next_token().unwrap().is_none());
        }
    }

    #[test]
    fn connection_part_stops_at_the_first_plus() {
        let data: &[u8] =
            b"- n1 ( u1 A ) ( \"x+y\" B ) # c+\n + ROUTED M1 ( 0 0 ) ;- n2 ( u2 Z ) ;";
        for step in [1, 2, 3, 5, data.len()] {
            let mut lexer = Lexer::new(Trickle { data, step });
            let mut first = Vec::new();
            assert!(lexer.read_until_plus_or_semicolon(&mut first).unwrap());
            assert_eq!(first, b"- n1 ( u1 A ) ( \"x+y\" B )   ", "step {step}");
            lexer.skip_until_semicolon().unwrap();
            let mut second = Vec::new();
            assert!(!lexer.read_until_plus_or_semicolon(&mut second).unwrap());
            assert_eq!(second, b"- n2 ( u2 Z ) ");
        }
    }

    #[test]
    fn connection_part_skips_plus_inside_parentheses() {
        let data: &[u8] = b"- n1 ( u1 A + SYNTHESIZED ) ( u2 B ) + ROUTED M1 ( 0 0 ) ;";
        for step in [1, 2, 3, 5, data.len()] {
            let mut lexer = Lexer::new(Trickle { data, step });
            let mut out = Vec::new();
            assert!(lexer.read_until_plus_or_semicolon(&mut out).unwrap());
            assert_eq!(out, b"- n1 ( u1 A + SYNTHESIZED ) ( u2 B ) ", "step {step}");
            lexer.skip_until_semicolon().unwrap();
        }
    }

    #[test]
    fn unterminated_record_is_an_error() {
        let mut lexer = Lexer::new(Trickle {
            data: b"- u1 \"open;",
            step: 2,
        });
        let mut out = Vec::new();
        let err = lexer.read_until_semicolon(&mut out).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
}
