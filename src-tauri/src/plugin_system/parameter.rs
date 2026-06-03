//! Unified parameter architecture.
//!
//! Every parameter in the system — whether on an internal device, a VST3
//! plugin, or an AU instrument — is represented as a `Param`.
//!
//! ## Value model
//!
//! Parameters carry two parallel representations:
//!
//! - **Plain value**: in the parameter's natural unit (e.g. Hz, dB, 0–100%).
//! - **Normalised value**: always 0.0 … 1.0; used by the modulation system
//!   and by automation curves so that format differences are invisible.
//!
//! Conversion between representations is provided by `ParameterRange`.
//!
//! ## Modulation + automation
//!
//! Parameters record the IDs of modulation sources and automation lanes
//! that target them.  The graph runtime resolves these to float offsets each
//! block and delivers the final value to the node.

use std::collections::HashMap;

/// Stable, opaque string identifier for a parameter within a device.
/// Chosen by the device author; must be unique per device.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ParamKey(pub String);

impl ParamKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }
}

impl std::fmt::Display for ParamKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Defines the numeric range and mapping of a parameter.
#[derive(Debug, Clone)]
pub struct ParameterRange {
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub mapping: ParameterMapping,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParameterMapping {
    /// Plain linear mapping.
    Linear,
    /// Logarithmic mapping — useful for frequency parameters.
    Logarithmic,
    /// Decibel mapping (plain = dB, normalised ∈ [0,1]).
    Decibels { min_db: f32, max_db: f32 },
}

impl ParameterRange {
    pub fn linear(min: f32, max: f32, default: f32) -> Self {
        Self {
            min,
            max,
            default,
            mapping: ParameterMapping::Linear,
        }
    }

    pub fn percent() -> Self {
        Self::linear(0.0, 100.0, 100.0)
    }

    pub fn gain_db(min_db: f32, max_db: f32, default_db: f32) -> Self {
        Self {
            min: min_db,
            max: max_db,
            default: default_db,
            mapping: ParameterMapping::Decibels { min_db, max_db },
        }
    }

    /// Convert a plain value to normalised [0, 1].
    pub fn normalise(&self, plain: f32) -> f32 {
        match self.mapping {
            ParameterMapping::Linear => {
                ((plain - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
            }
            ParameterMapping::Logarithmic => {
                if self.min <= 0.0 || self.max <= 0.0 {
                    return 0.0;
                }
                let log_min = self.min.ln();
                let log_max = self.max.ln();
                ((plain.ln() - log_min) / (log_max - log_min)).clamp(0.0, 1.0)
            }
            ParameterMapping::Decibels { min_db, max_db } => {
                ((plain - min_db) / (max_db - min_db)).clamp(0.0, 1.0)
            }
        }
    }

    /// Convert a normalised [0, 1] value to plain.
    pub fn denormalise(&self, normalised: f32) -> f32 {
        let n = normalised.clamp(0.0, 1.0);
        match self.mapping {
            ParameterMapping::Linear => self.min + n * (self.max - self.min),
            ParameterMapping::Logarithmic => {
                if self.min <= 0.0 || self.max <= 0.0 {
                    return self.min;
                }
                let log_min = self.min.ln();
                let log_max = self.max.ln();
                (log_min + n * (log_max - log_min)).exp()
            }
            ParameterMapping::Decibels { min_db, max_db } => min_db + n * (max_db - min_db),
        }
    }
}

/// A single automatable, modulatable parameter.
#[derive(Debug, Clone)]
pub struct Param {
    pub key: ParamKey,
    pub name: String,
    pub range: ParameterRange,

    /// Current plain value (authoritative for non-modulated state).
    pub value: f32,

    /// Accumulated modulation offset (plain units) for the current block.
    /// Set to 0.0 at the start of each block; filled by the graph runtime.
    pub modulation_offset: f32,

    /// IDs of automation lanes that target this parameter.
    pub automation_lane_ids: Vec<String>,
}

impl Param {
    pub fn new(key: impl Into<String>, name: impl Into<String>, range: ParameterRange) -> Self {
        let range_default = range.default;
        Self {
            key: ParamKey::new(key),
            name: name.into(),
            value: range_default,
            range,
            modulation_offset: 0.0,
            automation_lane_ids: Vec::new(),
        }
    }

    /// The effective value for this block = plain value + modulation offset,
    /// clamped to the parameter range.
    #[inline]
    pub fn effective_value(&self) -> f32 {
        (self.value + self.modulation_offset).clamp(self.range.min, self.range.max)
    }

    pub fn normalised(&self) -> f32 {
        self.range.normalise(self.value)
    }

    pub fn set_normalised(&mut self, n: f32) {
        self.value = self.range.denormalise(n);
    }
}

/// The complete set of parameters for one device.
#[derive(Debug, Default, Clone)]
pub struct ParameterMap {
    params: HashMap<ParamKey, Param>,
    /// Ordered list of keys for consistent iteration / display.
    order: Vec<ParamKey>,
}

impl ParameterMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, param: Param) {
        let key = param.key.clone();
        if !self.params.contains_key(&key) {
            self.order.push(key.clone());
        }
        self.params.insert(key, param);
    }

    pub fn get(&self, key: &ParamKey) -> Option<&Param> {
        self.params.get(key)
    }

    pub fn get_mut(&mut self, key: &ParamKey) -> Option<&mut Param> {
        self.params.get_mut(key)
    }

    pub fn set_value(&mut self, key: &ParamKey, value: f32) -> bool {
        if let Some(p) = self.params.get_mut(key) {
            p.value = value.clamp(p.range.min, p.range.max);
            true
        } else {
            false
        }
    }

    pub fn set_normalised(&mut self, key: &ParamKey, normalised: f32) -> bool {
        if let Some(p) = self.params.get_mut(key) {
            p.set_normalised(normalised);
            true
        } else {
            false
        }
    }

    /// Ordered parameter iteration.
    pub fn iter(&self) -> impl Iterator<Item = &Param> {
        self.order.iter().filter_map(|k| self.params.get(k))
    }

    pub fn len(&self) -> usize {
        self.params.len()
    }

    pub fn is_empty(&self) -> bool {
        self.params.is_empty()
    }
}
