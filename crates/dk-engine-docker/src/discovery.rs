//! Docker discovery (ENG-001…006, 009, 010) and `DockerFactory`. SKELETON — `engine-integrator`.

pub struct DockerFactory {
    _private: (),
}

impl DockerFactory {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for DockerFactory {
    fn default() -> Self {
        Self::new()
    }
}
