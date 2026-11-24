use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use crate::Error;

// Tcl reads this single trusted command before the command itself consumes the
// remaining stdin bytes as the SDC script.
const PRELUDE: &str = concat!("namespace eval :: {\n", include_str!("evaluate.tcl"), "}\n");

pub(super) struct Word {
    pub(super) text: String,
    pub(super) endpoints: Vec<String>,
}

pub(super) struct Evaluation {
    pub(super) records: Vec<Vec<Word>>,
    pub(super) error: Option<String>,
}

fn launch(program: &OsStr) -> io::Result<Child> {
    Command::new(program)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

fn io_error(error: io::Error, context: impl std::fmt::Display) -> Error {
    io::Error::new(error.kind(), format!("{context}: {error}")).into()
}

fn launch_tclsh() -> Result<Child, Error> {
    if let Some(program) = std::env::var_os("TCLSH").filter(|path| !path.is_empty()) {
        return launch(&program).map_err(|error| {
            io_error(
                error,
                format_args!("cannot run {}", program.to_string_lossy()),
            )
        });
    }
    for name in ["tclsh8.6", "tclsh"] {
        match launch(OsStr::new(name)) {
            Ok(child) => return Ok(child),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
                ) => {}
            Err(error) => return Err(io_error(error, format_args!("cannot run {name}"))),
        }
    }
    Err(Error::Message(
        "sdc-tools requires Tcl >= 8.6 (tclsh); install it or set TCLSH to its path".into(),
    ))
}

pub(super) fn evaluate(path: &Path) -> Result<Evaluation, Error> {
    let mut input = crate::input::open(path).map_err(|error| io_error(error, path.display()))?;
    let mut child = launch_tclsh()?;
    let mut stdin = child.stdin.take().expect("Tcl stdin is piped");
    let written = match stdin
        .write_all(PRELUDE.as_bytes())
        .and_then(|()| io::copy(&mut input, &mut stdin))
    {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => {
            // Tcl evaluates only after EOF. Stop it before closing a partial script.
            let _ = child.kill();
            let _ = child.wait();
            return Err(io_error(
                error,
                format_args!("{}: feeding SDC to tclsh", path.display()),
            ));
        }
        result => result,
    };
    drop(stdin);
    let output = child
        .wait_with_output()
        .map_err(|error| io_error(error, format_args!("{}: tclsh failed", path.display())))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        if output.status.code() == Some(3) {
            return Err(Error::Message(if detail.is_empty() {
                "Tcl >= 8.6 required".into()
            } else {
                detail.into()
            }));
        }
        let detail = if detail.is_empty() {
            output.status.to_string()
        } else {
            detail.into()
        };
        return Err(Error::Message(format!(
            "{}: tclsh failed: {detail}",
            path.display()
        )));
    }
    written.map_err(|error| {
        io_error(
            error,
            format_args!("{}: feeding SDC to tclsh", path.display()),
        )
    })?;
    decode(&output.stdout).ok_or_else(|| {
        Error::Message(format!(
            "{}: unexpected tclsh output: {:?}",
            path.display(),
            String::from_utf8_lossy(&output.stdout[..output.stdout.len().min(200)])
        ))
    })
}

fn read_line<'a>(input: &mut &'a [u8]) -> Option<&'a [u8]> {
    let newline = input.iter().position(|&byte| byte == b'\n')?;
    let line = &input[..newline];
    *input = &input[newline + 1..];
    Some(line)
}

fn read_count(input: &mut &[u8]) -> Option<usize> {
    std::str::from_utf8(read_line(input)?).ok()?.parse().ok()
}

fn read_field(input: &mut &[u8]) -> Option<String> {
    let len = read_count(input)?;
    let (field, remaining) = input.split_at_checked(len)?;
    *input = remaining;
    Some(std::str::from_utf8(field).ok()?.into())
}

fn read_record(input: &mut &[u8]) -> Option<Vec<Word>> {
    let mut record = Vec::new();
    for _ in 0..read_count(input)? {
        let text = read_field(input)?;
        let mut endpoints = Vec::new();
        for _ in 0..read_count(input)? {
            endpoints.push(read_field(input)?);
        }
        record.push(Word { text, endpoints });
    }
    Some(record)
}

fn decode(mut input: &[u8]) -> Option<Evaluation> {
    let error = match read_line(&mut input)? {
        b"ok" => None,
        b"error" => Some(read_field(&mut input)?),
        _ => return None,
    };
    let mut records = Vec::new();
    for _ in 0..read_count(&mut input)? {
        records.push(read_record(&mut input)?);
    }
    input.is_empty().then_some(Evaluation { records, error })
}
