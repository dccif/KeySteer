//! Precompiled, platform-independent key lookup.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::api::{Binding, Key, KeyChord};

#[derive(Debug, Clone)]
pub(super) struct CompiledBinding {
    pub chord: KeyChord,
    pub binding: Arc<Binding>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct CompiledKeymap {
    by_activation: BTreeMap<Key, Vec<CompiledBinding>>,
}

impl CompiledKeymap {
    pub fn compile(bindings: Vec<(String, Binding)>, aliases: &BTreeMap<String, String>) -> Self {
        let mut map = Self::default();
        for (text, binding) in bindings {
            if binding == Binding::Disabled {
                continue;
            }
            if let Ok(chord) = KeyChord::parse_with_aliases(&text, aliases) {
                map.insert(chord, binding);
            }
        }
        map
    }

    pub fn insert(&mut self, chord: KeyChord, binding: Binding) {
        let activation = chord.activation_key().clone();
        let entries = self.by_activation.entry(activation).or_default();
        if entries
            .iter()
            .any(|entry| entry.chord.canonical() == chord.canonical())
        {
            return;
        }
        entries.push(CompiledBinding {
            chord,
            binding: Arc::new(binding),
        });
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.chord.keys().len()));
    }

    pub fn contains_chord(&self, chord: &KeyChord) -> bool {
        self.by_activation
            .values()
            .flatten()
            .any(|entry| entry.chord.canonical() == chord.canonical())
    }

    #[cfg(test)]
    pub fn lookup(&self, key: &Key, pressed: &[Key]) -> Option<Arc<Binding>> {
        self.lookup_with_specificity(key, pressed)
            .map(|(binding, _)| binding)
    }

    pub fn lookup_with_specificity(
        &self,
        key: &Key,
        pressed: &[Key],
    ) -> Option<(Arc<Binding>, usize)> {
        self.lookup_with_modifier_filter(key, pressed, |_| true)
    }

    /// Match a chord while requiring every held modifier outside that chord
    /// to be owned by KeySteer already. This prevents a bare `h` binding from
    /// stealing an external `Alt+H` shortcut, while a consumed `left_shift`
    /// speed binding can still modify `h`.
    pub fn lookup_with_specificity_strict(
        &self,
        key: &Key,
        pressed: &[Key],
        modifier_is_owned: impl Fn(&Key) -> bool,
    ) -> Option<(Arc<Binding>, usize)> {
        self.lookup_with_modifier_filter(key, pressed, |entry| {
            pressed
                .iter()
                .filter(|pressed| pressed.is_modifier())
                .all(|pressed_modifier| {
                    entry.chord.keys().iter().any(|configured| {
                        configured.is_modifier() && modifier_matches(configured, pressed_modifier)
                    }) || modifier_is_owned(pressed_modifier)
                })
        })
    }

    fn lookup_with_modifier_filter(
        &self,
        key: &Key,
        pressed: &[Key],
        modifier_filter: impl Fn(&CompiledBinding) -> bool,
    ) -> Option<(Arc<Binding>, usize)> {
        let generic = match key.as_str() {
            "left_alt" | "right_alt" => Some("alt"),
            "left_ctrl" | "right_ctrl" => Some("ctrl"),
            "left_shift" | "right_shift" => Some("shift"),
            "left_win" | "right_win" => Some("win"),
            _ => None,
        };
        self.by_activation
            .get(key)
            .into_iter()
            .chain(
                generic
                    .and_then(|name| Key::new(name).ok())
                    .and_then(|key| self.by_activation.get(&key)),
            )
            .flatten()
            .find(|entry| {
                entry.chord.activation_matches(key)
                    && entry.chord.matches_pressed(pressed)
                    && modifier_filter(entry)
            })
            .map(|entry| (Arc::clone(&entry.binding), entry.chord.keys().len()))
    }

    pub fn entries(&self) -> Vec<(String, Binding)> {
        self.by_activation
            .values()
            .flatten()
            .map(|entry| (entry.chord.canonical(), entry.binding.as_ref().clone()))
            .collect()
    }
}

fn modifier_matches(configured: &Key, physical: &Key) -> bool {
    configured == physical
        || matches!(
            (configured.as_str(), physical.as_str()),
            ("alt", "left_alt" | "right_alt")
                | ("ctrl", "left_ctrl" | "right_ctrl")
                | ("shift", "left_shift" | "right_shift")
                | ("win", "left_win" | "right_win")
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ModeId;
    use std::collections::BTreeSet;

    fn mode(name: &str) -> Binding {
        Binding::Mode(ModeId::new(name).unwrap())
    }

    #[test]
    fn custom_primary_side_is_enforced_by_the_compiled_keymap() {
        let aliases = BTreeMap::from([("primary".into(), "left_alt".into())]);
        let map = CompiledKeymap::compile(vec![("primary+e".into(), mode("normal"))], &aliases);
        let e = Key::new("e").unwrap();
        let right = BTreeSet::from([Key::new("right_alt").unwrap(), e.clone()]);
        let left = BTreeSet::from([Key::new("left_alt").unwrap(), e.clone()]);

        assert_eq!(map.lookup(&e, &right.into_iter().collect::<Vec<_>>()), None);
        assert_eq!(
            map.lookup(&e, &left.into_iter().collect::<Vec<_>>()),
            Some(Arc::new(mode("normal")))
        );
    }

    #[test]
    fn generic_modifier_alias_accepts_both_physical_sides() {
        let aliases = BTreeMap::from([("primary".into(), "alt".into())]);
        let map = CompiledKeymap::compile(vec![("primary+e".into(), mode("normal"))], &aliases);
        let e = Key::new("e").unwrap();

        for side in ["left_alt", "right_alt"] {
            let pressed = BTreeSet::from([Key::new(side).unwrap(), e.clone()]);
            assert_eq!(
                map.lookup(&e, &pressed.into_iter().collect::<Vec<_>>()),
                Some(Arc::new(mode("normal")))
            );
        }
    }

    #[test]
    fn strict_lookup_rejects_foreign_modifiers_but_accepts_owned_ones() {
        let map = CompiledKeymap::compile(
            vec![("h".into(), mode("normal")), ("alt+h".into(), mode("grid"))],
            &BTreeMap::new(),
        );
        let h = Key::new("h").unwrap();
        let alt = Key::new("left_alt").unwrap();
        let shift = Key::new("left_shift").unwrap();

        let alt_h = vec![alt.clone(), h.clone()];
        assert_eq!(
            map.lookup_with_specificity_strict(&h, &alt_h, |_| false),
            Some((Arc::new(mode("grid")), 2))
        );

        let shift_h = vec![shift.clone(), h.clone()];
        assert_eq!(
            map.lookup_with_specificity_strict(&h, &shift_h, |_| false),
            None
        );
        assert_eq!(
            map.lookup_with_specificity_strict(&h, &shift_h, |key| key == &shift),
            Some((Arc::new(mode("normal")), 1))
        );
    }
}
