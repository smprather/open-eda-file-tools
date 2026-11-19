//! The plain-Rust core of open-eda-file-tools: LEF/DEF readers and the
//! Liberty parser with no pyo3, Arrow or Parquet in scope.
//!
//! The Python extension (`open-eda-file-tools-native`, the `_native` module)
//! wraps these types behind pyo3; nothing here requires Python.

pub mod def;
pub mod lef;
pub mod liberty;
pub mod threads;
