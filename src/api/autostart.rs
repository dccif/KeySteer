//! Platform-neutral control of login-time application startup.

/// A platform implementation that can register the current executable to run
/// when the current user signs in.
pub trait Autostart {
    /// Whether the current executable is the registered login item.
    fn is_enabled(&self) -> Result<bool, String>;

    /// Register or unregister the current executable for the current user.
    fn set_enabled(&self, enabled: bool) -> Result<(), String>;

    /// Flip the current state and return the new value.
    fn toggle(&self) -> Result<bool, String> {
        let enabled = !self.is_enabled()?;
        self.set_enabled(enabled)?;
        Ok(enabled)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    struct MemoryAutostart(Cell<bool>);

    impl Autostart for MemoryAutostart {
        fn is_enabled(&self) -> Result<bool, String> {
            Ok(self.0.get())
        }

        fn set_enabled(&self, enabled: bool) -> Result<(), String> {
            self.0.set(enabled);
            Ok(())
        }
    }

    #[test]
    fn toggle_returns_and_persists_the_new_state() {
        let autostart = MemoryAutostart(Cell::new(false));
        assert_eq!(autostart.toggle(), Ok(true));
        assert_eq!(autostart.toggle(), Ok(false));
    }
}
