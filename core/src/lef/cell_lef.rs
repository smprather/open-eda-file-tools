//! Public cell-LEF API.
//!
//! Parses `MACRO` bodies from cell LEF libraries into an immutable model kept
//! entirely in LEF microns. The DEF database-unit conversion used by
//! `instance_bboxes` is applied downstream of this model so a cell library
//! never changes meaning based on a design's `UNITS DISTANCE MICRONS` value.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::lexer::{Lexer, Token, open_input};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum LefError {
    Io(io::Error),
    Value(String),
}

impl fmt::Display for LefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LefError::Io(err) => write!(f, "{err}"),
            LefError::Value(msg) => write!(f, "{msg}"),
        }
    }
}

fn value_error<T>(msg: String) -> LefResult<T> {
    Err(LefError::Value(msg))
}

/// Keep the `io::ErrorKind` so the Python bindings still map to the right
/// `OSError` subclass while the message carries the offending path.
fn io_error(path: &Path, err: io::Error) -> LefError {
    LefError::Io(io::Error::new(
        err.kind(),
        format!("{}: {err}", path.display()),
    ))
}

impl From<LefError> for crate::Error {
    fn from(err: LefError) -> Self {
        match err {
            LefError::Io(err) => crate::Error::Io(err),
            LefError::Value(message) => crate::Error::Message(message),
        }
    }
}

type LefResult<T> = Result<T, LefError>;

/// `"cells.lef: MACRO BUF "` or `"cells.lef: MACRO BUF PIN A: "`.
fn context(path: &Path, macro_name: &str, pin_name: Option<&str>) -> String {
    match pin_name {
        Some(pin) => format!("{}: MACRO {macro_name} PIN {pin}: ", path.display()),
        None => format!("{}: MACRO {macro_name} ", path.display()),
    }
}

// ---------------------------------------------------------------------------
// Micron-domain model
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct RectData {
    pub layer: String,
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

#[derive(Debug, Default)]
pub struct PortData {
    pub rects: Vec<Arc<RectData>>,
    pub unsupported: bool,
}

#[derive(Debug)]
pub struct PinData {
    pub name: String,
    pub direction: Option<String>,
    pub use_kind: Option<String>,
    pub ports: Vec<Arc<PortData>>,
}

#[derive(Debug)]
pub struct MacroData {
    pub name: String,
    pub width: f64,
    pub height: f64,
    pub origin_x: f64,
    pub origin_y: f64,
    pub pins: Vec<Arc<PinData>>,
    pub pin_index: HashMap<String, usize>,
}

#[derive(Debug, Default)]
pub struct LibraryData {
    pub macros: Vec<Arc<MacroData>>,
    pub macro_index: HashMap<String, usize>,
    pub duplicates: Vec<String>,
}

impl LibraryData {
    /// Last definition wins; the macro keeps its first-seen position so macro
    /// order stays deterministic across files.
    fn insert(&mut self, data: MacroData) {
        match self.macro_index.get(&data.name) {
            Some(&index) => {
                let name = data.name.clone();
                self.macros[index] = Arc::new(data);
                if !self.duplicates.contains(&name) {
                    self.duplicates.push(name);
                }
            }
            None => {
                self.macro_index
                    .insert(data.name.clone(), self.macros.len());
                self.macros.push(Arc::new(data));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Lexer wrapper with single-token pushback
// ---------------------------------------------------------------------------

struct LefLexer<R: Read> {
    lexer: Lexer<R>,
    pushed: Option<Token>,
}

impl<R: Read> LefLexer<R> {
    fn new(reader: R) -> Self {
        Self {
            lexer: Lexer::new(reader),
            pushed: None,
        }
    }

    fn next(&mut self) -> LefResult<Option<Token>> {
        if let Some(token) = self.pushed.take() {
            return Ok(Some(token));
        }
        self.lexer.next_token().map_err(LefError::Io)
    }

    fn push(&mut self, token: Token) {
        self.pushed = Some(token);
    }

    fn required(&mut self, missing: impl FnOnce() -> String) -> LefResult<Token> {
        match self.next()? {
            Some(token) => Ok(token),
            None => value_error(missing()),
        }
    }

    fn consume_statement(&mut self, missing: impl Fn() -> String) -> LefResult<()> {
        loop {
            match self.next()? {
                Some(token) if token.text == ";" => return Ok(()),
                Some(_) => continue,
                None => return value_error(missing()),
            }
        }
    }

    /// LEF closes `MACRO`/`PIN` groups with `END <name>` and `PORT`/`OBS`
    /// groups with a bare `END`. Some writers omit the name, so only consume a
    /// following token when it actually matches.
    fn consume_end_name(&mut self, expected: Option<&str>) -> LefResult<()> {
        let Some(expected) = expected else {
            return Ok(());
        };
        if let Some(token) = self.next()?
            && token.text != expected
        {
            self.push(token);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Parse `paths` in order.
///
/// `selection` limits which `MACRO` bodies are materialized; unselected bodies
/// are skipped without allocating their contents. `collect_pins` is false for
/// the DEF instance-bounding-box path, which needs only `SIZE` and `ORIGIN`.
///
/// An empty `paths` slice yields an empty library here; rejecting it is the
/// public `parse_lef` entry point's job, so the DEF path keeps its old behavior.
pub fn parse_library(
    paths: &[PathBuf],
    selection: Option<&HashSet<String>>,
    collect_pins: bool,
) -> LefResult<LibraryData> {
    let mut library = LibraryData::default();
    for path in paths {
        let input = open_input(path).map_err(|err| io_error(path, err))?;
        let mut lexer = LefLexer::new(input);
        while let Some(token) = lexer.next()? {
            if token.text.eq_ignore_ascii_case("PROPERTYDEFINITIONS") {
                // Property definitions may declare `MACRO <name> <TYPE> ;`
                // entries; those are property types, not macro bodies.
                skip_property_definitions(&mut lexer, path)?;
                continue;
            }
            if !token.text.eq_ignore_ascii_case("MACRO") {
                continue;
            }
            let name = lexer
                .required(|| format!("{}: expected macro name after MACRO", path.display()))?
                .text;
            let selected = selection.is_none_or(|wanted| wanted.contains(&name));
            if selected {
                let data = parse_macro_body(&mut lexer, path, &name, collect_pins)?;
                library.insert(data);
            } else {
                consume_group(&mut lexer, Some(&name), path, "MACRO", &name)?;
            }
        }
    }
    Ok(library)
}

fn parse_macro_body<R: Read>(
    lexer: &mut LefLexer<R>,
    path: &Path,
    macro_name: &str,
    collect_pins: bool,
) -> LefResult<MacroData> {
    let ctx = || context(path, macro_name, None);
    let mut width: Option<f64> = None;
    let mut height: Option<f64> = None;
    let mut origin_x = 0.0f64;
    let mut origin_y = 0.0f64;
    let mut pins: Vec<Arc<PinData>> = Vec::new();
    let mut pin_index: HashMap<String, usize> = HashMap::new();

    loop {
        let Some(token) = lexer.next()? else {
            return value_error(format!("{}missing END", ctx()));
        };
        if token.text.eq_ignore_ascii_case("END") {
            lexer.consume_end_name(Some(macro_name))?;
            break;
        }
        if token.text.eq_ignore_ascii_case("PIN") {
            let pin_name = lexer
                .required(|| format!("{}expected pin name after PIN", ctx()))?
                .text;
            if collect_pins {
                let pin = parse_pin_body(lexer, path, macro_name, &pin_name)?;
                if pin_index.contains_key(&pin.name) {
                    return value_error(format!("{}duplicate PIN {}", ctx(), pin.name));
                }
                pin_index.insert(pin.name.clone(), pins.len());
                pins.push(Arc::new(pin));
            } else {
                consume_group(lexer, Some(&pin_name), path, "PIN", &pin_name)?;
            }
            continue;
        }
        if token.text.eq_ignore_ascii_case("OBS") || token.text.eq_ignore_ascii_case("DENSITY") {
            consume_group(lexer, None, path, &token.text, macro_name)?;
            continue;
        }
        if token.text.eq_ignore_ascii_case("ORIGIN") {
            origin_x = parse_number(lexer, &ctx, "ORIGIN x")?;
            origin_y = parse_number(lexer, &ctx, "ORIGIN y")?;
            lexer.consume_statement(|| format!("{}unterminated ORIGIN", ctx()))?;
            continue;
        }
        if token.text.eq_ignore_ascii_case("SIZE") {
            let parsed_width = parse_number(lexer, &ctx, "SIZE width")?;
            let by = lexer.required(|| format!("{}expected BY in SIZE", ctx()))?;
            if !by.text.eq_ignore_ascii_case("BY") {
                return value_error(format!("{}expected BY in SIZE, got '{}'", ctx(), by.text));
            }
            let parsed_height = parse_number(lexer, &ctx, "SIZE height")?;
            lexer.consume_statement(|| format!("{}unterminated SIZE", ctx()))?;
            width = Some(parsed_width);
            height = Some(parsed_height);
            continue;
        }
        lexer.consume_statement(|| format!("{}unterminated {} statement", ctx(), token.text))?;
    }

    let (Some(width), Some(height)) = (width, height) else {
        return value_error(format!("{}missing SIZE", ctx()));
    };
    Ok(MacroData {
        name: macro_name.to_string(),
        width,
        height,
        origin_x,
        origin_y,
        pins,
        pin_index,
    })
}

fn parse_pin_body<R: Read>(
    lexer: &mut LefLexer<R>,
    path: &Path,
    macro_name: &str,
    pin_name: &str,
) -> LefResult<PinData> {
    let ctx = || context(path, macro_name, Some(pin_name));
    let mut direction: Option<String> = None;
    let mut use_kind: Option<String> = None;
    let mut ports: Vec<Arc<PortData>> = Vec::new();

    loop {
        let Some(token) = lexer.next()? else {
            return value_error(format!("{}missing END", ctx()));
        };
        if token.text.eq_ignore_ascii_case("END") {
            lexer.consume_end_name(Some(pin_name))?;
            break;
        }
        if token.text.eq_ignore_ascii_case("PORT") {
            ports.push(Arc::new(parse_port_body(lexer, &ctx)?));
            continue;
        }
        if token.text.eq_ignore_ascii_case("DIRECTION") {
            let value = lexer.required(|| format!("{}expected DIRECTION value", ctx()))?;
            direction = Some(value.text.to_ascii_lowercase());
            lexer.consume_statement(|| format!("{}unterminated DIRECTION", ctx()))?;
            continue;
        }
        if token.text.eq_ignore_ascii_case("USE") {
            let value = lexer.required(|| format!("{}expected USE value", ctx()))?;
            use_kind = Some(value.text.to_ascii_lowercase());
            lexer.consume_statement(|| format!("{}unterminated USE", ctx()))?;
            continue;
        }
        lexer.consume_statement(|| format!("{}unterminated {} statement", ctx(), token.text))?;
    }

    Ok(PinData {
        name: pin_name.to_string(),
        direction,
        use_kind,
        ports,
    })
}

fn parse_port_body<R: Read, F: Fn() -> String>(
    lexer: &mut LefLexer<R>,
    ctx: &F,
) -> LefResult<PortData> {
    let mut layer: Option<String> = None;
    let mut rects: Vec<Arc<RectData>> = Vec::new();
    let mut unsupported = false;

    loop {
        let Some(token) = lexer.next()? else {
            return value_error(format!("{}PORT missing END", ctx()));
        };
        if token.text.eq_ignore_ascii_case("END") {
            break;
        }
        if token.text.eq_ignore_ascii_case("LAYER") {
            let value = lexer.required(|| format!("{}expected LAYER name", ctx()))?;
            layer = Some(value.text);
            lexer.consume_statement(|| format!("{}unterminated LAYER", ctx()))?;
            continue;
        }
        if token.text.eq_ignore_ascii_case("RECT") {
            let Some(layer_name) = layer.as_deref() else {
                return value_error(format!("{}RECT appears before LAYER", ctx()));
            };
            let mut first = lexer.required(|| format!("{}truncated RECT", ctx()))?;
            if first.text.eq_ignore_ascii_case("MASK") {
                let _mask = lexer.required(|| format!("{}expected RECT MASK number", ctx()))?;
                first = lexer.required(|| format!("{}truncated RECT", ctx()))?;
            }
            if first.text.eq_ignore_ascii_case("ITERATE") {
                unsupported = true;
                lexer.consume_statement(|| format!("{}unterminated RECT", ctx()))?;
                continue;
            }
            let x0 = parse_coordinate(&first, ctx)?;
            let y0 = parse_rect_coordinate(lexer, ctx)?;
            let x1 = parse_rect_coordinate(lexer, ctx)?;
            let y1 = parse_rect_coordinate(lexer, ctx)?;
            lexer.consume_statement(|| format!("{}unterminated RECT", ctx()))?;
            rects.push(Arc::new(RectData {
                layer: layer_name.to_string(),
                x0: x0.min(x1),
                y0: y0.min(y1),
                x1: x0.max(x1),
                y1: y0.max(y1),
            }));
            continue;
        }
        if token.text.eq_ignore_ascii_case("POLYGON")
            || token.text.eq_ignore_ascii_case("PATH")
            || token.text.eq_ignore_ascii_case("VIA")
        {
            unsupported = true;
            lexer.consume_statement(|| format!("{}unterminated {}", ctx(), token.text))?;
            continue;
        }
        lexer.consume_statement(|| format!("{}unterminated {} statement", ctx(), token.text))?;
    }

    Ok(PortData { rects, unsupported })
}

/// Skip the `PROPERTYDEFINITIONS` section without interpreting it.
///
/// Property declarations can begin with the same words as groups (`MACRO`,
/// `PIN`, `PORT`), so the whole section is consumed rather than parsed.
fn skip_property_definitions<R: Read>(lexer: &mut LefLexer<R>, path: &Path) -> LefResult<()> {
    loop {
        let Some(token) = lexer.next()? else {
            return value_error(format!(
                "{}: unterminated PROPERTYDEFINITIONS",
                path.display()
            ));
        };
        if token.text.eq_ignore_ascii_case("END") {
            let Some(next) = lexer.next()? else {
                return value_error(format!(
                    "{}: unterminated PROPERTYDEFINITIONS",
                    path.display()
                ));
            };
            if next.text.eq_ignore_ascii_case("PROPERTYDEFINITIONS") {
                return Ok(());
            }
        }
    }
}

/// Skip a group without allocating its contents, tracking nested groups so the
/// terminating `END` is matched at the right level.
fn consume_group<R: Read>(
    lexer: &mut LefLexer<R>,
    end_name: Option<&str>,
    path: &Path,
    kind: &str,
    label: &str,
) -> LefResult<()> {
    loop {
        let Some(token) = lexer.next()? else {
            return value_error(format!("{}: unterminated {kind} {label}", path.display()));
        };
        if token.text.eq_ignore_ascii_case("END") {
            lexer.consume_end_name(end_name)?;
            return Ok(());
        }
        if token.text.eq_ignore_ascii_case("PIN") {
            let pin_name = lexer
                .required(|| format!("{}: expected pin name after PIN", path.display()))?
                .text;
            consume_group(lexer, Some(&pin_name), path, "PIN", &pin_name)?;
            continue;
        }
        if token.text.eq_ignore_ascii_case("PORT")
            || token.text.eq_ignore_ascii_case("OBS")
            || token.text.eq_ignore_ascii_case("DENSITY")
        {
            consume_group(lexer, None, path, &token.text, label)?;
            continue;
        }
        lexer.consume_statement(|| {
            format!(
                "{}: unterminated {} statement in {kind} {label}",
                path.display(),
                token.text
            )
        })?;
    }
}

fn parse_number<R: Read, F: Fn() -> String>(
    lexer: &mut LefLexer<R>,
    ctx: &F,
    what: &str,
) -> LefResult<f64> {
    let token = lexer.required(|| format!("{}expected {what}", ctx()))?;
    match token.text.parse::<f64>() {
        Ok(value) if value.is_finite() => Ok(value),
        _ => value_error(format!("{}invalid {what}: '{}'", ctx(), token.text)),
    }
}

fn parse_rect_coordinate<R: Read, F: Fn() -> String>(
    lexer: &mut LefLexer<R>,
    ctx: &F,
) -> LefResult<f64> {
    let token = lexer.required(|| format!("{}truncated RECT", ctx()))?;
    parse_coordinate(&token, ctx)
}

fn parse_coordinate<F: Fn() -> String>(token: &Token, ctx: &F) -> LefResult<f64> {
    match token.text.parse::<f64>() {
        Ok(value) if value.is_finite() => Ok(value),
        _ => value_error(format!(
            "{}malformed RECT coordinate '{}'",
            ctx(),
            token.text
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempfile_path() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "{}_{}_cell_lef_test.lef",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn parse_str(text: &str) -> LibraryData {
        let path = tempfile_path();
        std::fs::write(&path, text).unwrap();
        let result = parse_library(std::slice::from_ref(&path), None, true);
        std::fs::remove_file(&path).unwrap();
        result.unwrap()
    }

    const BUF: &str = "MACRO BUF\n  ORIGIN -0.5 0.25 ;\n  SIZE 5.76 BY 3.33 ;\n  PIN A\n    DIRECTION INPUT ;\n    USE SIGNAL ;\n    PORT\n      LAYER li1 ;\n      RECT 0.125 1.35 1.43 1.78 ;\n    END\n  END A\n  OBS\n    LAYER met1 ;\n    RECT 0 0 1 1 ;\n  END\nEND BUF\n";

    #[test]
    fn parses_macro_geometry_in_microns() {
        let library = parse_str(BUF);
        let macro_data = &library.macros[0];
        assert_eq!(macro_data.name, "BUF");
        assert_eq!(macro_data.width, 5.76);
        assert_eq!(macro_data.height, 3.33);
        assert_eq!(macro_data.origin_x, -0.5);
        assert_eq!(macro_data.origin_y, 0.25);
    }

    #[test]
    fn parses_pin_ports_and_skips_obs() {
        let library = parse_str(BUF);
        let pin = &library.macros[0].pins[0];
        assert_eq!(pin.name, "A");
        assert_eq!(pin.direction.as_deref(), Some("input"));
        assert_eq!(pin.use_kind.as_deref(), Some("signal"));
        let rect = &pin.ports[0].rects[0];
        assert_eq!(rect.layer, "li1");
        assert_eq!(
            (rect.x0, rect.y0, rect.x1, rect.y1),
            (0.125, 1.35, 1.43, 1.78)
        );
        assert!(!pin.ports[0].unsupported);
    }

    #[test]
    fn normalizes_reversed_rect_coordinates() {
        let library = parse_str(
            "MACRO M\n SIZE 1 BY 1 ;\n PIN P\n  PORT\n   LAYER li1 ;\n   RECT 2 3 1 0 ;\n  END\n END P\nEND M\n",
        );
        let rect = &library.macros[0].pins[0].ports[0].rects[0];
        assert_eq!((rect.x0, rect.y0, rect.x1, rect.y1), (1.0, 0.0, 2.0, 3.0));
    }

    #[test]
    fn flags_unsupported_geometry_without_dropping_rects() {
        let library = parse_str(
            "MACRO M\n SIZE 1 BY 1 ;\n PIN P\n  PORT\n   LAYER li1 ;\n   POLYGON 0 0 1 0 1 1 ;\n   RECT 0 0 1 1 ;\n  END\n END P\nEND M\n",
        );
        let port = &library.macros[0].pins[0].ports[0];
        assert!(port.unsupported);
        assert_eq!(port.rects.len(), 1);
    }

    #[test]
    fn reports_rect_before_layer_with_context() {
        let path = tempfile_path();
        std::fs::write(
            &path,
            "MACRO BUF\n SIZE 1 BY 1 ;\n PIN A\n  PORT\n   RECT 0 0 1 1 ;\n  END\n END A\nEND BUF\n",
        )
        .unwrap();
        let err = parse_library(std::slice::from_ref(&path), None, true).unwrap_err();
        std::fs::remove_file(&path).unwrap();
        let message = err.to_string();
        assert!(
            message.contains("MACRO BUF PIN A: RECT appears before LAYER"),
            "{message}"
        );
    }

    #[test]
    fn last_duplicate_macro_wins() {
        let library = parse_str("MACRO M\n SIZE 1 BY 1 ;\nEND M\nMACRO M\n SIZE 2 BY 2 ;\nEND M\n");
        assert_eq!(library.macros.len(), 1);
        assert_eq!(library.macros[0].width, 2.0);
        assert_eq!(library.duplicates, vec!["M".to_string()]);
    }

    #[test]
    fn macro_filter_skips_unselected_bodies() {
        let path = tempfile_path();
        std::fs::write(
            &path,
            "MACRO KEEP\n SIZE 1 BY 1 ;\nEND KEEP\nMACRO DROP\n SIZE 2 BY 2 ;\n PIN P\n  PORT\n   LAYER li1 ;\n   RECT 0 0 1 1 ;\n  END\n END P\nEND DROP\n",
        )
        .unwrap();
        let wanted: HashSet<String> = HashSet::from(["KEEP".to_string()]);
        let library = parse_library(std::slice::from_ref(&path), Some(&wanted), true).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(library.macros.len(), 1);
        assert_eq!(library.macros[0].name, "KEEP");
    }
}

// ---------------------------------------------------------------------------
// Public Rust API (issue #12)
// ---------------------------------------------------------------------------

/// A LEF macro's size in microns (`SIZE w BY h`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MacroSize {
    pub width_um: f64,
    pub height_um: f64,
}

/// Parse `paths` in order and return every macro's size in microns. The last
/// definition of a duplicated macro wins.
pub fn macro_sizes(paths: &[PathBuf]) -> Result<HashMap<String, MacroSize>, crate::Error> {
    let library = parse_library(paths, None, false)?;
    Ok(library
        .macros
        .iter()
        .map(|item| {
            (
                item.name.clone(),
                MacroSize {
                    width_um: item.width,
                    height_um: item.height,
                },
            )
        })
        .collect())
}
