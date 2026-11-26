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
use std::sync::atomic::{AtomicBool, AtomicI8, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::arpeggiator_settings::{ArpMode, ArpRate, ArpeggiatorSettings};
use crate::audio_graph::parameter_block::ParameterBlock;
use crate::note_effect_settings::{
    ChordSettings, ChordShape, NoteEffectSettings, ScaleKind, ScaleSettings, TransposeSettings,
};
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

/// One atomic per setting, written by the command thread and read once per
/// block by the instrument node (ADR-030).
pub struct ArpeggiatorCells {
    enabled: AtomicBool,
    mode: AtomicU8,
    rate: AtomicU8,
    triplet: AtomicBool,
    gate_percent: AtomicU8,
    octaves: AtomicU8,
}

impl ArpeggiatorCells {
    fn new(settings: &ArpeggiatorSettings) -> Arc<Self> {
        let cells = Arc::new(Self {
            enabled: AtomicBool::new(false),
            mode: AtomicU8::new(0),
            rate: AtomicU8::new(0),
            triplet: AtomicBool::new(false),
            gate_percent: AtomicU8::new(0),
            octaves: AtomicU8::new(0),
        });
        cells.store(settings);
        cells
    }

    pub fn store(&self, settings: &ArpeggiatorSettings) {
        self.enabled.store(settings.enabled, Ordering::Relaxed);
        self.mode.store(settings.mode.index(), Ordering::Relaxed);
        self.rate.store(settings.rate.index(), Ordering::Relaxed);
        self.triplet.store(settings.triplet, Ordering::Relaxed);
        self.gate_percent
            .store(settings.gate_percent, Ordering::Relaxed);
        self.octaves.store(settings.octaves, Ordering::Relaxed);
    }

    #[inline]
    pub fn load(&self) -> ArpeggiatorSettings {
        ArpeggiatorSettings {
            enabled: self.enabled.load(Ordering::Relaxed),
            mode: ArpMode::from_index(self.mode.load(Ordering::Relaxed)),
            rate: ArpRate::from_index(self.rate.load(Ordering::Relaxed)),
            triplet: self.triplet.load(Ordering::Relaxed),
            gate_percent: self.gate_percent.load(Ordering::Relaxed),
            octaves: self.octaves.load(Ordering::Relaxed),
        }
        .clamped()
    }
}

/// One atomic per setting, written by the command thread and read once per
/// block by the instrument node (ADR-038).
pub struct NoteEffectCells {
    transpose_enabled: AtomicBool,
    transpose_semitones: AtomicI8,
    scale_enabled: AtomicBool,
    scale_root: AtomicU8,
    scale_kind: AtomicU8,
    chord_enabled: AtomicBool,
    chord_shape: AtomicU8,
}

impl NoteEffectCells {
    fn new(settings: &NoteEffectSettings) -> Arc<Self> {
        let cells = Arc::new(Self {
            transpose_enabled: AtomicBool::new(false),
            transpose_semitones: AtomicI8::new(0),
            scale_enabled: AtomicBool::new(false),
            scale_root: AtomicU8::new(0),
            scale_kind: AtomicU8::new(0),
            chord_enabled: AtomicBool::new(false),
            chord_shape: AtomicU8::new(0),
        });
        cells.store(settings);
        cells
    }

    pub fn store(&self, settings: &NoteEffectSettings) {
        self.transpose_enabled
            .store(settings.transpose.enabled, Ordering::Relaxed);
        self.transpose_semitones
            .store(settings.transpose.semitones, Ordering::Relaxed);
        self.scale_enabled
            .store(settings.scale.enabled, Ordering::Relaxed);
        self.scale_root
            .store(settings.scale.root, Ordering::Relaxed);
        self.scale_kind
            .store(settings.scale.scale.index(), Ordering::Relaxed);
        self.chord_enabled
            .store(settings.chord.enabled, Ordering::Relaxed);
        self.chord_shape
            .store(settings.chord.shape.index(), Ordering::Relaxed);
    }

    #[inline]
    pub fn load(&self) -> NoteEffectSettings {
        NoteEffectSettings {
            transpose: TransposeSettings {
                enabled: self.transpose_enabled.load(Ordering::Relaxed),
                semitones: self.transpose_semitones.load(Ordering::Relaxed),
            },
            scale: ScaleSettings {
                enabled: self.scale_enabled.load(Ordering::Relaxed),
                root: self.scale_root.load(Ordering::Relaxed),
                scale: ScaleKind::from_index(self.scale_kind.load(Ordering::Relaxed)),
            },
            chord: ChordSettings {
                enabled: self.chord_enabled.load(Ordering::Relaxed),
                shape: ChordShape::from_index(self.chord_shape.load(Ordering::Relaxed)),
            },
        }
        .clamped()
    }
}

#[derive(Default)]
struct LiveParamsInner {
    arpeggiators: HashMap<String, Arc<ArpeggiatorCells>>,
    note_effects: HashMap<String, Arc<NoteEffectCells>>,
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
    midi_notes: HashMap<String, Arc<crate::audio_graph::midi::MidiTrackPublisher>>,
    /// Piano-roll audition. Reused across compiles so only the live instrument
    /// drains it (one consumer).
    midi_preview: HashMap<String, Arc<crate::audio_graph::midi::MidiPreviewQueue>>,
    /// Live hardware MIDI input (ADR-022). Reused across compiles so only the
    /// live instrument drains it (one consumer); the MIDI capture thread is
    /// the sole producer.
    midi_live_input: HashMap<String, Arc<crate::audio_graph::midi::MidiLiveInputQueue>>,
}

pub struct LiveParams {
    inner: Mutex<LiveParamsInner>,
    pub any_solo: Arc<AtomicBool>,
    pub master_soft_clip: Arc<AtomicBool>,
    /// Stamped by the MIDI capture threads and by the audio callback (ADR-022).
    pub live_input_clock: Arc<crate::audio_graph::midi::LiveInputClock>,
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
            live_input_clock: Arc::new(crate::audio_graph::midi::LiveInputClock::new()),
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

    /// Cells the track's instrument node reads. A compile writes the project's
    /// values over any previous ones, so an undo that rebuilt the project reaches
    /// the audio thread.
    pub fn install_arpeggiator(
        &self,
        track_id: &str,
        settings: &ArpeggiatorSettings,
    ) -> Arc<ArpeggiatorCells> {
        let mut inner = self.lock_inner();
        if let Some(existing) = inner.arpeggiators.get(track_id) {
            existing.store(settings);
            return Arc::clone(existing);
        }
        let cells = ArpeggiatorCells::new(settings);
        inner
            .arpeggiators
            .insert(track_id.to_string(), Arc::clone(&cells));
        cells
    }

    /// No recompile. Returns false when the track has no cells yet (graph not
    /// compiled); the caller then relies on the next compile.
    pub fn set_arpeggiator(&self, track_id: &str, settings: &ArpeggiatorSettings) -> bool {
        let inner = self.lock_inner();
        let Some(cells) = inner.arpeggiators.get(track_id) else {
            return false;
        };
        cells.store(settings);
        true
    }

    /// Same contract as `install_arpeggiator`.
    pub fn install_note_effects(
        &self,
        track_id: &str,
        settings: &NoteEffectSettings,
    ) -> Arc<NoteEffectCells> {
        let mut inner = self.lock_inner();
        if let Some(existing) = inner.note_effects.get(track_id) {
            existing.store(settings);
            return Arc::clone(existing);
        }
        let cells = NoteEffectCells::new(settings);
        inner
            .note_effects
            .insert(track_id.to_string(), Arc::clone(&cells));
        cells
    }

    /// Same contract as `set_arpeggiator`.
    pub fn set_note_effects(&self, track_id: &str, settings: &NoteEffectSettings) -> bool {
        let inner = self.lock_inner();
        let Some(cells) = inner.note_effects.get(track_id) else {
            return false;
        };
        cells.store(settings);
        true
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
        cells
            .beat_division
            .store(beat_division.to_bits(), Ordering::Relaxed);
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

        let live_track_ids: HashSet<&str> = project.tracks.iter().map(|t| t.id.as_str()).collect();
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
            .arpeggiators
            .retain(|id, _| live_track_ids.contains(id.as_str()));
        inner
            .note_effects
            .retain(|id, _| live_track_ids.contains(id.as_str()));
        inner
            .midi_notes
            .retain(|id, _| live_track_ids.contains(id.as_str()));
        inner
            .midi_preview
            .retain(|id, _| live_track_ids.contains(id.as_str()));
        inner
            .midi_live_input
            .retain(|id, _| live_track_ids.contains(id.as_str()));

        let any_solo = project.tracks.iter().any(|t| t.solo);
        self.any_solo.store(any_solo, Ordering::Relaxed);
    }

    /// Install a fresh note and control content for `track_id` and return the publisher the
    /// new instrument node must hold. Replaces any previous slot so later
    /// edits reach this compile, not a retired graph.
    pub fn install_track_midi(
        &self,
        track_id: &str,
        content: crate::audio_graph::midi::MidiTrackContent,
    ) -> Arc<crate::audio_graph::midi::MidiTrackPublisher> {
        let publisher = crate::audio_graph::midi::MidiTrackPublisher::with_content(content);
        self.lock_inner()
            .midi_notes
            .insert(track_id.to_string(), Arc::clone(&publisher));
        publisher
    }

    /// Publish a note or control edit to the live instrument. Returns false
    /// when the track has no slot yet (the graph has not been compiled).
    pub fn publish_track_midi(
        &self,
        track_id: &str,
        content: crate::audio_graph::midi::MidiTrackContent,
    ) -> bool {
        let Some(publisher) = self.lock_inner().midi_notes.get(track_id).cloned() else {
            return false;
        };
        publisher.publish(content);
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

    /// Queue the live instrument already drains for hardware MIDI input
    /// (ADR-022). Created once per track.
    pub fn live_input_queue(
        &self,
        track_id: &str,
    ) -> Arc<crate::audio_graph::midi::MidiLiveInputQueue> {
        let mut inner = self.lock_inner();
        if let Some(queue) = inner.midi_live_input.get(track_id) {
            return Arc::clone(queue);
        }
        let queue = Arc::new(crate::audio_graph::midi::MidiLiveInputQueue::new(
            Arc::clone(&self.live_input_clock),
        ));
        inner
            .midi_live_input
            .insert(track_id.to_string(), Arc::clone(&queue));
        queue
    }

    /// Hardware MIDI capture threads. No project event and no recompile.
    /// Returns false when the track has no slot yet (graph not compiled, or
    /// the armed track no longer exists).
    pub fn push_live_input(
        &self,
        track_id: &str,
        message: crate::audio_graph::midi::MidiLiveInputMessage,
    ) -> bool {
        let Some(queue) = self.lock_inner().midi_live_input.get(track_id).cloned() else {
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
