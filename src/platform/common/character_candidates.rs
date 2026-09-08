//! Conservative, allocation-free character observation shared by native hooks.

#![forbid(unsafe_code)]

use crate::api::input::{Key, single_printable_character};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Shared demand compilation and fast rejection of unrelated ASCII text.
/// macOS already supplies Unicode; decoding that text must not require a second
/// layout query. Windows applies this after its native physical candidate gate.
pub(crate) struct CharacterDemand {
    enabled: AtomicBool,
    ascii: [AtomicU64; 2],
}

impl CharacterDemand {
    pub(crate) const fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            ascii: [const { AtomicU64::new(0) }; 2],
        }
    }

    /// Configuration writers are serialized by the backend/policy owner.
    pub(crate) fn configure(&self, keys: &[Key], physical: impl Fn(&Key) -> bool) -> Vec<char> {
        let characters = Self::characters(keys, physical);
        self.publish(&characters);
        characters
    }

    pub(crate) fn characters(keys: &[Key], physical: impl Fn(&Key) -> bool) -> Vec<char> {
        let mut characters: Vec<_> = keys
            .iter()
            .filter(|key| !physical(key))
            .filter_map(Key::as_char)
            .collect();
        characters.sort_unstable();
        characters.dedup();
        characters
    }

    pub(crate) fn publish(&self, characters: &[char]) {
        let mut ascii = [0u64; 2];
        for character in characters {
            if character.is_ascii() {
                for byte in [*character as u8, character.to_ascii_uppercase() as u8] {
                    ascii[usize::from(byte / 64)] |= 1 << (byte % 64);
                }
            }
        }
        // Each ASCII character consults exactly one atomic word, so there is
        // no mixed-snapshot read and no seqlock on this hot path.
        for (slot, mask) in self.ascii.iter().zip(ascii) {
            slot.store(mask, Ordering::Release);
        }
        self.enabled
            .store(!characters.is_empty(), Ordering::Release);
    }

    #[inline]
    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    pub(crate) fn decode(&self, units: &[u16]) -> Option<char> {
        if let [unit] = units
            && *unit < 128
        {
            let mask = self.ascii[usize::from(*unit / 64)].load(Ordering::Acquire);
            return ((32..127).contains(unit) && mask & (1 << (*unit % 64)) != 0)
                .then_some(char::from(*unit as u8));
        }
        // Preserve the existing handling of surrogate pairs, non-ASCII case
        // folding, invalid UTF-16 and multi-character text. No lossy casts.
        single_printable_character(units)
    }
}

/// Native code supplies layout identity, physical-key identity and modifier
/// masks. Only a complete, unchanged snapshot may reject an input event.
pub(crate) struct CharacterCandidates {
    pub(crate) enabled: AtomicBool,
    revision: AtomicU64,
    layout: AtomicUsize,
    masks: [AtomicU64; 256],
    identities: [AtomicU32; 256],
    refresh: AtomicBool,
}

impl CharacterCandidates {
    pub(crate) const fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            revision: AtomicU64::new(0),
            layout: AtomicUsize::new(0),
            masks: [const { AtomicU64::new(u64::MAX) }; 256],
            identities: [const { AtomicU32::new(0) }; 256],
            refresh: AtomicBool::new(false),
        }
    }

    /// Writers must be serialized by their platform owner. Invalidate before
    /// releasing the old native layout owner or starting native enumeration.
    pub(crate) fn begin_update(&self, enabled: bool) {
        self.revision.fetch_add(1, Ordering::SeqCst);
        self.enabled.store(enabled, Ordering::Release);
    }

    pub(crate) fn publish(&self, layout: usize, masks: [u64; 256], identities: [u32; 256]) {
        for (slot, mask) in self.masks.iter().zip(masks) {
            slot.store(mask, Ordering::SeqCst);
        }
        for (slot, identity) in self.identities.iter().zip(identities) {
            slot.store(identity, Ordering::SeqCst);
        }
        self.layout.store(layout, Ordering::SeqCst);
        self.revision.fetch_add(1, Ordering::SeqCst);
    }

    #[inline]
    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    pub(crate) fn take_refresh(&self) -> bool {
        self.refresh.swap(false, Ordering::Relaxed)
    }

    pub(crate) fn is_candidate(
        &self,
        key: usize,
        identity: u32,
        modifiers: u8,
        layout: usize,
    ) -> bool {
        let revision = self.revision.load(Ordering::SeqCst);
        if layout != 0 && self.layout.load(Ordering::SeqCst) == layout {
            let candidate = self.masks.get(key).is_none_or(|mask| {
                modifiers >= 64 || mask.load(Ordering::SeqCst) & (1u64 << modifiers) != 0
            });
            let known = self
                .identities
                .get(key)
                .is_some_and(|expected| expected.load(Ordering::SeqCst) == identity);
            if !candidate
                && known
                && revision & 1 == 0
                && self.revision.load(Ordering::SeqCst) == revision
            {
                return false;
            }
            if !known {
                self.refresh.store(true, Ordering::Relaxed);
            }
        } else {
            self.refresh.store(true, Ordering::Relaxed);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_demand_skips_unbound_ascii_and_preserves_unicode_validation() {
        let demand = CharacterDemand::new();
        assert!(!demand.is_enabled());
        let keys = ["?", "~", "!", "+", "€", "🦀", "h"].map(|key| Key::new(key).unwrap());
        let characters = demand.configure(&keys, |key| key.as_str() == "h");
        assert!(demand.is_enabled());
        assert!(!characters.contains(&'h'));
        for character in ['?', '~', '!', '+'] {
            assert_eq!(demand.decode(&[character as u16]), Some(character));
        }
        for character in ['h', 'H', 'j', '/', '='] {
            assert_eq!(demand.decode(&[character as u16]), None);
        }
        assert_eq!(demand.decode(&[0x20AC]), Some('€'));
        assert_eq!(demand.decode(&[0xD83E, 0xDD80]), Some('🦀'));
        assert_eq!(demand.decode(&[0xD83E]), None);
        assert_eq!(demand.decode(&[63, 33]), None);
        assert_eq!(demand.decode(&[]), None);
        demand.configure(&[], |_| false);
        assert!(!demand.is_enabled());
        assert_eq!(demand.decode(&[63]), None);
    }

    #[test]
    fn rejects_only_matching_stable_native_snapshot() {
        let table = CharacterCandidates::new();
        assert!(!table.is_enabled());
        table.begin_update(true);
        let mut masks = [0; 256];
        masks[42] = 1 << 5;
        table.publish(7, masks, [9; 256]);
        assert!(!table.is_candidate(41, 9, 5, 7));
        assert!(!table.is_candidate(42, 9, 4, 7));
        assert!(table.is_candidate(42, 9, 5, 7));
        assert!(table.is_candidate(41, 9, 5, 8));
        assert!(table.take_refresh());
        assert!(table.is_candidate(41, 10, 5, 7));
        assert!(table.take_refresh());
        assert!(table.is_candidate(256, 9, 0, 7));
        assert!(table.is_candidate(41, 9, 64, 7));
        assert!(table.is_candidate(41, 9, 0, 0));
        table.begin_update(true);
        assert!(table.is_candidate(41, 9, 5, 7));
        table.publish(8, [u64::MAX; 256], [9; 256]);
        assert!(table.is_candidate(41, 9, 5, 8));
        table.begin_update(false);
        table.publish(0, [u64::MAX; 256], [0; 256]);
        assert!(!table.is_enabled());
    }
}
