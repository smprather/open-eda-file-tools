//! LEF parsing in plain Rust: the shared streaming tokenizer and the cell-LEF
//! model in microns.

mod cell_lef;
mod layer;
mod lexer;

pub use cell_lef::{
    LibraryData, MacroData, MacroSize, PinData, PortData, RectData, SiteData, macro_sizes,
    parse_cell_lef, parse_cell_lef_metadata,
};
pub use layer::{LayerData, LayerDirection, LayerKind};

// Used by the Python bindings for macro filtering and Python error mapping.
#[doc(hidden)]
pub use cell_lef::{LefError, parse_library};
#[doc(hidden)]
pub use lexer::open_input;
pub(crate) use lexer::{Lexer, Token};
