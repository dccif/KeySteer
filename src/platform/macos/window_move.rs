//! Poll-driven native-fullscreen transitions; no sleeps or background AX owners.
use std::time::{Duration, Instant};

use crate::api::command::WindowScreenTarget;
use crate::api::geometry::{Point, Rect, Screen};
use crate::platform::common::window_placement::{
    destination, following_pointer, map_between_screens,
};

pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(50);
const STAGE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Snapshot {
    pub bounds: Rect,
    pub fullscreen: Option<bool>,
}

pub(super) trait WindowAccess {
    fn snapshot(&self) -> Result<Snapshot, String>;
    fn set_position(&self, point: Point) -> Result<(), String>;
    fn set_fullscreen(&self, enabled: bool) -> Result<(), String>;
}

#[derive(Debug)]
enum Stage {
    Exiting,
    Moving,
    Entering,
}

pub(super) struct WindowMove<W> {
    window: W,
    cursor: Point,
    original: Rect,
    screens: Vec<Screen>,
    source: usize,
    target: usize,
    stage: Stage,
    next_poll: Instant,
    deadline: Instant,
    previous: Option<Snapshot>,
    last_error: Option<String>,
}

impl<W: WindowAccess> WindowMove<W> {
    /// Normal windows complete immediately; fullscreen windows retain their AX
    /// identity until the transition finishes, even when focus/cursor changes.
    pub(super) fn start(
        window: W,
        cursor: Point,
        screens: &[Screen],
        target: WindowScreenTarget,
        now: Instant,
    ) -> Result<(Option<Self>, Option<Point>), String> {
        let snapshot = window.snapshot()?;
        let Some((source, target)) = destination(screens, snapshot.bounds, target) else {
            return Ok((None, None));
        };
        if snapshot.fullscreen != Some(true) {
            let mapped = map_between_screens(snapshot.bounds, &screens[source], &screens[target]);
            window.set_position(Point::new(mapped.x, mapped.y))?;
            return Ok((
                None,
                Some(following_pointer(
                    cursor,
                    snapshot.bounds,
                    mapped,
                    screens[target].bounds,
                )),
            ));
        }
        if let Err(error) = window.set_fullscreen(false) {
            // AX can report a timeout after accepting a write. Restore best effort.
            let recovery = window.set_fullscreen(true).err();
            return Err(with_recovery(error, recovery));
        }
        Ok((
            Some(Self {
                window,
                cursor,
                original: snapshot.bounds,
                screens: screens.to_vec(),
                source,
                target,
                stage: Stage::Exiting,
                next_poll: now + POLL_INTERVAL,
                deadline: now + STAGE_TIMEOUT,
                previous: None,
                last_error: None,
            }),
            None,
        ))
    }

    pub(super) fn cancel(&self) -> Result<(), String> {
        self.window.set_fullscreen(true)
    }

    fn advance(&mut self, stage: Stage, now: Instant) {
        self.stage = stage;
        self.deadline = now + STAGE_TIMEOUT;
        self.previous = None;
        self.last_error = None;
    }

    /// Caller removes the transaction after any Some result. A pair of stable
    /// observations avoids acting on AX flags that change before window bounds.
    pub(super) fn poll(
        &mut self,
        screens: &[Screen],
        now: Instant,
    ) -> Option<Result<Point, String>> {
        if screens != self.screens {
            return Some(Err(with_recovery(
                "displays changed during fullscreen window move".into(),
                self.cancel().err(),
            )));
        }
        if now < self.next_poll {
            return None;
        }
        self.next_poll = now + POLL_INTERVAL;
        if now >= self.deadline {
            let error = format!(
                "fullscreen window move timed out during {:?}{}",
                self.stage,
                self.last_error
                    .as_ref()
                    .map_or(String::new(), |error| format!(": {error}"))
            );
            return Some(Err(with_recovery(error, self.cancel().err())));
        }
        let snapshot = match self.window.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.last_error = Some(error);
                self.previous = None;
                return None;
            }
        };
        let stable = self.previous == Some(snapshot);
        self.previous = Some(snapshot);
        if !stable {
            return None;
        }
        let on_target = self.on_target(snapshot.bounds);
        let result = match self.stage {
            Stage::Exiting if snapshot.fullscreen == Some(false) => {
                // Use the restored frame, not the old fullscreen frame.
                let mapped = map_between_screens(
                    snapshot.bounds,
                    &screens[self.source],
                    &screens[self.target],
                );
                self.window
                    .set_position(Point::new(mapped.x, mapped.y))
                    .map(|()| self.advance(Stage::Moving, now))
            }
            Stage::Moving if snapshot.fullscreen == Some(false) && on_target => self
                .window
                .set_fullscreen(true)
                .map(|()| self.advance(Stage::Entering, now)),
            Stage::Entering
                if snapshot.fullscreen == Some(true)
                    && on_target
                    && fills_display(snapshot.bounds, screens[self.target].bounds) =>
            {
                return Some(Ok(following_pointer(
                    self.cursor,
                    self.original,
                    snapshot.bounds,
                    screens[self.target].bounds,
                )));
            }
            _ => Ok(()),
        };
        if let Err(error) = result {
            return Some(Err(with_recovery(error, self.cancel().err())));
        }
        None
    }

    fn on_target(&self, bounds: Rect) -> bool {
        // Asking for Next always gives the source when at least two screens exist.
        destination(&self.screens, bounds, WindowScreenTarget::Next)
            .is_some_and(|(source, _)| source == self.target)
            && bounds
                .intersect(&self.screens[self.target].bounds)
                .is_some()
    }
}

fn fills_display(window: Rect, display: Rect) -> bool {
    // Fullscreen AX frames may exclude the notch/menu-bar safe area. Require
    // near-full coverage rather than exact NSScreen equality.
    window.intersect(&display).is_some_and(|overlap| {
        overlap.width >= display.width * 0.9 && overlap.height >= display.height * 0.9
    })
}

fn with_recovery(error: String, recovery: Option<String>) -> String {
    recovery.map_or_else(
        || error.clone(),
        |recovery| format!("{error}; restoring fullscreen also failed: {recovery}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct State {
        snapshot: Option<Snapshot>,
        writes: Vec<String>,
        fail_position: bool,
    }
    #[derive(Clone)]
    struct Window(Rc<RefCell<State>>);
    impl WindowAccess for Window {
        fn snapshot(&self) -> Result<Snapshot, String> {
            self.0
                .borrow()
                .snapshot
                .ok_or("window temporarily unavailable".into())
        }
        fn set_position(&self, point: Point) -> Result<(), String> {
            let mut state = self.0.borrow_mut();
            state.writes.push(format!("move {},{}", point.x, point.y));
            if state.fail_position {
                Err("move rejected".into())
            } else {
                Ok(())
            }
        }
        fn set_fullscreen(&self, enabled: bool) -> Result<(), String> {
            self.0
                .borrow_mut()
                .writes
                .push(format!("fullscreen {enabled}"));
            Ok(())
        }
    }
    fn screens() -> Vec<Screen> {
        [
            Rect::new(0.0, 0.0, 1000.0, 800.0),
            Rect::new(-1500.0, 0.0, 1500.0, 1000.0),
        ]
        .into_iter()
        .map(|bounds| Screen {
            bounds,
            work_area: bounds,
            is_primary: bounds.x == 0.0,
            scale: 1.0,
            name: None,
        })
        .collect()
    }
    fn window(bounds: Rect, fullscreen: bool) -> Window {
        Window(Rc::new(RefCell::new(State {
            snapshot: Some(Snapshot {
                bounds,
                fullscreen: Some(fullscreen),
            }),
            ..State::default()
        })))
    }
    fn start(window: &Window, screens: &[Screen], now: Instant) -> WindowMove<Window> {
        let (movement, pointer) = WindowMove::start(
            window.clone(),
            Point::new(250.0, 400.0),
            screens,
            WindowScreenTarget::Next,
            now,
        )
        .unwrap();
        assert!(pointer.is_none());
        movement.unwrap()
    }
    fn observe(
        movement: &mut WindowMove<Window>,
        window: &Window,
        screens: &[Screen],
        bounds: Rect,
        fullscreen: bool,
        now: Instant,
    ) -> Option<Result<Point, String>> {
        window.0.borrow_mut().snapshot = Some(Snapshot {
            bounds,
            fullscreen: Some(fullscreen),
        });
        assert!(movement.poll(screens, now).is_none());
        movement.poll(screens, now + POLL_INTERVAL)
    }

    #[test]
    fn fullscreen_move_waits_for_restored_frame_and_warps_only_after_target_fullscreen() {
        let screens = screens();
        let now = Instant::now();
        let window = window(screens[0].bounds, true);
        let mut movement = start(&window, &screens, now);
        assert_eq!(window.0.borrow().writes, ["fullscreen false"]);
        // While the full-screen animation runs, do not submit a position.
        assert!(movement.poll(&screens, now + POLL_INTERVAL).is_none());
        assert!(movement.poll(&screens, now + POLL_INTERVAL * 2).is_none());
        let restored = Rect::new(100.0, 100.0, 600.0, 400.0);
        assert!(
            observe(
                &mut movement,
                &window,
                &screens,
                restored,
                false,
                now + POLL_INTERVAL * 3
            )
            .is_none()
        );
        assert_eq!(
            window.0.borrow().writes,
            ["fullscreen false", "move -1275,150"]
        );
        let moved = Rect::new(-1275.0, 150.0, 600.0, 400.0);
        assert!(
            observe(
                &mut movement,
                &window,
                &screens,
                moved,
                false,
                now + POLL_INTERVAL * 5
            )
            .is_none()
        );
        assert_eq!(window.0.borrow().writes.last().unwrap(), "fullscreen true");
        // An early fullscreen flag on the restored frame isn't completion.
        assert!(
            observe(
                &mut movement,
                &window,
                &screens,
                moved,
                true,
                now + POLL_INTERVAL * 7
            )
            .is_none()
        );
        let pointer = observe(
            &mut movement,
            &window,
            &screens,
            screens[1].bounds,
            true,
            now + POLL_INTERVAL * 9,
        )
        .unwrap()
        .unwrap();
        assert_eq!(pointer, Point::new(-1125.0, 500.0));
    }

    #[test]
    fn timeout_restores_fullscreen_without_reporting_pointer_success() {
        let screens = screens();
        let now = Instant::now();
        let window = window(screens[0].bounds, true);
        let mut movement = start(&window, &screens, now);
        let error = movement
            .poll(&screens, now + STAGE_TIMEOUT)
            .unwrap()
            .unwrap_err();
        assert!(error.contains("Exiting"));
        assert_eq!(
            window.0.borrow().writes,
            ["fullscreen false", "fullscreen true"]
        );
    }

    #[test]
    fn display_disconnect_cancels_and_restores_fullscreen() {
        let screens = screens();
        let now = Instant::now();
        let window = window(screens[0].bounds, true);
        let mut movement = start(&window, &screens, now);
        assert!(movement.poll(&screens[..1], now).unwrap().is_err());
        assert_eq!(
            window.0.borrow().writes,
            ["fullscreen false", "fullscreen true"]
        );
    }

    #[test]
    fn rejected_position_restores_fullscreen_without_replaying_move() {
        let screens = screens();
        let now = Instant::now();
        let window = window(screens[0].bounds, true);
        window.0.borrow_mut().fail_position = true;
        let mut movement = start(&window, &screens, now);
        assert!(
            observe(
                &mut movement,
                &window,
                &screens,
                Rect::new(100.0, 100.0, 600.0, 400.0),
                false,
                now + POLL_INTERVAL
            )
            .unwrap()
            .is_err()
        );
        assert_eq!(
            window.0.borrow().writes,
            ["fullscreen false", "move -1275,150", "fullscreen true"]
        );
    }

    #[test]
    fn one_display_does_not_exit_fullscreen() {
        let screens = screens();
        let window = window(screens[0].bounds, true);
        let (movement, pointer) = WindowMove::start(
            window.clone(),
            Point::default(),
            &screens[..1],
            WindowScreenTarget::Next,
            Instant::now(),
        )
        .unwrap();
        assert!(movement.is_none() && pointer.is_none());
        assert!(window.0.borrow().writes.is_empty());
    }

    #[test]
    fn ordinary_window_moves_without_fullscreen_transition() {
        let screens = screens();
        let window = window(Rect::new(100.0, 100.0, 600.0, 400.0), false);
        let (movement, pointer) = WindowMove::start(
            window.clone(),
            Point::new(200.0, 200.0),
            &screens,
            WindowScreenTarget::Next,
            Instant::now(),
        )
        .unwrap();
        assert!(movement.is_none());
        assert_eq!(pointer, Some(Point::new(-1175.0, 250.0)));
        assert_eq!(window.0.borrow().writes, ["move -1275,150"]);
    }

    #[test]
    fn temporary_ax_failure_does_not_advance_or_finish_the_move() {
        let screens = screens();
        let now = Instant::now();
        let window = window(screens[0].bounds, true);
        let mut movement = start(&window, &screens, now);
        window.0.borrow_mut().snapshot = None;
        assert!(movement.poll(&screens, now + POLL_INTERVAL).is_none());
        assert_eq!(window.0.borrow().writes, ["fullscreen false"]);
        assert!(
            observe(
                &mut movement,
                &window,
                &screens,
                Rect::new(100.0, 100.0, 600.0, 400.0),
                false,
                now + POLL_INTERVAL * 2
            )
            .is_none()
        );
        assert_eq!(window.0.borrow().writes.len(), 2);
    }
}
