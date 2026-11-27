use std::collections::HashMap;

/// Technology layer class from a LEF `LAYER` declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayerKind {
    Routing,
    Cut,
    Masterslice,
    Overlap,
    Implant,
    /// Unrecognized `TYPE` text, or an empty string when `TYPE` is absent.
    Other(String),
}

impl LayerKind {
    pub(super) fn from_lef(value: String) -> Self {
        match value.to_ascii_uppercase().as_str() {
            "ROUTING" => Self::Routing,
            "CUT" => Self::Cut,
            "MASTERSLICE" => Self::Masterslice,
            "OVERLAP" => Self::Overlap,
            "IMPLANT" => Self::Implant,
            _ => Self::Other(value),
        }
    }
}

/// Preferred routing direction from `DIRECTION`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerDirection {
    Horizontal,
    Vertical,
    Diag45,
    Diag135,
}

impl LayerDirection {
    pub(super) fn from_lef(value: &str) -> Option<Self> {
        match value.to_ascii_uppercase().as_str() {
            "HORIZONTAL" => Some(Self::Horizontal),
            "VERTICAL" => Some(Self::Vertical),
            "DIAG45" => Some(Self::Diag45),
            "DIAG135" => Some(Self::Diag135),
            _ => None,
        }
    }
}

/// A technology layer in first-declaration order. Dimensions are in microns.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerData {
    pub name: String,
    pub kind: LayerKind,
    /// Zero-based ordinal across the full stack.
    pub stack_index: u16,
    /// Zero-based ordinal among routing layers.
    pub routing_order: Option<u16>,
    /// Zero-based ordinal among cut layers.
    pub cut_order: Option<u16>,
    pub direction: Option<LayerDirection>,
    pub width_um: Option<f64>,
    pub min_width_um: Option<f64>,
    pub pitch_x_um: Option<f64>,
    pub pitch_y_um: Option<f64>,
    pub offset_x_um: Option<f64>,
    pub offset_y_um: Option<f64>,
    /// First unqualified `SPACING` value, if present.
    pub spacing_um: Option<f64>,
    /// Scalar `RESISTANCE RPERSQ` in ohms per square.
    pub sheet_resistance: Option<f64>,
    /// `RESISTANCE` in ohms per cut.
    pub cut_resistance: Option<f64>,
    pub thickness_um: Option<f64>,
}

impl LayerData {
    pub(super) fn new(name: String) -> Self {
        Self {
            name,
            kind: LayerKind::Other(String::new()),
            stack_index: 0,
            routing_order: None,
            cut_order: None,
            direction: None,
            width_um: None,
            min_width_um: None,
            pitch_x_um: None,
            pitch_y_um: None,
            offset_x_um: None,
            offset_y_um: None,
            spacing_um: None,
            sheet_resistance: None,
            cut_resistance: None,
            thickness_um: None,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct LayerStack {
    pub(super) layers: Vec<LayerData>,
    index: HashMap<String, u16>,
    routing_count: usize,
    cut_count: usize,
}

impl LayerStack {
    pub(super) fn get(&self, name: &str) -> Option<&LayerData> {
        self.index
            .get(name)
            .map(|&index| &self.layers[usize::from(index)])
    }

    pub(super) fn insert(&mut self, mut layer: LayerData) -> Result<(), String> {
        if self.index.contains_key(&layer.name) {
            return Ok(());
        }
        layer.stack_index = u16::try_from(self.layers.len())
            .map_err(|_| format!("too many LEF layers at {}", layer.name))?;
        // Each per-kind count is at most the stack index of this layer.
        match layer.kind {
            LayerKind::Routing => {
                layer.routing_order = Some(self.routing_count as u16);
                self.routing_count += 1;
            }
            LayerKind::Cut => {
                layer.cut_order = Some(self.cut_count as u16);
                self.cut_count += 1;
            }
            _ => {}
        }
        self.index.insert(layer.name.clone(), layer.stack_index);
        self.layers.push(layer);
        Ok(())
    }
}
