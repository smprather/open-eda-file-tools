//! LEF parsing in plain Rust: the shared streaming tokenizer and the cell-LEF
//! model in microns.

mod cell_lef;
mod lexer;

pub use cell_lef::{
    LibraryData, MacroData, MacroSize, PinData, PortData, RectData, macro_sizes, parse_cell_lef,
};

// Used by the Python bindings for macro filtering and Python error mapping.
#[doc(hidden)]
pub use cell_lef::{LefError, parse_library};
pub(crate) use lexer::Lexer;
#[doc(hidden)]
pub use lexer::open_input;
