use std::time::Instant;

use dk_core::EngineError;

/// A remote collection's load state (spec 10 §4.1, exactly as specified). Old data stays
/// visible while refreshing (SHL-004).
#[derive(Debug, Clone)]
pub enum Resource<T> {
    Idle,
    Loading {
        previous: Option<T>,
    },
    Ready {
        data: T,
        fetched_at: Instant,
    },
    Failed {
        error: EngineError,
        previous: Option<T>,
    },
}

// Manual impl: `#[derive(Default)]` would require `T: Default`.
#[allow(clippy::derivable_impls)]
impl<T> Default for Resource<T> {
    fn default() -> Self {
        Self::Idle
    }
}

impl<T> Resource<T> {
    /// The data to render: fresh, or the previous data while loading/after a failure.
    pub fn data(&self) -> Option<&T> {
        match self {
            Resource::Ready { data, .. } => Some(data),
            Resource::Loading { previous } | Resource::Failed { previous, .. } => previous.as_ref(),
            Resource::Idle => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, Resource::Loading { .. })
    }

    /// First load: nothing to show yet (render skeletons).
    pub fn is_first_load(&self) -> bool {
        matches!(self, Resource::Loading { previous: None } | Resource::Idle)
    }

    pub fn error(&self) -> Option<&EngineError> {
        match self {
            Resource::Failed { error, .. } => Some(error),
            _ => None,
        }
    }

    /// Transition to `Loading`, keeping the previous data.
    pub fn start_loading(&mut self) {
        let previous = match std::mem::take(self) {
            Resource::Ready { data, .. } => Some(data),
            Resource::Loading { previous } | Resource::Failed { previous, .. } => previous,
            Resource::Idle => None,
        };
        *self = Resource::Loading { previous };
    }

    /// Apply a fetch result. A failure keeps the previous data (SHL-004).
    pub fn finish(&mut self, result: Result<T, EngineError>) {
        *self = match result {
            Ok(data) => Resource::Ready {
                data,
                fetched_at: Instant::now(),
            },
            Err(error) => {
                let previous = match std::mem::take(self) {
                    Resource::Ready { data, .. } => Some(data),
                    Resource::Loading { previous } | Resource::Failed { previous, .. } => previous,
                    Resource::Idle => None,
                };
                Resource::Failed { error, previous }
            }
        };
    }

    pub fn fetched_at(&self) -> Option<Instant> {
        match self {
            Resource::Ready { fetched_at, .. } => Some(*fetched_at),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shl_004_previous_data_survives_refresh_and_failure() {
        let mut r: Resource<Vec<u8>> = Resource::Idle;
        assert!(r.is_first_load());
        r.start_loading();
        assert!(r.is_first_load());
        r.finish(Ok(vec![1]));
        assert_eq!(r.data(), Some(&vec![1]));
        r.start_loading();
        assert!(r.is_loading());
        assert_eq!(r.data(), Some(&vec![1]));
        r.finish(Err(EngineError::Cancelled));
        assert_eq!(r.data(), Some(&vec![1]));
        assert!(r.error().is_some());
        r.start_loading();
        assert_eq!(r.data(), Some(&vec![1]));
    }
}
