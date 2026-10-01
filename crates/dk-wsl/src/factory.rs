//! `WslDistroFactory` (ENG-007, 011, 012, 106). SKELETON — `windows-platform`.

pub struct WslDistroFactory {
    _private: (),
}

impl WslDistroFactory {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for WslDistroFactory {
    fn default() -> Self {
        Self::new()
    }
}
