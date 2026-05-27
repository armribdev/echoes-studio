//! In-session event store for project-structural events.
//!
//! Responsibilities:
//! - Append events (discards redo-future on new append)
//! - Track a `head` index so undo/redo is a pointer move, not a data mutation
//! - Maintain periodic snapshots so replay is O(events since last snapshot)
//!   rather than O(total events)
//!
//! The store is held behind a `Mutex<EventStore>` in `AppState`. All access
//! happens outside the audio callback — it is NOT on the real-time path.

use super::events::ProjectEvent;
use crate::project::AppProject;

/// How often (in events) to capture a project snapshot for fast replay.
const SNAPSHOT_INTERVAL: usize = 50;

pub struct EventStore {
    /// Immutable log of all events ever appended (including redo-future).
    events: Vec<ProjectEvent>,
    /// Index of the next event to apply (i.e., "current present" in the log).
    /// Invariant: `0 <= head <= events.len()`.
    head: usize,
    /// Cached snapshot: `(event_index, project_at_that_index)`.
    /// The index is the head value when the snapshot was taken.
    snapshot: Option<(usize, AppProject)>,
}

impl EventStore {
    pub fn new() -> Self {
        EventStore { events: Vec::new(), head: 0, snapshot: None }
    }

    // ── Write path ────────────────────────────────────────────────────────────

    /// Append a new event. Truncates any redo-future events first.
    pub fn append(&mut self, event: ProjectEvent) {
        // Discard any events that were "in the future" (from previous redos)
        self.events.truncate(self.head);
        self.events.push(event);
        self.head = self.events.len();
    }

    /// Store a snapshot of the current project state.
    /// Called periodically by the gateway after structural mutations.
    pub fn take_snapshot(&mut self, project: AppProject) {
        self.snapshot = Some((self.head, project));
    }

    /// Reset the entire store and seed from a loaded project (e.g., open-file).
    /// The snapshot becomes the baseline; no events above it.
    pub fn init_from_project(&mut self, project: AppProject) {
        self.events.clear();
        self.head = 0;
        self.snapshot = Some((0, project));
    }

    // ── Undo / Redo ───────────────────────────────────────────────────────────

    pub fn can_undo(&self) -> bool {
        self.head > 0
    }

    pub fn can_redo(&self) -> bool {
        self.head < self.events.len()
    }

    /// Move head back one position. Returns true if undo was possible.
    pub fn undo(&mut self) -> bool {
        if self.can_undo() {
            self.head -= 1;
            true
        } else {
            false
        }
    }

    /// Move head forward one position. Returns true if redo was possible.
    pub fn redo(&mut self) -> bool {
        if self.can_redo() {
            self.head += 1;
            true
        } else {
            false
        }
    }

    // ── Read path ─────────────────────────────────────────────────────────────

    /// Events that form the current state: `events[0..head]`.
    pub fn active_events(&self) -> &[ProjectEvent] {
        &self.events[..self.head]
    }

    /// The best starting point for rebuilding: the most recent snapshot at or
    /// before `head` plus any events after it.
    ///
    /// Returns `(snapshot_head, Option<&AppProject>, events_to_replay)`.
    pub fn replay_plan(&self) -> (usize, Option<&AppProject>, &[ProjectEvent]) {
        match &self.snapshot {
            Some((snap_idx, snap_proj)) if *snap_idx <= self.head => {
                let events_after = &self.events[*snap_idx..self.head];
                (*snap_idx, Some(snap_proj), events_after)
            }
            _ => (0, None, &self.events[..self.head]),
        }
    }

    pub fn head(&self) -> usize {
        self.head
    }

    pub fn total_events(&self) -> usize {
        self.events.len()
    }

    /// True if it's a good moment to snapshot (every N events).
    pub fn should_snapshot(&self) -> bool {
        self.head > 0 && self.head % SNAPSHOT_INTERVAL == 0
    }
}
