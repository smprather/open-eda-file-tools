//! Native backend for the LEF tool.
//!
//! The shared tokenizer and the cell-LEF parser live in
//! `open_eda_file_tools_core::lef`; this module adds the Python classes and
//! entry point over them.

mod python;

pub use open_eda_file_tools_core::lef::*;
pub use python::register;
