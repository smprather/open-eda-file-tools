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

use rayon::prelude::*;

use super::layer::{LayerData, LayerDirection, LayerKind, LayerStack};
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

/// A pin-port rectangle in LEF microns.
#[derive(Debug)]
pub struct RectData {
    /// LEF layer name.
    pub layer: String,
    /// Lower x coordinate.
    pub x0: f64,
    /// Lower y coordinate.
    pub y0: f64,
    /// Upper x coordinate.
    pub x1: f64,
    /// Upper y coordinate.
    pub y1: f64,
}

/// One LEF `PORT` group.
#[derive(Debug, Default)]
pub struct PortData {
    /// Supported `RECT` geometries in file order.
    pub rects: Vec<Arc<RectData>>,
    /// Whether the port contained geometry the parser skipped.
    pub unsupported: bool,
}

/// A LEF macro pin and its metadata.
#[derive(Debug)]
pub struct PinData {
    /// Name from the `PIN` declaration.
    pub name: String,
    /// Lowercase `DIRECTION` keyword (for example, `"inout"`), if present.
    pub direction: Option<String>,
    /// Lowercase `USE` keyword (for example, `"power"`), if present.
    pub use_kind: Option<String>,
    /// Port groups in file order.
    pub ports: Vec<Arc<PortData>>,
}

/// A cell macro with dimensions and origin in LEF microns.
#[derive(Debug)]
pub struct MacroData {
    /// Name from the `MACRO` declaration.
    pub name: String,
    /// Width from `SIZE`.
    pub width: f64,
    /// Height from `SIZE`.
    pub height: f64,
    /// Origin x coordinate, or zero if omitted.
    pub origin_x: f64,
    /// Origin y coordinate, or zero if omitted.
    pub origin_y: f64,
    /// Uppercase `CLASS` and optional subclass, populated by the metadata reader.
    pub class: Option<String>,
    /// Name from the first `SITE` statement, populated by the metadata reader.
    pub site: Option<String>,
    pins: Vec<Arc<PinData>>,
    pin_index: HashMap<String, usize>,
}

impl MacroData {
    /// Pins in LEF declaration order.
    pub fn pins(&self) -> &[Arc<PinData>] {
        &self.pins
    }

    /// Look up a pin by its case-sensitive LEF name.
    pub fn pin(&self, name: &str) -> Option<&Arc<PinData>> {
        self.pin_index.get(name).map(|&index| &self.pins[index])
    }
}

/// A top-level LEF `SITE` declaration and its size in microns.
#[derive(Debug)]
pub struct SiteData {
    /// Name from the `SITE` declaration.
    pub name: String,
    /// Width from `SIZE`.
    pub width: f64,
    /// Height from `SIZE`.
    pub height: f64,
}

/// Cell macros and any collected sites and layers from one or more LEF files.
#[derive(Debug, Default)]
pub struct LibraryData {
    macros: Vec<Arc<MacroData>>,
    macro_index: HashMap<String, usize>,
    pub(crate) duplicates: Vec<String>,
    sites: Vec<SiteData>,
    site_index: HashMap<String, usize>,
    layer_stack: LayerStack,
}

impl LibraryData {
    /// Macros in first-seen order, with the last definition of each name.
    pub fn macros(&self) -> &[Arc<MacroData>] {
        &self.macros
    }

    /// Look up a macro by its case-sensitive LEF name.
    pub fn macro_by_name(&self, name: &str) -> Option<&Arc<MacroData>> {
        self.macro_index.get(name).map(|&index| &self.macros[index])
    }

    /// Names of macros defined more than once, listed once each in the order
    /// the duplicates were encountered.
    pub fn duplicate_macro_names(&self) -> &[String] {
        &self.duplicates
    }

    /// Sites collected by the metadata reader, in first-seen order with the
    /// last definition of each name.
    pub fn sites(&self) -> &[SiteData] {
        &self.sites
    }

    /// Look up a collected site by its case-sensitive LEF name.
    pub fn site_by_name(&self, name: &str) -> Option<&SiteData> {
        self.site_index.get(name).map(|&index| &self.sites[index])
    }

    /// Technology layers collected by the metadata reader, in first-seen order.
    /// The first definition of each name supplies the layer's properties.
    pub fn layers(&self) -> &[LayerData] {
        &self.layer_stack.layers
    }

    /// Look up a collected layer by its case-sensitive LEF name.
    pub fn layer(&self, name: &str) -> Option<&LayerData> {
        self.layer_stack.get(name)
    }

    fn insert_site(&mut self, site: SiteData) {
        if let Some(&index) = self.site_index.get(&site.name) {
            self.sites[index] = site;
        } else {
            self.site_index.insert(site.name.clone(), self.sites.len());
            self.sites.push(site);
        }
    }

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

    /// Skip the remainder of a statement after consuming its keyword.
    fn skip_statement(&mut self, missing: impl FnOnce() -> String) -> LefResult<()> {
        debug_assert!(self.pushed.is_none());
        match self.lexer.skip_until_semicolon() {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => value_error(missing()),
            Err(err) => Err(LefError::Io(err)),
        }
    }

    fn skip_extension(&mut self, missing: impl FnOnce() -> String) -> LefResult<()> {
        while let Some(token) = self.next()? {
            if token.is_keyword("ENDEXT") {
                return Ok(());
            }
        }
        value_error(missing())
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

#[derive(Clone, Copy)]
enum MacroMode {
    Full,
    Dimensions,
    Metadata { include_pins: bool },
}

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
    let mode = if collect_pins {
        MacroMode::Full
    } else {
        MacroMode::Dimensions
    };
    let mut library = LibraryData::default();
    let files = parse_lef_files(paths, |path| parse_library_file(path, selection, mode));
    for file in files {
        for data in file? {
            library.insert(data);
        }
    }
    Ok(library)
}

fn parse_lef_files<T: Send>(
    paths: &[PathBuf],
    parse: impl Fn(&Path) -> LefResult<T> + Sync,
) -> Vec<LefResult<T>> {
    match paths {
        [] => Vec::new(),
        [path] => vec![parse(path)],
        _ => crate::threads::install(|| paths.par_iter().map(|path| parse(path)).collect()),
    }
}

fn parse_library_file(
    path: &Path,
    selection: Option<&HashSet<String>>,
    mode: MacroMode,
) -> LefResult<Vec<MacroData>> {
    let input = open_input(path).map_err(|err| io_error(path, err))?;
    let mut lexer = LefLexer::new(input);
    let mut macros = Vec::new();
    while let Some(token) = lexer.next()? {
        if token.text.eq_ignore_ascii_case("PROPERTYDEFINITIONS") {
            // Property definitions can contain MACRO property declarations.
            skip_property_definitions(&mut lexer, path)?;
            continue;
        }
        if !token.text.eq_ignore_ascii_case("MACRO") {
            continue;
        }
        let name = group_name(&mut lexer, path, "MACRO")?;
        if selection.is_none_or(|wanted| wanted.contains(&name)) {
            macros.push(parse_macro_body(&mut lexer, path, &name, mode)?);
        } else {
            consume_group(&mut lexer, Some(&name), path, "MACRO", &name)?;
        }
    }
    Ok(macros)
}

#[derive(Default)]
struct MetadataFile {
    macros: Vec<MacroData>,
    sites: Vec<SiteData>,
    layers: Vec<LayerData>,
}

fn parse_metadata_file(path: &Path, mode: MacroMode) -> LefResult<MetadataFile> {
    let parse = || {
        let input = open_input(path).map_err(LefError::Io)?;
        let mut lexer = LefLexer::new(input);
        let mut file = MetadataFile::default();
        while let Some(token) = lexer.next()? {
            match token.text.to_ascii_uppercase().as_str() {
                "MACRO" => {
                    let name = group_name(&mut lexer, path, "MACRO")?;
                    file.macros
                        .push(parse_macro_body(&mut lexer, path, &name, mode)?);
                }
                "SITE" => {
                    let name = group_name(&mut lexer, path, "SITE")?;
                    file.sites.push(parse_site_body(&mut lexer, path, name)?);
                }
                "LAYER" => {
                    let name = group_name(&mut lexer, path, "LAYER")?;
                    file.layers.push(parse_layer_body(&mut lexer, path, name)?);
                }
                "VIA" | "VIARULE" | "NONDEFAULTRULE" | "ARRAY" => {
                    let name = group_name(&mut lexer, path, &token.text)?;
                    skip_named_metadata_group(&mut lexer, path, &token.text, Some(&name))?;
                }
                "UNITS" | "SPACING" | "IRDROP" | "PROPERTYDEFINITIONS" => {
                    skip_named_metadata_group(&mut lexer, path, &token.text, None)?;
                }
                "NOISETABLE" | "CORRECTIONTABLE" => {
                    lexer.skip_statement(|| {
                        format!("{}: unterminated {} header", path.display(), token.text)
                    })?;
                    skip_named_metadata_group(&mut lexer, path, &token.text, None)?;
                }
                "BEGINEXT" => {
                    lexer.skip_extension(|| format!("{}: unterminated BEGINEXT", path.display()))?
                }
                "END" => {
                    if lexer
                        .required(|| format!("{}: expected LIBRARY after END", path.display()))?
                        .is_keyword("LIBRARY")
                    {
                        break;
                    }
                    return value_error(format!("{}: expected END LIBRARY", path.display()));
                }
                ";" => (),
                _ => lexer.skip_statement(|| {
                    format!("{}: unterminated {} statement", path.display(), token.text)
                })?,
            }
        }
        Ok(file)
    };
    parse().map_err(|err| match err {
        LefError::Io(err) => io_error(path, err),
        err => err,
    })
}

fn group_name<R: Read>(lexer: &mut LefLexer<R>, path: &Path, kind: &str) -> LefResult<String> {
    let missing = || format!("{}: expected name after {kind}", path.display());
    let token = lexer.required(missing)?;
    if token.is_keyword(";") {
        return value_error(missing());
    }
    Ok(token.text)
}

fn parse_site_body<R: Read>(
    lexer: &mut LefLexer<R>,
    path: &Path,
    name: String,
) -> LefResult<SiteData> {
    let ctx = || format!("{}: SITE {name} ", path.display());
    let mut size = None;
    loop {
        let token = lexer.required(|| format!("{}missing END", ctx()))?;
        if token.is_keyword("END") {
            lexer.consume_end_name(Some(&name))?;
            break;
        }
        if token.is_keyword("SIZE") {
            size = Some(parse_size(lexer, &ctx)?);
        } else {
            lexer.skip_statement(|| format!("{}unterminated {} statement", ctx(), token.text))?;
        }
    }
    let (width, height) = size.ok_or_else(|| LefError::Value(format!("{}missing SIZE", ctx())))?;
    Ok(SiteData {
        name,
        width,
        height,
    })
}

fn parse_layer_body<R: Read>(
    lexer: &mut LefLexer<R>,
    path: &Path,
    name: String,
) -> LefResult<LayerData> {
    let mut layer = LayerData::new(name);
    let layer_name = layer.name.clone();
    let ctx = || format!("{}: LAYER {layer_name} ", path.display());
    loop {
        let token = lexer.required(|| format!("{}missing END {layer_name}", ctx()))?;
        let key = token.text.to_ascii_uppercase();
        let missing = || format!("{}unterminated {key} statement", ctx());
        match key.as_str() {
            "END" => {
                let end = lexer.required(|| format!("{}expected END {layer_name}", ctx()))?;
                if end.text != layer_name {
                    return value_error(format!(
                        "{}expected END {layer_name}, got END {}",
                        ctx(),
                        end.text
                    ));
                }
                return Ok(layer);
            }
            "TYPE" | "DIRECTION" => {
                let value = lexer.required(missing)?;
                if value.is_keyword(";") {
                    return value_error(format!("{}missing {key} value", ctx()));
                }
                if key == "TYPE" {
                    layer.kind = LayerKind::from_lef(value.text);
                } else {
                    layer.direction = LayerDirection::from_lef(&value.text);
                }
                lexer.consume_statement(missing)?;
            }
            "PITCH" | "OFFSET" => {
                let x = parse_number(lexer, &ctx, &key)?;
                let next = lexer.required(missing)?;
                let y = if next.is_keyword(";") {
                    x
                } else {
                    lexer.push(next);
                    let y = parse_number(lexer, &ctx, &key)?;
                    lexer.consume_statement(missing)?;
                    y
                };
                if key == "PITCH" {
                    (layer.pitch_x_um, layer.pitch_y_um) = (Some(x), Some(y));
                } else {
                    (layer.offset_x_um, layer.offset_y_um) = (Some(x), Some(y));
                }
            }
            "WIDTH" | "MINWIDTH" | "THICKNESS" => {
                let value = Some(parse_number(lexer, &ctx, &key)?);
                match key.as_str() {
                    "WIDTH" => layer.width_um = value,
                    "MINWIDTH" => layer.min_width_um = value,
                    _ => layer.thickness_um = value,
                }
                lexer.consume_statement(missing)?;
            }
            "SPACING" => {
                let value = parse_number(lexer, &ctx, "SPACING")?;
                if lexer.required(missing)?.is_keyword(";") {
                    layer.spacing_um.get_or_insert(value);
                } else {
                    lexer.skip_statement(missing)?;
                }
            }
            "RESISTANCE" => {
                let suffix = lexer.required(missing)?;
                if suffix.is_keyword("RPERSQ") {
                    let value = lexer.required(missing)?;
                    if value.is_keyword("PWL") {
                        lexer.skip_statement(missing)?;
                        continue;
                    }
                    lexer.push(value);
                    layer.sheet_resistance = Some(parse_number(lexer, &ctx, "RESISTANCE RPERSQ")?);
                } else {
                    lexer.push(suffix);
                    layer.cut_resistance = Some(parse_number(lexer, &ctx, "RESISTANCE")?);
                }
                lexer.consume_statement(missing)?;
            }
            "BEGINEXT" => lexer.skip_extension(missing)?,
            ";" => (),
            _ => lexer.skip_statement(missing)?,
        }
    }
}

/// Skip LEF technology sections. Recognize nested groups at statement
/// boundaries so their names cannot be mistaken for a top-level declaration.
fn skip_named_metadata_group<R: Read>(
    lexer: &mut LefLexer<R>,
    path: &Path,
    kind: &str,
    name: Option<&str>,
) -> LefResult<()> {
    let expected = name.unwrap_or(kind);
    let missing = || format!("{}: unterminated {kind} {expected}", path.display());
    let kind = kind.to_ascii_uppercase();
    loop {
        let token = lexer.required(missing)?;
        if token.is_keyword("END") {
            let end = lexer.required(missing)?;
            let matches = name.map_or_else(|| end.is_keyword(expected), |name| end.text == name);
            if matches {
                return Ok(());
            }
            return value_error(format!(
                "{}: expected END {expected}, got END {}",
                path.display(),
                end.text,
            ));
        }
        if (kind == "VIA" && (token.is_keyword("DEFAULT") || token.is_keyword("GENERATED")))
            || (kind == "VIARULE" && (token.is_keyword("GENERATE") || token.is_keyword("DEFAULT")))
        {
            continue;
        }
        if (kind == "NONDEFAULTRULE" && (token.is_keyword("LAYER") || token.is_keyword("VIA")))
            || (kind == "ARRAY" && token.is_keyword("FLOORPLAN"))
        {
            let nested = group_name(lexer, path, &token.text)?;
            skip_named_metadata_group(lexer, path, &token.text, Some(&nested))?;
        } else if kind == "ARRAY" && token.is_keyword("DEFAULTCAP") {
            lexer.required(missing)?; // Number of entries.
            skip_named_metadata_group(lexer, path, "DEFAULTCAP", None)?;
        } else if kind == "NONDEFAULTRULE" && token.is_keyword("SPACING") {
            skip_named_metadata_group(lexer, path, "SPACING", None)?;
        } else if token.is_keyword("BEGINEXT") {
            lexer.skip_extension(missing)?;
        } else if !token.is_keyword(";") {
            lexer.skip_statement(missing)?;
        }
    }
}

/// Consume unrequested PIN, PORT, OBS, or DENSITY contents with the raw
/// statement skipper. A named PIN may contain nested bare-END PORTs.
fn skip_metadata_group<R: Read>(
    lexer: &mut LefLexer<R>,
    end_name: Option<&str>,
    ctx: &impl Fn() -> String,
) -> LefResult<()> {
    loop {
        let token = lexer.required(|| format!("{}missing END", ctx()))?;
        if token.is_keyword("END") {
            return lexer.consume_end_name(end_name);
        }
        if end_name.is_some() && token.is_keyword("PORT") {
            skip_metadata_group(lexer, None, ctx)?;
        } else if token.is_keyword("BEGINEXT") {
            lexer.skip_extension(|| format!("{}unterminated BEGINEXT", ctx()))?;
        } else if !token.is_keyword(";") {
            lexer.skip_statement(|| format!("{}unterminated {} statement", ctx(), token.text))?;
        }
    }
}

fn parse_macro_body<R: Read>(
    lexer: &mut LefLexer<R>,
    path: &Path,
    macro_name: &str,
    mode: MacroMode,
) -> LefResult<MacroData> {
    let ctx = || context(path, macro_name, None);
    let metadata = matches!(mode, MacroMode::Metadata { .. });
    let mut size = None;
    let mut origin_x = 0.0f64;
    let mut origin_y = 0.0f64;
    let mut class = None;
    let mut site = None;
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
            match mode {
                MacroMode::Full | MacroMode::Metadata { include_pins: true } => {
                    let pin = parse_pin_body(lexer, path, macro_name, &pin_name, !metadata)?;
                    if pin_index.contains_key(&pin.name) {
                        return value_error(format!("{}duplicate PIN {}", ctx(), pin.name));
                    }
                    pin_index.insert(pin.name.clone(), pins.len());
                    pins.push(Arc::new(pin));
                }
                MacroMode::Dimensions => {
                    consume_group(lexer, Some(&pin_name), path, "PIN", &pin_name)?;
                }
                MacroMode::Metadata {
                    include_pins: false,
                } => {
                    skip_metadata_group(lexer, Some(&pin_name), &|| {
                        context(path, macro_name, Some(&pin_name))
                    })?;
                }
            }
            continue;
        }
        if token.text.eq_ignore_ascii_case("OBS") || token.text.eq_ignore_ascii_case("DENSITY") {
            if metadata {
                skip_metadata_group(lexer, None, &|| format!("{}{}: ", ctx(), token.text))?;
            } else {
                consume_group(lexer, None, path, &token.text, macro_name)?;
            }
            continue;
        }
        if token.text.eq_ignore_ascii_case("ORIGIN") {
            origin_x = parse_number(lexer, &ctx, "ORIGIN x")?;
            origin_y = parse_number(lexer, &ctx, "ORIGIN y")?;
            lexer.consume_statement(|| format!("{}unterminated ORIGIN", ctx()))?;
            continue;
        }
        if token.text.eq_ignore_ascii_case("SIZE") {
            size = Some(parse_size(lexer, &ctx)?);
            continue;
        }
        if metadata {
            if token.is_keyword("CLASS") {
                class = Some(parse_class(lexer, &ctx)?);
                continue;
            }
            if token.is_keyword("SITE") && site.is_none() {
                let name = lexer.required(|| format!("{}expected SITE name", ctx()))?;
                if name.is_keyword(";") {
                    return value_error(format!("{}expected SITE name", ctx()));
                }
                site = Some(name.text);
            }
            if token.is_keyword("BEGINEXT") {
                lexer.skip_extension(|| format!("{}unterminated BEGINEXT", ctx()))?;
            } else {
                lexer
                    .skip_statement(|| format!("{}unterminated {} statement", ctx(), token.text))?;
            }
        } else {
            lexer
                .consume_statement(|| format!("{}unterminated {} statement", ctx(), token.text))?;
        }
    }

    let Some((width, height)) = size else {
        return value_error(format!("{}missing SIZE", ctx()));
    };
    Ok(MacroData {
        name: macro_name.to_string(),
        width,
        height,
        origin_x,
        origin_y,
        class,
        site,
        pins,
        pin_index,
    })
}

fn parse_size<R: Read>(
    lexer: &mut LefLexer<R>,
    ctx: &impl Fn() -> String,
) -> LefResult<(f64, f64)> {
    let width = parse_number(lexer, ctx, "SIZE width")?;
    let by = lexer.required(|| format!("{}expected BY in SIZE", ctx()))?;
    if !by.text.eq_ignore_ascii_case("BY") {
        return value_error(format!("{}expected BY in SIZE, got '{}'", ctx(), by.text));
    }
    let height = parse_number(lexer, ctx, "SIZE height")?;
    lexer.consume_statement(|| format!("{}unterminated SIZE", ctx()))?;
    Ok((width, height))
}

fn parse_class<R: Read>(lexer: &mut LefLexer<R>, ctx: &impl Fn() -> String) -> LefResult<String> {
    let missing = || format!("{}expected CLASS value", ctx());
    let first = lexer.required(missing)?;
    if first.is_keyword(";") {
        return value_error(missing());
    }
    let mut class = first.text;
    loop {
        let token = lexer.required(|| format!("{}unterminated CLASS", ctx()))?;
        if token.is_keyword(";") {
            class.make_ascii_uppercase();
            return Ok(class);
        }
        class.push(' ');
        class.push_str(&token.text);
    }
}

fn parse_pin_body<R: Read>(
    lexer: &mut LefLexer<R>,
    path: &Path,
    macro_name: &str,
    pin_name: &str,
    collect_ports: bool,
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
            if collect_ports {
                ports.push(Arc::new(parse_port_body(lexer, &ctx)?));
            } else {
                skip_metadata_group(lexer, None, &|| format!("{}PORT ", ctx()))?;
            }
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
        let missing = || format!("{}unterminated {} statement", ctx(), token.text);
        if collect_ports {
            lexer.consume_statement(missing)?;
        } else if token.is_keyword("BEGINEXT") {
            lexer.skip_extension(missing)?;
        } else {
            lexer.skip_statement(missing)?;
        }
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
    let mut token = lexer.required(|| format!("{}expected {what}", ctx()))?;
    // The shared lexer treats '+' as a DEF delimiter. In a LEF number it
    // can also be an exponent sign adjacent to the mantissa's E.
    if token.text.ends_with(['E', 'e'])
        && let Some(exponent) = lexer.next()?
    {
        if exponent.offset == token.offset + token.text.len() as u64
            && exponent.text.starts_with('+')
        {
            token.text.push_str(&exponent.text);
        } else {
            lexer.push(exponent);
        }
    }
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
// Public Rust API
// ---------------------------------------------------------------------------

/// Read cell macros, pin metadata, and supported pin-port geometry from plain
/// or gzipped LEF files. Files parse in parallel and merge in path order.
/// Duplicate macros use the last definition. An empty path list yields an
/// empty library.
///
/// All dimensions and coordinates are in LEF microns. Pass the same library to
/// [`crate::def::scan_instance_bboxes_with_library`] to scan a DEF using that
/// design's database units.
///
/// ```no_run
/// use std::path::{Path, PathBuf};
/// use open_eda_file_tools_core::{def, lef};
///
/// # fn main() -> Result<(), open_eda_file_tools_core::Error> {
/// let library = lef::parse_cell_lef(&[PathBuf::from("cells.lef")])?;
/// if let Some(cell) = library.macro_by_name("BUF") {
///     for pin in cell.pins() {
///         println!("{} {:?} {:?}", pin.name, pin.direction, pin.use_kind);
///     }
/// }
/// for bbox in def::scan_instance_bboxes_with_library(Path::new("chip.def"), &library)? {
///     println!("{:?}", bbox?);
/// }
/// # Ok(())
/// # }
/// ```
pub fn parse_cell_lef(paths: &[PathBuf]) -> Result<LibraryData, crate::Error> {
    Ok(parse_library(paths, None, true)?)
}

/// Read macro sizes, origins, classes and site names, plus top-level site
/// dimensions and technology layers. All dimensions are in microns.
///
/// With `include_pins`, collect pin names, directions and uses. Pin ports are
/// empty. With `include_pins` set to `false`, skip the pin bodies.
///
/// Files parse in parallel under the process-wide thread limit. The merge and
/// error order follow `paths`; the last macro or site definition wins and
/// keeps its first-seen position. The first declaration of each layer wins,
/// and the layer stack follows declaration order. An empty path list yields
/// an empty library.
pub fn parse_cell_lef_metadata(
    paths: &[PathBuf],
    include_pins: bool,
) -> Result<LibraryData, crate::Error> {
    let mut library = LibraryData::default();
    let mode = MacroMode::Metadata { include_pins };
    let files = parse_lef_files(paths, |path| parse_metadata_file(path, mode));
    for file in files {
        let file = file?;
        for data in file.macros {
            library.insert(data);
        }
        for site in file.sites {
            library.insert_site(site);
        }
        for layer in file.layers {
            library.layer_stack.insert(layer)?;
        }
    }
    Ok(library)
}

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
