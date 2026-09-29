//! Pre-compiled automation curves for real-time evaluation.
//!
//! ## Design
//!
//! `CompiledAutomationCurve` is built at graph compile time (command thread)
//! from `AutomationLane` data.  It holds a sorted, boxed slice of time+value
//! knots.  During playback the scheduler calls `evaluate(time_s)` per block —
//! a binary search on a pre-allocated slice with no heap allocation.
//!
//! Automation is delivered through the same `apply_modulation` path as
//! LFO/Envelope modulation.  For Phase 0 the depth is always 1.0 (the curve
//! value IS the delivered value).  The additive semantic means automation
//! should target parameters whose `ParameterBlock` base is 0 for full
//! absolute control; this constraint will be lifted in a later phase.

use std::sync::Arc;

use crate::audio_graph::node::NodeId;
use crate::project::{AutomationLane, InterpolationMode};

/// One compiled knot (sorted by `time_s` at construction time).
#[derive(Debug, Clone, Copy)]
pub struct CompiledAutomationPoint {
    pub time_s: f32,
    pub value: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompiledInterpolation {
    Linear,
    Step,
}

/// Sorted, immutable automation curve.  RT-safe: binary search, no allocation.
pub struct CompiledAutomationCurve {
    points: Box<[CompiledAutomationPoint]>,
    interpolation: CompiledInterpolation,
}

// SAFETY: `Box<[...]>` is `Send + Sync` when the element type is.
unsafe impl Send for CompiledAutomationCurve {}
unsafe impl Sync for CompiledAutomationCurve {}

impl CompiledAutomationCurve {
    /// Build from an `AutomationLane`.
    ///
    /// Points from all clips are collected, sorted by `time_s`, and
    /// de-duplicated (within 1 µs).  Points outside their parent clip window
    /// are discarded.  Runs on the command thread — may allocate.
    pub fn from_lane(lane: &AutomationLane) -> Self {
        let interp = match lane.interpolation {
            InterpolationMode::Linear => CompiledInterpolation::Linear,
            InterpolationMode::Step => CompiledInterpolation::Step,
        };

        let mut pts: Vec<CompiledAutomationPoint> = lane
            .clips
            .iter()
            .flat_map(|clip| {
                clip.points.iter().filter_map(|p| {
                    if p.time_s >= clip.start_s && p.time_s <= clip.end_s {
                        Some(CompiledAutomationPoint {
                            time_s: p.time_s as f32,
                            value: p.value.clamp(0.0, 1.0),
                        })
                    } else {
                        None
                    }
                })
            })
            .collect();

        pts.sort_by(|a, b| a.time_s.partial_cmp(&b.time_s).unwrap_or(std::cmp::Ordering::Equal));

        // Remove points closer than 1 µs (float precision guard).
        pts.dedup_by(|newer, older| (newer.time_s - older.time_s).abs() < 1e-6);

        Self {
            points: pts.into_boxed_slice(),
            interpolation: interp,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Evaluate the curve at `time_s`.
    ///
    /// - Before the first point: returns the first point's value.
    /// - After the last point: returns the last point's value (hold).
    /// - Between points: linear interpolation or step-hold.
    ///
    /// RT-safe: only binary search + float arithmetic on a pre-allocated slice.
    #[inline]
    pub fn evaluate(&self, time_s: f32) -> f32 {
        if self.points.is_empty() {
            return 0.0;
        }
        let idx = self.points.partition_point(|p| p.time_s <= time_s);
        if idx == 0 {
            return self.points[0].value;
        }
        let last = self.points.len();
        if idx >= last {
            return self.points[last - 1].value;
        }
        let p0 = &self.points[idx - 1];
        let p1 = &self.points[idx];
        match self.interpolation {
            CompiledInterpolation::Linear => {
                let denom = p1.time_s - p0.time_s;
                if denom < 1e-10 {
                    return p1.value;
                }
                p0.value + (time_s - p0.time_s) / denom * (p1.value - p0.value)
            }
            CompiledInterpolation::Step => p0.value,
        }
    }
}

/// Automation action resolved to a `NodeId` by `AudioGraph::from_project`.
///
/// Stored in `AudioGraph`; the `GraphCompiler` resolves the node to a step
/// index and calls `param_id()` to get the final `u32` for the scheduler.
pub struct AutomationAction {
    pub target_node_id: NodeId,
    /// Parameter name passed to `AudioNode::param_id()` at compile time.
    pub param_name: String,
    pub curve: Arc<CompiledAutomationCurve>,
}

/// Automation action with step/param already resolved.  Lives in `CompiledGraph`.
pub struct SchedulerAutomationAction {
    pub target_step: usize,
    pub param_id: u32,
    pub curve: Arc<CompiledAutomationCurve>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{AutomationClip, AutomationLane, AutomationPoint, InterpolationMode, ModulationTargetRef};

    fn lane(pts: &[(f64, f32)], mode: InterpolationMode) -> AutomationLane {
        let points = pts
            .iter()
            .map(|&(t, v)| AutomationPoint { time_s: t, value: v })
            .collect();
        AutomationLane {
            id: "test".into(),
            target: ModulationTargetRef::TrackGain { track_id: "t1".into() },
            clips: vec![AutomationClip {
                id: "c1".into(),
                start_s: 0.0,
                end_s: 100.0,
                points,
            }],
            interpolation: mode,
        }
    }

    #[test]
    fn empty_curve_returns_zero() {
        let l = lane(&[], InterpolationMode::Linear);
        let curve = CompiledAutomationCurve::from_lane(&l);
        assert!(curve.is_empty());
        assert_eq!(curve.evaluate(1.0), 0.0);
    }

    #[test]
    fn single_point_holds_everywhere() {
        let l = lane(&[(2.0, 0.5)], InterpolationMode::Linear);
        let curve = CompiledAutomationCurve::from_lane(&l);
        assert_eq!(curve.evaluate(0.0), 0.5, "before point must hold");
        assert_eq!(curve.evaluate(2.0), 0.5);
        assert_eq!(curve.evaluate(5.0), 0.5, "after point must hold");
    }

    #[test]
    fn linear_interpolation_midpoint() {
        let l = lane(&[(0.0, 0.0), (1.0, 1.0)], InterpolationMode::Linear);
        let curve = CompiledAutomationCurve::from_lane(&l);
        let mid = curve.evaluate(0.5);
        assert!((mid - 0.5).abs() < 1e-5, "linear midpoint must be 0.5, got {mid}");
    }

    #[test]
    fn step_interpolation_holds_prev_value() {
        let l = lane(&[(0.0, 0.0), (1.0, 1.0)], InterpolationMode::Step);
        let curve = CompiledAutomationCurve::from_lane(&l);
        assert_eq!(curve.evaluate(0.5), 0.0, "step must hold previous value");
        assert_eq!(curve.evaluate(1.0), 1.0, "step at exact point must use that value");
        assert_eq!(curve.evaluate(1.5), 1.0, "step after last point holds");
    }

    #[test]
    fn points_outside_clip_window_are_discarded() {
        let mut l = lane(&[(0.5, 0.3), (2.5, 0.8)], InterpolationMode::Linear);
        l.clips[0].start_s = 1.0;
        l.clips[0].end_s = 2.0;
        let curve = CompiledAutomationCurve::from_lane(&l);
        assert!(curve.is_empty(), "both points outside clip window must be discarded");
    }

    #[test]
    fn multi_clip_points_are_merged_and_sorted() {
        let l = AutomationLane {
            id: "m".into(),
            target: ModulationTargetRef::TrackGain { track_id: "t".into() },
            clips: vec![
                AutomationClip {
                    id: "c1".into(),
                    start_s: 2.0,
                    end_s: 3.0,
                    points: vec![AutomationPoint { time_s: 2.0, value: 0.2 }],
                },
                AutomationClip {
                    id: "c2".into(),
                    start_s: 0.0,
                    end_s: 1.0,
                    points: vec![AutomationPoint { time_s: 0.0, value: 0.0 }],
                },
            ],
            interpolation: InterpolationMode::Linear,
        };
        let curve = CompiledAutomationCurve::from_lane(&l);
        assert_eq!(curve.points.len(), 2);
        assert!(curve.points[0].time_s < curve.points[1].time_s, "must be sorted by time");
    }
}
