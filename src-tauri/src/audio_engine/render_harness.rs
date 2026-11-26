//! C-2 offline render harness (migration plan V1).
//!
//! Renders synthetic reference projects through the real offline pipeline and
//! checks two properties:
//! - **Block-size independence** (ADR-019): the same project rendered with
//!   different `process_block` sizes is bit-identical, i.e. offline == live.
//! - **Golden master**: each project's render hash matches the recorded value,
//!   so any change to rendered output is caught.  Updating a golden hash is only
//!   legitimate for a render change sanctioned by an ADR.

use std::sync::Arc;

use super::offline::{render_project_in_blocks, render_project_offline};
use crate::arpeggiator_settings::{ArpMode, ArpRate, ArpeggiatorSettings};
use crate::audio_graph::LiveParams;
use crate::note_effect_settings::{
    ChordSettings, ChordShape, NoteEffectSettings, ScaleKind, ScaleSettings, TransposeSettings,
};
use crate::plugin_system::PluginHost;
use crate::project::{
    AppClip, AppMidiClip, AppProject, AppTrack, AutomationClip, AutomationLane, AutomationPoint,
    CompressorParams, DelayParams, EffectParams, EqParams, InterpolationMode, LfoParams,
    LfoWaveform, MacroParams, MidiContent, MidiControlKind, MidiControlPoint, MidiNote,
    ModulationTargetRef, ModulatorParams, ProjectModulationRoute, ProjectModulator, ReverbParams,
    TrackEffect,
};
use crate::sampler_kit::{PadPlayMode, PadSample, SamplerKit, SamplerPad, SAMPLER_INSTRUMENT_ID};

const SAMPLE_RATE: u32 = 48_000;
const CLIP_SAMPLE_RATE: u32 = 44_100;
const DURATION_S: f64 = 2.5;

/// Includes sizes that are not multiples of the 32-sample control period.
const BLOCK_SIZES: [usize; 5] = [4096, 512, 441, 64, 37];

fn render(project: &AppProject, block_size: usize) -> Vec<f32> {
    let live_params = LiveParams::new();
    let mut host = PluginHost::new();
    let (left, right) =
        render_project_in_blocks(project, &live_params, &mut host, SAMPLE_RATE, block_size)
            .expect("reference project must render");
    left.into_iter()
        .zip(right)
        .flat_map(|(l, r)| [l, r])
        .collect()
}

/// FNV-1a over the raw `f32` bits: bit-exact, dependency-free.
fn render_hash(samples: &[f32]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for sample in samples {
        for byte in sample.to_bits().to_le_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

fn first_difference(a: &[f32], b: &[f32]) -> Option<usize> {
    a.iter()
        .zip(b)
        .position(|(x, y)| x.to_bits() != y.to_bits())
        .or_else(|| (a.len() != b.len()).then(|| a.len().min(b.len())))
}

/// Deterministic sine sweep plus LCG noise, so the audio path needs no file.
fn synthetic_samples() -> Vec<f32> {
    let frames = (DURATION_S * CLIP_SAMPLE_RATE as f64) as usize;
    let mut seed: u32 = 0x1234_5678;
    (0..frames)
        .map(|i| {
            let t = i as f32 / CLIP_SAMPLE_RATE as f32;
            let sweep = (2.0 * std::f32::consts::PI * (110.0 + 400.0 * t) * t).sin();
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
            0.6 * sweep + 0.1 * noise
        })
        .collect()
}

fn empty_track(id: &str) -> AppTrack {
    AppTrack {
        id: id.into(),
        name: id.into(),
        volume: 80.0,
        pan: 0.0,
        muted: false,
        solo: false,
        clips: vec![],
        effects: vec![],
        modulators: vec![],
        mod_routes: vec![],
        midi_notes: vec![],
        midi_clips: vec![],
        instrument: None,
        sampler: None,
        arpeggiator: None,
        note_effects: None,
        input_source: None,
        automation_lanes: vec![],
        color: None,
    }
}

fn effect(id: &str, params: EffectParams) -> TrackEffect {
    TrackEffect {
        id: id.into(),
        enabled: true,
        params,
    }
}

fn full_effect_chain() -> Vec<TrackEffect> {
    vec![
        effect(
            "eq",
            EffectParams::Eq(EqParams {
                mid_gain_db: 4.0,
                ..EqParams::default()
            }),
        ),
        effect(
            "comp",
            EffectParams::Compressor(CompressorParams::default()),
        ),
        effect(
            "delay",
            EffectParams::Delay(DelayParams {
                delay_ms: 90.0,
                ..DelayParams::default()
            }),
        ),
        effect("reverb", EffectParams::Reverb(ReverbParams::default())),
    ]
}

fn audio_track() -> AppTrack {
    let mut track = empty_track("audio");
    track.clips.push(AppClip {
        id: "clip".into(),
        start: 0.0,
        end: DURATION_S,
        offset: 0.0,
        file_path: String::new(),
        raw_samples: Arc::new(synthetic_samples()),
        sample_rate: CLIP_SAMPLE_RATE,
        peaks_64: vec![],
        peaks_512: vec![],
        peaks_4096: vec![],
    });
    track.effects = full_effect_chain();
    track
}

fn reference_midi_notes() -> Vec<MidiNote> {
    let note = |id: &str, pitch: u8, start: f32, duration: f32| MidiNote {
        id: id.into(),
        pitch,
        start,
        duration,
        velocity: 100,
    };
    vec![
        note("n1", 60, 0.0, 1.0),
        note("n2", 64, 0.5, 1.5),
        note("n3", 67, 1.25, 2.0),
        note("n4", 72, 3.0, 1.5),
    ]
}

fn reference_midi_content() -> MidiContent {
    MidiContent {
        notes: reference_midi_notes(),
        channel_controls: vec![],
    }
}

/// Its clip plays the content `"midi-clip"`; build the project with
/// `project_with_reference_midi`.
fn midi_track() -> AppTrack {
    let mut track = empty_track("midi");
    track.midi_clips.push(AppMidiClip {
        id: "midi-clip".into(),
        start: 0.0,
        end: DURATION_S,
        offset: 0.0,
        content_id: "midi-clip".into(),
    });
    track.effects = full_effect_chain();
    track
}

/// Two full repeats of a one-bar clip (120 bpm) and a cut-off third, made by the
/// repeat handle. `n4` runs past the bar and `n2` past the cut, so the clip
/// window matters (ADR-018). ADR-032: sharing one content between these clips
/// must not change a sample. Each clip has a content of its own (`repeat-N`).
const REPEAT_LENGTH_S: f64 = 2.0;
const REPEAT_CUT_OFF_S: f64 = 0.6;

fn repeated_midi_track() -> AppTrack {
    let mut track = empty_track("repeats");
    let ends = [REPEAT_LENGTH_S, REPEAT_LENGTH_S, REPEAT_CUT_OFF_S];
    for (index, length) in ends.into_iter().enumerate() {
        let start = index as f64 * REPEAT_LENGTH_S;
        track.midi_clips.push(AppMidiClip {
            id: format!("repeat-{index}"),
            start,
            end: start + length,
            offset: 0.0,
            content_id: format!("repeat-{index}"),
        });
    }
    track.effects = full_effect_chain();
    track
}

fn lfo(id: &str, rate_hz: f32, waveform: LfoWaveform) -> ProjectModulator {
    ProjectModulator {
        id: id.into(),
        params: ModulatorParams::Lfo(LfoParams {
            rate_hz,
            waveform,
            ..LfoParams::default()
        }),
    }
}

fn effect_route(
    track_id: &str,
    modulator_id: &str,
    effect_id: &str,
    param: &str,
    depth: f32,
) -> ProjectModulationRoute {
    ProjectModulationRoute {
        id: format!("{modulator_id}->{effect_id}.{param}"),
        modulator_id: modulator_id.into(),
        target: ModulationTargetRef::EffectParam {
            track_id: track_id.into(),
            effect_id: effect_id.into(),
            param: param.into(),
        },
        depth,
    }
}

fn add_effect_modulation(track: &mut AppTrack) {
    let id = track.id.clone();
    track
        .modulators
        .push(lfo("lfo-fast", 7.3, LfoWaveform::Sine));
    track
        .modulators
        .push(lfo("lfo-slow", 0.9, LfoWaveform::Triangle));
    track.mod_routes.extend([
        effect_route(&id, "lfo-fast", "eq", "mid_freq", 600.0),
        effect_route(&id, "lfo-slow", "comp", "threshold_db", 8.0),
        effect_route(&id, "lfo-fast", "delay", "feedback", 0.3),
        effect_route(&id, "lfo-slow", "reverb", "room_size", 0.3),
    ]);
}

fn add_gain_modulation(track: &mut AppTrack) {
    track
        .modulators
        .push(lfo("lfo-gain", 5.1, LfoWaveform::Square));
    track.modulators.push(ProjectModulator {
        id: "macro".into(),
        params: ModulatorParams::Macro(MacroParams { value: 0.5 }),
    });
    for (modulator_id, depth) in [("lfo-gain", 0.4), ("macro", -0.2)] {
        track.mod_routes.push(ProjectModulationRoute {
            id: format!("{modulator_id}->gain"),
            modulator_id: modulator_id.into(),
            target: ModulationTargetRef::TrackGain {
                track_id: track.id.clone(),
            },
            depth,
        });
    }
}

fn automation_lane(id: &str, target: ModulationTargetRef, points: &[(f64, f32)]) -> AutomationLane {
    AutomationLane {
        id: id.into(),
        target,
        clips: vec![AutomationClip {
            id: format!("{id}-clip"),
            start_s: 0.0,
            end_s: DURATION_S,
            points: points
                .iter()
                .map(|&(time_s, value)| AutomationPoint { time_s, value })
                .collect(),
        }],
        interpolation: InterpolationMode::Linear,
    }
}

fn add_automation(track: &mut AppTrack) {
    let track_id = track.id.clone();
    track.automation_lanes.push(automation_lane(
        "wet-ramp",
        ModulationTargetRef::EffectParam {
            track_id: track_id.clone(),
            effect_id: "delay".into(),
            param: "wet".into(),
        },
        &[(0.0, 0.0), (1.1, 0.5), (2.2, 0.1)],
    ));
    track.automation_lanes.push(automation_lane(
        "gain-ramp",
        ModulationTargetRef::TrackGain { track_id },
        &[(0.0, 0.0), (0.7, 0.3), (1.9, 0.0)],
    ));
}

fn project_with(tracks: Vec<AppTrack>) -> AppProject {
    let mut project = AppProject::new();
    project.tracks = tracks;
    project
}

fn project_with_midi(
    tracks: Vec<AppTrack>,
    contents: impl IntoIterator<Item = (String, MidiContent)>,
) -> AppProject {
    let mut project = project_with(tracks);
    project.midi_contents.extend(contents);
    project
}

fn project_with_reference_midi(tracks: Vec<AppTrack>) -> AppProject {
    project_with_midi(
        tracks,
        [("midi-clip".to_string(), reference_midi_content())],
    )
}

fn static_mix() -> AppProject {
    project_with_reference_midi(vec![audio_track(), midi_track()])
}

fn modulated_effects() -> AppProject {
    let mut audio = audio_track();
    let mut midi = midi_track();
    add_effect_modulation(&mut audio);
    add_effect_modulation(&mut midi);
    project_with_reference_midi(vec![audio, midi])
}

fn modulated_gain() -> AppProject {
    let mut audio = audio_track();
    add_gain_modulation(&mut audio);
    project_with_reference_midi(vec![audio, midi_track()])
}

fn automated() -> AppProject {
    let mut audio = audio_track();
    let mut midi = midi_track();
    add_automation(&mut audio);
    add_automation(&mut midi);
    project_with_reference_midi(vec![audio, midi])
}

/// One project per automation target family (ADR-025), so converting a node to
/// absolute automation must move only its own golden hash.
fn effect_lanes(track: &mut AppTrack, effect_id: &str, params: &[(&str, &[(f64, f32)])]) {
    for (param, points) in params {
        track.automation_lanes.push(automation_lane(
            &format!("{effect_id}.{param}"),
            ModulationTargetRef::EffectParam {
                track_id: track.id.clone(),
                effect_id: effect_id.into(),
                param: (*param).into(),
            },
            points,
        ));
    }
}

fn automated_effect(effect_id: &str, params: &[(&str, &[(f64, f32)])]) -> AppProject {
    let mut audio = audio_track();
    let mut midi = midi_track();
    effect_lanes(&mut audio, effect_id, params);
    effect_lanes(&mut midi, effect_id, params);
    project_with_reference_midi(vec![audio, midi])
}

fn automated_gain() -> AppProject {
    let mut audio = audio_track();
    let mut midi = midi_track();
    for track in [&mut audio, &mut midi] {
        track.automation_lanes.push(automation_lane(
            "volume",
            ModulationTargetRef::TrackGain {
                track_id: track.id.clone(),
            },
            &[(0.0, 0.2), (0.7, 0.6), (1.4, 0.05), (2.2, 0.45)],
        ));
    }
    project_with_reference_midi(vec![audio, midi])
}

fn automated_eq() -> AppProject {
    automated_effect(
        "eq",
        &[
            ("low_gain_db", &[(0.0, 0.5), (1.0, 0.9), (2.2, 0.1)]),
            ("mid_gain_db", &[(0.0, 0.5), (0.8, 1.0), (2.0, 0.2)]),
            ("mid_freq", &[(0.0, 0.3), (1.2, 0.7), (2.2, 0.4)]),
            ("mid_q", &[(0.0, 0.2), (1.5, 0.6)]),
            ("high_gain_db", &[(0.0, 0.8), (1.6, 0.3)]),
            ("high_freq", &[(0.0, 0.6), (2.0, 0.9)]),
        ],
    )
}

fn automated_compressor() -> AppProject {
    automated_effect(
        "comp",
        &[
            ("threshold_db", &[(0.0, 0.9), (1.2, 0.4), (2.2, 0.7)]),
            ("ratio", &[(0.0, 0.1), (1.0, 0.5), (2.2, 0.3)]),
            ("attack_ms", &[(0.0, 0.2), (2.0, 0.6)]),
            ("release_ms", &[(0.0, 0.3), (2.0, 0.5)]),
            ("makeup_db", &[(0.0, 0.2), (1.5, 0.4)]),
            ("knee_db", &[(0.0, 0.1), (2.0, 0.5)]),
        ],
    )
}

fn automated_reverb() -> AppProject {
    automated_effect(
        "reverb",
        &[
            ("room_size", &[(0.0, 0.3), (1.3, 0.8), (2.2, 0.5)]),
            ("damping", &[(0.0, 0.7), (1.5, 0.2)]),
            ("wet", &[(0.0, 0.1), (0.9, 0.6), (2.2, 0.3)]),
            ("width", &[(0.0, 1.0), (2.0, 0.4)]),
        ],
    )
}

fn automated_delay() -> AppProject {
    automated_effect(
        "delay",
        &[
            ("delay_ms", &[(0.0, 0.05), (1.1, 0.2), (2.2, 0.1)]),
            ("feedback", &[(0.0, 0.2), (1.4, 0.7)]),
            ("wet", &[(0.0, 0.2), (1.0, 0.7), (2.2, 0.3)]),
        ],
    )
}

/// EQ frequencies modulated in octaves (ADR-019 amendment, 2026-09-24).
fn eq_octave_modulation() -> AppProject {
    let mut audio = audio_track();
    let id = audio.id.clone();
    audio
        .modulators
        .push(lfo("lfo-fast", 7.3, LfoWaveform::Sine));
    audio
        .modulators
        .push(lfo("lfo-slow", 0.9, LfoWaveform::Triangle));
    audio.mod_routes.extend([
        effect_route(&id, "lfo-fast", "eq", "mid_freq_octaves", 2.0),
        effect_route(&id, "lfo-slow", "eq", "low_freq_octaves", -1.0),
    ]);
    project_with_reference_midi(vec![audio, midi_track()])
}

fn decaying_noise(seconds: f32, decay: f32, mut seed: u32) -> Vec<f32> {
    let frames = (seconds * CLIP_SAMPLE_RATE as f32) as usize;
    (0..frames)
        .map(|i| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
            noise * (-(i as f32 / CLIP_SAMPLE_RATE as f32) * decay).exp()
        })
        .collect()
}

fn kick_samples() -> Vec<f32> {
    let frames = (0.4 * CLIP_SAMPLE_RATE as f32) as usize;
    (0..frames)
        .map(|i| {
            let t = i as f32 / CLIP_SAMPLE_RATE as f32;
            let frequency = 45.0 + 75.0 * (-t * 20.0).exp();
            (std::f32::consts::TAU * frequency * t).sin() * (-t * 8.0).exp()
        })
        .collect()
}

/// Native rate equal to the engine's, unlike the other pads.
fn tone_samples() -> Vec<f32> {
    let frames = (1.2 * SAMPLE_RATE as f32) as usize;
    (0..frames)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            (std::f32::consts::TAU * 330.0 * t).sin() * (-t * 1.5).exp()
        })
        .collect()
}

fn pad_with(
    kit: &mut SamplerKit,
    index: usize,
    samples: Vec<f32>,
    sample_rate: u32,
    settings: SamplerPad,
) {
    kit.pads[index] = SamplerPad {
        note: kit.pads[index].note,
        sample: Some(PadSample {
            samples: Arc::new(samples),
            sample_rate,
        }),
        ..settings
    };
}

/// One-shot, gate, choke group, layering, tuning, 44.1 → 48 kHz and voice
/// stealing (ADR-028).
fn sampler_track() -> (AppTrack, MidiContent) {
    let mut kit = SamplerKit::default();
    pad_with(
        &mut kit,
        0,
        kick_samples(),
        CLIP_SAMPLE_RATE,
        SamplerPad::default(),
    );
    pad_with(
        &mut kit,
        2,
        decaying_noise(0.3, 12.0, 7),
        CLIP_SAMPLE_RATE,
        SamplerPad {
            mode: PadPlayMode::Gate,
            attack_ms: 2.0,
            release_ms: 60.0,
            tune_semitones: 3.5,
            pan: -0.4,
            gain_db: -2.0,
            ..SamplerPad::default()
        },
    );
    let hat = |pan: f32, gain_db: f32| SamplerPad {
        choke_group: 1,
        pan,
        gain_db,
        ..SamplerPad::default()
    };
    let closed_hat = decaying_noise(0.08, 50.0, 11);
    pad_with(&mut kit, 6, closed_hat, CLIP_SAMPLE_RATE, hat(0.5, -6.0));
    let open_hat = decaying_noise(0.8, 4.0, 13);
    pad_with(&mut kit, 10, open_hat, CLIP_SAMPLE_RATE, hat(0.3, -4.0));
    pad_with(
        &mut kit,
        13,
        tone_samples(),
        SAMPLE_RATE,
        SamplerPad {
            tune_semitones: -7.25,
            gain_db: -12.0,
            ..SamplerPad::default()
        },
    );
    pad_with(
        &mut kit,
        15,
        tone_samples(),
        SAMPLE_RATE,
        SamplerPad {
            tune_semitones: 12.0,
            gain_db: -18.0,
            ..SamplerPad::default()
        },
    );
    kit.pads[15].note = 36;

    let note = |index: usize, pitch: u8, start: f32, duration: f32, velocity: u8| MidiNote {
        id: format!("n{index}"),
        pitch,
        start,
        duration,
        velocity,
    };
    let mut notes = vec![
        note(0, 36, 0.0, 0.2, 110),
        note(1, 36, 1.0, 0.2, 100),
        note(2, 36, 2.0, 0.2, 110),
        note(3, 36, 3.0, 0.2, 90),
        note(4, 36, 4.0, 0.2, 120),
        note(5, 38, 1.0, 0.25, 100),
        note(6, 38, 3.0, 0.5, 80),
        note(7, 46, 0.5, 0.25, 90),
        note(8, 42, 0.75, 0.1, 70),
        note(9, 42, 1.5, 0.1, 75),
        note(10, 46, 2.5, 0.25, 95),
        note(11, 42, 2.75, 0.1, 60),
        note(12, 42, 3.5, 0.1, 80),
    ];
    notes.extend((0..40).map(|i| {
        note(
            100 + i,
            49,
            2.0 + i as f32 / 40.0,
            0.02,
            40 + (i * 7 % 80) as u8,
        )
    }));

    let mut track = empty_track("drums");
    track.instrument = Some(SAMPLER_INSTRUMENT_ID.into());
    track.sampler = Some(kit);
    track.midi_clips.push(AppMidiClip {
        id: "drum-clip".into(),
        start: 0.0,
        end: DURATION_S,
        offset: 0.0,
        content_id: "drum-clip".into(),
    });
    let content = MidiContent {
        notes,
        channel_controls: vec![],
    };
    (track, content)
}

fn sampler_kit() -> AppProject {
    let (track, content) = sampler_track();
    project_with_midi(vec![track], [("drum-clip".to_string(), content)])
}

/// The built-in synth ignores controls (ADR-029): this proves recorded
/// controls cannot disturb the render, and that the clip path with controls
/// stays block-size independent.
fn midi_controls() -> AppProject {
    let control = |start: f32, kind, number: u8, value: u16| MidiControlPoint {
        start,
        kind,
        number,
        value,
    };
    let mut content = reference_midi_content();
    content.channel_controls = vec![
        control(0.0, MidiControlKind::ControlChange, 64, 127),
        control(0.5, MidiControlKind::PitchBend, 0, 0x3000),
        control(1.0, MidiControlKind::ControlChange, 1, 90),
        control(1.5, MidiControlKind::PitchBend, 0, 0x2000),
        control(2.0, MidiControlKind::ChannelPressure, 0, 70),
        control(2.5, MidiControlKind::PolyPressure, 60, 40),
        control(3.0, MidiControlKind::ControlChange, 64, 0),
    ];
    project_with_midi(
        vec![audio_track(), midi_track()],
        [("midi-clip".to_string(), content)],
    )
}

/// A track whose clip plays the given chords: `(pitches, start, duration)`.
fn chord_track(id: &str, chords: &[(&[u8], f32, f32)]) -> (AppTrack, MidiContent) {
    let notes = chords
        .iter()
        .enumerate()
        .flat_map(|(chord_index, &(pitches, start, duration))| {
            pitches.iter().map(move |&pitch| MidiNote {
                id: format!("{chord_index}-{pitch}"),
                pitch,
                start,
                duration,
                velocity: 90 + pitch % 7,
            })
        })
        .collect();
    let mut track = empty_track(id);
    track.midi_clips.push(AppMidiClip {
        id: format!("{id}-clip"),
        start: 0.0,
        end: DURATION_S,
        offset: 0.0,
        content_id: format!("{id}-clip"),
    });
    let content = MidiContent {
        notes,
        channel_controls: vec![],
    };
    (track, content)
}

/// Chords of one, three and four notes through the arpeggiator (ADR-030): two
/// modes, two rates (one a triplet), octaves, gate, and a chord change.
fn arpeggiator_track(
    id: &str,
    chords: &[(&[u8], f32, f32)],
    settings: ArpeggiatorSettings,
) -> (AppTrack, MidiContent) {
    let (mut track, content) = chord_track(id, chords);
    track.arpeggiator = Some(settings);
    (track, content)
}

fn arpeggiator() -> AppProject {
    let (up_down, up_down_content) = arpeggiator_track(
        "arp-up-down",
        &[(&[60, 64, 67], 0.0, 2.0), (&[62, 65, 69, 72], 2.0, 3.0)],
        ArpeggiatorSettings {
            enabled: true,
            mode: ArpMode::UpDown,
            rate: ArpRate::Sixteenth,
            gate_percent: 70,
            octaves: 2,
            ..ArpeggiatorSettings::default()
        },
    );
    let (random, random_content) = arpeggiator_track(
        "arp-random",
        &[(&[48, 55, 60], 0.5, 2.5), (&[43], 3.0, 2.0)],
        ArpeggiatorSettings {
            enabled: true,
            mode: ArpMode::Random,
            rate: ArpRate::Eighth,
            triplet: true,
            gate_percent: 100,
            ..ArpeggiatorSettings::default()
        },
    );
    project_with_midi(
        vec![up_down, random],
        [
            ("arp-up-down-clip".to_string(), up_down_content),
            ("arp-random-clip".to_string(), random_content),
        ],
    )
}

/// Transpose, scale and chord (ADR-038), alone and ahead of the arpeggiator:
/// - a chord whose voices overlap (C and E major share their fifth with G),
/// - out-of-scale melody notes pulled into D dorian after a transposition,
/// - a chord shape played one note at a time by the arpeggiator.
fn note_effects() -> AppProject {
    let (mut stacked, stacked_content) = chord_track(
        "fx-chord",
        &[(&[60, 64, 67], 0.0, 2.0), (&[62, 65], 2.0, 3.0)],
    );
    stacked.note_effects = Some(NoteEffectSettings {
        transpose: TransposeSettings {
            enabled: true,
            semitones: 5,
        },
        chord: ChordSettings {
            enabled: true,
            shape: ChordShape::Minor7,
        },
        ..NoteEffectSettings::default()
    });
    let (mut melody, melody_content) = chord_track(
        "fx-scale",
        &[
            (&[61], 0.0, 0.5),
            (&[63], 0.5, 0.5),
            (&[66], 1.0, 0.5),
            (&[68], 1.5, 0.5),
            (&[70], 2.0, 0.5),
            (&[72], 2.5, 0.5),
        ],
    );
    melody.note_effects = Some(NoteEffectSettings {
        transpose: TransposeSettings {
            enabled: true,
            semitones: -3,
        },
        scale: ScaleSettings {
            enabled: true,
            root: 2,
            scale: ScaleKind::Dorian,
        },
        chord: ChordSettings {
            enabled: true,
            shape: ChordShape::Fifth,
        },
    });
    let (mut arpeggiated, arpeggiated_content) = arpeggiator_track(
        "fx-arp",
        &[(&[55, 62], 0.5, 2.0), (&[57], 3.0, 2.0)],
        ArpeggiatorSettings {
            enabled: true,
            mode: ArpMode::Up,
            rate: ArpRate::Eighth,
            ..ArpeggiatorSettings::default()
        },
    );
    arpeggiated.note_effects = Some(NoteEffectSettings {
        chord: ChordSettings {
            enabled: true,
            shape: ChordShape::Dominant7,
        },
        ..NoteEffectSettings::default()
    });
    project_with_midi(
        vec![stacked, melody, arpeggiated],
        [
            ("fx-chord-clip".to_string(), stacked_content),
            ("fx-scale-clip".to_string(), melody_content),
            ("fx-arp-clip".to_string(), arpeggiated_content),
        ],
    )
}

fn repeated_midi() -> AppProject {
    project_with_midi(
        vec![audio_track(), repeated_midi_track()],
        (0..3).map(|index| (format!("repeat-{index}"), reference_midi_content())),
    )
}

/// Same clips as `repeated_midi`, all playing one content (ADR-032).
fn repeated_midi_linked() -> AppProject {
    let mut project = project_with_midi(
        vec![audio_track(), repeated_midi_track()],
        [("repeat".to_string(), reference_midi_content())],
    );
    for clip in &mut project.tracks[1].midi_clips {
        clip.content_id = "repeat".into();
    }
    project
}

fn reference_projects() -> [(&'static str, AppProject); 15] {
    [
        ("static_mix", static_mix()),
        ("modulated_effects", modulated_effects()),
        ("modulated_gain", modulated_gain()),
        ("automated", automated()),
        ("eq_octave_modulation", eq_octave_modulation()),
        ("automated_gain", automated_gain()),
        ("automated_eq", automated_eq()),
        ("automated_compressor", automated_compressor()),
        ("automated_reverb", automated_reverb()),
        ("automated_delay", automated_delay()),
        ("sampler_kit", sampler_kit()),
        ("midi_controls", midi_controls()),
        ("arpeggiator", arpeggiator()),
        ("note_effects", note_effects()),
        ("repeated_midi", repeated_midi()),
    ]
}

/// Recorded on Windows x86_64.  Only update a value when an ADR sanctions the
/// render change, and say so in the commit.
const GOLDEN_HASHES: [(&str, u64); 15] = [
    ("static_mix", 0x7dec_d259_cfd9_6694),
    ("modulated_effects", 0x29ea_1120_1ae0_2d53),
    ("modulated_gain", 0x23ef_388b_80d0_9a52),
    ("automated", 0x95cf_b495_b3d2_18b4),
    ("eq_octave_modulation", 0xc71b_3d76_d2c0_8257),
    ("automated_gain", 0x8d7b_15cf_82b5_ab90),
    ("automated_eq", 0x2dfa_041a_dfc4_e05c),
    ("automated_compressor", 0x6f12_51b4_7ba9_2a1a),
    ("automated_reverb", 0x8aef_85dc_383f_a3bc),
    ("automated_delay", 0xed2b_9373_7df4_ac29),
    ("sampler_kit", 0x8151_d7f3_7a3c_1e57),
    ("midi_controls", 0x7dec_d259_cfd9_6694),
    ("arpeggiator", 0x94de_c710_d2b7_1fe1),
    ("note_effects", 0xf98b_cd02_1dc9_c040),
    ("repeated_midi", 0x50b2_08b5_fb7b_9696),
];

fn assert_block_size_independent(name: &str, project: &AppProject) {
    let reference = render(project, BLOCK_SIZES[0]);
    assert!(
        reference.iter().any(|s| s.abs() > 1e-3),
        "{name}: reference render is silent — the harness would prove nothing"
    );
    for &block_size in &BLOCK_SIZES[1..] {
        let rendered = render(project, block_size);
        if let Some(index) = first_difference(&reference, &rendered) {
            panic!(
                "{name}: block size {block_size} diverges from {} at frame {} (channel {}): {} vs {}",
                BLOCK_SIZES[0],
                index / 2,
                index % 2,
                reference.get(index).copied().unwrap_or(f32::NAN),
                rendered.get(index).copied().unwrap_or(f32::NAN),
            );
        }
    }
}

#[test]
fn static_mix_is_block_size_independent() {
    assert_block_size_independent("static_mix", &static_mix());
}

#[test]
fn modulated_effects_are_block_size_independent() {
    assert_block_size_independent("modulated_effects", &modulated_effects());
}

#[test]
fn modulated_gain_is_block_size_independent() {
    assert_block_size_independent("modulated_gain", &modulated_gain());
}

#[test]
fn automation_is_block_size_independent() {
    assert_block_size_independent("automated", &automated());
}

#[test]
fn eq_octave_modulation_is_block_size_independent() {
    assert_block_size_independent("eq_octave_modulation", &eq_octave_modulation());
}

#[test]
fn automated_gain_is_block_size_independent() {
    assert_block_size_independent("automated_gain", &automated_gain());
}

#[test]
fn automated_eq_is_block_size_independent() {
    assert_block_size_independent("automated_eq", &automated_eq());
}

#[test]
fn automated_compressor_is_block_size_independent() {
    assert_block_size_independent("automated_compressor", &automated_compressor());
}

#[test]
fn automated_reverb_is_block_size_independent() {
    assert_block_size_independent("automated_reverb", &automated_reverb());
}

#[test]
fn automated_delay_is_block_size_independent() {
    assert_block_size_independent("automated_delay", &automated_delay());
}

#[test]
fn sampler_kit_is_block_size_independent() {
    assert_block_size_independent("sampler_kit", &sampler_kit());
}

#[test]
fn midi_controls_are_block_size_independent() {
    assert_block_size_independent("midi_controls", &midi_controls());
}

#[test]
fn arpeggiator_is_block_size_independent() {
    assert_block_size_independent("arpeggiator", &arpeggiator());
}

#[test]
fn note_effects_are_block_size_independent() {
    assert_block_size_independent("note_effects", &note_effects());
}

#[test]
fn repeated_midi_is_block_size_independent() {
    assert_block_size_independent("repeated_midi", &repeated_midi());
}

#[test]
fn every_repeat_of_a_midi_clip_is_heard() {
    let mut first_repeat_only = repeated_midi();
    first_repeat_only.tracks[1].midi_clips.truncate(1);
    assert_ne!(
        render(&repeated_midi(), BLOCK_SIZES[0]),
        render(&first_repeat_only, BLOCK_SIZES[0])
    );

    let mut without_cut_off = repeated_midi();
    without_cut_off.tracks[1].midi_clips.truncate(2);
    assert_ne!(
        render(&repeated_midi(), BLOCK_SIZES[0]),
        render(&without_cut_off, BLOCK_SIZES[0])
    );
}

#[test]
fn linked_repeats_are_block_size_independent() {
    assert_block_size_independent("repeated_midi_linked", &repeated_midi_linked());
}

#[test]
fn linked_repeats_render_like_independent_copies() {
    let golden = GOLDEN_HASHES
        .iter()
        .find(|(name, _)| *name == "repeated_midi")
        .map(|(_, hash)| *hash)
        .expect("repeated_midi golden hash");
    for block_size in BLOCK_SIZES {
        let linked = render(&repeated_midi_linked(), block_size);
        assert_eq!(linked, render(&repeated_midi(), block_size), "{block_size}");
    }
    assert_eq!(
        render_hash(&render(&repeated_midi_linked(), BLOCK_SIZES[0])),
        golden
    );
}

fn without_note_effects(mut project: AppProject) -> AppProject {
    for track in &mut project.tracks {
        track.note_effects = None;
    }
    project
}

fn with_note_effects_disabled(mut project: AppProject) -> AppProject {
    for track in &mut project.tracks {
        track.note_effects = track.note_effects.map(|_| NoteEffectSettings::default());
    }
    project
}

#[test]
fn enabled_note_effects_change_the_render() {
    assert_ne!(
        render(&note_effects(), BLOCK_SIZES[0]),
        render(&without_note_effects(note_effects()), BLOCK_SIZES[0])
    );
}

#[test]
fn disabled_note_effects_render_exactly_like_none() {
    assert_eq!(
        render(&with_note_effects_disabled(note_effects()), BLOCK_SIZES[0]),
        render(&without_note_effects(note_effects()), BLOCK_SIZES[0])
    );
}

fn without_arpeggiator(mut project: AppProject) -> AppProject {
    for track in &mut project.tracks {
        track.arpeggiator = None;
    }
    project
}

fn with_arpeggiator_disabled(mut project: AppProject) -> AppProject {
    for track in &mut project.tracks {
        if let Some(settings) = &mut track.arpeggiator {
            settings.enabled = false;
        }
    }
    project
}

#[test]
fn an_enabled_arpeggiator_changes_the_render() {
    assert_ne!(
        render(&arpeggiator(), BLOCK_SIZES[0]),
        render(&without_arpeggiator(arpeggiator()), BLOCK_SIZES[0])
    );
}

#[test]
fn a_disabled_arpeggiator_renders_exactly_like_none() {
    assert_eq!(
        render(&with_arpeggiator_disabled(arpeggiator()), BLOCK_SIZES[0]),
        render(&without_arpeggiator(arpeggiator()), BLOCK_SIZES[0])
    );
}

#[test]
fn controls_do_not_change_what_the_built_in_synth_plays() {
    assert_eq!(
        render(&midi_controls(), BLOCK_SIZES[0]),
        render(&static_mix(), BLOCK_SIZES[0])
    );
}

#[test]
fn sampler_is_not_reported_as_a_missing_plugin() {
    let live_params = LiveParams::new();
    let mut host = PluginHost::new();
    render_project_in_blocks(&sampler_kit(), &live_params, &mut host, SAMPLE_RATE, 512)
        .expect("project must render");
    assert!(host.unavailable_plugins().is_empty());
}

#[test]
fn renders_match_golden_hashes() {
    let mismatches: Vec<String> = reference_projects()
        .iter()
        .zip(GOLDEN_HASHES)
        .filter_map(|((name, project), (golden_name, golden))| {
            assert_eq!(
                *name, golden_name,
                "GOLDEN_HASHES order must match reference_projects"
            );
            let actual = render_hash(&render(project, BLOCK_SIZES[0]));
            (actual != golden)
                .then(|| format!("{name}: expected {golden:#018x}, got {actual:#018x}"))
        })
        .collect();
    assert!(
        mismatches.is_empty(),
        "golden render changed:\n{}",
        mismatches.join("\n")
    );
}

#[test]
fn missing_instrument_plugin_is_silent_and_reported() {
    let mut track = midi_track();
    track.effects.clear();
    track.instrument = Some("clap:not-installed".into());
    let project = project_with_reference_midi(vec![track]);

    let live_params = LiveParams::new();
    let mut host = PluginHost::new();
    let (left, right) =
        render_project_in_blocks(&project, &live_params, &mut host, SAMPLE_RATE, 512)
            .expect("project must render");

    assert!(
        left.iter().chain(&right).all(|s| *s == 0.0),
        "a missing instrument must not be replaced by another sound"
    );
    let unavailable = host.unavailable_plugins();
    assert_eq!(unavailable.len(), 1);
    assert_eq!(unavailable[0].instance_key, "instrument:midi");
    assert_eq!(
        unavailable[0].reason,
        crate::plugin_system::host::UnavailableReason::NotInstalled
    );
}

const EXPORT_CLIP_SECONDS: f64 = 0.5;
const EXPORT_TAIL_CAP_SECONDS: f64 = 10.0;

fn clip_through_delay(delay: DelayParams) -> AppProject {
    let mut track = audio_track();
    track.clips[0].end = EXPORT_CLIP_SECONDS;
    track.effects = vec![effect("delay", EffectParams::Delay(delay))];
    project_with(vec![track])
}

fn export(project: &AppProject) -> (Vec<f32>, Vec<f32>) {
    render_project_offline(
        project,
        &LiveParams::new(),
        &mut PluginHost::new(),
        SAMPLE_RATE,
    )
    .expect("reference project must export")
}

#[test]
fn export_keeps_the_echoes_after_the_last_clip() {
    let project = clip_through_delay(DelayParams {
        delay_ms: 400.0,
        feedback: 0.5,
        wet: 0.5,
        dry: 1.0,
        ping_pong: false,
    });
    let clip_frames = (EXPORT_CLIP_SECONDS * SAMPLE_RATE as f64) as usize;
    let (left, right) = export(&project);

    assert!(left.len() > clip_frames, "the echoes must not be cut");
    assert!(
        left.len() < clip_frames + (EXPORT_TAIL_CAP_SECONDS * SAMPLE_RATE as f64) as usize,
        "the tail must end on silence, not on the cap"
    );
    assert!(
        left[clip_frames..].iter().any(|s| s.abs() > 1e-3),
        "an echo must sound after the clip"
    );
    assert!(
        left[left.len() - 1].abs() > 1e-4 || right[right.len() - 1].abs() > 1e-4,
        "the file must end on the last audible sample"
    );

    let cut = render(&project, BLOCK_SIZES[0]);
    let exported: Vec<f32> = left
        .iter()
        .zip(&right)
        .flat_map(|(l, r)| [*l, *r])
        .collect();
    assert_eq!(
        first_difference(&exported[..cut.len()], &cut),
        None,
        "the tail must not change what comes before it"
    );
}

#[test]
fn export_tail_stops_at_the_cap_for_a_sound_that_never_ends() {
    let project = clip_through_delay(DelayParams {
        delay_ms: 200.0,
        feedback: 0.95,
        wet: 1.0,
        dry: 1.0,
        ping_pong: false,
    });
    let clip_frames = (EXPORT_CLIP_SECONDS * SAMPLE_RATE as f64) as usize;
    let (left, _) = export(&project);

    assert_eq!(
        left.len(),
        clip_frames + (EXPORT_TAIL_CAP_SECONDS * SAMPLE_RATE as f64) as usize
    );
}

/// 16 tracks, each with three EQ lanes, so the cost of the automation mapping shows up in a render.
fn stress_eq_project(params: [&str; 3]) -> AppProject {
    let points: &[(f64, f32)] = &[(0.0, 0.2), (0.6, 0.8), (1.3, 0.3), (2.2, 0.7)];
    let tracks = (0..16)
        .map(|index| {
            let mut track = audio_track();
            track.id = format!("audio{index}");
            effect_lanes(&mut track, "eq", &params.map(|param| (param, points)));
            track
        })
        .collect();
    project_with(tracks)
}

fn best_render_time(project: &AppProject) -> std::time::Duration {
    (0..3)
        .map(|_| {
            let started = std::time::Instant::now();
            std::hint::black_box(render(project, 512));
            started.elapsed()
        })
        .min()
        .expect("three timings")
}

/// ADR-025 cost check. Timings mean nothing in a debug build:
/// `cargo test --release --bin echoes-studio automation_cost -- --ignored --nocapture`
#[test]
#[ignore = "timing report, run in release"]
fn automation_cost_report() {
    use crate::audio_graph::nodes::{automated_value, ParamScale};

    const CALLS: u32 = 20_000_000;
    let range = (20.0_f32, 20_000.0_f32);
    for scale in [ParamScale::Linear, ParamScale::Log] {
        let started = std::time::Instant::now();
        let mut sum = 0.0_f32;
        for call in 0..CALLS {
            sum += automated_value(
                std::hint::black_box(call as f32 / CALLS as f32),
                range,
                scale,
            );
        }
        std::hint::black_box(sum);
        let nanos = started.elapsed().as_nanos() as f64 / CALLS as f64;
        println!("automated_value {scale:?}: {nanos:.1} ns per call");
    }

    let linear = best_render_time(&stress_eq_project([
        "low_gain_db",
        "mid_gain_db",
        "high_gain_db",
    ]));
    let log = best_render_time(&stress_eq_project(["low_freq", "mid_freq", "high_freq"]));
    let baseline = best_render_time(&project_with(
        (0..16)
            .map(|index| {
                let mut track = audio_track();
                track.id = format!("audio{index}");
                track
            })
            .collect(),
    ));
    let audio_seconds = DURATION_S;
    println!(
        "16 tracks, no automation:        {baseline:?} ({:.0}x real time)",
        audio_seconds / baseline.as_secs_f64()
    );
    println!(
        "16 tracks, 3 linear EQ lanes each: {linear:?} ({:.0}x real time)",
        audio_seconds / linear.as_secs_f64()
    );
    println!(
        "16 tracks, 3 log EQ lanes each:    {log:?} ({:.0}x real time)",
        audio_seconds / log.as_secs_f64()
    );
}

#[cfg(windows)]
mod clap_plugins {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::OnceLock;

    use super::*;
    use crate::plugin_system::host::PluginIsolation;
    use crate::plugin_system::registry::PluginManifest;
    use crate::plugin_system::wrapper::DeviceFormat;
    use crate::project::PluginParams;

    pub const PLUGIN_ID: &str = "clap:echoes.test.gain";

    /// `(test plugin dll, bridge exe)`, built once per test run.
    pub(super) fn artifacts() -> &'static (PathBuf, PathBuf) {
        static ARTIFACTS: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
        ARTIFACTS.get_or_init(|| {
            let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
            let target = workspace.join("target").join("clap-render-harness");
            let status = Command::new(env!("CARGO"))
                .args([
                    "build",
                    "--quiet",
                    "-p",
                    "test_clap_plugin",
                    "-p",
                    "plugin_host_bridge",
                    "--target-dir",
                ])
                .arg(&target)
                .current_dir(workspace)
                .status()
                .expect("run cargo");
            assert!(
                status.success(),
                "building test_clap_plugin and plugin_host_bridge failed"
            );
            let debug = target.join("debug");
            (
                debug.join("test_clap_plugin.dll"),
                debug.join("plugin_host_bridge.exe"),
            )
        })
    }

    fn test_plugin_dll() -> &'static PathBuf {
        &artifacts().0
    }

    fn host_with_test_plugins(plugin_id: &str) -> PluginHost {
        let mut host = PluginHost::new();
        host.registry_mut().add(PluginManifest {
            id: plugin_id.into(),
            name: "Echoes Test Gain".into(),
            vendor: "Echoes Studio".into(),
            format: DeviceFormat::Clap,
            path: Some(test_plugin_dll().clone()),
            categories: vec![],
            num_sidechain_inputs: 0,
            is_instrument: false,
        });
        host
    }

    pub fn render_with_plugin(
        project: &AppProject,
        plugin_id: &str,
        block_size: usize,
    ) -> Vec<f32> {
        render_with_host(&mut host_with_test_plugins(plugin_id), project, block_size)
    }

    fn render_with_host(
        host: &mut PluginHost,
        project: &AppProject,
        block_size: usize,
    ) -> Vec<f32> {
        let live_params = LiveParams::new();
        let (left, right) =
            render_project_in_blocks(project, &live_params, host, SAMPLE_RATE, block_size)
                .expect("reference project must render");
        assert!(
            host.unavailable_plugins().is_empty(),
            "the test plugin must load"
        );
        left.into_iter()
            .zip(right)
            .flat_map(|(l, r)| [l, r])
            .collect()
    }

    fn gain_effect(plugin_id: &str, params: Vec<(u32, f32)>) -> TrackEffect {
        effect(
            "plugin",
            EffectParams::Plugin(PluginParams {
                plugin_id: plugin_id.into(),
                name: "Echoes Test Gain".into(),
                format: "clap".into(),
                params,
                sandboxed: false,
                sidechain_source: None,
            }),
        )
    }

    /// Clip starting at 1.0 s (silent lead-in) through the plugin as an
    /// effect, and a CLAP instrument playing two notes with a gap between.
    pub fn project(plugin_id: &str, effect_params: Vec<(u32, f32)>) -> AppProject {
        let mut audio = audio_track();
        audio.clips[0].start = 1.0;
        audio.clips[0].end = 1.0 + DURATION_S;
        audio.effects = vec![gain_effect(plugin_id, effect_params)];

        let mut midi = midi_track();
        midi.effects.clear();
        midi.instrument = Some(plugin_id.into());
        let note = |id: &str, pitch: u8, start: f32| MidiNote {
            id: id.into(),
            pitch,
            start,
            duration: 1.0,
            velocity: 100,
        };
        let content = MidiContent {
            notes: vec![note("n1", 60, 0.0), note("n2", 64, 3.0)],
            channel_controls: vec![],
        };

        project_with_midi(vec![audio, midi], [("midi-clip".to_string(), content)])
    }

    fn peak_frame(samples: &[f32]) -> usize {
        samples
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .map(|(frame, _)| frame)
            .expect("render must not be empty")
    }

    /// A plugin that reports 64 samples of latency delays the whole mix (the other
    /// tracks wait for it); the export removes that delay, the harness path does
    /// not.  The test plugin only reports its latency without delaying its audio,
    /// so the impulse rides a plain track.
    #[test]
    fn export_lines_up_with_the_timeline_despite_plugin_latency() {
        const PLUGIN_LATENCY: usize = 64;
        const IMPULSE_FRAME: usize = 100;
        const CLIP_START_SECONDS: f64 = 0.5;

        let mut samples = vec![0.0_f32; SAMPLE_RATE as usize / 10];
        samples[IMPULSE_FRAME] = 1.0;
        let mut track = empty_track("audio");
        track.clips.push(AppClip {
            id: "impulse".into(),
            start: CLIP_START_SECONDS,
            end: CLIP_START_SECONDS + 0.1,
            offset: 0.0,
            file_path: String::new(),
            raw_samples: Arc::new(samples),
            sample_rate: SAMPLE_RATE,
            peaks_64: vec![],
            peaks_512: vec![],
            peaks_4096: vec![],
        });
        let mut latent = empty_track("latent");
        latent.effects = vec![gain_effect(PLUGIN_ID, vec![])];
        let project = project_with(vec![track, latent]);

        let (left, _) = render_project_offline(
            &project,
            &LiveParams::new(),
            &mut host_with_test_plugins(PLUGIN_ID),
            SAMPLE_RATE,
        )
        .expect("project must export");
        let uncompensated: Vec<f32> = render_with_plugin(&project, PLUGIN_ID, BLOCK_SIZES[0])
            .chunks_exact(2)
            .map(|frame| frame[0])
            .collect();

        let timeline_frame = (CLIP_START_SECONDS * SAMPLE_RATE as f64) as usize + IMPULSE_FRAME;
        assert_eq!(peak_frame(&left), timeline_frame);
        assert_eq!(peak_frame(&uncompensated), timeline_frame + PLUGIN_LATENCY);
        assert_eq!(left.len(), uncompensated.len());
    }

    #[test]
    fn clap_plugins_are_audible_with_silent_gaps() {
        let render = render_with_plugin(&project(PLUGIN_ID, vec![]), PLUGIN_ID, BLOCK_SIZES[0]);
        assert!(render.iter().any(|s| s.abs() > 1e-3), "silent render");
        let frame = |seconds: f64| (seconds * SAMPLE_RATE as f64) as usize * 2;
        assert!(
            render[frame(0.1)..frame(0.2)]
                .iter()
                .any(|s| s.abs() > 1e-3),
            "first instrument note must sound"
        );
        assert!(
            render[frame(0.6)..frame(0.9)]
                .iter()
                .all(|s| s.abs() <= 1e-3),
            "the gap before the second note and the audio clip must be silent"
        );
    }

    #[test]
    fn clap_plugins_match_golden_hash() {
        let render = render_with_plugin(&project(PLUGIN_ID, vec![]), PLUGIN_ID, BLOCK_SIZES[0]);
        assert_eq!(render_hash(&render), CLAP_PLUGINS_GOLDEN_HASH);
    }

    /// Track info is read by the plugins during the compile and never reaches the
    /// audio: the golden hash holds, and a rename only tells the plugins of its track.
    #[test]
    fn clap_plugins_read_their_track_without_changing_the_render() {
        const TRACK_NAME_LENGTH: u32 = 15;
        const TRACK_INFO_READS: u32 = 16;
        const TRACK_INFO_CHANGES: u32 = 17;
        let mut project = project(PLUGIN_ID, vec![]);
        let instrument_key = format!("instrument:{}", project.tracks[1].id);
        let mut host = host_with_test_plugins(PLUGIN_ID);
        let seen = |host: &PluginHost, key: &str, native_id: u32| {
            host.instance_by_key(key)
                .unwrap()
                .param_value(native_id)
                .unwrap()
        };

        let render = render_with_host(&mut host, &project, BLOCK_SIZES[0]);
        assert_eq!(render_hash(&render), CLAP_PLUGINS_GOLDEN_HASH);
        for (key, track) in [("plugin", 0), (instrument_key.as_str(), 1)] {
            assert_eq!(
                seen(&host, key, TRACK_NAME_LENGTH),
                project.tracks[track].name.len() as f64
            );
            assert_eq!(seen(&host, key, TRACK_INFO_READS), 1.0);
            assert_eq!(seen(&host, key, TRACK_INFO_CHANGES), 0.0);
        }

        project.tracks[0].name = "Renamed".into();
        let render = render_with_host(&mut host, &project, BLOCK_SIZES[0]);
        assert_eq!(render_hash(&render), CLAP_PLUGINS_GOLDEN_HASH);
        assert_eq!(seen(&host, "plugin", TRACK_INFO_CHANGES), 1.0);
        assert_eq!(seen(&host, "plugin", TRACK_NAME_LENGTH), 7.0);
        assert_eq!(seen(&host, &instrument_key, TRACK_INFO_CHANGES), 0.0);
    }

    const SLEEPY_ID: &str = "clap:echoes.test.sleepy";
    const STATUS: u32 = 6;
    const TAIL: u32 = 7;
    const PROCESS_CALLS: u32 = 8;
    const TAIL_RANGE: f32 = 1_000_000.0;
    const TAIL_SAMPLES: f32 = 200.0;

    fn effect_process_calls(host: &PluginHost, key: &str) -> u32 {
        use crate::plugin_formats::clap::ffi::{ClapPluginParams, CLAP_EXT_PARAMS};
        let plugin = host
            .instance_by_key(key)
            .unwrap()
            .clap_plugin()
            .expect("in-process CLAP plugin");
        // SAFETY: the render is over; the host still holds the live instance.
        unsafe {
            let params = ((*plugin).get_extension)(plugin, CLAP_EXT_PARAMS.as_ptr() as *const _)
                as *const ClapPluginParams;
            let mut value = f64::NAN;
            assert!(((*params).get_value)(plugin, PROCESS_CALLS, &mut value));
            value as u32
        }
    }

    /// `(render, process calls of the effect, process calls of the instrument)`.
    fn render_counting_calls(
        project: &AppProject,
        plugin_id: &str,
        queued_effect_params: &[(u32, f32)],
        block_size: usize,
    ) -> (Vec<f32>, u32, u32) {
        let live_params = LiveParams::new();
        let mut host = host_with_test_plugins(plugin_id);
        if !queued_effect_params.is_empty() {
            // The compiler reuses a live instance (ADR-016): configure it first.
            host.instantiate("plugin", plugin_id, false)
                .expect("the effect must load");
            for &(native_id, value) in queued_effect_params {
                assert!(host.push_plugin_param("plugin", native_id, value).unwrap());
            }
        }
        let (left, right) =
            render_project_in_blocks(project, &live_params, &mut host, SAMPLE_RATE, block_size)
                .expect("reference project must render");
        let render = left
            .into_iter()
            .zip(right)
            .flat_map(|(l, r)| [l, r])
            .collect();
        (
            render,
            effect_process_calls(&host, "plugin"),
            effect_process_calls(&host, "instrument:midi"),
        )
    }

    fn tail_effect_params() -> [(u32, f32); 2] {
        [(STATUS, 3.0 / 4.0), (TAIL, TAIL_SAMPLES / TAIL_RANGE)]
    }

    #[test]
    fn a_sleeping_instrument_renders_exactly_like_an_awake_one() {
        for block_size in BLOCK_SIZES {
            let (awake, _, awake_calls) =
                render_counting_calls(&project(PLUGIN_ID, vec![]), PLUGIN_ID, &[], block_size);
            let (asleep, _, asleep_calls) =
                render_counting_calls(&project(SLEEPY_ID, vec![]), SLEEPY_ID, &[], block_size);
            assert_eq!(
                first_difference(&awake, &asleep),
                None,
                "block size {block_size}"
            );
            assert!(
                asleep_calls < awake_calls,
                "block size {block_size}: the instrument must really sleep                  ({asleep_calls} calls vs {awake_calls})"
            );
        }
    }

    #[test]
    fn a_sleeping_effect_renders_exactly_like_an_awake_one() {
        for block_size in BLOCK_SIZES {
            let (awake, awake_calls, _) =
                render_counting_calls(&project(PLUGIN_ID, vec![]), PLUGIN_ID, &[], block_size);
            let (asleep, asleep_calls, _) = render_counting_calls(
                &project(PLUGIN_ID, vec![]),
                PLUGIN_ID,
                &tail_effect_params(),
                block_size,
            );
            assert_eq!(
                first_difference(&awake, &asleep),
                None,
                "block size {block_size}"
            );
            assert!(
                asleep_calls < awake_calls,
                "block size {block_size}: the effect must really sleep                  ({asleep_calls} calls vs {awake_calls})"
            );
        }
    }

    /// Recorded on Windows x86_64, separate from `GOLDEN_HASHES` because it
    /// needs the built test plugin. Moved on 2026-11-25 from `0x928da7dcd260c4e1`:
    /// the CLAP instrument now tells the delay compensation its `clap.latency`
    /// (ADR-013 amendment 2026-11-25), and the test plugin reports 64 samples
    /// without delaying its audio.
    const CLAP_PLUGINS_GOLDEN_HASH: u64 = 0x906f691e9328705d;

    const GAIN: u32 = 0;

    /// The `clap_plugins` project with the plugin's gain automated and modulated
    /// on the effect and on the instrument, so parameter events with sample
    /// offsets cross the bridge as well as notes.
    fn automated_and_modulated_project() -> AppProject {
        let mut project = project(PLUGIN_ID, vec![(GAIN, 0.8)]);
        for (track, plugin_key) in [(0, "plugin"), (1, "instrument:midi")] {
            let track = &mut project.tracks[track];
            let target = ModulationTargetRef::PluginParam {
                effect_id: plugin_key.into(),
                native_id: GAIN,
            };
            track
                .modulators
                .push(lfo("gain-lfo", 3.7, LfoWaveform::Triangle));
            track.mod_routes.push(ProjectModulationRoute {
                id: "gain-lfo->gain".into(),
                modulator_id: "gain-lfo".into(),
                target: target.clone(),
                depth: 0.3,
            });
            track.automation_lanes.push(automation_lane(
                "gain-ramp",
                target,
                &[(0.0, 0.6), (1.3, 1.4), (2.4, 0.9)],
            ));
        }
        project
    }

    /// A host that sends every plugin it can to the bridge, whatever the effect asks.
    fn host_isolating_everything() -> PluginHost {
        let mut host = host_with_test_plugins(PLUGIN_ID);
        host.set_bridge_exe(artifacts().1.clone());
        host.set_isolation(PluginIsolation::All, Default::default());
        host
    }

    /// ADR-033, C-2: the plugin in the bridge process renders the same samples as
    /// in the Echoes process (offline wait, ADR-015), whatever the block size.
    fn assert_sandbox_matches_in_process(project: &AppProject) {
        for block_size in BLOCK_SIZES {
            let in_process =
                render_with_host(&mut host_with_test_plugins(PLUGIN_ID), project, block_size);
            let mut sandboxed_host = host_isolating_everything();
            let sandboxed = render_with_host(&mut sandboxed_host, project, block_size);
            for key in ["plugin", "instrument:midi"] {
                assert!(
                    sandboxed_host.instance_by_key(key).unwrap().is_sandboxed(),
                    "{key} must really run in the bridge"
                );
            }
            assert_eq!(
                first_difference(&in_process, &sandboxed),
                None,
                "block size {block_size}"
            );
        }
    }

    #[test]
    fn a_sandboxed_plugin_renders_like_an_in_process_one() {
        assert_sandbox_matches_in_process(&project(PLUGIN_ID, vec![(GAIN, 0.8)]));
    }

    #[test]
    fn a_sandboxed_plugin_renders_like_an_in_process_one_with_automation_and_modulation() {
        let automated = automated_and_modulated_project();
        let render = render_with_plugin(&automated, PLUGIN_ID, BLOCK_SIZES[0]);
        let static_render = render_with_plugin(
            &project(PLUGIN_ID, vec![(GAIN, 0.8)]),
            PLUGIN_ID,
            BLOCK_SIZES[0],
        );
        assert!(
            first_difference(&render, &static_render).is_some(),
            "the automation and the modulation must change the sound"
        );
        assert_sandbox_matches_in_process(&automated);
    }
}

/// The VST3 counterpart of `clap_plugins`: the built `crates/test_vst3_plugin` as an
/// effect and as an instrument, in the Echoes process (ADR-033, step 2). Recorded
/// before the VST3 host code moves into its own crate, so the move must not change it.
#[cfg(windows)]
mod vst3_plugins {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    use test_vst3_plugin::{DIAGNOSTIC_RANGE, PROCESSING_STOPS};

    use super::*;
    use crate::audio_engine::graph::AudioGraphCompiler;
    use crate::audio_engine::stream_gate::{LiveStreamGate, EXPORT_HOLD_BUDGET};
    use crate::audio_graph::node::TransportInfo;
    use crate::audio_graph::CompileConfig;
    use crate::plugin_formats::vst3::test_dll::{effect_id, instrument_id, test_vst3_dll};
    use crate::plugin_system::host::PluginIsolation;
    use crate::plugin_system::instance::PluginInstance;
    use crate::plugin_system::registry::PluginManifest;
    use crate::plugin_system::wrapper::DeviceFormat;
    use crate::project::PluginParams;

    fn host_with_test_plugins() -> PluginHost {
        host_with_sidechain_inputs(0)
    }

    /// The registry as a scan would fill it: `sidechain_inputs` is what it counted for
    /// the effect (the test effect has one), and sizes the effect's input ports.
    fn host_with_sidechain_inputs(sidechain_inputs: usize) -> PluginHost {
        let mut host = PluginHost::new();
        for (id, name, is_instrument) in [
            (effect_id(), "Echoes Test Gain", false),
            (instrument_id(), "Echoes Test Synth", true),
        ] {
            host.registry_mut().add(PluginManifest {
                id,
                name: name.into(),
                vendor: "Echoes Studio".into(),
                format: DeviceFormat::Vst3,
                path: Some(test_vst3_dll().clone()),
                categories: vec![],
                num_sidechain_inputs: if is_instrument { 0 } else { sidechain_inputs },
                is_instrument,
            });
        }
        host
    }

    fn render(project: &AppProject, block_size: usize) -> Vec<f32> {
        let mut host = host_with_test_plugins();
        let (left, right) = render_project_in_blocks(
            project,
            &LiveParams::new(),
            &mut host,
            SAMPLE_RATE,
            block_size,
        )
        .expect("reference project must render");
        assert!(
            host.unavailable_plugins().is_empty(),
            "the test plugin must load"
        );
        left.into_iter()
            .zip(right)
            .flat_map(|(l, r)| [l, r])
            .collect()
    }

    /// The audio clip starts at 1.0 s (silent lead-in) and goes through the effect;
    /// the instrument plays two notes with a gap between.
    fn project() -> AppProject {
        let mut audio = audio_track();
        audio.clips[0].start = 1.0;
        audio.clips[0].end = 1.0 + DURATION_S;
        audio.effects = vec![effect(
            "plugin",
            EffectParams::Plugin(PluginParams {
                plugin_id: effect_id(),
                name: "Echoes Test Gain".into(),
                format: "vst3".into(),
                params: vec![],
                sandboxed: false,
                sidechain_source: None,
            }),
        )];

        let mut midi = midi_track();
        midi.effects.clear();
        midi.instrument = Some(instrument_id());
        let note = |id: &str, pitch: u8, start: f32| MidiNote {
            id: id.into(),
            pitch,
            start,
            duration: 1.0,
            velocity: 100,
        };
        let content = MidiContent {
            notes: vec![note("n1", 60, 0.0), note("n2", 64, 3.0)],
            channel_controls: vec![],
        };

        project_with_midi(vec![audio, midi], [("midi-clip".to_string(), content)])
    }

    #[test]
    fn vst3_plugins_are_audible_with_silent_gaps() {
        let render = render(&project(), BLOCK_SIZES[0]);
        assert!(render.iter().any(|s| s.abs() > 1e-3), "silent render");
        let frame = |seconds: f64| (seconds * SAMPLE_RATE as f64) as usize * 2;
        assert!(
            render[frame(0.1)..frame(0.2)]
                .iter()
                .any(|s| s.abs() > 1e-3),
            "first instrument note must sound"
        );
        assert!(
            render[frame(0.6)..frame(0.9)]
                .iter()
                .all(|s| s.abs() <= 1e-3),
            "the gap before the second note and the audio clip must be silent"
        );
        assert!(
            render[frame(1.1)..frame(1.3)]
                .iter()
                .any(|s| s.abs() > 1e-3),
            "the audio clip must come through the effect"
        );
    }

    #[test]
    fn vst3_plugins_are_block_size_independent() {
        let reference = render(&project(), BLOCK_SIZES[0]);
        for block_size in &BLOCK_SIZES[1..] {
            assert_eq!(
                first_difference(&reference, &render(&project(), *block_size)),
                None,
                "block size {block_size}"
            );
        }
    }

    #[test]
    fn vst3_plugins_match_golden_hash() {
        let render = render(&project(), BLOCK_SIZES[0]);
        assert_eq!(render_hash(&render), VST3_PLUGINS_GOLDEN_HASH);
    }

    /// The test effect's state is its gain; loading it at half scale makes the
    /// plugin's processing audible in the render (its default gain is 1).
    fn with_half_gain(mut host: PluginHost) -> PluginHost {
        host.set_pending_state_blobs(HashMap::from([(
            "plugin".to_string(),
            0.5_f64.to_le_bytes().to_vec(),
        )]));
        host
    }

    fn render_with_host(
        host: &mut PluginHost,
        project: &AppProject,
        block_size: usize,
    ) -> Vec<f32> {
        let (left, right) =
            render_project_in_blocks(project, &LiveParams::new(), host, SAMPLE_RATE, block_size)
                .expect("reference project must render");
        assert!(
            host.unavailable_plugins().is_empty(),
            "the test plugin must load"
        );
        left.into_iter()
            .zip(right)
            .flat_map(|(l, r)| [l, r])
            .collect()
    }

    /// A host that sends every plugin it can to the bridge, instruments included.
    fn isolating_everything(mut host: PluginHost) -> PluginHost {
        host.set_bridge_exe(super::clap_plugins::artifacts().1.clone());
        host.set_isolation(PluginIsolation::All, Default::default());
        host
    }

    fn host_isolating_everything() -> PluginHost {
        isolating_everything(with_half_gain(host_with_test_plugins()))
    }

    /// The staged state blobs make every compile build a new instance (project load,
    /// ADR-010); once the first compile has consumed them, the next one reuses the
    /// instance, as an export does after the project was opened.
    fn shared_with_the_next_compile(host: &mut PluginHost) -> Arc<PluginInstance> {
        host.set_pending_state_blobs(HashMap::new());
        host.instance_by_key("plugin").expect("the effect is live")
    }

    /// Enough exports for an ungated one to meet the live stream inside the plugin.
    const EXPORTS_BESIDE_THE_LIVE_STREAM: usize = 20;

    fn live_config() -> CompileConfig {
        CompileConfig {
            sample_rate: SAMPLE_RATE as f32,
            max_block_size: 4096,
            num_channels: 2,
            live: true,
        }
    }

    /// ADR-036: while the live stream keeps calling a plugin the export shares with it,
    /// the export holds the plugins, so no block of the file is left dry and the file is
    /// the one an export alone gives.
    #[test]
    fn an_export_is_the_same_file_while_the_live_stream_runs_beside_it() {
        let project = project();
        let alone = render_project_offline(
            &project,
            &LiveParams::new(),
            &mut with_half_gain(host_with_test_plugins()),
            SAMPLE_RATE,
        )
        .expect("the project exports");

        let mut host = with_half_gain(host_with_test_plugins());
        let live_params = LiveParams::new();
        let (mut live_scheduler, _cells, _duration) =
            AudioGraphCompiler::compile_scheduler(&project, live_config(), &live_params, &mut host);
        let live_instance = shared_with_the_next_compile(&mut host);
        let gate = LiveStreamGate::new();
        let stop = Arc::new(AtomicBool::new(false));
        let live_blocks = Arc::new(AtomicU32::new(0));
        let live_stream = {
            let (gate, stop, live_blocks) = (
                Arc::clone(&gate),
                Arc::clone(&stop),
                Arc::clone(&live_blocks),
            );
            std::thread::spawn(move || {
                // Plays the stretch of the timeline where the audio clip sounds, so
                // every live block really calls the effect.
                let first = SAMPLE_RATE as u64;
                let mut position = first;
                let mut output = vec![0.0_f32; 128 * 2];
                while !stop.load(Ordering::Relaxed) {
                    let Some(_block) = gate.enter_live() else {
                        std::thread::yield_now();
                        continue;
                    };
                    let info = TransportInfo {
                        is_playing: true,
                        position_samples: position,
                        position_seconds: position as f64 / SAMPLE_RATE as f64,
                        block_size: 128,
                        ..TransportInfo::stopped(SAMPLE_RATE as f32)
                    };
                    live_scheduler.process_block(&info, &mut output);
                    live_blocks.fetch_add(1, Ordering::Relaxed);
                    position = first + (position - first + 128) % (2 * SAMPLE_RATE as u64);
                }
            })
        };
        while live_blocks.load(Ordering::Relaxed) < 50 {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        let mut exports = Vec::new();
        for _ in 0..EXPORTS_BESIDE_THE_LIVE_STREAM {
            let _hold = gate
                .hold_for_export(EXPORT_HOLD_BUDGET)
                .expect("the live stream lets go");
            exports.push(
                render_project_offline(&project, &live_params, &mut host, SAMPLE_RATE)
                    .expect("the project exports"),
            );
        }
        stop.store(true, Ordering::Relaxed);
        live_stream.join().expect("the live stream thread ends");
        assert!(
            Arc::ptr_eq(&live_instance, &host.instance_by_key("plugin").unwrap()),
            "the export must share the plugin with the live stream"
        );

        for (index, exported) in exports.iter().enumerate() {
            assert_eq!(exported.0.len(), alone.0.len(), "export {index}");
            assert_eq!(
                first_difference(&alone.0, &exported.0),
                None,
                "left of export {index}"
            );
            assert_eq!(
                first_difference(&alone.1, &exported.1),
                None,
                "right of export {index}"
            );
        }
    }

    /// ADR-036: the plugin forgets what the live stream played before the export starts.
    #[test]
    fn an_export_resets_the_plugins_it_shares_with_the_live_stream() {
        let project = project();
        let mut host = with_half_gain(host_with_test_plugins());
        let live_params = LiveParams::new();
        let (_live_scheduler, _cells, _duration) =
            AudioGraphCompiler::compile_scheduler(&project, live_config(), &live_params, &mut host);
        let live_instance = shared_with_the_next_compile(&mut host);
        let stops = |instance: &PluginInstance| {
            let normalised = instance
                .param_value(PROCESSING_STOPS)
                .expect("the plugin reports its stops");
            (normalised * DIAGNOSTIC_RANGE).round() as u32
        };
        let before = stops(&live_instance);

        render_project_offline(&project, &live_params, &mut host, SAMPLE_RATE)
            .expect("the project exports");

        assert!(
            Arc::ptr_eq(&live_instance, &host.instance_by_key("plugin").unwrap()),
            "the export must share the plugin with the live stream"
        );
        assert!(
            stops(&live_instance) > before,
            "the export must reset the plugin"
        );
    }

    /// ADR-033 step 5, C-2: the effect in the bridge process renders the same samples
    /// as in the Echoes process (offline wait, ADR-015), whatever the block size.
    #[test]
    fn a_sandboxed_effect_renders_like_an_in_process_one() {
        let project = project();
        let unprocessed = render(&project, BLOCK_SIZES[0]);
        for block_size in BLOCK_SIZES {
            let in_process = render_with_host(
                &mut with_half_gain(host_with_test_plugins()),
                &project,
                block_size,
            );
            assert!(
                first_difference(&unprocessed, &in_process).is_some(),
                "the loaded state must change the sound"
            );
            let mut sandboxed_host = host_isolating_everything();
            let sandboxed = render_with_host(&mut sandboxed_host, &project, block_size);
            assert!(
                sandboxed_host
                    .instance_by_key("plugin")
                    .unwrap()
                    .is_sandboxed(),
                "the effect must really run in the bridge"
            );
            assert_eq!(
                first_difference(&in_process, &sandboxed),
                None,
                "block size {block_size}"
            );
        }
    }

    /// ADR-033 step 8, C-2: the instrument in the bridge process plays the same
    /// samples as in the Echoes process (notes with their offsets and velocity), whatever
    /// the block size.
    #[test]
    fn a_sandboxed_instrument_renders_like_an_in_process_one() {
        let project = project();
        let frame = |seconds: f64| (seconds * SAMPLE_RATE as f64) as usize * 2;
        for block_size in BLOCK_SIZES {
            let in_process = render_with_host(&mut host_with_test_plugins(), &project, block_size);
            let mut sandboxed_host = isolating_everything(host_with_test_plugins());
            let sandboxed = render_with_host(&mut sandboxed_host, &project, block_size);
            assert!(
                sandboxed_host
                    .instance_by_key("instrument:midi")
                    .unwrap()
                    .is_sandboxed(),
                "the instrument must really run in the bridge"
            );
            assert!(
                sandboxed[frame(0.1)..frame(0.2)]
                    .iter()
                    .any(|s| s.abs() > 1e-3),
                "the first note must sound from the bridge"
            );
            assert_eq!(
                first_difference(&in_process, &sandboxed),
                None,
                "block size {block_size}"
            );
        }
    }

    /// A second track takes the test synth's `Aux Out` bus (port 1), which carries half
    /// of the main sound.
    fn aux_output_project() -> AppProject {
        let mut project = project();
        let mut aux = empty_track("aux");
        aux.input_source = Some(crate::project::PluginOutputSource {
            effect_id: "instrument:midi".into(),
            port: 1,
        });
        project.tracks.push(aux);
        project
    }

    /// ADR-033 step 8b, C-2: the auxiliary output of a bridged instrument reaches the
    /// track that reads it with the same samples as in the Echoes process.
    #[test]
    fn a_sandboxed_instrument_renders_its_aux_output_like_an_in_process_one() {
        let with_aux = aux_output_project();
        for block_size in BLOCK_SIZES {
            let main_only = render_with_host(&mut host_with_test_plugins(), &project(), block_size);
            let in_process = render_with_host(&mut host_with_test_plugins(), &with_aux, block_size);
            assert!(
                first_difference(&main_only, &in_process).is_some(),
                "the track reading the aux bus must add its sound"
            );
            let mut sandboxed_host = isolating_everything(host_with_test_plugins());
            let sandboxed = render_with_host(&mut sandboxed_host, &with_aux, block_size);
            assert!(
                sandboxed_host
                    .instance_by_key("instrument:midi")
                    .unwrap()
                    .is_sandboxed(),
                "the instrument must really run in the bridge"
            );
            assert_eq!(
                first_difference(&in_process, &sandboxed),
                None,
                "block size {block_size}"
            );
        }
    }

    /// The test effect reads the instrument track on its sidechain input, which adds a
    /// quarter of that signal (mixed to mono) to the effect's output.
    fn sidechain_project() -> AppProject {
        let mut project = project();
        let EffectParams::Plugin(params) = &mut project.tracks[0].effects[0].params else {
            panic!("the first track carries the plugin effect");
        };
        params.sidechain_source = Some("midi".into());
        project
    }

    /// ADR-033 step 8b, C-2: the sidechain of a bridged effect carries the same samples
    /// as in the Echoes process, whatever the block size.
    #[test]
    fn a_sandboxed_effect_hears_its_sidechain_like_an_in_process_one() {
        let with_sidechain = sidechain_project();
        for block_size in BLOCK_SIZES {
            let unwired = render_with_host(
                &mut with_half_gain(host_with_sidechain_inputs(1)),
                &project(),
                block_size,
            );
            let in_process = render_with_host(
                &mut with_half_gain(host_with_sidechain_inputs(1)),
                &with_sidechain,
                block_size,
            );
            assert!(
                first_difference(&unwired, &in_process).is_some(),
                "the sidechain must add its sound"
            );
            let mut sandboxed_host =
                isolating_everything(with_half_gain(host_with_sidechain_inputs(1)));
            let sandboxed = render_with_host(&mut sandboxed_host, &with_sidechain, block_size);
            assert!(
                sandboxed_host
                    .instance_by_key("plugin")
                    .unwrap()
                    .is_sandboxed(),
                "the effect must really run in the bridge"
            );
            assert_eq!(
                first_difference(&in_process, &sandboxed),
                None,
                "block size {block_size}"
            );
        }
    }

    /// The instrument's first note with the mod wheel up and the pitch bend raised, which
    /// the test plugin's `IMidiMapping` turns into parameters that scale its output.
    fn controlled_project() -> AppProject {
        let mut project = project();
        let control = |start: f32, kind, number: u8, value: u16| MidiControlPoint {
            start,
            kind,
            number,
            value,
        };
        project
            .midi_contents
            .get_mut("midi-clip")
            .expect("the reference clip")
            .channel_controls = vec![
            control(0.25, MidiControlKind::ControlChange, 1, 127),
            control(0.5, MidiControlKind::PitchBend, 0, 0x3000),
        ];
        project
    }

    /// ADR-033 step 8, C-2: the mod wheel and the pitch bend reach the instrument in the
    /// bridge through its `IMidiMapping` and change the sound as they do in process.
    #[test]
    fn a_sandboxed_instrument_renders_like_an_in_process_one_with_midi_controls() {
        let controlled = controlled_project();
        let plain = render(&project(), BLOCK_SIZES[0]);
        assert!(
            first_difference(&plain, &render(&controlled, BLOCK_SIZES[0])).is_some(),
            "the controls must change the sound"
        );
        for block_size in BLOCK_SIZES {
            let in_process =
                render_with_host(&mut host_with_test_plugins(), &controlled, block_size);
            let mut sandboxed_host = isolating_everything(host_with_test_plugins());
            let sandboxed = render_with_host(&mut sandboxed_host, &controlled, block_size);
            assert!(
                sandboxed_host
                    .instance_by_key("instrument:midi")
                    .unwrap()
                    .is_sandboxed(),
                "the instrument must really run in the bridge"
            );
            assert_eq!(
                first_difference(&in_process, &sandboxed),
                None,
                "block size {block_size}"
            );
        }
    }

    /// The effect's gain driven the way the app drives a parameter: by a modulation
    /// route (an LFO), by an automation lane, or by both on the same track.
    fn driven_gain_project(modulated: bool, automated: bool) -> AppProject {
        let mut project = project();
        let track = &mut project.tracks[0];
        let target = ModulationTargetRef::PluginParam {
            effect_id: "plugin".into(),
            native_id: test_vst3_plugin::GAIN,
        };
        if modulated {
            track
                .modulators
                .push(lfo("gain-lfo", 3.7, LfoWaveform::Triangle));
            track.mod_routes.push(ProjectModulationRoute {
                id: "gain-lfo->gain".into(),
                modulator_id: "gain-lfo".into(),
                target: target.clone(),
                depth: 0.3,
            });
        }
        if automated {
            track.automation_lanes.push(automation_lane(
                "gain-ramp",
                target,
                &[(0.0, 0.3), (1.3, 0.7), (2.4, 0.45)],
            ));
        }
        project
    }

    /// ADR-033 step 6, C-2: parameters cross the process boundary without changing a
    /// sample.  The driven project must sound unlike the plain one, so that an
    /// undelivered parameter cannot pass for a delivered one.
    fn assert_driven_gain_matches_in_process(modulated: bool, automated: bool) {
        let driven = driven_gain_project(modulated, automated);
        let plain = render(&project(), BLOCK_SIZES[0]);
        assert!(
            first_difference(&plain, &render(&driven, BLOCK_SIZES[0])).is_some(),
            "the automation and the modulation must change the sound"
        );
        for block_size in BLOCK_SIZES {
            let in_process = render_with_host(&mut host_with_test_plugins(), &driven, block_size);
            let mut sandboxed_host = isolating_everything(host_with_test_plugins());
            let sandboxed = render_with_host(&mut sandboxed_host, &driven, block_size);
            assert!(
                sandboxed_host
                    .instance_by_key("plugin")
                    .unwrap()
                    .is_sandboxed(),
                "the effect must really run in the bridge"
            );
            assert_eq!(
                first_difference(&in_process, &sandboxed),
                None,
                "block size {block_size}"
            );
        }
    }

    #[test]
    fn a_sandboxed_effect_renders_like_an_in_process_one_with_automation() {
        assert_driven_gain_matches_in_process(false, true);
    }

    #[test]
    fn a_sandboxed_effect_renders_like_an_in_process_one_with_modulation() {
        assert_driven_gain_matches_in_process(true, false);
    }

    #[test]
    fn a_sandboxed_effect_renders_like_an_in_process_one_with_automation_and_modulation() {
        assert_driven_gain_matches_in_process(true, true);
    }

    /// Recorded on Windows x86_64, separate from `GOLDEN_HASHES` because it needs the
    /// built test plugin.
    const VST3_PLUGINS_GOLDEN_HASH: u64 = 0x5c0f0710b6813439;
}
