//! Live parameter registry — atomic cells shared between the command thread
//! (writer) and the audio thread (reader) for hot-path parameter changes.
//!
//! ## Why this exists
//!
//! Recompiling the graph on every knob turn is the wrong shape: it rebuilds
//! every node, calls `prepare` on every reverb / EQ, and resets DSP state
//! mid-stream.  The fix is the standard DAW pattern: hot parameters live in
//! `Arc<Atomic…>` cells that nodes read per-block, while structural changes
//! (adding/removing tracks, clips, effects, routes) remain the only triggers
//! for a full recompile.
//!
//! ## Threading model
//!
//! - `LiveParams::inner` is a `Mutex<HashMap<…>>` — touched only by the
//!   command thread (gateway handlers, compiler).  Held briefly to look up
//!   or insert per-track cells; **never** taken by the audio thread.
//! - The audio thread only sees `Arc<TrackParams>` / `Arc<ModulatorParams>`
//!   / `Arc<RouteParams>` handles embedded in nodes.  It reads the inner
//!   `AtomicU32` / `AtomicBool` fields directly — lock-free, allocation-free.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::audio_graph::parameter_block::ParameterBlock;
use crate::project::{AppProject, EnvelopeParams};

fn write_parameter_cells(block: &ParameterBlock, values: &[f32]) {
    for (i, v) in values.iter().enumerate() {
        block.set(i, *v);
    }
}

pub struct TrackParams {
    pub volume: AtomicU32,
    pub pan: AtomicU32,
    pub muted: AtomicBool,
    pub solo: AtomicBool,
}

impl TrackParams {
    fn new(volume: f32, pan: f32, muted: bool, solo: bool) -> Arc<Self> {
        Arc::new(Self {
            volume: AtomicU32::new(volume.to_bits()),
            pan: AtomicU32::new(pan.to_bits()),
            muted: AtomicBool::new(muted),
            solo: AtomicBool::new(solo),
        })
    }

    #[inline]
    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    /// Pan position, −1 (hard left) … 0 (centre) … +1 (hard right).
    #[inline]
    pub fn pan(&self) -> f32 {
        f32::from_bits(self.pan.load(Ordering::Relaxed))
    }

    #[inline]
    pub fn muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn solo(&self) -> bool {
        self.solo.load(Ordering::Relaxed)
    }
}

pub struct ModulatorCells {
    pub rate_hz: AtomicU32,
    /// When true, `beat_division` governs the LFO rate; `rate_hz` is ignored.
    pub tempo_sync: AtomicBool,
    /// Beats per LFO cycle.  `1.0` = one cycle per beat.
    pub beat_division: AtomicU32,
}

impl ModulatorCells {
    fn new(rate_hz: f32, tempo_sync: bool, beat_division: f32) -> Arc<Self> {
        Arc::new(Self {
            rate_hz: AtomicU32::new(rate_hz.to_bits()),
            tempo_sync: AtomicBool::new(tempo_sync),
            beat_division: AtomicU32::new(beat_division.to_bits()),
        })
    }

    #[inline]
    pub fn rate_hz(&self) -> f32 {
        f32::from_bits(self.rate_hz.load(Ordering::Relaxed))
    }

    #[inline]
    pub fn tempo_sync(&self) -> bool {
        self.tempo_sync.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn beat_division(&self) -> f32 {
        f32::from_bits(self.beat_division.load(Ordering::Relaxed))
    }
}

pub struct RouteCells {
    pub depth: AtomicU32,
}

impl RouteCells {
    fn new(depth: f32) -> Arc<Self> {
        Arc::new(Self {
            depth: AtomicU32::new(depth.to_bits()),
        })
    }

    #[inline]
    pub fn depth(&self) -> f32 {
        f32::from_bits(self.depth.load(Ordering::Relaxed))
    }
}

/// Atomic gate + ADSR parameter cells for one Envelope modulator.
///
/// `gate` is set by the `InstrumentNode` on the audio thread when a note starts
/// or ends on the same track.  All ADSR params are written by the command thread
/// and read lock-free by `EnvelopeNode::process`.
pub struct EnvelopeCells {
    /// `true` = at least one note is sounding; `false` = all notes released.
    pub gate: AtomicBool,
    pub attack_ms: AtomicU32,
    pub decay_ms: AtomicU32,
    pub sustain_level: AtomicU32,
    pub release_ms: AtomicU32,
}

impl EnvelopeCells {
    pub fn new(p: &EnvelopeParams) -> Arc<Self> {
        Arc::new(Self {
            gate: AtomicBool::new(false),
            attack_ms: AtomicU32::new(p.attack_ms.to_bits()),
            decay_ms: AtomicU32::new(p.decay_ms.to_bits()),
            sustain_level: AtomicU32::new(p.sustain_level.to_bits()),
            release_ms: AtomicU32::new(p.release_ms.to_bits()),
        })
    }

    #[inline]
    pub fn attack_ms_value(&self) -> f32 {
        f32::from_bits(self.attack_ms.load(Ordering::Relaxed))
    }

    #[inline]
    pub fn decay_ms_value(&self) -> f32 {
        f32::from_bits(self.decay_ms.load(Ordering::Relaxed))
    }

    #[inline]
    pub fn sustain_value(&self) -> f32 {
        f32::from_bits(self.sustain_level.load(Ordering::Relaxed))
    }

    #[inline]
    pub fn release_ms_value(&self) -> f32 {
        f32::from_bits(self.release_ms.load(Ordering::Relaxed))
    }
}

/// Atomic value cell for a Macro modulator knob.  Written by the command thread,
/// read lock-free by `MacroNode::process` on the audio thread.
pub struct MacroCells {
    pub value: AtomicU32,
}

impl MacroCells {
    fn new(value: f32) -> Arc<Self> {
        Arc::new(Self {
            value: AtomicU32::new(value.to_bits()),
        })
    }

    #[inline]
    pub fn value(&self) -> f32 {
        f32::from_bits(self.value.load(Ordering::Relaxed))
    }
}

struct LiveParamsInner {
    tracks: HashMap<String, Arc<TrackParams>>,
    modulators: HashMap<String, Arc<ModulatorCells>>,
    macros: HashMap<String, Arc<MacroCells>>,
    envelopes: HashMap<String, Arc<EnvelopeCells>>,
    routes: HashMap<String, Arc<RouteCells>>,
    effect_params: HashMap<String, Arc<ParameterBlock>>,
    /// Baseline normalised values for modulated plugin parameters.
    /// Outer key = effect `instance_key`; inner key = `native_id`.
    /// Written by the command thread when a parameter change is pushed
    /// (via `set_plugin_param_shadow`); read lock-free by plugin wrappers in `process()`.
    plugin_param_shadows: HashMap<String, HashMap<u32, Arc<AtomicU32>>>,
    /// Latest note list per track. Replaced on each compile; note edits publish
    /// into the slot the live instrument node already holds.
    midi_notes: HashMap<String, Arc<crate::audio_graph::midi::MidiNotePublisher>>,
    /// Piano-roll audition. Reused across compiles so only the live instrument
    /// drains it (one consumer).
    midi_preview: HashMap<String, Arc<crate::audio_graph::midi::MidiPreviewQueue>>,
}

impl Default for LiveParamsInner {
    fn default() -> Self {
        Self {
            tracks: HashMap::new(),
            modulators: HashMap::new(),
            macros: HashMap::new(),
            envelopes: HashMap::new(),
            routes: HashMap::new(),
            effect_params: HashMap::new(),
            plugin_param_shadows: HashMap::new(),
            midi_notes: HashMap::new(),
            midi_preview: HashMap::new(),
        }
    }
}

pub struct LiveParams {
    inner: Mutex<LiveParamsInner>,
    pub any_solo: Arc<AtomicBool>,
    pub master_soft_clip: Arc<AtomicBool>,
}

impl Default for LiveParams {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveParams {
    fn lock_inner(&self) -> MutexGuard<'_, LiveParamsInner> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn new() -> Self {
        Self {
            inner: Mutex::new(LiveParamsInner::default()),
            any_solo: Arc::new(AtomicBool::new(false)),
            master_soft_clip: Arc::new(AtomicBool::new(false)),
        }
    }

    fn with_track<F>(&self, track_id: &str, write: F) -> bool
    where
        F: FnOnce(&TrackParams),
    {
        let Some(cells) = self.lookup_track(track_id) else {
            return false;
        };
        write(&cells);
        true
    }

    pub fn get_or_init_track(
        &self,
        track_id: &str,
        volume: f32,
        pan: f32,
        muted: bool,
        solo: bool,
    ) -> Arc<TrackParams> {
        let mut inner = self.lock_inner();
        if let Some(existing) = inner.tracks.get(track_id) {
            return Arc::clone(existing);
        }
        let cells = TrackParams::new(volume, pan, muted, solo);
        inner
            .tracks
            .insert(track_id.to_string(), Arc::clone(&cells));
        cells
    }

    pub fn get_or_init_modulator(
        &self,
        id: &str,
        rate_hz: f32,
        tempo_sync: bool,
        beat_division: f32,
    ) -> Arc<ModulatorCells> {
        let mut inner = self.lock_inner();
        if let Some(existing) = inner.modulators.get(id) {
            return Arc::clone(existing);
        }
        let cells = ModulatorCells::new(rate_hz, tempo_sync, beat_division);
        inner.modulators.insert(id.to_string(), Arc::clone(&cells));
        cells
    }

    pub fn get_or_init_effect_block(&self, effect_id: &str, values: &[f32]) -> Arc<ParameterBlock> {
        let mut inner = self.lock_inner();
        if let Some(existing) = inner.effect_params.get(effect_id) {
            write_parameter_cells(existing, values);
            return Arc::clone(existing);
        }
        let block = ParameterBlock::new(values);
        inner
            .effect_params
            .insert(effect_id.to_string(), Arc::clone(&block));
        block
    }

    pub fn effect_block(&self, effect_id: &str) -> Option<Arc<ParameterBlock>> {
        let inner = self.inner.lock().ok()?;
        inner.effect_params.get(effect_id).cloned()
    }

    pub fn get_or_init_macro(&self, id: &str, value: f32) -> Arc<MacroCells> {
        let mut inner = self.lock_inner();
        if let Some(existing) = inner.macros.get(id) {
            return Arc::clone(existing);
        }
        let cells = MacroCells::new(value);
        inner.macros.insert(id.to_string(), Arc::clone(&cells));
        cells
    }

    pub fn set_macro_value(&self, id: &str, value: f32) -> bool {
        let inner = self.lock_inner();
        let Some(cells) = inner.macros.get(id) else {
            return false;
        };
        cells.value.store(value.to_bits(), Ordering::Relaxed);
        true
    }

    pub fn get_or_init_envelope(&self, id: &str, params: &EnvelopeParams) -> Arc<EnvelopeCells> {
        let mut inner = self.lock_inner();
        if let Some(existing) = inner.envelopes.get(id) {
            return Arc::clone(existing);
        }
        let cells = EnvelopeCells::new(params);
        inner.envelopes.insert(id.to_string(), Arc::clone(&cells));
        cells
    }

    pub fn get_envelope(&self, id: &str) -> Option<Arc<EnvelopeCells>> {
        let inner = self.lock_inner();
        inner.envelopes.get(id).cloned()
    }

    /// Hot-update ADSR params (no recompile).  The `EnvelopeNode` reads these
    /// each block so changes take effect within one block.
    pub fn set_envelope_params(
        &self,
        id: &str,
        attack_ms: Option<f32>,
        decay_ms: Option<f32>,
        sustain_level: Option<f32>,
        release_ms: Option<f32>,
    ) -> bool {
        let inner = self.lock_inner();
        let Some(cells) = inner.envelopes.get(id) else {
            return false;
        };
        if let Some(v) = attack_ms {
            cells.attack_ms.store(v.to_bits(), Ordering::Relaxed);
        }
        if let Some(v) = decay_ms {
            cells.decay_ms.store(v.to_bits(), Ordering::Relaxed);
        }
        if let Some(v) = sustain_level {
            cells.sustain_level.store(v.to_bits(), Ordering::Relaxed);
        }
        if let Some(v) = release_ms {
            cells.release_ms.store(v.to_bits(), Ordering::Relaxed);
        }
        true
    }

    /// Returns the `Arc<AtomicU32>` shadow cell for the given `(instance_key, native_id)` pair,
    /// creating it with `initial` if it does not exist yet.  Called from the compiler (command thread).
    pub fn get_or_init_plugin_param_shadow(
        &self,
        instance_key: &str,
        native_id: u32,
        initial: f32,
    ) -> Arc<AtomicU32> {
        let mut inner = self.lock_inner();
        let cell = inner
            .plugin_param_shadows
            .entry(instance_key.to_string())
            .or_default()
            .entry(native_id)
            .or_insert_with(|| Arc::new(AtomicU32::new(initial.to_bits())));
        Arc::clone(cell)
    }

    /// Update the baseline shadow for `(instance_key, native_id)` when the command thread
    /// pushes a new normalised value via `push_plugin_param`.
    pub fn set_plugin_param_shadow(&self, instance_key: &str, native_id: u32, normalised: f32) {
        let inner = self.lock_inner();
        if let Some(params) = inner.plugin_param_shadows.get(instance_key) {
            if let Some(cell) = params.get(&native_id) {
                cell.store(normalised.to_bits(), Ordering::Relaxed);
            }
        }
    }

    pub fn get_or_init_route(&self, id: &str, depth: f32) -> Arc<RouteCells> {
        let mut inner = self.lock_inner();
        if let Some(existing) = inner.routes.get(id) {
            return Arc::clone(existing);
        }
        let cells = RouteCells::new(depth);
        inner.routes.insert(id.to_string(), Arc::clone(&cells));
        cells
    }

    pub fn any_solo(&self) -> bool {
        self.any_solo.load(Ordering::Relaxed)
    }

    pub fn master_soft_clip_enabled(&self) -> bool {
        self.master_soft_clip.load(Ordering::Relaxed)
    }

    pub fn set_master_soft_clip(&self, enabled: bool) {
        self.master_soft_clip.store(enabled, Ordering::Relaxed);
    }

    pub fn set_track_volume(&self, track_id: &str, volume: f32) -> bool {
        self.with_track(track_id, |cells| {
            cells.volume.store(volume.to_bits(), Ordering::Relaxed);
        })
    }

    pub fn set_track_pan(&self, track_id: &str, pan: f32) -> bool {
        self.with_track(track_id, |cells| {
            cells.pan.store(pan.to_bits(), Ordering::Relaxed);
        })
    }

    pub fn set_track_muted(&self, track_id: &str, muted: bool) -> bool {
        self.with_track(track_id, |cells| {
            cells.muted.store(muted, Ordering::Relaxed);
        })
    }

    pub fn set_track_solo_exclusive(&self, track_id: &str, solo: bool) {
        let inner = self.lock_inner();
        if solo {
            for (id, cells) in inner.tracks.iter() {
                cells.solo.store(id == track_id, Ordering::Relaxed);
            }
            self.any_solo.store(true, Ordering::Relaxed);
        } else {
            if let Some(cells) = inner.tracks.get(track_id) {
                cells.solo.store(false, Ordering::Relaxed);
            }
            let still_any = inner
                .tracks
                .values()
                .any(|c| c.solo.load(Ordering::Relaxed));
            self.any_solo.store(still_any, Ordering::Relaxed);
        }
    }

    pub fn set_modulator_rate(&self, id: &str, rate_hz: f32) -> bool {
        let inner = self.lock_inner();
        let Some(cells) = inner.modulators.get(id) else {
            return false;
        };
        cells.rate_hz.store(rate_hz.to_bits(), Ordering::Relaxed);
        true
    }

    pub fn set_modulator_tempo_sync(&self, id: &str, tempo_sync: bool) -> bool {
        let inner = self.lock_inner();
        let Some(cells) = inner.modulators.get(id) else {
            return false;
        };
        cells.tempo_sync.store(tempo_sync, Ordering::Relaxed);
        true
    }

    pub fn set_modulator_beat_division(&self, id: &str, beat_division: f32) -> bool {
        let inner = self.lock_inner();
        let Some(cells) = inner.modulators.get(id) else {
            return false;
        };
        cells.beat_division.store(beat_division.to_bits(), Ordering::Relaxed);
        true
    }

    pub fn set_route_depth(&self, id: &str, depth: f32) -> bool {
        let inner = self.lock_inner();
        let Some(cells) = inner.routes.get(id) else {
            return false;
        };
        cells.depth.store(depth.to_bits(), Ordering::Relaxed);
        true
    }

    /// Write one cell of an effect's hot-parameter block (no recompile).
    /// Returns false if no block exists yet (effect not in the active graph) —
    /// the caller then falls back to the legacy recompile path (ADR-009, T4).
    pub fn set_effect_param(&self, effect_id: &str, idx: usize, value: f32) -> bool {
        let Some(block) = self.effect_block(effect_id) else {
            return false;
        };
        block.set(idx, value);
        true
    }

    /// Write every cell of an effect's hot-parameter block in one call
    /// (no recompile).  `values` must be in the node's cell order.  Returns
    /// false if no block is registered — caller falls back to recompile.
    pub fn write_effect_block(&self, effect_id: &str, values: &[f32]) -> bool {
        let Some(block) = self.effect_block(effect_id) else {
            return false;
        };
        write_parameter_cells(&block, values);
        true
    }

    fn lookup_track(&self, track_id: &str) -> Option<Arc<TrackParams>> {
        let inner = self.inner.lock().ok()?;
        inner.tracks.get(track_id).cloned()
    }

    /// Drop registry entries no longer present in the project, and refresh
    /// `any_solo`.  Called from the compiler so the registry mirrors the
    /// project's live structural state.
    ///
    /// Existing atomics are *not* overwritten from `project` — the project
    /// is already updated in lock-step with the atomic by the gateway
    /// handlers, so the values agree.  Skipping the writes also avoids
    /// clobbering an in-flight command racing with the recompile.
    pub fn prune_to_project(&self, project: &AppProject) {
        let mut inner = self.lock_inner();

        let live_track_ids: HashSet<&str> =
            project.tracks.iter().map(|t| t.id.as_str()).collect();
        inner
            .tracks
            .retain(|id, _| live_track_ids.contains(id.as_str()));

        let live_mod_ids: HashSet<&str> = project
            .tracks
            .iter()
            .flat_map(|t| t.modulators.iter().map(|m| m.id.as_str()))
            .collect();
        inner
            .modulators
            .retain(|id, _| live_mod_ids.contains(id.as_str()));
        inner
            .macros
            .retain(|id, _| live_mod_ids.contains(id.as_str()));
        inner
            .envelopes
            .retain(|id, _| live_mod_ids.contains(id.as_str()));

        let live_route_ids: HashSet<&str> = project
            .tracks
            .iter()
            .flat_map(|t| t.mod_routes.iter().map(|r| r.id.as_str()))
            .collect();
        inner
            .routes
            .retain(|id, _| live_route_ids.contains(id.as_str()));

        let live_effect_ids: HashSet<&str> = project
            .tracks
            .iter()
            .flat_map(|t| t.effects.iter().map(|e| e.id.as_str()))
            .collect();
        inner
            .effect_params
            .retain(|id, _| live_effect_ids.contains(id.as_str()));
        inner
            .plugin_param_shadows
            .retain(|id, _| live_effect_ids.contains(id.as_str()));

        inner
            .midi_notes
            .retain(|id, _| live_track_ids.contains(id.as_str()));
        inner
            .midi_preview
            .retain(|id, _| live_track_ids.contains(id.as_str()));

        let any_solo = project.tracks.iter().any(|t| t.solo);
        self.any_solo.store(any_solo, Ordering::Relaxed);
    }

    /// Install a fresh note list for `track_id` and return the publisher the
    /// new instrument node must hold. Replaces any previous slot so later
    /// edits reach this compile, not a retired graph.
    pub fn install_track_notes(
        &self,
        track_id: &str,
        notes: Vec<crate::audio_graph::midi::MidiNoteSnapshot>,
    ) -> Arc<crate::audio_graph::midi::MidiNotePublisher> {
        let publisher = crate::audio_graph::midi::MidiNotePublisher::with_notes(notes);
        self.lock_inner()
            .midi_notes
            .insert(track_id.to_string(), Arc::clone(&publisher));
        publisher
    }

    /// Publish a note edit to the live instrument. Returns false when the
    /// track has no slot yet (the graph has not been compiled).
    pub fn publish_track_notes(
        &self,
        track_id: &str,
        notes: Vec<crate::audio_graph::midi::MidiNoteSnapshot>,
    ) -> bool {
        let Some(publisher) = self.lock_inner().midi_notes.get(track_id).cloned() else {
            return false;
        };
        publisher.publish(notes);
        true
    }

    /// Queue the live instrument already drains. Created once per track.
    pub fn preview_queue(&self, track_id: &str) -> Arc<crate::audio_graph::midi::MidiPreviewQueue> {
        let mut inner = self.lock_inner();
        if let Some(queue) = inner.midi_preview.get(track_id) {
            return Arc::clone(queue);
        }
        let queue = Arc::new(crate::audio_graph::midi::MidiPreviewQueue::new());
        inner
            .midi_preview
            .insert(track_id.to_string(), Arc::clone(&queue));
        queue
    }

    /// Piano-roll audition. No project event and no recompile.
    pub fn push_preview(
        &self,
        track_id: &str,
        message: crate::audio_graph::midi::MidiPreviewMessage,
    ) -> bool {
        let Some(queue) = self.lock_inner().midi_preview.get(track_id).cloned() else {
            return false;
        };
        queue.push(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_param_shadow_init_and_update() {
        let lp = LiveParams::new();

        let shadow = lp.get_or_init_plugin_param_shadow("effect-1", 42, 0.5);
        assert!((f32::from_bits(shadow.load(Ordering::Relaxed)) - 0.5).abs() < 1e-6);

        lp.set_plugin_param_shadow("effect-1", 42, 0.8);
        assert!((f32::from_bits(shadow.load(Ordering::Relaxed)) - 0.8).abs() < 1e-6);
    }

    #[test]
    fn plugin_param_shadow_deduplicates_per_native_id() {
        let lp = LiveParams::new();
        let a = lp.get_or_init_plugin_param_shadow("effect-1", 10, 0.3);
        let b = lp.get_or_init_plugin_param_shadow("effect-1", 10, 0.9);
        // Second call must return the same Arc — value not reset.
        assert!(Arc::ptr_eq(&a, &b));
        assert!((f32::from_bits(a.load(Ordering::Relaxed)) - 0.3).abs() < 1e-6);
    }

    #[test]
    fn plugin_param_shadow_scoped_per_instance_key() {
        let lp = LiveParams::new();
        let a = lp.get_or_init_plugin_param_shadow("effect-A", 1, 0.2);
        let b = lp.get_or_init_plugin_param_shadow("effect-B", 1, 0.7);
        assert!(!Arc::ptr_eq(&a, &b));
        lp.set_plugin_param_shadow("effect-A", 1, 1.0);
        assert!((f32::from_bits(a.load(Ordering::Relaxed)) - 1.0).abs() < 1e-6);
        // effect-B must be unaffected
        assert!((f32::from_bits(b.load(Ordering::Relaxed)) - 0.7).abs() < 1e-6);
    }
}
