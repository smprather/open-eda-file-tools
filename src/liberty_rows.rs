use std::sync::Arc;

use arrow::array::{Array, ArrayRef, Float64Builder, Int64Builder, LargeStringBuilder};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::error::ArrowError;
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use open_eda_file_tools_core::liberty::{TimingTableData, table_position};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

const BATCH_ROWS: usize = 4096;

#[derive(Clone, Copy)]
pub(crate) struct TableRows<'a> {
    pub(crate) cell: &'a str,
    pub(crate) pin: &'a str,
    pub(crate) related_pin: Option<&'a str>,
    pub(crate) category: Option<&'a str>,
    pub(crate) when: Option<&'a str>,
    pub(crate) table: &'a TimingTableData,
}

impl<'a> TableRows<'a> {
    pub(crate) fn standalone(table: &'a TimingTableData) -> Self {
        Self {
            cell: "",
            pin: "",
            related_pin: None,
            category: None,
            when: None,
            table,
        }
    }

    fn strings(&self) -> [Option<&str>; 6] {
        [
            Some(self.cell),
            Some(self.pin),
            self.related_pin,
            self.category,
            self.when,
            Some(&self.table.name),
        ]
    }

    fn points(&self) -> impl Iterator<Item = ([Option<f64>; 3], [usize; 3], f64)> {
        self.table.values.iter().enumerate().map(|(flat, &value)| {
            let (row, col, depth) = table_position(self.table, flat);
            let axes = [
                self.table.index_1.get(row).copied(),
                self.table.index_2.get(col).copied(),
                self.table.index_3.get(depth).copied(),
            ];
            (axes, [row, col, depth], value)
        })
    }

    pub(crate) fn append_dicts(
        &self,
        py: Python<'_>,
        rows: &Bound<'_, PyList>,
        category: &str,
    ) -> PyResult<()> {
        let names = column_names(category);
        let strings = self.strings();
        for (axes, positions, value) in self.points() {
            let row = PyDict::new(py);
            for (name, value) in names[..6].iter().zip(strings) {
                row.set_item(*name, value)?;
            }
            for (name, value) in names[6..9].iter().zip(axes) {
                row.set_item(*name, value)?;
            }
            for (name, value) in names[9..12].iter().zip(positions) {
                row.set_item(*name, value)?;
            }
            row.set_item("value", value)?;
            rows.append(row)?;
        }
        Ok(())
    }
}

fn column_names(category: &str) -> [&str; 13] {
    [
        "cell",
        "pin",
        "related_pin",
        category,
        "when",
        "table",
        "index_1",
        "index_2",
        "index_3",
        "row",
        "col",
        "depth",
        "value",
    ]
}

pub(crate) struct TableIpc {
    schema: Arc<Schema>,
    writer: StreamWriter<Vec<u8>>,
    strings: [LargeStringBuilder; 6],
    axes: [Float64Builder; 3],
    positions: [Int64Builder; 3],
    values: Float64Builder,
    rows: usize,
    seen: u16,
}

impl TableIpc {
    pub(crate) fn new(category: &str) -> Result<Self, ArrowError> {
        let types = [const { DataType::LargeUtf8 }; 6]
            .into_iter()
            .chain([const { DataType::Float64 }; 3])
            .chain([const { DataType::Int64 }; 3])
            .chain([DataType::Float64]);
        let schema = Arc::new(Schema::new(
            column_names(category)
                .into_iter()
                .zip(types)
                .map(|(name, dtype)| Field::new(name, dtype, true))
                .collect::<Vec<_>>(),
        ));
        Ok(Self {
            writer: StreamWriter::try_new(Vec::new(), &schema)?,
            schema,
            strings: std::array::from_fn(|_| {
                LargeStringBuilder::with_capacity(BATCH_ROWS, BATCH_ROWS * 16)
            }),
            axes: std::array::from_fn(|_| Float64Builder::with_capacity(BATCH_ROWS)),
            positions: std::array::from_fn(|_| Int64Builder::with_capacity(BATCH_ROWS)),
            values: Float64Builder::with_capacity(BATCH_ROWS),
            rows: 0,
            seen: 0,
        })
    }

    pub(crate) fn append(&mut self, rows: TableRows<'_>) -> Result<(), ArrowError> {
        let strings = rows.strings();
        for (axes, positions, value) in rows.points() {
            for (column, value) in self.strings.iter_mut().zip(strings) {
                column.append_option(value);
            }
            for (column, value) in self.axes.iter_mut().zip(axes) {
                column.append_option(value);
            }
            for (column, value) in self.positions.iter_mut().zip(positions) {
                column.append_value(value as i64);
            }
            self.values.append_value(value);
            self.rows += 1;
            if self.rows == BATCH_ROWS {
                self.flush()?;
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), ArrowError> {
        if self.rows == 0 {
            return Ok(());
        }
        let columns: Vec<ArrayRef> = self
            .strings
            .iter_mut()
            .map(|column| Arc::new(column.finish()) as ArrayRef)
            .chain(
                self.axes
                    .iter_mut()
                    .map(|column| Arc::new(column.finish()) as ArrayRef),
            )
            .chain(
                self.positions
                    .iter_mut()
                    .map(|column| Arc::new(column.finish()) as ArrayRef),
            )
            .chain(std::iter::once(Arc::new(self.values.finish()) as ArrayRef))
            .collect();
        for (index, column) in columns.iter().enumerate() {
            if column.null_count() < self.rows {
                self.seen |= 1 << index;
            }
        }
        self.writer
            .write(&RecordBatch::try_new(Arc::clone(&self.schema), columns)?)?;
        self.rows = 0;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<Option<(Vec<u8>, u16)>, ArrowError> {
        self.flush()?;
        if self.seen == 0 {
            Ok(None)
        } else {
            Ok(Some((self.writer.into_inner()?, self.seen)))
        }
    }
}
