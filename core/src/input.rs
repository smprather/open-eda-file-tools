//! Custom stream opening and built-in plain/gzip detection for text inputs.

use flate2::bufread::MultiGzDecoder;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Cursor, Read};
use std::path::Path;
use std::sync::OnceLock;

const GZIP_MAGIC: &[u8] = b"\x1f\x8b";
const INPUT_BUFFER_SIZE: usize = 128 * 1024;
static STREAM_OPENER: OnceLock<Box<StreamOpener>> = OnceLock::new();

/// Opens decoded text for a path, or returns `Ok(None)` to use the built-in
/// plain/gzip file reader. An error is returned to the caller directly.
/// The callback can run concurrently, and each reader must start at byte zero.
pub type StreamOpener = dyn Fn(&Path) -> io::Result<Option<Box<dyn Read + Send>>> + Send + Sync;

/// Register the process-wide opener for LEF, DEF, Liberty, SPEF/DSPF, and SDC
/// source files. Register it before starting readers. A second registration
/// returns [`io::ErrorKind::AlreadyExists`].
pub fn set_stream_opener(opener: Box<StreamOpener>) -> io::Result<()> {
    STREAM_OPENER.set(opener).map_err(|_| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "stream opener already registered",
        )
    })
}

#[doc(hidden)]
pub fn has_gzip_magic(prefix: &[u8]) -> bool {
    prefix.starts_with(GZIP_MAGIC)
}

#[doc(hidden)]
pub fn open(path: &Path) -> io::Result<Box<dyn BufRead + Send>> {
    match open_custom(path)? {
        Some(reader) => Ok(reader),
        None => open_file(path),
    }
}

pub(crate) fn open_custom(path: &Path) -> io::Result<Option<Box<dyn BufRead + Send>>> {
    let Some(opener) = STREAM_OPENER.get() else {
        return Ok(None);
    };
    Ok(opener(path)?.map(|reader| {
        let buffered: Box<dyn BufRead + Send> =
            Box::new(BufReader::with_capacity(INPUT_BUFFER_SIZE, reader));
        buffered
    }))
}

pub(crate) fn open_file(path: &Path) -> io::Result<Box<dyn BufRead + Send>> {
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
