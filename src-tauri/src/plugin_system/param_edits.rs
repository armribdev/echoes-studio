//! What the user does to a plugin parameter in its own window, kept so it can be
//! written into an automation lane (ADR-037).
//!
//! The audio thread pushes [`ParamEdit`]s into a per-instance ring while writing
//! is armed and the transport plays; [`GestureRecorder`] turns them back into one
//! thinned [`RecordedGesture`] per drag, off the audio thread.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;

use crate::audio_graph::node::TransportInfo;
use crate::audio_graph::plugin_params::PluginParamRange;
use crate::plugin_system::instance::PluginInstance;

pub const PARAM_EDIT_CAPACITY: usize = 256;

/// Points closer than this are merged, as for the recorded MIDI controls.
const MIN_POINT_SPACING_S: f64 = 0.005;

static WRITE_ARMED: AtomicBool = AtomicBool::new(false);

/// True while at least one automation lane waits for a gesture.  The audio thread
/// reads it once per output event.
pub fn write_armed() -> bool {
    WRITE_ARMED.load(Ordering::Relaxed)
}

pub fn set_write_armed(armed: bool) {
    WRITE_ARMED.store(armed, Ordering::Relaxed);
}

/// The flag is process-wide: the tests that raise it take this lock.
#[cfg(test)]
pub(crate) static WRITE_ARMED_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ParamEditKind {
    #[default]
    GestureBegin,
    Value,
    GestureEnd,
}

/// One edit, stamped with the project time it happened at.
#[derive(Debug, Clone, Copy, Default)]
pub struct ParamEdit {
    pub kind: ParamEditKind,
    pub native_id: u32,
    pub normalised: f32,
    pub time_s: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct GesturePoint {
    pub time_s: f64,
    pub value: f32,
}

/// A finished drag of one parameter.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordedGesture {
    pub native_id: u32,
    pub points: Vec<GesturePoint>,
}

const MAX_OPEN_GESTURES: usize = 8;

/// The drags the user makes in the plugin window while an automation lane waits
/// for them: each one is stamped with the project time and handed to the
/// instance's edit ring.  Audio thread only; nothing here allocates.
#[derive(Default)]
pub struct EditCapture {
    instance: Option<Arc<PluginInstance>>,
    ranges: HashMap<u32, PluginParamRange>,
    block_start_s: f64,
    sample_rate: f64,
    playing: bool,
    open: [u32; MAX_OPEN_GESTURES],
    open_count: usize,
}

impl EditCapture {
    /// A plugin whose values are already normalised (VST3).
    pub fn for_instance(instance: &Arc<PluginInstance>) -> Self {
        let mut capture = Self::default();
        capture.attach(instance, &HashMap::new());
        capture
    }

    /// Off the audio thread, at compile time.  `ranges` turn the plain values of a
    /// CLAP plugin into normalised ones.
    pub fn attach(
        &mut self,
        instance: &Arc<PluginInstance>,
        ranges: &HashMap<u32, PluginParamRange>,
    ) {
        self.instance = Some(Arc::clone(instance));
        self.ranges = ranges.clone();
    }

    pub fn begin_block(&mut self, transport: &TransportInfo) {
        self.block_start_s = transport.position_seconds;
        self.sample_rate = f64::from(transport.sample_rate);
        self.playing = transport.is_playing;
    }

    /// The host leaves this parameter's automation alone while it is dragged, so
    /// the lane does not fight the hand.
    pub fn is_open(&self, native_id: u32) -> bool {
        self.open[..self.open_count].contains(&native_id)
    }

    fn push(&self, kind: ParamEditKind, time: u32, native_id: u32, plain: f64) {
        let Some(instance) = &self.instance else {
            return;
        };
        let range = self
            .ranges
            .get(&native_id)
            .copied()
            .unwrap_or(PluginParamRange::UNKNOWN);
        instance.push_param_edit(ParamEdit {
            kind,
            native_id,
            normalised: range.normalise(plain as f32),
            time_s: self.block_start_s + f64::from(time) / self.sample_rate,
        });
    }

    /// `time` is the sample offset in the block.
    pub fn observe_gesture(&mut self, time: u32, native_id: u32, is_begin: bool) {
        if !write_armed() {
            self.open_count = 0;
            return;
        }
        if is_begin {
            if self.playing && !self.is_open(native_id) && self.open_count < MAX_OPEN_GESTURES {
                self.open[self.open_count] = native_id;
                self.open_count += 1;
                self.push(ParamEditKind::GestureBegin, time, native_id, 0.0);
            }
        } else if let Some(index) = self.open[..self.open_count]
            .iter()
            .position(|&id| id == native_id)
        {
            self.open_count -= 1;
            self.open[index] = self.open[self.open_count];
            self.push(ParamEditKind::GestureEnd, time, native_id, 0.0);
        }
    }

    pub fn observe_value(&mut self, time: u32, native_id: u32, plain: f64) {
        if self.playing && write_armed() && self.is_open(native_id) {
            self.push(ParamEditKind::Value, time, native_id, plain);
        }
    }

    /// A plugin whose window reports to the host between blocks (VST3 in the Echoes
    /// process) is read in three steps per block, all at the block start: the
    /// gestures it holds open, the values it set, then the gestures that ended.
    pub fn open_dragged(&mut self, dragged: &[Option<u32>]) {
        for &native_id in dragged.iter().flatten() {
            self.observe_gesture(0, native_id, true);
        }
    }

    /// A value set without a gesture in progress (a click) opens its own gesture,
    /// which `close_released` ends in the same block.
    pub fn window_value(&mut self, native_id: u32, normalised: f64) {
        self.observe_gesture(0, native_id, true);
        self.observe_value(0, native_id, normalised);
    }

    pub fn close_released(&mut self, dragged: &[Option<u32>]) {
        let mut index = 0;
        while index < self.open_count {
            let native_id = self.open[index];
            if dragged.contains(&Some(native_id)) {
                index += 1;
            } else {
                self.observe_gesture(0, native_id, false);
            }
        }
    }
}

/// Payload of `plugin-param-gesture`: a finished drag and the placement it was on.
#[derive(Debug, Clone, Serialize)]
pub struct GestureEvent<'a> {
    pub effect_id: &'a str,
    #[serde(flatten)]
    pub gesture: RecordedGesture,
}

struct OpenGesture {
    native_id: u32,
    points: Vec<GesturePoint>,
    latest: Option<GesturePoint>,
}

/// Pairs the edits of one plugin instance into gestures.
#[derive(Default)]
pub struct GestureRecorder {
    open: Vec<OpenGesture>,
}

impl GestureRecorder {
    /// A value outside a gesture is ignored: a plugin that echoes the value the
    /// host sent must not write points.
    pub fn accept(&mut self, edit: ParamEdit) -> Option<RecordedGesture> {
        match edit.kind {
            ParamEditKind::GestureBegin => {
                if self.open.iter().all(|g| g.native_id != edit.native_id) {
                    self.open.push(OpenGesture {
                        native_id: edit.native_id,
                        points: Vec::new(),
                        latest: None,
                    });
                }
                None
            }
            ParamEditKind::Value => {
                let gesture = self
                    .open
                    .iter_mut()
                    .find(|g| g.native_id == edit.native_id)?;
                let point = GesturePoint {
                    time_s: edit.time_s,
                    value: edit.normalised.clamp(0.0, 1.0),
                };
                let too_close = gesture
                    .points
                    .last()
                    .is_some_and(|last| point.time_s - last.time_s < MIN_POINT_SPACING_S);
                if !too_close {
                    gesture.points.push(point);
                    gesture.latest = None;
                } else {
                    gesture.latest = Some(point);
                }
                None
            }
            ParamEditKind::GestureEnd => {
                let index = self
                    .open
                    .iter()
                    .position(|g| g.native_id == edit.native_id)?;
                let mut gesture = self.open.swap_remove(index);
                gesture.points.extend(gesture.latest);
                (!gesture.points.is_empty()).then_some(RecordedGesture {
                    native_id: gesture.native_id,
                    points: gesture.points,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(kind: ParamEditKind, native_id: u32, normalised: f32, time_s: f64) -> ParamEdit {
        ParamEdit {
            kind,
            native_id,
            normalised,
            time_s,
        }
    }

    fn drag(
        recorder: &mut GestureRecorder,
        native_id: u32,
        values: &[(f64, f32)],
    ) -> Option<RecordedGesture> {
        recorder.accept(edit(ParamEditKind::GestureBegin, native_id, 0.0, 0.0));
        for &(time_s, value) in values {
            recorder.accept(edit(ParamEditKind::Value, native_id, value, time_s));
        }
        let end_time = values.last().map_or(0.0, |&(t, _)| t);
        recorder.accept(edit(ParamEditKind::GestureEnd, native_id, 0.0, end_time))
    }

    #[test]
    fn a_drag_becomes_one_gesture() {
        let mut recorder = GestureRecorder::default();
        let gesture = drag(&mut recorder, 7, &[(1.0, 0.2), (1.1, 0.5), (1.2, 0.9)]).unwrap();
        assert_eq!(gesture.native_id, 7);
        let values: Vec<f32> = gesture.points.iter().map(|p| p.value).collect();
        assert_eq!(values, vec![0.2, 0.5, 0.9]);
    }

    #[test]
    fn a_value_outside_a_gesture_is_ignored() {
        let mut recorder = GestureRecorder::default();
        assert!(recorder
            .accept(edit(ParamEditKind::Value, 7, 0.5, 1.0))
            .is_none());
        assert!(recorder
            .accept(edit(ParamEditKind::GestureEnd, 7, 0.0, 1.0))
            .is_none());
    }

    #[test]
    fn points_closer_than_five_milliseconds_are_merged_but_the_last_value_is_kept() {
        let mut recorder = GestureRecorder::default();
        let gesture = drag(
            &mut recorder,
            7,
            &[(1.000, 0.1), (1.001, 0.2), (1.002, 0.3), (1.003, 0.4)],
        )
        .unwrap();
        let values: Vec<f32> = gesture.points.iter().map(|p| p.value).collect();
        assert_eq!(values, vec![0.1, 0.4]);
    }

    #[test]
    fn two_parameters_can_be_dragged_at_once() {
        let mut recorder = GestureRecorder::default();
        recorder.accept(edit(ParamEditKind::GestureBegin, 1, 0.0, 0.0));
        recorder.accept(edit(ParamEditKind::GestureBegin, 2, 0.0, 0.0));
        recorder.accept(edit(ParamEditKind::Value, 2, 0.8, 1.0));
        recorder.accept(edit(ParamEditKind::Value, 1, 0.3, 1.0));
        let second = recorder
            .accept(edit(ParamEditKind::GestureEnd, 2, 0.0, 1.0))
            .unwrap();
        let first = recorder
            .accept(edit(ParamEditKind::GestureEnd, 1, 0.0, 1.0))
            .unwrap();
        assert_eq!((first.points[0].value, second.points[0].value), (0.3, 0.8));
    }

    #[test]
    fn a_gesture_without_value_writes_nothing() {
        let mut recorder = GestureRecorder::default();
        assert!(drag(&mut recorder, 7, &[]).is_none());
    }

    #[test]
    fn values_are_kept_in_range() {
        let mut recorder = GestureRecorder::default();
        let gesture = drag(&mut recorder, 7, &[(1.0, 1.4)]).unwrap();
        assert_eq!(gesture.points[0].value, 1.0);
    }
}
