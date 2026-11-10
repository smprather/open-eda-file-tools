//! Streaming LEF/DEF tokenizer shared by both native backends.

use flate2::read::MultiGzDecoder;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

const LEXER_BUF_SIZE: usize = 1024 * 1024;

pub struct Token {
    pub text: String,
    pub offset: u64,
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

pub struct Lexer<R: Read> {
    reader: R,
    buffer: Vec<u8>,
    pos: usize,
    end: usize,
    offset: u64,
    pushed: Option<(u8, u64)>,
}

impl<R: Read> Lexer<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: vec![0; LEXER_BUF_SIZE],
            pos: 0,
            end: 0,
            offset: 0,
            pushed: None,
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
                if let Some((next, next_offset)) = self.next_byte()? {
                    if next.is_ascii_digit() || next == b'.' {
                        self.pushed = Some((next, next_offset));
                        return self.read_word(byte, offset);
                    }
                    self.pushed = Some((next, next_offset));
                }
                return Ok(Some(Token {
                    text: (byte as char).to_string(),
                    offset,
                }));
            }
            if matches!(byte, b'(' | b')' | b';') {
                return Ok(Some(Token {
                    text: (byte as char).to_string(),
                    offset,
                }));
            }
            if byte == b'"' {
                return self.read_quoted(offset);
            }
            return self.read_word(byte, offset);
        }
    }

    fn read_word(&mut self, first: u8, offset: u64) -> io::Result<Option<Token>> {
        let mut bytes = vec![first];
        while let Some((byte, byte_offset)) = self.next_byte()? {
            if byte == b'#' {
                self.skip_comment()?;
                break;
            }
            if byte.is_ascii_whitespace() || matches!(byte, b'(' | b')' | b';' | b'+') {
                self.pushed = Some((byte, byte_offset));
                break;
            }
            bytes.push(byte);
        }
        Ok(Some(Token {
            text: String::from_utf8_lossy(&bytes).into_owned(),
            offset,
        }))
    }

    fn read_quoted(&mut self, offset: u64) -> io::Result<Option<Token>> {
        let mut bytes = Vec::new();
        while let Some((byte, _)) = self.next_byte()? {
            if byte == b'"' {
                break;
            }
            bytes.push(byte);
        }
        Ok(Some(Token {
            text: String::from_utf8_lossy(&bytes).into_owned(),
            offset,
        }))
    }

    fn skip_comment(&mut self) -> io::Result<()> {
        while let Some((byte, _)) = self.next_byte()? {
            if byte == b'\n' {
                break;
            }
        }
        Ok(())
    }

    fn next_byte(&mut self) -> io::Result<Option<(u8, u64)>> {
        if let Some(item) = self.pushed.take() {
            return Ok(Some(item));
        }
        if self.pos >= self.end {
            self.end = self.reader.read(&mut self.buffer)?;
            self.pos = 0;
            if self.end == 0 {
                return Ok(None);
            }
        }
        let byte = self.buffer[self.pos];
        let offset = self.offset;
        self.pos += 1;
        self.offset += 1;
        Ok(Some((byte, offset)))
    }

    pub fn read_until_semicolon(&mut self, out: &mut Vec<u8>) -> io::Result<()> {
        let mut in_quote = false;
        while let Some((byte, _)) = self.next_byte()? {
            if in_quote {
                if byte == b'"' {
                    in_quote = false;
                }
                out.push(byte);
                continue;
            }
            if byte == b'#' {
                while let Some((comment_byte, _)) = self.next_byte()? {
                    if comment_byte == b'\n' {
                        out.push(b' ');
                        break;
                    }
                }
                continue;
            }
            if byte == b'"' {
                in_quote = true;
                out.push(byte);
                continue;
            }
            if byte == b';' {
                return Ok(());
            }
            out.push(byte);
        }
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "unterminated component record",
        ))
    }
}
