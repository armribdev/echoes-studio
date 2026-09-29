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

use super::offline::render_project_in_blocks;
use crate::audio_graph::LiveParams;
use crate::plugin_system::PluginHost;
use crate::project::{
    AppClip, AppMidiClip, AppProject, AppTrack, AutomationClip, AutomationLane, AutomationPoint,
    CompressorParams, DelayParams, EffectParams, EqParams, InterpolationMode, LfoParams,
    LfoWaveform, MacroParams, MidiNote, ModulationTargetRef, ModulatorParams,
    ProjectModulationRoute, ProjectModulator, ReverbParams, TrackEffect,
};

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

fn midi_track() -> AppTrack {
    let note = |id: &str, pitch: u8, start: f32, duration: f32| MidiNote {
        id: id.into(),
        pitch,
        start,
        duration,
        velocity: 100,
    };
    let mut track = empty_track("midi");
    track.midi_clips.push(AppMidiClip {
        id: "midi-clip".into(),
        start: 0.0,
        end: DURATION_S,
        offset: 0.0,
        notes: vec![
            note("n1", 60, 0.0, 1.0),
            note("n2", 64, 0.5, 1.5),
            note("n3", 67, 1.25, 2.0),
            note("n4", 72, 3.0, 1.5),
        ],
    });
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

fn static_mix() -> AppProject {
    project_with(vec![audio_track(), midi_track()])
}

fn modulated_effects() -> AppProject {
    let mut audio = audio_track();
    let mut midi = midi_track();
    add_effect_modulation(&mut audio);
    add_effect_modulation(&mut midi);
    project_with(vec![audio, midi])
}

fn modulated_gain() -> AppProject {
    let mut audio = audio_track();
    add_gain_modulation(&mut audio);
    project_with(vec![audio, midi_track()])
}

fn automated() -> AppProject {
    let mut audio = audio_track();
    let mut midi = midi_track();
    add_automation(&mut audio);
    add_automation(&mut midi);
    project_with(vec![audio, midi])
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
    project_with(vec![audio, midi])
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
    project_with(vec![audio, midi])
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
    project_with(vec![audio, midi_track()])
}

fn reference_projects() -> [(&'static str, AppProject); 10] {
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
    ]
}

/// Recorded on Windows x86_64.  Only update a value when an ADR sanctions the
/// render change, and say so in the commit.
const GOLDEN_HASHES: [(&str, u64); 10] = [
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
    let project = project_with(vec![track]);

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
