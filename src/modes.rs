#![forbid(unsafe_code)]

//! The five built-in modes.
//!
//! Each is an ordinary [`Mode`] implementation with no
//! privileged access: they receive [`ModeEvent`](crate::api::ModeEvent)s and
//! return [`Command`]s, exactly as a plugin does. They are
//! therefore also worked examples of how to build a mode.
//!
//! The flow between them:
//!
//! ```text
//!   idle ──alt+e──► normal ──g───────► grid ─────────┐
//!    ▲               │  ▲   ──v───────► recursive_grid │
//!    │               │  │   ──Primary+f► ui_hint ───────┤
//!    └──────esc──────┘  └───────────────────esc/pick───┘
//! ```
//!
//! `idle` is silent, `normal` does the work, and the three targeting modes each
//! pick a point and hand control back.

pub mod grid;
pub mod hint;
pub mod idle;
pub mod normal;
pub mod recursive_grid;
pub(crate) mod targeting;

pub use grid::GridMode;
pub use hint::HintMode;
pub use idle::IdleMode;
pub use normal::NormalMode;
pub use recursive_grid::RecursiveGridMode;

use crate::api::Mode;
use crate::config::Config;

pub struct BuiltInModeDescriptor {
    pub id: &'static str,
    enabled: fn(&Config) -> bool,
    factory: fn(&Config) -> Box<dyn Mode>,
}

impl BuiltInModeDescriptor {
    fn create(&self, config: &Config) -> Option<Box<dyn Mode>> {
        (self.enabled)(config).then(|| (self.factory)(config))
    }
}

const fn always(_: &Config) -> bool {
    true
}

/// Single registration table for every shipped mode.
pub const BUILT_IN_MODES: &[BuiltInModeDescriptor] = &[
    BuiltInModeDescriptor {
        id: "idle",
        enabled: always,
        factory: |config| Box::new(IdleMode::new(config)),
    },
    BuiltInModeDescriptor {
        id: "normal",
        enabled: always,
        factory: |config| Box::new(NormalMode::new(config)),
    },
    BuiltInModeDescriptor {
        id: "grid",
        enabled: |config| config.grid.enabled,
        factory: |config| Box::new(GridMode::new(config)),
    },
    BuiltInModeDescriptor {
        id: "recursive_grid",
        enabled: |config| config.recursive_grid.enabled,
        factory: |config| Box::new(RecursiveGridMode::new(config)),
    },
    BuiltInModeDescriptor {
        id: "ui_hint",
        enabled: |config| config.ui_hint.enabled,
        factory: |config| Box::new(HintMode::new(config)),
    },
];

/// Instantiate the built-in modes enabled by `config`.
///
/// `idle` and `normal` are always present: they are the resting state and the
/// working state, and every other mode returns to one of them.
pub fn built_in(config: &Config) -> Vec<Box<dyn Mode>> {
    BUILT_IN_MODES
        .iter()
        .filter_map(|descriptor| descriptor.create(config))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ModeId;

    #[test]
    fn defaults_register_all_five_modes() {
        let ids: Vec<ModeId> = built_in(&Config::default())
            .iter()
            .map(|m| m.id())
            .collect();
        for expected in [
            ModeId::idle(),
            ModeId::normal(),
            ModeId::grid(),
            ModeId::recursive_grid(),
            ModeId::ui_hint(),
        ] {
            assert!(
                ids.contains(&expected),
                "{expected} is missing from {ids:?}"
            );
        }
    }

    #[test]
    fn descriptor_table_is_complete_and_has_unique_ids() {
        let declared: std::collections::BTreeSet<_> = BUILT_IN_MODES
            .iter()
            .map(|descriptor| descriptor.id)
            .collect();
        let expected: std::collections::BTreeSet<_> = ModeId::BUILT_IN.into_iter().collect();
        assert_eq!(declared, expected);
        assert_eq!(declared.len(), BUILT_IN_MODES.len());
    }

    #[test]
    fn disabled_modes_are_not_registered_but_idle_and_normal_survive() {
        let mut config = Config::default();
        config.grid.enabled = false;
        config.recursive_grid.enabled = false;
        config.ui_hint.enabled = false;
        let ids: Vec<ModeId> = built_in(&config).iter().map(|m| m.id()).collect();
        assert_eq!(ids, vec![ModeId::idle(), ModeId::normal()]);
    }

    #[test]
    fn idle_and_default_normal_let_unbound_keystrokes_through() {
        for mode in built_in(&Config::default()) {
            let expected = !matches!(mode.id().as_str(), "idle" | "normal");
            assert_eq!(
                mode.captures_keyboard(),
                expected,
                "{} has the wrong capture policy",
                mode.id()
            );
        }
    }
}
