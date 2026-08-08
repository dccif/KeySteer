//! Plugin registration.
//!
//! A plugin is a [`Mode`] plus a manifest. There is no separate "plugin API":
//! plugins get the same [`Command`]/[`ModeEvent`] vocabulary and the same
//! [`HostContext`] as the built-in modes, so anything `grid` or `ui_hint` can
//! do — full-screen overlays, custom grids, pointer warping, key injection — a
//! plugin can do too.
//!
//! [`Command`]: super::command::Command
//! [`ModeEvent`]: super::command::ModeEvent
//! [`HostContext`]: super::command::HostContext

use super::binding::Binding;
use super::command::Mode;
use super::input::KeyChord;

/// Version of the mode/command vocabulary. Bumped on breaking changes.
pub const API_VERSION: u32 = 6;

/// Metadata describing a plugin to the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// Reverse-DNS-ish unique id, e.g. `com.example.zoom`.
    pub id: String,
    pub name: String,
    pub description: String,
    /// Must equal [`API_VERSION`].
    pub api_version: u32,
    /// Legacy chords that activate the plugin mode directly.
    pub default_chords: Vec<KeyChord>,
    /// Parameterized verbs routed to this plugin while any mode is active.
    pub verbs: Vec<String>,
    /// Suggested bindings merged into Normal when the user has not claimed the
    /// chord. Unlike `default_chords`, these may invoke a verb with arguments.
    pub default_bindings: Vec<(KeyChord, Binding)>,
}

impl Manifest {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            api_version: API_VERSION,
            default_chords: Vec::new(),
            verbs: Vec::new(),
            default_bindings: Vec::new(),
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn with_chord(mut self, chord: KeyChord) -> Self {
        self.default_chords.push(chord);
        self
    }

    pub fn with_verb(mut self, verb: impl Into<String>) -> Self {
        self.verbs.push(verb.into());
        self
    }

    pub fn with_default_binding(mut self, chord: KeyChord, binding: Binding) -> Self {
        self.default_bindings.push((chord, binding));
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.api_version != API_VERSION {
            return Err(format!(
                "plugin {} targets API v{}, host provides v{API_VERSION}",
                self.id, self.api_version
            ));
        }
        if self.id.is_empty()
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(format!("invalid plugin id: {:?}", self.id));
        }
        for verb in &self.verbs {
            if verb.is_empty()
                || !verb
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                return Err(format!("invalid plugin verb: {verb:?}"));
            }
        }
        Ok(())
    }
}

/// A mode contributed by a plugin.
pub trait Plugin: Mode {
    fn manifest(&self) -> &Manifest;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_mismatched_api_version() {
        let mut m = Manifest::new("com.example.a", "A");
        m.api_version = API_VERSION + 1;
        assert!(m.validate().is_err());
    }

    #[test]
    fn rejects_ids_with_spaces() {
        assert!(Manifest::new("bad id", "Bad").validate().is_err());
        assert!(Manifest::new("com.example.ok", "Ok").validate().is_ok());
    }
}
