//! LEF parsing in plain Rust: the shared streaming tokenizer and the cell-LEF
//! model in microns.

mod cell_lef;
mod lexer;

pub use cell_lef::{MacroSize, macro_sizes};

// Used by the Python bindings; not part of the supported API.
#[doc(hidden)]
pub use cell_lef::{LefError, LibraryData, MacroData, PinData, PortData, RectData, parse_library};
pub(crate) use lexer::Lexer;
#[doc(hidden)]
pub use lexer::open_input;
