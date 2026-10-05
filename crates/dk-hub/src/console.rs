//! Console attachment for the Windows GUI-subsystem build (REL-028).

/// Attaches to the console of the launching terminal; `false` when there is none.
pub fn attach_parent_console() -> bool {
    imp::attach_parent_console()
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};

    pub(super) fn attach_parent_console() -> bool {
        // SAFETY: takes a process id (the parent sentinel) and no pointers.
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) != 0 }
    }
}

#[cfg(not(windows))]
mod imp {
    pub(super) fn attach_parent_console() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_028_attach_is_best_effort_and_repeatable() {
        let _ = attach_parent_console();
        let _ = attach_parent_console();
        #[cfg(not(windows))]
        assert!(
            !attach_parent_console(),
            "no console to attach to off Windows"
        );
    }
}
