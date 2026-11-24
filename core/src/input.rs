//! Shared compression detection for text-format inputs.

use flate2::bufread::MultiGzDecoder;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Cursor, Read};
use std::path::Path;

const GZIP_MAGIC: &[u8] = b"\x1f\x8b";
const INPUT_BUFFER_SIZE: usize = 128 * 1024;

pub fn has_gzip_magic(prefix: &[u8]) -> bool {
    prefix.starts_with(GZIP_MAGIC)
}

pub fn open(path: &Path) -> io::Result<Box<dyn BufRead + Send>> {
    let mut reader = BufReader::with_capacity(INPUT_BUFFER_SIZE, File::open(path)?);
    let mut header = [0; GZIP_MAGIC.len()];
    let mut length = 0;
    while length < header.len() {
        match reader.read(&mut header[length..]) {
            Ok(0) => break,
            Ok(count) => length += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    let gzip = has_gzip_magic(&header[..length]);
    // Replay the signature so the decoder or plain parser sees the whole file.
    let input = Cursor::new(header).take(length as u64).chain(reader);
    if gzip {
        Ok(Box::new(BufReader::new(MultiGzDecoder::new(input))))
    } else {
        Ok(Box::new(input))
    }
}
