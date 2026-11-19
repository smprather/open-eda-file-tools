//! The plain-Rust core of open-eda-file-tools: LEF/DEF readers and the
//! Liberty parser with no pyo3, Arrow or Parquet in scope.
//!
//! The Python extension (`open-eda-file-tools-native`, the `_native` module)
//! wraps these types behind pyo3; nothing here requires Python.

pub mod def;
pub mod lef;
pub mod liberty;
pub mod threads;

/// A plain failure from the core API. Wraps the internals' `String` and
/// `io::Error` failures without changing their text.
#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self(value.to_string())
    }
}
