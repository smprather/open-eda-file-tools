//! Native backend for the LEF tool.
//!
//! The shared tokenizer lives here as well: `def_tools` links this crate for
//! its lexer and for the LEF macro sizes used by instance bounding boxes.

mod cell_lef;
mod lexer;

pub use cell_lef::{LefError, LibraryData, MacroData, parse_library, register};
pub use lexer::{Lexer, Token, open_input};
