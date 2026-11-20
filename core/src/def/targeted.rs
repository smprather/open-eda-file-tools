//! One-pass placement and selected clock connectivity scan.

use std::borrow::Cow;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use super::{
    BATCH_SIZE, ByteTokens, InstanceBBox, Lexer, MacroDef, RawRecord, expect_semicolon,
    macro_sizes_in_dbu, open_input, parse_components_with_fallback, parse_connection_list,
    parse_def_units, skip_unrequested_def_data, unescaped,
};
use crate::Error;
use crate::connectivity::Conn;

type DefLexer = Lexer<Box<dyn io::Read + Send>>;

/// One connection on a selected DEF net. A top-level port has no instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefConnection {
    pub instance: Option<String>,
    pub pin: String,
}

/// Connections on one selected regular NETS record, in DEF file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockNetRecord {
    pub name: String,
    pub connections: Vec<DefConnection>,
}

/// Results of a targeted scan. Instance bboxes are streamed to the callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetedDefScan {
    pub dbu_per_micron: i64,
    pub clock_nets: Vec<ClockNetRecord>,
}

/// Stream the placed instance boxes and collect connections for selected regular
/// clock nets in one pass over the DEF. Both plain DEF and gzip are supported.
/// Coordinates are in the DEF's database units.
///
/// LEF sizes take precedence. If a macro is absent, the scan uses a square
/// marker with sides four times the smallest positive STEP in the preceding
/// ROW statements. A missing macro without a usable ROW pitch is an error.
///
/// The callback runs in COMPONENTS order and can stop the scan by returning
/// an error. Clock net names are compared without DEF escapes and returned
/// with their source spelling. Routing, unselected nets, and SPECIALNETS are
/// skipped without materializing record data.
pub fn scan_clock_nets_and_instances(
    path: &Path,
    lefs: &[PathBuf],
    clock_nets: &HashSet<String>,
    mut visit: impl FnMut(InstanceBBox) -> Result<(), Error>,
) -> Result<TargetedDefScan, Error> {
    let mut lexer = Lexer::new(open_input(path)?);
    let wanted: HashSet<Cow<'_, str>> = clock_nets.iter().map(|name| unescaped(name)).collect();
    let mut dbu = None;
    let mut site_pitch = None;
    let mut seen_components = false;
    let mut clock_nets = Vec::new();
    while let Some(token) = lexer.next_token()? {
        if token.is_keyword("UNITS") {
            if seen_components {
                return Err(invalid("DEF UNITS must precede COMPONENTS").into());
            }
            parse_def_units(&mut lexer, &mut dbu)?;
            if dbu.is_some_and(|value| value <= 0) {
                return Err(invalid("DEF UNITS must be positive").into());
            }
        } else if token.is_keyword("ROW") && !seen_components {
            if let Some(pitch) = row_pitch(&mut lexer, token.offset)? {
                site_pitch = Some(site_pitch.map_or(pitch, |prev: i64| prev.min(pitch)));
            }
        } else if token.is_keyword("COMPONENTS") {
            if seen_components {
                return Err(invalid("multiple DEF COMPONENTS sections").into());
            }
            scan_instances(
                &mut lexer,
                lefs,
                dbu.unwrap_or(1000),
                site_pitch,
                &mut visit,
            )?;
            seen_components = true;
        } else if token.is_keyword("NETS") {
            scan_nets(&mut lexer, &wanted, &mut clock_nets)?;
        } else if token.is_keyword("SPECIALNETS") {
            let count = section_count(&mut lexer, "SPECIALNETS")?;
            let mut seen = 0;
            while next_record(&mut lexer, "SPECIALNETS")?.is_some() {
                lexer.skip_until_semicolon()?;
                seen += 1;
            }
            check_count("SPECIALNETS", count, seen)?;
        } else if token.is_keyword("END") {
            if lexer
                .next_token()?
                .is_some_and(|token| token.is_keyword("DESIGN"))
            {
                break;
            }
        } else {
            skip_unrequested_def_data(&mut lexer, &token)?;
        }
    }
    if !seen_components {
        return Err(invalid("DEF missing COMPONENTS section").into());
    }
    Ok(TargetedDefScan {
        dbu_per_micron: dbu.unwrap_or(1000),
        clock_nets,
    })
}

fn scan_instances(
    lexer: &mut DefLexer,
    lefs: &[PathBuf],
    dbu: i64,
    pitch: Option<i64>,
    visit: &mut impl FnMut(InstanceBBox) -> Result<(), Error>,
) -> Result<(), Error> {
    let expected = section_count(lexer, "COMPONENTS")?;
    let library = crate::lef::parse_cell_lef_metadata(lefs, false)?;
    let macros = macro_sizes_in_dbu(&library, dbu)?;
    let fallback = pitch
        .map(|pitch| {
            let size = pitch
                .checked_mul(4)
                .ok_or_else(|| invalid("DEF ROW marker size overflows"))?;
            Ok::<_, io::Error>(MacroDef {
                width: size,
                height: size,
                _origin_x: 0,
                _origin_y: 0,
            })
        })
        .transpose()?;
    let mut seen = 0;
    let mut batch = Vec::with_capacity(expected.min(BATCH_SIZE));
    loop {
        while batch.len() < BATCH_SIZE {
            let Some(offset) = next_record(lexer, "COMPONENTS")? else {
                break;
            };
            let mut bytes = Vec::with_capacity(96);
            bytes.push(b'-');
            lexer.read_until_semicolon(&mut bytes)?;
            batch.push(RawRecord { bytes, offset });
        }
        let complete = batch.len() < BATCH_SIZE;
        seen += batch.len();
        for row in parse_components_with_fallback(&batch, &macros, fallback.as_ref())? {
            visit(row.to_bbox())?;
        }
        if complete {
            return Ok(check_count("COMPONENTS", expected, seen)?);
        }
        batch.clear();
    }
}

fn scan_nets(
    lexer: &mut DefLexer,
    wanted: &HashSet<Cow<'_, str>>,
    output: &mut Vec<ClockNetRecord>,
) -> Result<(), Error> {
    let expected = section_count(lexer, "NETS")?;
    let mut seen = 0;
    while let Some(offset) = next_record(lexer, "NETS")? {
        let name = lexer
            .next_token()?
            .filter(|token| !token.is_keyword(";"))
            .ok_or_else(|| invalid("missing DEF net name"))?
            .text;
        seen += 1;
        if !wanted.contains(&unescaped(&name)) {
            lexer.skip_until_semicolon()?;
            continue;
        }
        let mut bytes = Vec::new();
        if lexer.read_until_plus_or_semicolon(&mut bytes)? {
            lexer.skip_until_semicolon()?;
        }
        let connections = parse_connection_list(&bytes, offset)?
            .into_iter()
            .map(|connection| match connection {
                Conn::Port(pin) => DefConnection {
                    instance: None,
                    pin: pin.to_owned(),
                },
                Conn::Pin { instance, pin } => DefConnection {
                    instance: Some(instance.to_owned()),
                    pin: pin.to_owned(),
                },
            })
            .collect();
        output.push(ClockNetRecord { name, connections });
    }
    Ok(check_count("NETS", expected, seen)?)
}

fn row_pitch(lexer: &mut DefLexer, offset: u64) -> Result<Option<i64>, Error> {
    let mut record = Vec::new();
    lexer.read_until_semicolon(&mut record)?;
    let mut tokens = ByteTokens::new(&record);
    for _ in 0..5 {
        tokens.next();
    }
    if !tokens
        .next()
        .is_some_and(|token| token.eq_ignore_ascii_case(b"DO"))
    {
        return Ok(None);
    }
    tokens.next();
    if !tokens
        .next()
        .is_some_and(|token| token.eq_ignore_ascii_case(b"BY"))
    {
        return Err(invalid(format!("expected BY in ROW near DEF byte {offset}")).into());
    }
    tokens.next();
    if !tokens
        .next()
        .is_some_and(|token| token.eq_ignore_ascii_case(b"STEP"))
    {
        return Ok(None);
    }
    let x = tokens.next_i64(offset, "ROW x step")?.unwrap_or(0);
    let y = tokens.next_i64(offset, "ROW y step")?.unwrap_or(0);
    Ok([x, y].into_iter().filter(|&value| value > 0).min())
}

fn next_record(lexer: &mut DefLexer, section: &str) -> io::Result<Option<u64>> {
    let token = lexer.next_token()?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("missing END {section}"),
        )
    })?;
    if token.is_keyword("-") {
        return Ok(Some(token.offset));
    }
    if token.is_keyword("END") {
        expect_end(lexer, section, token.offset)?;
        return Ok(None);
    }
    Err(invalid(format!(
        "expected {section} record near DEF byte {}",
        token.offset
    )))
}

fn expect_end(lexer: &mut DefLexer, section: &str, offset: u64) -> io::Result<()> {
    if !lexer
        .next_token()?
        .is_some_and(|token| token.is_keyword(section))
    {
        return Err(invalid(format!(
            "expected END {section} near DEF byte {offset}"
        )));
    }
    Ok(())
}

fn section_count(lexer: &mut DefLexer, section: &str) -> io::Result<usize> {
    let count = lexer
        .next_token()?
        .and_then(|token| token.text.parse().ok())
        .ok_or_else(|| invalid(format!("invalid DEF {section} count")))?;
    expect_semicolon(lexer)?;
    Ok(count)
}

fn check_count(section: &str, expected: usize, seen: usize) -> io::Result<()> {
    if expected != seen {
        return Err(invalid(format!(
            "DEF {section} declares {expected} records but contains {seen}"
        )));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
