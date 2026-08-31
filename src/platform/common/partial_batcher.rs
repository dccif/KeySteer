//! Count-driven batching shared by native UI scan providers.

#[cfg(any(target_os = "macos", test))]
use smallvec::SmallVec;

/// Accumulate provider items into deterministic, exponentially growing batches.
pub(crate) struct PartialBatcher<T> {
    pending: Vec<T>,
    published: usize,
    next_total: usize,
    maximum: usize,
}

impl<T> PartialBatcher<T> {
    pub(crate) fn new(first: usize, maximum: usize) -> Self {
        debug_assert!(first > 0);
        debug_assert!(maximum >= first);
        Self {
            pending: Vec::with_capacity(first.min(maximum)),
            published: 0,
            next_total: first.min(maximum),
            maximum,
        }
    }

    pub(crate) fn push_one(&mut self, item: T) -> Option<Vec<T>> {
        self.pending.push(item);
        if self.published + self.pending.len() != self.next_total {
            return None;
        }
        Some(self.take_boundary())
    }

    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn extend(&mut self, items: impl IntoIterator<Item = T>) -> SmallVec<[Vec<T>; 1]> {
        let mut ready = SmallVec::new();
        for item in items {
            if let Some(batch) = self.push_one(item) {
                ready.push(batch);
            }
        }
        ready
    }

    pub(crate) fn finish(&mut self) -> Option<Vec<T>> {
        (!self.pending.is_empty()).then(|| std::mem::take(&mut self.pending))
    }

    /// Publish an early provider batch without losing the cumulative boundary.
    ///
    /// If 10 items are flushed before the first 24-item boundary, the next
    /// boundary contains the following 14 items and still lands at total 24.
    #[cfg(any(target_os = "windows", test))]
    pub(crate) fn flush_pending(&mut self) -> Option<Vec<T>> {
        if self.pending.is_empty() {
            return None;
        }
        let following_capacity = self
            .next_total
            .saturating_sub(self.published.saturating_add(self.pending.len()));
        let batch = std::mem::replace(&mut self.pending, Vec::with_capacity(following_capacity));
        self.published += batch.len();
        Some(batch)
    }

    fn take_boundary(&mut self) -> Vec<T> {
        let current_total = self.next_total;
        let following_total = current_total.saturating_mul(2).min(self.maximum);
        let next_capacity = following_total.saturating_sub(current_total);
        let batch = std::mem::replace(&mut self.pending, Vec::with_capacity(next_capacity));
        self.published += batch.len();
        self.next_total = following_total;
        batch
    }

    #[cfg(test)]
    fn pending_capacity(&self) -> usize {
        self.pending.capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_boundaries_and_terminal_flush_are_deterministic() {
        for (count, expected_sizes) in [
            (0, vec![]),
            (24, vec![24]),
            (25, vec![24, 1]),
            (100, vec![24, 24, 48, 4]),
            (2_000, vec![24, 24, 48, 96, 192, 384, 768, 464]),
        ] {
            let mut batches = PartialBatcher::new(24, 2_000);
            let mut output = Vec::new();
            let mut sizes = Vec::new();
            for batch in batches.extend(0..count) {
                sizes.push(batch.len());
                output.extend(batch);
            }
            if let Some(batch) = batches.finish() {
                sizes.push(batch.len());
                output.extend(batch);
            }
            assert_eq!(sizes, expected_sizes);
            assert_eq!(output, Vec::from_iter(0..count));
        }
    }

    #[test]
    fn push_one_publishes_at_the_same_boundaries() {
        let mut batches = PartialBatcher::new(24, 2_000);
        for value in 0..23 {
            assert!(batches.push_one(value).is_none());
        }
        assert_eq!(batches.push_one(23), Some(Vec::from_iter(0..24)));
        assert_eq!(batches.extend(24..48).as_slice(), &[Vec::from_iter(24..48)]);
    }

    #[test]
    fn final_batch_reserves_only_the_remaining_limit() {
        let mut batches = PartialBatcher::new(24, 2_000);
        let ready = batches.extend(0..1_536);
        assert_eq!(ready.iter().map(Vec::len).sum::<usize>(), 1_536);
        assert_eq!(batches.pending_capacity(), 464);
    }

    #[test]
    fn early_flush_preserves_the_next_cumulative_boundary() {
        let mut batches = PartialBatcher::new(24, 2_000);
        assert!(batches.extend(0..10).is_empty());
        assert_eq!(batches.flush_pending(), Some(Vec::from_iter(0..10)));
        assert!(batches.extend(10..23).is_empty());
        assert_eq!(batches.push_one(23), Some(Vec::from_iter(10..24)));
    }
}
