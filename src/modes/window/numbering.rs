//! Number parsing with deadlines only for prefixes of multiple live labels.
use smallvec::SmallVec;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default)]
struct Prefix {
    exact: Option<u32>,
    longer: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct NumberIndex(BTreeMap<String, Prefix>);

impl NumberIndex {
    pub fn new(values: impl IntoIterator<Item = u32>) -> Self {
        let mut index = Self::default();
        for value in values {
            let label = value.to_string();
            for length in 1..=label.len() {
                let prefix = index.0.entry(label[..length].into()).or_default();
                if length == label.len() {
                    prefix.exact = Some(value);
                } else {
                    prefix.longer = true;
                }
            }
        }
        index
    }
}

#[derive(Clone, Debug)]
pub(super) struct NumberInput {
    pub prefix: String,
    pub slot: bool,
    pub display: String,
}

impl Default for NumberInput {
    fn default() -> Self {
        // Every u32 label fits; typing and clearing reuse this one small buffer.
        Self {
            prefix: String::with_capacity(10),
            slot: false,
            display: String::with_capacity(11),
        }
    }
}

impl NumberInput {
    pub fn pending(&self) -> bool {
        self.slot || !self.prefix.is_empty()
    }

    pub fn cancel(&mut self) -> bool {
        let pending = self.pending();
        self.prefix.clear();
        self.slot = false;
        self.display.clear();
        pending
    }

    pub fn finish(&mut self, windows: &NumberIndex, slots: &NumberIndex) -> Option<(bool, u32)> {
        let index = if self.slot { slots } else { windows };
        let result = index
            .0
            .get(&self.prefix)
            .and_then(|p| p.exact)
            .map(|n| (self.slot, n));
        self.prefix.clear();
        self.slot = false;
        result
    }

    pub fn begin_slot(&mut self) {
        self.cancel();
        self.slot = true;
        self.display.push('`');
    }

    pub fn queue_digit(&mut self, digit: char) {
        if self.prefix.len() == 10 {
            self.prefix.clear();
        }
        self.prefix.push(digit);
        self.update_display();
    }

    fn update_display(&mut self) {
        self.display.clear();
        if self.slot {
            self.display.push('`');
        }
        self.display.push_str(&self.prefix);
    }

    pub fn digit(
        &mut self,
        digit: char,
        windows: &NumberIndex,
        slots: &NumberIndex,
    ) -> SmallVec<[(bool, u32); 2]> {
        let mut completed = SmallVec::new();
        let index = if self.slot { slots } else { windows };
        let had_prefix = !self.prefix.is_empty();
        self.prefix.push(digit);
        if !index.0.contains_key(&self.prefix) && had_prefix {
            self.prefix.pop();
            if let Some(value) = self.finish(windows, slots) {
                completed.push(value);
            }
            self.prefix.push(digit);
        }
        self.update_display();
        let index = if self.slot { slots } else { windows };
        match index.0.get(&self.prefix) {
            Some(prefix) if !prefix.longer => {
                if let Some(value) = self.finish(windows, slots) {
                    completed.push(value);
                }
            }
            None => {
                self.cancel();
            }
            _ => {}
        }
        completed
    }

    pub fn needs_timer(&self, windows: &NumberIndex, slots: &NumberIndex) -> bool {
        let index = if self.slot { slots } else { windows };
        index.0.get(&self.prefix).is_some_and(|p| p.longer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_real_ambiguous_prefixes_wait() {
        let empty = NumberIndex::default();
        for count in [1, 9, 10, 20, 23, 30] {
            let index = NumberIndex::new(1..=count);
            for digit in 1..=9.min(count) {
                let mut input = NumberInput::default();
                let completed = input.digit(char::from_digit(digit, 10).unwrap(), &index, &empty);
                let ambiguous = (10..=count).any(|n| n.to_string().starts_with(&digit.to_string()));
                assert_eq!(
                    input.needs_timer(&index, &empty),
                    ambiguous,
                    "count={count}, digit={digit}"
                );
                if ambiguous {
                    assert!(completed.is_empty());
                    assert_eq!(input.finish(&index, &empty), Some((false, digit)));
                } else {
                    assert_eq!(completed.as_slice(), [(false, digit)]);
                }
            }
        }
    }

    #[test]
    fn large_numbers_do_not_emit_intermediate_windows_or_swaps() {
        let windows = NumberIndex::new(1..=23);
        let slots = NumberIndex::new(1..=12);
        let mut input = NumberInput::default();
        assert!(input.digit('1', &windows, &slots).is_empty());
        assert_eq!(input.digit('2', &windows, &slots).as_slice(), [(false, 12)]);
        assert!(!input.pending());
        input.slot = true;
        assert!(input.digit('1', &windows, &slots).is_empty());
        assert_eq!(input.digit('2', &windows, &slots).as_slice(), [(true, 12)]);
        assert!(input.digit('1', &windows, &slots).is_empty());
        assert_eq!(input.finish(&windows, &slots), Some((false, 1)));
        assert!(input.digit('2', &windows, &slots).is_empty());
        assert_eq!(input.finish(&windows, &slots), Some((false, 2)));
    }

    #[test]
    fn gaps_and_closed_windows_are_not_prefixes() {
        let windows = NumberIndex::new([1, 2, 3, 20]);
        let slots = NumberIndex::default();
        let mut input = NumberInput::default();
        assert_eq!(input.digit('1', &windows, &slots).as_slice(), [(false, 1)]);
        assert!(input.digit('2', &windows, &slots).is_empty());
        assert_eq!(
            input.digit('3', &windows, &slots).as_slice(),
            [(false, 2), (false, 3)]
        );
    }
}
