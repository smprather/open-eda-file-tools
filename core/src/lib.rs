//! The plain-Rust core of open-eda-file-tools: LEF, DEF, Liberty, SPEF, and
//! DSPF readers with no pyo3, Arrow, or Parquet in scope.
//!
//! The Python extension (`open-eda-file-tools-native`, the `_native` module)
//! wraps these types behind pyo3; nothing here requires Python.
//!
//! The documented items are the supported API. Items hidden from the docs
//! (`#[doc(hidden)]`) exist for the Python bindings in this repository, are
//! not part of the supported API and may change in any release. Everything
//! else is crate-private. Every public entry point that runs parallel work
//! does so inside the [`threads`] CPU limit.

pub(crate) mod connectivity;
pub mod def;
pub mod lef;
pub mod liberty;
pub mod pex;
pub mod threads;

/// A plain failure from the core API. A failure that surfaced as an
/// `io::Error` keeps it (and so its `ErrorKind`); every other failure is its
/// text.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// Any failure that surfaced as an `io::Error`: an unreadable file, and
    /// also malformed input reported by the tokenizer (`InvalidData`,
    /// `UnexpectedEof`). `Display` is the `io::Error`'s text. Branch on
    /// `kind()` (`NotFound`, `PermissionDenied`, ...) rather than on the
    /// variant alone.
    Io(std::io::Error),
    /// A failure that never was an `io::Error`, as a message.
    Message(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(err) => write!(f, "{err}"),
            Error::Message(message) => write!(f, "{message}"),
        }
    }
}

/// `Display` prints the wrapped `io::Error`, so `source()` returns that
/// error's own source rather than repeating it in an error chain.
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => err.source(),
            Error::Message(_) => None,
        }
    }
}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
