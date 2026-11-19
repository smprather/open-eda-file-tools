//! The plain-Rust core of open-eda-file-tools: LEF/DEF readers and the
//! Liberty parser with no pyo3, Arrow or Parquet in scope.
//!
//! The Python extension (`open-eda-file-tools-native`, the `_native` module)
//! wraps these types behind pyo3; nothing here requires Python.
//!
//! The documented items are the supported API. Items hidden from the docs
//! (`#[doc(hidden)]`) exist for the Python bindings in this repository, are
//! not part of the supported API and may change in any release. Everything
//! else is crate-private. Every public entry point that runs parallel work
//! does so inside the [`threads`] CPU limit.

pub mod def;
pub mod lef;
pub mod liberty;
pub mod threads;

/// A plain failure from the core API. An I/O failure keeps its
/// `io::Error` (and so its `ErrorKind`); every other failure is its text.
#[derive(Debug)]
pub enum Error {
    /// A file could not be opened or read; `Display` is the `io::Error`'s text.
    Io(std::io::Error),
    /// Any other failure, as a message.
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

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
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
