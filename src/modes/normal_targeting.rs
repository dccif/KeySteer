//! Opt-in composition: ordinary Normal gestures and an existing silent grid.
//! The catalog installs this type only when positioning is configured. The
//! ordinary Normal object and its frame/input paths are unchanged.

use crate::api::overlay::Color;
use crate::api::theme::Palette;
use crate::api::{Binding, Command, CommandBatch, HostContext, KeyState, Mode, ModeEvent, ModeId};

use super::NormalMode;
use super::targeting::{Selection, TargetingController};

pub(crate) struct NormalTargeting {
    normal: NormalMode,
    grid: TargetingController,
    reset_after_move: bool,
    reset_after_click: bool,
    pending_reset: bool,
}

impl NormalTargeting {
    pub(crate) fn new(
        normal: NormalMode,
        grid: TargetingController,
        reset_after_move: bool,
        reset_after_click: bool,
    ) -> Self {
        Self {
            normal,
            grid,
            reset_after_move,
            reset_after_click,
            pending_reset: true,
        }
    }

    fn prepare(&mut self, ctx: &HostContext<'_>) -> crate::api::Rect {
        // Silent grids emit no commands here. Checking the screen lazily avoids
        // subscribing Normal to global physical pointer motion.
        let bounds = ctx.active_bounds();
        if self.pending_reset || self.grid.session.root() != Some(bounds) {
            self.pending_reset = false;
            self.grid.reset(bounds);
        }
        bounds
    }
}

impl Mode for NormalTargeting {
    fn id(&self) -> ModeId {
        self.normal.id()
    }
    fn display_name(&self) -> String {
        self.normal.display_name()
    }
    fn captures_keyboard(&self) -> bool {
        self.normal.captures_keyboard()
    }
    fn wants_pointer_events(&self) -> bool {
        false
    }
    fn indicator_color(&self, palette: &Palette) -> Option<Color> {
        self.normal.indicator_color(palette)
    }

    fn handle(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        match event {
            ModeEvent::Binding {
                binding,
                state: KeyState::Down,
                ..
            } if matches!(binding.as_ref(), Binding::TargetingKey(_)) => {
                let Binding::TargetingKey(key) = binding.as_ref() else {
                    unreachable!()
                };
                let bounds = self.prepare(ctx);
                return match self.grid.input(key, bounds) {
                    Selection::Follow(point) | Selection::Commit(point) => {
                        // A one-level silent grid is an absolute positioning pad.
                        // Reuse the root on the next input, regardless of reset_on.
                        self.pending_reset = self.grid.max_depth == 1;
                        Command::warp_to(point).into()
                    }
                    _ => CommandBatch::new(),
                };
            }
            ModeEvent::Activated { .. }
            | ModeEvent::Deactivated
            | ModeEvent::Restarted
            | ModeEvent::ScreensChanged(_) => self.pending_reset = true,
            ModeEvent::Clicked { .. } if self.reset_after_click => self.pending_reset = true,
            ModeEvent::ScreenRetargeted { screen, preserve } => {
                // The host cursor may already point at the destination screen.
                // Only initialize if no live path exists; otherwise replay it.
                if self.pending_reset {
                    self.grid.reset(screen.bounds);
                    self.pending_reset = false;
                }
                return Command::warp_to(self.grid.retarget(screen.bounds, *preserve)).into();
            }
            _ => {}
        }
        let commands = self.normal.handle(event, ctx);
        if self.reset_after_move
            && !self.pending_reset
            && commands.iter().any(|command| {
                matches!(command,
                Command::MovePointer { dx, dy } if *dx != 0.0 || *dy != 0.0)
            })
        {
            self.pending_reset = true;
        }
        commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Key, Point, Rect, Screen, TargetingLifecycle};
    use crate::modes::targeting::Layout;
    use stats_alloc::Region;
    use std::time::Duration;

    struct NoGridPresenter;
    impl crate::api::presentation::Presenter for NoGridPresenter {
        fn prepare_hints(
            &self,
            _: crate::api::presentation::HintContent<'_>,
            _: &mut crate::api::presentation::VisualLayerPlan,
            _: &mut Option<Vec<(usize, Rect)>>,
            _: &HostContext<'_>,
        ) {
            panic!("blind targeting must not prepare hints")
        }
        fn compose(
            &self,
            _: crate::api::presentation::View<'_>,
            _: &HostContext<'_>,
        ) -> crate::api::overlay::OverlayScene {
            panic!("blind targeting must not compose a scene")
        }
    }

    #[test]
    fn normal_targeting_deep_selection_navigation_and_motion_do_not_allocate_or_render() {
        let config = crate::config::Config::default();
        let mut mode = NormalTargeting::new(
            crate::app::mode_catalog::normal(&config),
            TargetingController::new(
                Layout {
                    rows: 2,
                    cols: 2,
                    keys: "asdf".chars().collect(),
                },
                &[],
                20,
                None,
                true,
                TargetingLifecycle::default(),
            ),
            true,
            true,
        );
        let palette = Palette::default();
        let screens = [Screen {
            bounds: Rect::new(0.0, 0.0, 1024.0, 1024.0),
            work_area: Rect::new(0.0, 0.0, 1024.0, 1024.0),
            is_primary: true,
            scale: 1.0,
            name: None,
        }];
        let ctx = HostContext {
            presenter: &NoGridPresenter,
            screens: &screens,
            cursor: Point::new(512.0, 512.0),
            focused_app: None,
            palette: &palette,
        };
        let events = ["a", "tab", "space"].map(|name| ModeEvent::Binding {
            binding: Binding::TargetingKey(Key::new(name).unwrap()).into(),
            key: Key::new(name).unwrap(),
            state: KeyState::Down,
        });
        let movement = ModeEvent::Binding {
            binding: Binding::Move(crate::api::Direction::Left).into(),
            key: Key::new("h").unwrap(),
            state: KeyState::Down,
        };
        let frame = ModeEvent::Frame {
            elapsed: Duration::from_millis(16),
        };
        let region = Region::new(crate::TEST_ALLOCATOR);
        for _ in 0..100 {
            for _ in 0..20 {
                mode.handle(&events[0], &ctx);
            }
            for _ in 0..20 {
                mode.handle(&events[1], &ctx);
            }
            mode.handle(&events[2], &ctx);
            mode.handle(
                &ModeEvent::ScreenRetargeted {
                    screen: screens[0].clone(),
                    preserve: true,
                },
                &ctx,
            );
        }
        let allocations = region.change();
        assert_eq!(allocations.allocations + allocations.reallocations, 0);
        mode.handle(&movement, &ctx);
        let region = Region::new(crate::TEST_ALLOCATOR);
        for _ in 0..1000 {
            mode.handle(&frame, &ctx);
        }
        let allocations = region.change();
        assert_eq!(allocations.allocations + allocations.reallocations, 0);
    }
}
