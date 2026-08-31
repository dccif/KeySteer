//! Platform-side multi-click bookkeeping for native APIs that require an
//! explicit click count on every mouse event.

use std::time::{Duration, Instant};

use crate::api::command::{ButtonAction, MouseButton};
use crate::api::geometry::Point;

/// macOS treats clicks within this fixed spatial slop as one multi-click
/// sequence. Keeping the check here also prevents a quick click on a different
/// target from being mislabeled as a double-click.
const MULTI_CLICK_SLOP: f64 = 4.0;

#[derive(Clone, Copy, Debug)]
struct CompletedClick {
    button: MouseButton,
    position: Point,
    count: i64,
    completed_at: Instant,
}

#[derive(Clone, Copy, Debug)]
struct PressedClick {
    position: Point,
    count: i64,
    pressed_at: Instant,
}

/// A transaction prepared before native events are allocated. The next state
/// is committed only after the whole native sequence has been posted.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ClickPlan {
    counts: [i64; 4],
    len: usize,
    next: ClickTracker,
}

impl ClickPlan {
    pub(crate) fn counts(&self) -> &[i64] {
        &self.counts[..self.len]
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ClickTracker {
    interval: Duration,
    last: Option<CompletedClick>,
    pressed: [Option<PressedClick>; 5],
}

impl ClickTracker {
    pub(crate) fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
            pressed: [None; 5],
        }
    }

    pub(crate) fn prepare(
        &self,
        button: MouseButton,
        action: ButtonAction,
        position: Point,
        now: Instant,
    ) -> ClickPlan {
        let mut next = *self;
        let mut counts = [0; 4];
        let len = match action {
            ButtonAction::Press => {
                let count = next.next_count(button, position, now);
                counts[0] = count;
                next.pressed[button_index(button)] = Some(PressedClick {
                    position,
                    count,
                    pressed_at: now,
                });
                1
            }
            ButtonAction::Release => {
                let pressed = next.pressed[button_index(button)].take();
                let valid_click = pressed.filter(|pressed| {
                    now.saturating_duration_since(pressed.pressed_at) <= next.interval
                        && pressed.position.distance_to(&position) <= MULTI_CLICK_SLOP
                });
                counts[0] = valid_click.map_or(0, |pressed| pressed.count);
                next.observe_completed(button, position, counts[0], now);
                1
            }
            ButtonAction::Click => {
                let count = next.next_count(button, position, now);
                counts[..2].fill(count);
                next.pressed[button_index(button)] = None;
                next.observe_completed(button, position, count, now);
                2
            }
            ButtonAction::DoubleClick => {
                let first = next.next_count(button, position, now);
                let second = first.saturating_add(1);
                counts = [first, first, second, second];
                next.pressed[button_index(button)] = None;
                next.observe_completed(button, position, second, now);
                4
            }
        };
        ClickPlan { counts, len, next }
    }

    pub(crate) fn commit(&mut self, plan: ClickPlan) {
        *self = plan.next;
    }

    /// Fold a physical mouse-up into the same sequence as keyboard-generated
    /// clicks. This keeps physical+synthetic double/triple clicks possible.
    pub(crate) fn observe_completed(
        &mut self,
        button: MouseButton,
        position: Point,
        count: i64,
        now: Instant,
    ) {
        self.pressed[button_index(button)] = None;
        self.last = (count > 0).then_some(CompletedClick {
            button,
            position,
            count,
            completed_at: now,
        });
    }

    fn next_count(&self, button: MouseButton, position: Point, now: Instant) -> i64 {
        self.last
            .filter(|last| {
                last.button == button
                    && now.saturating_duration_since(last.completed_at) <= self.interval
                    && last.position.distance_to(&position) <= MULTI_CLICK_SLOP
            })
            .map_or(1, |last| last.count.saturating_add(1))
    }
}

const fn button_index(button: MouseButton) -> usize {
    match button {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
        MouseButton::X1 => 3,
        MouseButton::X2 => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: Duration = Duration::from_millis(500);

    fn commit(
        tracker: &mut ClickTracker,
        button: MouseButton,
        action: ButtonAction,
        position: Point,
        now: Instant,
    ) -> Vec<i64> {
        let plan = tracker.prepare(button, action, position, now);
        let counts = plan.counts().to_vec();
        tracker.commit(plan);
        counts
    }

    #[test]
    fn rapid_independent_click_actions_become_double_and_triple_clicks() {
        let mut tracker = ClickTracker::new(INTERVAL);
        let start = Instant::now();
        let at = Point::new(100.0, 200.0);

        assert_eq!(
            commit(
                &mut tracker,
                MouseButton::Left,
                ButtonAction::Click,
                at,
                start
            ),
            [1, 1]
        );
        assert_eq!(
            commit(
                &mut tracker,
                MouseButton::Left,
                ButtonAction::Click,
                at,
                start + Duration::from_millis(100),
            ),
            [2, 2]
        );
        assert_eq!(
            commit(
                &mut tracker,
                MouseButton::Left,
                ButtonAction::Click,
                at,
                start + Duration::from_millis(200),
            ),
            [3, 3]
        );
    }

    #[test]
    fn explicit_double_click_continues_the_existing_click_sequence() {
        let mut tracker = ClickTracker::new(INTERVAL);
        let start = Instant::now();
        let at = Point::new(40.0, 50.0);
        let first = tracker.prepare(MouseButton::Left, ButtonAction::Click, at, start);
        tracker.commit(first);

        let double = tracker.prepare(
            MouseButton::Left,
            ButtonAction::DoubleClick,
            at,
            start + Duration::from_millis(50),
        );
        assert_eq!(double.counts(), [2, 2, 3, 3]);
    }

    #[test]
    fn time_position_and_button_changes_break_the_sequence() {
        let start = Instant::now();
        let at = Point::new(10.0, 10.0);

        for (button, position, delay) in [
            (MouseButton::Right, at, 100),
            (MouseButton::Left, Point::new(20.0, 10.0), 100),
            (MouseButton::Left, at, 501),
        ] {
            let mut tracker = ClickTracker::new(INTERVAL);
            let first = tracker.prepare(MouseButton::Left, ButtonAction::Click, at, start);
            tracker.commit(first);
            let next = tracker.prepare(
                button,
                ButtonAction::Click,
                position,
                start + Duration::from_millis(delay),
            );
            assert_eq!(next.counts(), [1, 1]);
        }
    }

    #[test]
    fn separate_press_and_release_preserve_one_click_count() {
        let mut tracker = ClickTracker::new(INTERVAL);
        let start = Instant::now();
        let at = Point::new(10.0, 10.0);
        assert_eq!(
            commit(
                &mut tracker,
                MouseButton::Left,
                ButtonAction::Press,
                at,
                start
            ),
            [1]
        );
        assert_eq!(
            commit(
                &mut tracker,
                MouseButton::Left,
                ButtonAction::Release,
                at,
                start + Duration::from_millis(20),
            ),
            [1]
        );
        assert_eq!(
            commit(
                &mut tracker,
                MouseButton::Left,
                ButtonAction::Press,
                at,
                start + Duration::from_millis(100),
            ),
            [2]
        );
        assert_eq!(
            commit(
                &mut tracker,
                MouseButton::Left,
                ButtonAction::Release,
                at,
                start + Duration::from_millis(120),
            ),
            [2]
        );
    }

    #[test]
    fn a_long_or_dragged_press_releases_with_zero_click_count() {
        let start = Instant::now();
        for (release_at, delay) in [(Point::new(10.0, 10.0), 501), (Point::new(20.0, 10.0), 20)] {
            let mut tracker = ClickTracker::new(INTERVAL);
            let press = tracker.prepare(
                MouseButton::Left,
                ButtonAction::Press,
                Point::new(10.0, 10.0),
                start,
            );
            tracker.commit(press);
            let release = tracker.prepare(
                MouseButton::Left,
                ButtonAction::Release,
                release_at,
                start + Duration::from_millis(delay),
            );
            assert_eq!(release.counts(), [0]);
        }
    }

    #[test]
    fn physical_and_synthetic_clicks_share_a_sequence() {
        let mut tracker = ClickTracker::new(INTERVAL);
        let start = Instant::now();
        let at = Point::new(20.0, 30.0);
        tracker.observe_completed(MouseButton::Left, at, 1, start);

        let synthetic = tracker.prepare(
            MouseButton::Left,
            ButtonAction::Click,
            at,
            start + Duration::from_millis(100),
        );
        assert_eq!(synthetic.counts(), [2, 2]);
    }

    #[test]
    fn failed_native_allocation_can_discard_a_plan_without_advancing_state() {
        let tracker = ClickTracker::new(INTERVAL);
        let start = Instant::now();
        let at = Point::new(1.0, 2.0);
        let discarded = tracker.prepare(MouseButton::Left, ButtonAction::Click, at, start);
        assert_eq!(discarded.counts(), [1, 1]);

        let retry = tracker.prepare(
            MouseButton::Left,
            ButtonAction::Click,
            at,
            start + Duration::from_millis(10),
        );
        assert_eq!(retry.counts(), [1, 1]);
    }
}
