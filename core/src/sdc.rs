//! Clock extraction from Synopsys Design Constraints using Tcl.
//!
//! Evaluation requires system `tclsh` 8.6 or newer. Set `TCLSH` to override
//! interpreter discovery. The SDC runs in a safe interpreter with stubbed
//! constraint commands so variables, control flow, and command substitution
//! are evaluated by Tcl.

mod tcl;

use std::collections::HashMap;
use std::path::Path;

use crate::Error;

use tcl::Word;

/// One clock defined by `create_clock` or `create_generated_clock`.
///
/// Period and waveform times use the SDC's units (typically nanoseconds).
#[derive(Debug, Clone, PartialEq)]
pub struct Clock {
    /// Explicit name, or a default derived from the first target.
    pub name: String,
    /// Specified period, or the period derived from a known master clock.
    pub period: Option<f64>,
    /// Rising and falling edge times of a primary clock.
    pub waveform: Vec<f64>,
    /// Target endpoints, e.g. `port:clk`, `pin:u/Q`, `net:clk`, or `cell:u`.
    pub sources: Vec<String>,
    /// Name of the generated clock's master, if known.
    pub generated_from: Option<String>,
    /// Frequency divider for a generated clock.
    pub divide_by: Option<i64>,
    /// Frequency multiplier for a generated clock.
    pub multiply_by: Option<i64>,
    /// Master-clock edges used by a generated clock.
    pub edges: Vec<f64>,
    /// Whether a generated clock inverts its master.
    pub invert: bool,
}

/// The constraint commands recorded by an evaluation, in execution order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    /// Each record starts with the command name, followed by its Tcl words.
    /// Unsupported commands are recorded with `unknown` before their name.
    pub records: Vec<Vec<String>>,
    /// Tcl's script error, if any. Records issued before the error are kept.
    pub error: Option<String>,
}

/// Evaluate a plain or gzip-compressed SDC with symbolic queries and return its raw Tcl records.
///
/// Interpreter startup and protocol failures are returned as `Err`. Script
/// failures are returned with the partial records in [`Evaluation`].
pub fn evaluate(path: impl AsRef<Path>) -> Result<Evaluation, Error> {
    let result = tcl::evaluate(path.as_ref())?;
    Ok(Evaluation {
        records: result
            .records
            .into_iter()
            .map(|record| record.into_iter().map(|word| word.text).collect())
            .collect(),
        error: result.error,
    })
}

/// Return the clocks a plain or gzip-compressed SDC creates, in execution order.
///
/// Generated clocks can resolve masters defined earlier in the script. Any
/// interpreter, script, or clock-option error includes failure context.
pub fn clocks(path: impl AsRef<Path>) -> Result<Vec<Clock>, Error> {
    let path = path.as_ref();
    let result = tcl::evaluate(path)?;
    let contextualize = |error| Error::Message(format!("{}: {error}", path.display()));
    if let Some(error) = result.error {
        return Err(contextualize(error));
    }
    let mut clocks = Vec::new();
    for record in result.records {
        let Some((command, args)) = record.split_first() else {
            continue;
        };
        let clock = match command.text.as_str() {
            "create_clock" => primary_clock(args),
            "create_generated_clock" => generated_clock(args, &clocks),
            _ => continue,
        };
        clocks.push(clock.map_err(contextualize)?);
    }
    Ok(clocks)
}

struct Options<'a> {
    values: HashMap<&'a str, &'a Word>,
    sources: Vec<String>,
    invert: bool,
}

impl Options<'_> {
    fn text(&self, option: &str) -> Option<&str> {
        self.values.get(option).map(|word| word.text.as_str())
    }

    fn name(&self, default: &str) -> String {
        self.text("-name")
            .filter(|name| !name.is_empty())
            .or_else(|| {
                self.sources.first().map(|source| {
                    source
                        .split_once(':')
                        .map_or(source.as_str(), |(_, name)| name)
                })
            })
            .unwrap_or(default)
            .into()
    }
}

fn split_options<'a>(words: &'a [Word], value_options: &[&str]) -> Result<Options<'a>, String> {
    let mut options = Options {
        values: HashMap::new(),
        sources: Vec::new(),
        invert: false,
    };
    let mut words = words.iter();
    while let Some(word) = words.next() {
        let text = word.text.as_str();
        if value_options.contains(&text) {
            let value = words
                .next()
                .ok_or_else(|| format!("{text} requires a value"))?;
            options.values.insert(text, value);
        } else if text == "-invert" {
            options.invert = true;
        } else if !text.starts_with('-') {
            if word.endpoints.is_empty() {
                options.sources.push(word.text.clone());
            } else {
                options.sources.extend(word.endpoints.iter().cloned());
            }
        }
    }
    Ok(options)
}

fn number(option: &str, text: &str) -> Result<f64, String> {
    text.trim()
        .parse()
        .map_err(|_| format!("{option}: expected a number, got {text:?}"))
}

fn numbers(option: &str, text: &str) -> Result<Vec<f64>, String> {
    text.split_whitespace()
        .map(|part| number(option, part))
        .collect()
}

fn ratio(options: &Options<'_>, option: &str) -> Result<Option<i64>, String> {
    let Some(text) = options.text(option) else {
        return Ok(None);
    };
    let value = number(option, text)?.trunc();
    let max_exclusive = -(i64::MIN as f64);
    if !(i64::MIN as f64..max_exclusive).contains(&value) {
        return Err(format!(
            "{option}: expected a finite i64 value, got {text:?}"
        ));
    }
    Ok(Some(value as i64))
}

fn primary_clock(words: &[Word]) -> Result<Clock, String> {
    let options = split_options(words, &["-name", "-period", "-waveform", "-comment"])?;
    let period = options
        .text("-period")
        .map(|text| number("-period", text))
        .transpose()?;
    let waveform = match options.text("-waveform") {
        Some(text) => numbers("-waveform", text)?,
        None => period.map_or_else(Vec::new, |period| vec![0.0, period / 2.0]),
    };
    Ok(Clock {
        name: options.name("clock"),
        period,
        waveform,
        sources: options.sources,
        generated_from: None,
        divide_by: None,
        multiply_by: None,
        edges: Vec::new(),
        invert: false,
    })
}

fn master_clock<'a>(options: &Options<'_>, clocks: &'a [Clock]) -> Option<&'a Clock> {
    if let Some(name) = options
        .text("-master_clock")
        .filter(|name| !name.is_empty())
        && let Some(clock) = clocks.iter().find(|clock| clock.name == name)
    {
        return Some(clock);
    }
    if let Some(source) = options.values.get("-source") {
        let mut matches = clocks.iter().filter(|clock| {
            clock.sources.iter().any(|endpoint| {
                if source.endpoints.is_empty() {
                    source.text.split_whitespace().any(|part| part == endpoint)
                } else {
                    source.endpoints.contains(endpoint)
                }
            })
        });
        if let Some(clock) = matches.next()
            && matches.next().is_none()
        {
            return Some(clock);
        }
    }
    match clocks {
        [only] => Some(only),
        _ => None,
    }
}

fn generated_clock(words: &[Word], clocks: &[Clock]) -> Result<Clock, String> {
    let options = split_options(
        words,
        &[
            "-name",
            "-source",
            "-master_clock",
            "-divide_by",
            "-multiply_by",
            "-edges",
            "-edge_shift",
            "-comment",
        ],
    )?;
    let master = master_clock(&options, clocks);
    let divide_by = ratio(&options, "-divide_by")?;
    let multiply_by = ratio(&options, "-multiply_by")?;
    let period = master.and_then(|clock| clock.period).and_then(|period| {
        if let Some(divide) = divide_by.filter(|&ratio| ratio != 0) {
            Some(period * divide as f64)
        } else {
            multiply_by
                .filter(|&ratio| ratio != 0)
                .map(|multiply| period / multiply as f64)
        }
    });
    let generated_from = master
        .map(|clock| clock.name.as_str())
        .or_else(|| options.text("-master_clock"))
        .map(str::to_owned);
    let edges = numbers("-edges", options.text("-edges").unwrap_or_default())?;
    Ok(Clock {
        name: options.name("generated"),
        period,
        waveform: Vec::new(),
        sources: options.sources,
        generated_from,
        divide_by,
        multiply_by,
        edges,
        invert: options.invert,
    })
}
