//! Precompiled, platform-independent key lookup.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::api::{Binding, Key, KeyChord, KeyNameResolver};

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
    pub fn compile(bindings: Vec<(String, Binding)>, resolver: &KeyNameResolver) -> Self {
        let mut map = Self::default();
        for (text, binding) in bindings {
            if let Ok(chord) = KeyChord::parse_with_resolver(&text, resolver) {
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

    /// Resolve a binding against an effective held-input set without treating
    /// unrelated modifiers as a reason to reject a shorter chord.
    ///
    /// Parameterless `toggle` uses this after it has taken ownership of its
    /// companions. The effective set can therefore include synthetic, latched
    /// keys that are no longer physically held. Returning a borrowed binding
    /// keeps that tiny path free of `Arc` reference-count traffic.
    pub fn lookup_ref_with_pressed(
        &self,
        key: &Key,
        mut is_pressed: impl FnMut(&Key) -> bool,
    ) -> Option<&Binding> {
        self.find_entry(key, |entry| entry.chord.keys().iter().all(&mut is_pressed))
            .map(|entry| entry.binding.as_ref())
    }

    /// Resolve only a pointer-movement binding against a caller-provided
    /// effective held set. Drag assist uses this to ignore the physical
    /// modifiers it deliberately forwarded without making click, send or mode
    /// bindings eligible for the same fallback.
    pub fn lookup_move_with_pressed(
        &self,
        key: &Key,
        mut is_pressed: impl FnMut(&Key) -> bool,
    ) -> Option<Arc<Binding>> {
        self.find_entry(key, |entry| {
            matches!(entry.binding.as_ref(), Binding::Move(_))
                && entry.chord.keys().iter().all(&mut is_pressed)
        })
        .map(|entry| Arc::clone(&entry.binding))
    }

    /// Match a chord while requiring every held modifier outside that chord
    /// to be owned by KeySteer already. This prevents a bare `h` binding from
    /// stealing an external `Alt+H` shortcut, while a consumed `left_shift`
    /// speed binding can still modify `h`.
    ///
    /// A bare parameterless `toggle` is deliberately the sole exception: its
    /// purpose is to capture whichever companion input is already held, so
    /// `Ctrl` then `n = "toggle"` must resolve just like `n` then `Ctrl`.
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
                || (entry.chord.keys().len() == 1
                    && matches!(entry.binding.as_ref(), Binding::Toggle(targets) if targets.is_empty()))
        })
    }

    fn lookup_with_modifier_filter(
        &self,
        key: &Key,
        pressed: &[Key],
        modifier_filter: impl Fn(&CompiledBinding) -> bool,
    ) -> Option<(Arc<Binding>, usize)> {
        self.find_entry(key, |entry| {
            entry.chord.matches_pressed(pressed) && modifier_filter(entry)
        })
        .map(|entry| (Arc::clone(&entry.binding), entry.chord.keys().len()))
    }

    fn find_entry(
        &self,
        key: &Key,
        mut matches: impl FnMut(&CompiledBinding) -> bool,
    ) -> Option<&CompiledBinding> {
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
            .chain(generic.and_then(|name| self.by_activation.get(name)))
            .flatten()
            .find(|entry| entry.chord.activation_matches(key) && matches(entry))
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
        let resolver =
            KeyNameResolver::from_resolved(BTreeMap::from([("primary".into(), "left_alt".into())]));
        let map = CompiledKeymap::compile(vec![("primary+e".into(), mode("normal"))], &resolver);
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
        let resolver =
            KeyNameResolver::from_resolved(BTreeMap::from([("primary".into(), "alt".into())]));
        let map = CompiledKeymap::compile(vec![("primary+e".into(), mode("normal"))], &resolver);
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
            &KeyNameResolver::default(),
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

    #[test]
    fn strict_lookup_allows_a_bare_parameterless_toggle_to_capture_a_modifier() {
        let map = CompiledKeymap::compile(
            vec![("n".into(), Binding::Toggle(Vec::new()))],
            &KeyNameResolver::default(),
        );
        let n = Key::new("n").unwrap();
        let ctrl = Key::new("left_ctrl").unwrap();

        assert_eq!(
            map.lookup_with_specificity_strict(&n, &[ctrl, n.clone()], |_| false),
            Some((Arc::new(Binding::Toggle(Vec::new())), 1))
        );
    }

    #[test]
    fn strict_lookup_prefers_a_specific_chord_over_a_bare_toggle() {
        let map = CompiledKeymap::compile(
            vec![
                ("n".into(), Binding::Toggle(Vec::new())),
                ("ctrl+n".into(), mode("grid")),
            ],
            &KeyNameResolver::default(),
        );
        let n = Key::new("n").unwrap();
        let ctrl = Key::new("left_ctrl").unwrap();

        assert_eq!(
            map.lookup_with_specificity_strict(&n, &[ctrl, n.clone()], |_| false),
            Some((Arc::new(mode("grid")), 2))
        );
    }

    #[test]
    fn disabled_binding_is_compiled_so_it_can_block_inheritance() {
        let map = CompiledKeymap::compile(
            vec![("h".into(), Binding::Disabled)],
            &KeyNameResolver::default(),
        );
        let h = Key::new("h").unwrap();
        assert_eq!(
            map.lookup(&h, std::slice::from_ref(&h)),
            Some(Arc::new(Binding::Disabled))
        );
    }
}
