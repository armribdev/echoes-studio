//! What each controlled plugin parameter receives during one block (ADR-021).
//!
//! Format-agnostic: plugin wrappers (CLAP, VST3, instruments) own a
//! [`PluginParamControls`] injected at compile time, the scheduler feeds it
//! through the `AudioNode` modulation/automation methods, and each wrapper
//! turns it into its format's events.  All storage is pre-allocated.
//!
//! - **Modulation** is an offset around the parameter's current value, in
//!   normalised units.  It never moves the parameter itself.
//! - **Automation** is the absolute normalised value the parameter must take.
//! - A parameter the plugin can only automate is **value-modulated**: the host
//!   sends `base + offset` as absolute values (ADR-021 amendment).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::audio_graph::node::MAX_CONTROL_POINTS;

const PLUGIN_PARAM_PREFIX: &str = "plugin_native:";

/// Graph parameter name for a plugin's native parameter id.
pub fn plugin_param_name(native_id: u32) -> String {
    format!("{PLUGIN_PARAM_PREFIX}{native_id}")
}

pub fn parse_plugin_param_name(name: &str) -> Option<u32> {
    name.strip_prefix(PLUGIN_PARAM_PREFIX)?.parse().ok()
}

/// Values delivered for one block: an optional value at the block start, then
/// values at interior control-grid offsets (ascending).  Deliveries at the same
/// offset from several routes add up.
pub struct BlockPoints {
    start: Option<f32>,
    points: [(u32, f32); MAX_CONTROL_POINTS],
    count: usize,
}

impl BlockPoints {
    fn new() -> Self {
        Self {
            start: None,
            points: [(0, 0.0); MAX_CONTROL_POINTS],
            count: 0,
        }
    }

    fn clear(&mut self) {
        self.start = None;
        self.count = 0;
    }

    fn add_start(&mut self, value: f32) {
        *self.start.get_or_insert(0.0) += value;
    }

    fn add_at(&mut self, sample_offset: u32, value: f32) {
        let used = &mut self.points[..self.count];
        if let Some(point) = used.iter_mut().find(|(offset, _)| *offset == sample_offset) {
            point.1 += value;
        } else if self.count < MAX_CONTROL_POINTS {
            self.points[self.count] = (sample_offset, value);
            self.count += 1;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.start.is_none() && self.count == 0
    }

    /// `(sample_offset, value)` in delivery order: block start first.
    pub fn iter(&self) -> impl Iterator<Item = (u32, f32)> + '_ {
        self.start
            .map(|value| (0, value))
            .into_iter()
            .chain(self.points[..self.count].iter().copied())
    }
}

/// Plain range and capabilities of a native parameter, as the plugin reports them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PluginParamRange {
    pub min: f32,
    pub max: f32,
    pub modulatable: bool,
    pub automatable: bool,
}

impl PluginParamRange {
    /// Used when the plugin did not report the parameter: values pass through as-is.
    pub const UNKNOWN: Self = Self {
        min: 0.0,
        max: 1.0,
        modulatable: false,
        automatable: false,
    };

    pub fn denormalise(&self, normalised: f32) -> f32 {
        self.min + normalised * (self.max - self.min)
    }

    pub fn normalise(&self, plain: f32) -> f32 {
        let span = self.max - self.min;
        if span > 0.0 {
            ((plain - self.min) / span).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Converts a normalised offset into the plugin's plain units.
    pub fn plain_offset(&self, normalised_offset: f32) -> f32 {
        normalised_offset * (self.max - self.min)
    }
}

pub struct PluginParamControl {
    pub native_id: u32,
    /// At least one modulation route targets this parameter (not only automation).
    pub modulated: bool,
    pub range: PluginParamRange,
    /// Base value for formats without native modulation (VST3): the host adds
    /// the modulation offset to it.  Shared with `LiveParams`.
    pub base: Arc<AtomicU32>,
    pub modulation: BlockPoints,
    pub automation: BlockPoints,
}

impl PluginParamControl {
    pub fn new(native_id: u32, base: Arc<AtomicU32>) -> Self {
        Self {
            native_id,
            modulated: false,
            range: PluginParamRange::UNKNOWN,
            base,
            modulation: BlockPoints::new(),
            automation: BlockPoints::new(),
        }
    }

    pub fn base_value(&self) -> f32 {
        f32::from_bits(self.base.load(Ordering::Relaxed))
    }
}

/// A modulated parameter the plugin cannot modulate itself: the host moves it
/// to `base + offset`, and restores `base` when the modulation stops or the
/// plugin state is saved.
#[derive(Clone)]
pub struct ValueModulatedParam {
    pub native_id: u32,
    pub range: PluginParamRange,
    /// Normalised value the modulation moves around; follows the user's drags
    /// in the plugin window.  Shared with `LiveParams` across recompiles.
    pub base: Arc<AtomicU32>,
    /// The user is dragging this control in the plugin window: modulation pauses.
    pub editing: bool,
}

impl ValueModulatedParam {
    pub fn new(native_id: u32, range: PluginParamRange, base: Arc<AtomicU32>) -> Self {
        Self {
            native_id,
            range,
            base,
            editing: false,
        }
    }

    pub fn base_value(&self) -> f32 {
        f32::from_bits(self.base.load(Ordering::Relaxed))
    }

    pub fn set_base(&self, normalised: f32) {
        self.base.store(normalised.to_bits(), Ordering::Relaxed);
    }

    pub fn base_plain(&self) -> f32 {
        self.range.denormalise(self.base_value())
    }

    pub fn modulated_plain(&self, offset: f32) -> f32 {
        self.range
            .denormalise((self.base_value() + offset).clamp(0.0, 1.0))
    }
}

/// The controlled parameters of one plugin placement.
#[derive(Default)]
pub struct PluginParamControls {
    controls: Vec<PluginParamControl>,
}

impl PluginParamControls {
    pub fn new(controls: Vec<PluginParamControl>) -> Self {
        Self { controls }
    }

    pub fn iter(&self) -> impl Iterator<Item = &PluginParamControl> {
        self.controls.iter()
    }

    pub fn clear_block(&mut self) {
        for control in &mut self.controls {
            control.modulation.clear();
            control.automation.clear();
        }
    }

    fn find(&mut self, native_id: u32) -> Option<&mut PluginParamControl> {
        self.controls
            .iter_mut()
            .find(|control| control.native_id == native_id)
    }

    pub fn modulate(&mut self, native_id: u32, offset: f32) {
        if let Some(control) = self.find(native_id) {
            control.modulation.add_start(offset);
        }
    }

    pub fn modulate_at(&mut self, native_id: u32, sample_offset: u32, offset: f32) {
        if let Some(control) = self.find(native_id) {
            control.modulation.add_at(sample_offset, offset);
        }
    }

    pub fn automate(&mut self, native_id: u32, value: f32) {
        if let Some(control) = self.find(native_id) {
            control.automation.add_start(value);
        }
    }

    pub fn automate_at(&mut self, native_id: u32, sample_offset: u32, value: f32) {
        if let Some(control) = self.find(native_id) {
            control.automation.add_at(sample_offset, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controls(ids: &[u32]) -> PluginParamControls {
        PluginParamControls::new(
            ids.iter()
                .map(|&id| PluginParamControl::new(id, Arc::new(AtomicU32::new(0.5f32.to_bits()))))
                .collect(),
        )
    }

    #[test]
    fn param_names_round_trip() {
        assert_eq!(parse_plugin_param_name(&plugin_param_name(42)), Some(42));
        assert_eq!(parse_plugin_param_name("gain"), None);
    }

    #[test]
    fn modulation_and_automation_are_kept_apart() {
        let mut c = controls(&[7]);
        c.modulate(7, 0.1);
        c.modulate(7, 0.2);
        c.modulate_at(7, 32, 0.3);
        c.modulate_at(7, 32, 0.1);
        c.automate_at(7, 64, 0.9);
        c.modulate(99, 1.0);

        let control = c.iter().next().unwrap();
        let modulation: Vec<_> = control.modulation.iter().collect();
        assert_eq!(modulation.len(), 2);
        assert_eq!(modulation[0].0, 0);
        assert!((modulation[0].1 - 0.3).abs() < 1e-6);
        assert_eq!(modulation[1].0, 32);
        assert!((modulation[1].1 - 0.4).abs() < 1e-6);
        assert_eq!(
            control.automation.iter().collect::<Vec<_>>(),
            vec![(64, 0.9)]
        );
    }

    #[test]
    fn clear_block_empties_every_control() {
        let mut c = controls(&[1, 2]);
        c.modulate(1, 0.5);
        c.automate(2, 0.5);
        c.clear_block();
        assert!(c
            .iter()
            .all(|control| control.modulation.is_empty() && control.automation.is_empty()));
    }

    #[test]
    fn range_converts_to_plain_units() {
        let range = PluginParamRange {
            min: 20.0,
            max: 220.0,
            modulatable: true,
            automatable: true,
        };
        assert_eq!(range.denormalise(0.5), 120.0);
        assert_eq!(range.plain_offset(-0.25), -50.0);
        assert_eq!(range.normalise(120.0), 0.5);
    }

    #[test]
    fn value_modulation_moves_around_the_base_and_stays_in_range() {
        let range = PluginParamRange {
            min: 0.0,
            max: 200.0,
            modulatable: false,
            automatable: true,
        };
        let param = ValueModulatedParam::new(1, range, Arc::new(AtomicU32::new(0.25f32.to_bits())));
        assert_eq!(param.base_plain(), 50.0);
        assert_eq!(param.modulated_plain(0.25), 100.0);
        assert_eq!(param.modulated_plain(-1.0), 0.0);
        assert_eq!(param.modulated_plain(1.0), 200.0);
    }
}
