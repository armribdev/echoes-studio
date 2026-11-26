//! What each plugin placement is told about its track (`clap.track-info`).
//!
//! Derived from the project, which owns the truth: a placement is an effect
//! (keyed by the effect id) or a track instrument (`instrument:{track_id}`), and
//! both sit on a track.  The project has no master or bus tracks, so the
//! return / bus / master flags are never set.  A track without a color of its
//! own gets no color: the UI picks its default from the track id, which the
//! host does not know.

use std::collections::HashMap;

use crate::audio_graph::graph::instrument_instance_key;
use crate::plugin_formats::clap::ffi::HostTrackInfo;
use crate::project::{AppProject, AppTrack};

/// Track info of every placement of the project, by placement key.
pub fn placement_track_infos(project: &AppProject) -> HashMap<String, HostTrackInfo> {
    let mut infos = HashMap::new();
    for track in &project.tracks {
        let info = HostTrackInfo::new(&track.name, track_color(track));
        for effect in &track.effects {
            infos.insert(effect.id.clone(), info.clone());
        }
        infos.insert(instrument_instance_key(&track.id), info);
    }
    infos
}

fn track_color(track: &AppTrack) -> Option<[u8; 3]> {
    parse_rgb(track.color.as_deref()?)
}

/// `#rrggbb`; anything else is no color.
fn parse_rgb(text: &str) -> Option<[u8; 3]> {
    let digits = text.strip_prefix('#').filter(|digits| {
        digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    })?;
    let value = u32::from_str_radix(digits, 16).ok()?;
    let [_, red, green, blue] = value.to_be_bytes();
    Some([red, green, blue])
}

/// A track with one (equalizer) effect per id, for tests that need a project.
#[cfg(test)]
pub(crate) fn test_track(
    id: &str,
    name: &str,
    color: Option<&str>,
    effect_ids: &[&str],
) -> AppTrack {
    use crate::project::{EffectParams, EqParams, TrackEffect};

    AppTrack {
        id: id.into(),
        name: name.into(),
        volume: 80.0,
        pan: 0.0,
        muted: false,
        solo: false,
        clips: vec![],
        effects: effect_ids
            .iter()
            .map(|effect_id| TrackEffect {
                id: (*effect_id).into(),
                enabled: true,
                params: EffectParams::Eq(EqParams::default()),
            })
            .collect(),
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
        color: color.map(str::to_owned),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_are_read_as_hex_rgb() {
        assert_eq!(parse_rgb("#ff8000"), Some([255, 128, 0]));
        assert_eq!(parse_rgb("#FF8000"), Some([255, 128, 0]));
        assert_eq!(parse_rgb("ff8000"), None);
        assert_eq!(parse_rgb("#ff80"), None);
        assert_eq!(parse_rgb("#gg0000"), None);
        assert_eq!(parse_rgb("#+f8000"), None);
        assert_eq!(parse_rgb(""), None);
    }

    #[test]
    fn effects_and_the_instrument_of_a_track_share_its_info() {
        let mut project = AppProject::new();
        project.tracks = vec![
            test_track("t1", "Lead", Some("#102030"), &["fx1", "fx2"]),
            test_track("t2", "Bass", None, &["fx3"]),
        ];
        let infos = placement_track_infos(&project);

        let lead = HostTrackInfo::new("Lead", Some([0x10, 0x20, 0x30]));
        assert_eq!(infos["fx1"], lead);
        assert_eq!(infos["fx2"], lead);
        assert_eq!(infos["instrument:t1"], lead);
        let bass = HostTrackInfo::new("Bass", None);
        assert_eq!(infos["fx3"], bass);
        assert_eq!(infos["instrument:t2"], bass);
        assert_eq!(infos.len(), 5);
    }

    #[test]
    fn a_renamed_track_changes_the_info_of_its_placements() {
        let mut project = AppProject::new();
        project.tracks = vec![test_track("t1", "Lead", None, &["fx1"])];
        let before = placement_track_infos(&project);
        project.tracks[0].name = "Pad".into();

        assert_ne!(placement_track_infos(&project), before);
        assert_eq!(
            placement_track_infos(&project)["fx1"],
            HostTrackInfo::new("Pad", None)
        );
    }
}
