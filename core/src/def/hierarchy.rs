use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{
    BATCH_SIZE, ColumnKind, ColumnSpec, DefScanner, InstanceBBox, Lexer, MacroDef, RowRenderer,
    expect_text, for_each_batch, load_lef_macros, parse_components_scaled, write_instance_csv_rows,
    write_instance_table_rows,
};
use crate::{Error, input};

type Bounds = [i64; 4];

/// A top-level DEF and its mapped block definitions, with shared parsed data.
#[derive(Clone, Debug)]
pub struct DefHierarchy(Arc<Design>);

#[derive(Debug)]
struct Design {
    definitions: Vec<Definition>,
    dbu_per_micron: i64,
    duplicate_macros: Vec<String>,
}

#[derive(Debug)]
struct Definition {
    path: PathBuf,
    die: Option<Bounds>,
    instances: Vec<LocalInstance>,
}

#[derive(Debug)]
struct LocalInstance {
    bbox: InstanceBBox,
    orientation: Orientation,
    child: Option<usize>,
}

struct Source {
    path: PathBuf,
    scanner: DefScanner,
    declared: usize,
    die: Option<Bounds>,
}

#[derive(PartialEq, Eq, Hash)]
enum SourcePath {
    Custom(OsString),
    File(PathBuf),
}

/// Read a hierarchy with explicit `(LEF macro name, child DEF path)` mappings.
///
/// Each referenced file is parsed once. Mapped blocks use their DEF `DIEAREA`
/// bounding rectangle for their footprint. Its lower-left corner anchors the
/// unrotated child. Other macros use LEF sizes. Coordinates are represented in
/// the least common multiple of all input DBU scales. Files use their canonical
/// path as identity and custom streams use the original path spelling.
/// Only placed components take part in expansion.
pub fn read_def_hierarchy(
    top: &Path,
    lefs: &[PathBuf],
    block_defs: &[(String, PathBuf)],
) -> Result<DefHierarchy, Error> {
    let mut sources = Vec::new();
    let mut paths = HashMap::new();
    let mut blocks = HashMap::new();
    open_source(top, &mut sources, &mut paths)?;
    for (name, path) in block_defs {
        if name.is_empty() || blocks.contains_key(name) {
            return Err(format!("empty or duplicate block DEF mapping for {name:?}").into());
        }
        let index = open_source(path, &mut sources, &mut paths)?;
        blocks.insert(name.clone(), index);
    }
    let mut common_dbu = 1;
    for source in &sources {
        let dbu = source.scanner.dbu_per_micron;
        common_dbu = (common_dbu / gcd(common_dbu, dbu))
            .checked_mul(dbu)
            .ok_or_else(|| {
                format!(
                    "common DEF DBU scale overflows at {}",
                    source.path.display()
                )
            })?;
    }
    for source in &mut sources {
        let scale = common_dbu / source.scanner.dbu_per_micron;
        if let Some(bounds) = &mut source.die {
            for coord in bounds {
                *coord = coord.checked_mul(scale).ok_or_else(|| {
                    format!(
                        "DIEAREA coordinate out of range in {}",
                        source.path.display()
                    )
                })?;
            }
        }
    }
    let (mut macros, duplicate_macros) = load_lef_macros(lefs, common_dbu)?;
    for (name, _) in block_defs {
        let source = &sources[blocks[name]];
        let [llx, lly, urx, ury] = source
            .die
            .ok_or_else(|| format!("block {name:?} has no DIEAREA in {}", source.path.display()))?;
        let overflow = || format!("DIEAREA size out of range in {}", source.path.display());
        macros.insert(
            name.clone(),
            MacroDef {
                width: urx.checked_sub(llx).ok_or_else(overflow)?,
                height: ury.checked_sub(lly).ok_or_else(overflow)?,
                _origin_x: 0,
                _origin_y: 0,
            },
        );
    }

    let mut definitions = Vec::with_capacity(sources.len());
    for Source {
        path,
        mut scanner,
        declared,
        die,
    } in sources
    {
        let scale = common_dbu / scanner.dbu_per_micron;
        let mut instances = Vec::new();
        let mut seen = 0;
        for_each_batch(
            || scanner.next_batch(BATCH_SIZE),
            |records, overlapped| {
                seen += records.len();
                let rows = crate::threads::install_for(overlapped, || {
                    parse_components_scaled(&records, &macros, scale)
                })?;
                instances.extend(rows.into_iter().map(|row| LocalInstance {
                    child: blocks.get(row.macro_name).copied(),
                    orientation: Orientation::from_name(row.orient),
                    bbox: row.to_bbox(),
                }));
                Ok(())
            },
        )
        .map_err(|err| format!("{}: {err}", path.display()))?;
        if !scanner.closed_components || seen != declared {
            if !scanner.closed_components {
                return Err(format!("{}: missing END COMPONENTS", path.display()).into());
            }
            return Err(format!(
                "{}: COMPONENTS declares {declared} records but contains {seen}",
                path.display()
            )
            .into());
        }
        definitions.push(Definition {
            path,
            die,
            instances,
        });
    }
    validate_acyclic(&definitions)?;
    Ok(DefHierarchy(Arc::new(Design {
        definitions,
        dbu_per_micron: common_dbu,
        duplicate_macros,
    })))
}

fn open_source(
    path: &Path,
    sources: &mut Vec<Source>,
    paths: &mut HashMap<SourcePath, usize>,
) -> io::Result<usize> {
    let context = |err: io::Error| io::Error::new(err.kind(), format!("{}: {err}", path.display()));
    let custom = SourcePath::Custom(path.as_os_str().to_os_string());
    if let Some(index) = paths.get(&custom) {
        return Ok(*index);
    }
    let (key, reader) = match input::open_custom(path).map_err(context)? {
        Some(reader) => (custom, reader),
        None => {
            let key = SourcePath::File(path.canonicalize().map_err(context)?);
            if let Some(index) = paths.get(&key) {
                return Ok(*index);
            }
            (key, input::open_file(path).map_err(context)?)
        }
    };
    let mut die = None;
    let (scanner, declared) = DefScanner::from_reader(reader, Some(&mut die)).map_err(context)?;
    if scanner.dbu_per_micron <= 0 {
        return Err(context(io::Error::new(
            io::ErrorKind::InvalidData,
            "DEF UNITS must be positive",
        )));
    }
    let index = sources.len();
    paths.insert(key, index);
    sources.push(Source {
        path: path.to_path_buf(),
        scanner,
        declared: declared.unwrap(),
        die,
    });
    Ok(index)
}

fn gcd(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

pub(super) fn read_die_area<R: Read>(lexer: &mut Lexer<R>) -> io::Result<Bounds> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid DEF DIEAREA");
    let mut bounds = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    let mut points = 0;
    loop {
        let token = lexer.next_token()?.ok_or_else(invalid)?;
        if token.text == ";" {
            break;
        }
        if token.text != "(" {
            return Err(invalid());
        }
        let x = lexer
            .next_token()?
            .and_then(|t| t.text.parse::<i64>().ok())
            .ok_or_else(invalid)?;
        let y = lexer
            .next_token()?
            .and_then(|t| t.text.parse::<i64>().ok())
            .ok_or_else(invalid)?;
        expect_text(lexer, ")")?;
        bounds = [
            bounds[0].min(x),
            bounds[1].min(y),
            bounds[2].max(x),
            bounds[3].max(y),
        ];
        points += 1;
    }
    if points < 2 || bounds[0] >= bounds[2] || bounds[1] >= bounds[3] {
        return Err(invalid());
    }
    Ok(bounds)
}

fn validate_acyclic(definitions: &[Definition]) -> Result<(), Error> {
    let mut visiting = vec![false; definitions.len()];
    let mut done = vec![false; definitions.len()];
    for id in 0..definitions.len() {
        if done[id] {
            continue;
        }
        let mut stack = vec![(id, definitions[id].instances.iter())];
        visiting[id] = true;
        while let Some((id, rows)) = stack.last_mut() {
            let Some(child) = rows.find_map(|row| row.child) else {
                visiting[*id] = false;
                done[*id] = true;
                stack.pop();
                continue;
            };
            if visiting[child] {
                let mut cycle = stack
                    .iter()
                    .skip_while(|(id, _)| *id != child)
                    .map(|(id, _)| definitions[*id].path.display().to_string())
                    .collect::<Vec<_>>();
                cycle.push(definitions[child].path.display().to_string());
                return Err(format!("cyclic DEF hierarchy: {}", cycle.join(" -> ")).into());
            }
            if !done[child] {
                visiting[child] = true;
                stack.push((child, definitions[child].instances.iter()));
            }
        }
    }
    Ok(())
}

impl DefHierarchy {
    /// Database units per micron for every returned bounding box.
    pub fn dbu_per_micron(&self) -> i64 {
        self.0.dbu_per_micron
    }

    /// Names defined more than once in the LEF inputs, in discovery order.
    pub fn duplicate_macro_names(&self) -> &[String] {
        &self.0.duplicate_macros
    }

    #[doc(hidden)]
    pub fn write_instances_text(
        &self,
        table: bool,
        path: Option<&Path>,
        columns: &[ColumnSpec],
        sequential: Option<&HashSet<String>>,
        clock: Option<&HashSet<String>>,
    ) -> Result<usize, String> {
        let mut instance_width = columns
            .iter()
            .find(|column| column.kind == ColumnKind::Instance)
            .map(|column| column.name.len());
        let mut macro_width = columns
            .iter()
            .find(|column| column.kind == ColumnKind::BaseCell)
            .map(|column| column.name.len());
        if table && (instance_width.is_some() || macro_width.is_some()) {
            for row in self.instances() {
                let row = row.map_err(|err| err.to_string())?;
                if let Some(width) = &mut instance_width {
                    *width = (*width).max(row.instance.len());
                }
                if let Some(width) = &mut macro_width {
                    *width = (*width).max(row.macro_name.len());
                }
            }
        }
        let stdout = io::stdout();
        let output: Box<dyn Write + '_> = match path {
            Some(path) => {
                Box::new(File::create(path).map_err(|err| format!("{}: {err}", path.display()))?)
            }
            None => Box::new(stdout.lock()),
        };
        let output = BufWriter::new(output);
        let dbu = self.dbu_per_micron();
        let emit = |out: &mut BufWriter<_>, render: &RowRenderer<'_>| self.render_rows(out, render);
        if table {
            write_instance_table_rows(
                dbu,
                columns,
                (instance_width, macro_width),
                sequential,
                clock,
                output,
                emit,
            )
        } else {
            write_instance_csv_rows(dbu, columns, sequential, clock, output, emit)
        }
    }

    fn render_rows(&self, out: &mut impl Write, render: &RowRenderer<'_>) -> Result<usize, String> {
        let mut count = 0;
        let mut text = String::with_capacity(64 * 1024);
        for row in self.instances() {
            let row = row.map_err(|err| err.to_string())?;
            render(&mut text, &row.as_component());
            count += 1;
            if text.len() >= 64 * 1024 {
                out.write_all(text.as_bytes())
                    .map_err(|err| err.to_string())?;
                text.clear();
            }
        }
        out.write_all(text.as_bytes())
            .map_err(|err| err.to_string())?;
        Ok(count)
    }

    /// Traverse placed instances in file order, visiting a block immediately
    /// before its descendants. Paths join component names with `/`. Coordinates
    /// and orientations are relative to the top DEF.
    pub fn instances(&self) -> HierarchicalInstanceIter {
        HierarchicalInstanceIter {
            hierarchy: self.clone(),
            stack: vec![Frame {
                definition: 0,
                next: 0,
                prefix: String::new(),
                transform: Transform {
                    orientation: Orientation::IDENTITY,
                    offset: [0, 0],
                },
            }],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Orientation([[i8; 2]; 2]);

const ORIENTATIONS: [(&str, Orientation); 8] = [
    ("N", Orientation::IDENTITY),
    ("S", Orientation([[-1, 0], [0, -1]])),
    ("E", Orientation([[0, 1], [-1, 0]])),
    ("W", Orientation([[0, -1], [1, 0]])),
    ("FN", Orientation([[-1, 0], [0, 1]])),
    ("FS", Orientation([[1, 0], [0, -1]])),
    ("FE", Orientation([[0, -1], [-1, 0]])),
    ("FW", Orientation([[0, 1], [1, 0]])),
];

impl Orientation {
    const IDENTITY: Self = Self([[1, 0], [0, 1]]);

    fn from_name(name: &str) -> Self {
        ORIENTATIONS
            .iter()
            .find(|(value, _)| *value == name)
            .unwrap()
            .1
    }

    fn name(self) -> &'static str {
        ORIENTATIONS
            .iter()
            .find(|(_, value)| *value == self)
            .unwrap()
            .0
    }

    fn compose(self, child: Self) -> Self {
        Self(std::array::from_fn(|i| {
            std::array::from_fn(|j| self.0[i][0] * child.0[0][j] + self.0[i][1] * child.0[1][j])
        }))
    }

    fn point(self, x: i64, y: i64) -> [i128; 2] {
        self.0
            .map(|[a, b]| i128::from(a) * i128::from(x) + i128::from(b) * i128::from(y))
    }

    fn bounds(self, [llx, lly, urx, ury]: Bounds) -> [i128; 4] {
        let [x1, y1] = self.point(llx, lly);
        let [x2, y2] = self.point(urx, ury);
        [x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2)]
    }
}

#[derive(Clone, Copy, Debug)]
struct Transform {
    orientation: Orientation,
    offset: [i128; 2],
}

#[derive(Debug)]
struct Frame {
    definition: usize,
    next: usize,
    prefix: String,
    transform: Transform,
}

/// Lazy traversal over a parsed DEF hierarchy. A coordinate error ends the iterator.
#[derive(Debug)]
pub struct HierarchicalInstanceIter {
    hierarchy: DefHierarchy,
    stack: Vec<Frame>,
}

impl Iterator for HierarchicalInstanceIter {
    type Item = Result<InstanceBBox, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let frame = self.stack.last_mut()?;
            let definitions = &self.hierarchy.0.definitions;
            let Some(row) = definitions[frame.definition].instances.get(frame.next) else {
                self.stack.pop();
                continue;
            };
            frame.next += 1;
            let instance = format!("{}{}", frame.prefix, row.bbox.instance);
            let orient = frame.transform.orientation.compose(row.orientation);
            let [x, y] = frame.transform.offset;
            let [llx, lly, urx, ury] = frame.transform.orientation.bounds([
                row.bbox.llx,
                row.bbox.lly,
                row.bbox.urx,
                row.bbox.ury,
            ]);
            let bounds = [llx + x, lly + y, urx + x, ury + y];
            let [Ok(llx), Ok(lly), Ok(urx), Ok(ury)] = bounds.map(i64::try_from) else {
                self.stack.clear();
                return Some(Err(format!(
                    "top-level coordinate out of range for {instance:?}"
                )
                .into()));
            };
            if let Some(child) = row.child {
                let [child_llx, child_lly, _, _] = orient.bounds(definitions[child].die.unwrap());
                self.stack.push(Frame {
                    definition: child,
                    next: 0,
                    prefix: format!("{instance}/"),
                    transform: Transform {
                        orientation: orient,
                        offset: [i128::from(llx) - child_llx, i128::from(lly) - child_lly],
                    },
                });
            }
            return Some(Ok(InstanceBBox {
                instance,
                macro_name: row.bbox.macro_name.clone(),
                status: row.bbox.status.clone(),
                orient: orient.name().to_owned(),
                llx,
                lly,
                urx,
                ury,
            }));
        }
    }
}
