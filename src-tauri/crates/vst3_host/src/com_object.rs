//! What the COM objects the host allocates for a plugin have in common.
//!
//! Unlike the static host context, these objects are counted: the plugin may keep
//! one past the call that gave it over, and the last `release` frees it.

use std::sync::atomic::{fence, AtomicU32, Ordering};

/// A COM reference count; a new object starts with the caller's reference.
pub(crate) struct ReferenceCount(AtomicU32);

impl ReferenceCount {
    pub fn new() -> Self {
        Self(AtomicU32::new(1))
    }

    /// Returns the new count.
    pub fn add(&self) -> u32 {
        self.0.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Returns the new count; the caller frees the object when it reaches 0.
    pub fn release(&self) -> u32 {
        let before = self.0.fetch_sub(1, Ordering::Release);
        if before == 1 {
            fence(Ordering::Acquire);
        }
        before - 1
    }
}

#[cfg(test)]
pub(crate) use probe::LifeProbe;

/// Lets a test see that an object was freed, without any global state.
#[cfg(test)]
mod probe {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[derive(Default)]
    pub(crate) struct LifeProbe(Option<Arc<AtomicBool>>);

    impl LifeProbe {
        pub fn watching(freed: &Arc<AtomicBool>) -> Self {
            Self(Some(Arc::clone(freed)))
        }
    }

    impl Drop for LifeProbe {
        fn drop(&mut self) {
            if let Some(freed) = &self.0 {
                freed.store(true, Ordering::Release);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_count_starts_at_the_callers_reference() {
        let count = ReferenceCount::new();
        assert_eq!(count.add(), 2);
        assert_eq!(count.release(), 1);
        assert_eq!(count.release(), 0);
    }
}
