//! Geometry-only connectivity cache.
use super::*;
/// Cache only geometry and identity: activation's Z-order changes do not alter
/// connectivity. All scratch storage survives successive worker requests.
#[derive(Default)]
pub(super) struct OverlapCache {
    pub(super) inventory: Vec<(WindowId, Rect)>,
    pub(super) members: Vec<WindowId>,
    pub(super) queue: Vec<usize>,
    #[cfg(test)]
    pub(super) comparisons: usize,
}

impl OverlapCache {
    pub(super) fn contains(&self, id: WindowId) -> bool {
        self.members.binary_search(&id).is_ok()
    }

    pub(super) fn prepare(
        &mut self,
        windows: &[WindowInfo],
        anchor: Option<WindowId>,
        cancelled: &dyn Fn() -> bool,
    ) {
        // Validate every live rectangle, including windows outside the cached
        // component: moving one of those can create a new connecting bridge.
        let changed = windows.len() != self.inventory.len()
            || windows.iter().any(|window| {
                self.inventory
                    .binary_search_by_key(&window.id, |&(id, _)| id)
                    .ok()
                    .is_none_or(|index| self.inventory[index].1 != window.bounds)
            });
        if changed {
            self.inventory.clear();
            self.inventory
                .extend(windows.iter().map(|w| (w.id, w.bounds)));
            self.inventory.sort_unstable_by_key(|&(id, _)| id);
            self.members.clear();
            self.queue.clear();
            // Do not retain a historic peak after most windows have closed.
            // Hysteresis avoids reallocating for ordinary small fluctuations.
            pub(super) fn trim<T>(buffer: &mut Vec<T>, count: usize) {
                if buffer.capacity() > count.saturating_mul(4).max(64) {
                    buffer.shrink_to(count.saturating_mul(2));
                }
            }
            trim(&mut self.inventory, windows.len());
            trim(&mut self.members, windows.len());
            trim(&mut self.queue, windows.len());
        }
        if anchor.is_some_and(|id| self.contains(id)) {
            return;
        }
        self.members.clear();
        let Some(start) =
            anchor.and_then(|id| self.inventory.binary_search_by_key(&id, |&(id, _)| id).ok())
        else {
            return;
        };
        self.queue.clear();
        self.queue.extend(0..self.inventory.len());
        self.queue.swap(0, start);
        // One array holds both the discovered prefix and the unvisited suffix.
        let mut discovered = 1;
        let mut head = 0;
        while head < discovered && discovered < self.queue.len() {
            if cancelled() {
                // Never publish a partial component as a reusable cache hit.
                return;
            }
            let bounds = self.inventory[self.queue[head]].1;
            head += 1;
            let unvisited_start = discovered;
            for i in unvisited_start..self.queue.len() {
                let index = self.queue[i];
                #[cfg(test)]
                {
                    self.comparisons += 1;
                }
                if bounds.intersect(&self.inventory[index].1).is_some() {
                    self.queue.swap(i, discovered);
                    discovered += 1;
                }
            }
        }
        self.members.extend(
            self.queue[..discovered]
                .iter()
                .map(|&i| self.inventory[i].0),
        );
        self.members.sort_unstable();
    }
}
