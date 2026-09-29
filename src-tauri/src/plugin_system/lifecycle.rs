//! Plugin lifecycle state machine.
//!
//! All devices move through a well-defined set of states.  The `PluginHost`
//! enforces valid transitions; invalid transitions are rejected with an error.
//!
//! ## State diagram
//!
//! ```text
//!                  ┌─────────────────────────────────────────────────┐
//!                  │                                                 ▼
//! (start) ──► Created ──► Loaded ──► Initialized ──► Running ──► Suspended
//!                                                        │            │
//!                                                        └────────────┘
//!                                                              │
//!                                                              ▼
//!                                                          Destroyed  (terminal)
//!                                                         Error        (terminal)
//! ```

/// The lifecycle state of a plugin device instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LifecycleState {
    /// Allocated but not yet loaded (binary not yet in memory).
    Created,
    /// Binary loaded; descriptor / manifest available.
    Loaded,
    /// `initialize()` called; sample-rate and block-size set.
    Initialized,
    /// Actively processing audio (`process` may be called).
    Running,
    /// Paused: resources retained but `process` is not called.
    Suspended,
    /// All resources released; the instance must not be used further.
    Destroyed,
    /// An unrecoverable error occurred.  The instance is invalid.
    Error,
}

impl LifecycleState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, LifecycleState::Destroyed | LifecycleState::Error)
    }
}

impl std::fmt::Display for LifecycleState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            LifecycleState::Created => "Created",
            LifecycleState::Loaded => "Loaded",
            LifecycleState::Initialized => "Initialized",
            LifecycleState::Running => "Running",
            LifecycleState::Suspended => "Suspended",
            LifecycleState::Destroyed => "Destroyed",
            LifecycleState::Error => "Error",
        };
        write!(f, "{}", s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleError {
    InvalidTransition {
        from: LifecycleState,
        to: LifecycleState,
    },
    AlreadyDestroyed,
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LifecycleError::InvalidTransition { from, to } => {
                write!(f, "invalid lifecycle transition {} → {}", from, to)
            }
            LifecycleError::AlreadyDestroyed => {
                write!(f, "device has been destroyed")
            }
        }
    }
}

/// Validate and perform a lifecycle transition.
///
/// Returns `Ok(new_state)` or `Err(LifecycleError)`.
pub fn transition(
    current: LifecycleState,
    target: LifecycleState,
) -> Result<LifecycleState, LifecycleError> {
    if current.is_terminal() {
        return Err(LifecycleError::AlreadyDestroyed);
    }

    let valid = matches!(
        (current, target),
        (LifecycleState::Created, LifecycleState::Loaded)
            | (LifecycleState::Loaded, LifecycleState::Initialized)
            | (LifecycleState::Initialized, LifecycleState::Running)
            | (LifecycleState::Running, LifecycleState::Suspended)
            | (LifecycleState::Suspended, LifecycleState::Running)
            | (LifecycleState::Running, LifecycleState::Destroyed)
            | (LifecycleState::Suspended, LifecycleState::Destroyed)
            | (_, LifecycleState::Error)
    );

    if valid {
        Ok(target)
    } else {
        Err(LifecycleError::InvalidTransition {
            from: current,
            to: target,
        })
    }
}
