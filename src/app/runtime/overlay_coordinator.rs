//! Overlay scene decoration, deduplication, and presentation.

use super::*;

use crate::presentation::dynamic::HeldTargetsText;

fn held_targets_text(
    targets: &LatchedTargets,
    speed: Option<crate::api::binding::Speed>,
) -> Option<HeldTargetsText> {
    if targets.is_empty() && speed.is_none() {
        return None;
    }

    const PREFIX: &str = "● ";
    const SEPARATOR: &str = " · ";
    let (names_len, names_characters) = targets
        .iter()
        .map(|target| {
            let name = target.canonical_str();
            (name.len(), name.chars().count())
        })
        .fold(
            (0, 0),
            |(bytes, characters), (name_bytes, name_characters)| {
                (bytes + name_bytes, characters + name_characters)
            },
        );
    let target_count = targets.iter().len();
    let speed_name = speed.map(|speed| match speed {
        crate::api::binding::Speed::Precision => "PRECISION",
        crate::api::binding::Speed::Slow => "SLOW",
        crate::api::binding::Speed::Fast => "FAST",
    });
    let speed_characters = speed_name.map_or(0, |name| name.chars().count());
    let speed_separator = usize::from(speed_name.is_some() && target_count > 0);
    let mut text = String::with_capacity(
        PREFIX.len()
            + names_len
            + speed_name.map_or(0, str::len)
            + SEPARATOR.len() * (target_count + speed_separator).saturating_sub(1),
    );
    text.push_str(PREFIX);
    if let Some(speed_name) = speed_name {
        text.push_str(speed_name);
    }
    for (index, target) in targets.iter().enumerate() {
        if index != 0 || speed.is_some() {
            text.push_str(SEPARATOR);
        }
        for character in target.canonical_str().chars() {
            text.push(if character == '_' {
                ' '
            } else {
                character.to_ascii_uppercase()
            });
        }
    }
    Some(HeldTargetsText {
        value: text,
        character_count: PREFIX.chars().count()
            + names_characters
            + speed_characters
            + SEPARATOR.chars().count() * (target_count + speed_separator).saturating_sub(1),
    })
}

/// Last overlay intent produced by one (possibly nested) command batch.
/// Pointer injection still happens immediately; only expensive presentation is
/// coalesced until the outermost batch completes.
pub(super) enum PendingOverlay {
    Refresh,
    Positions,
    Show(Arc<OverlayScene>),
    Hide,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct OverlayPositions {
    pub(super) cursor: Option<Point>,
    pub(super) indicator: Option<Point>,
}

use crate::presentation::dynamic::IndicatorGeometry;

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct DynamicOverlayState {
    pub(super) cursor: bool,
    pub(super) indicator: Option<IndicatorGeometry>,
    pub(super) follows_cursor_screen: bool,
}

#[derive(Default)]
pub(super) struct OverlayCoordinator {
    pub(super) key_help_visible: bool,
    pub(super) key_help_cache: Option<Box<super::key_help::KeyHelpCache>>,
    pub(super) last_scene: Option<Arc<OverlayScene>>,
    pub(super) content: Option<Arc<OverlayScene>>,
    pub(super) visible: bool,
    pub(super) dynamic: DynamicOverlayState,
    pub(super) positions: Option<OverlayPositions>,
    pub(super) position_fast_path_disabled: bool,
    pub(super) command_batch_depth: usize,
    pub(super) pending: Option<PendingOverlay>,
    pub(super) speed_toggle: Option<crate::api::binding::Speed>,
}

impl OverlayCoordinator {
    pub(super) fn reset(&mut self) {
        self.last_scene = None;
        self.content = None;
        self.visible = false;
        self.dynamic = DynamicOverlayState::default();
        self.positions = None;
        self.position_fast_path_disabled = false;
        self.command_batch_depth = 0;
        self.pending = None;
        self.speed_toggle = None;
        self.key_help_visible = false;
        self.key_help_cache = None;
    }
}
impl Engine {
    pub(super) fn show_overlay(
        &mut self,
        mut scene: Arc<OverlayScene>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        self.overlay.key_help_cache = None;
        if self.overlay.command_batch_depth > 0 {
            self.overlay.pending = Some(PendingOverlay::Show(scene));
            return Ok(());
        }
        // Static primitives keep their order for the lifetime of the mode
        // scene. Sort once before sharing them instead of sorting (and
        // detaching copy-on-write storage) on every cursor refresh.
        Arc::make_mut(&mut scene).sort_in_place();
        self.show_shared_overlay_now(scene, backend)
    }

    fn show_shared_overlay_now(
        &mut self,
        scene: Arc<OverlayScene>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        self.overlay.content = Some(Arc::clone(&scene));
        self.present_overlay(scene.as_ref().clone(), backend)
    }

    fn present_overlay(
        &mut self,
        mut scene: OverlayScene,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let starts_visible_session = !self.overlay.visible;
        let cursor_only = scene.clip.is_none()
            && scene.backdrop.is_none()
            && scene.labels.is_empty()
            && scene.shapes.is_empty()
            && scene.indicator.is_none();
        // The engine owns mode decoration. A temporary mode changes only this
        // display identity; the active mode keeps its underlying scene/state.
        let display_mode = self.display_mode();
        let mut dynamic = DynamicOverlayState {
            follows_cursor_screen: scene.clip.is_none()
                && scene.backdrop.is_none()
                && scene.labels.is_empty()
                && scene.shapes.is_empty(),
            ..DynamicOverlayState::default()
        };
        if let Some(cursor) = self
            .settings
            .mode_indicator
            .cursor_for_mode_ref(display_mode.as_str())
        {
            dynamic.cursor = true;
            let pressed_button = [
                crate::api::binding::Button::Left,
                crate::api::binding::Button::Middle,
                crate::api::binding::Button::Right,
            ]
            .into_iter()
            .find(|button| self.input.latched.contains(&InputTarget::Mouse(*button)))
            .or_else(|| self.input.active_click_indicators.latest_button());
            scene.cursor_marker = Some(crate::presentation::dynamic::cursor_marker(
                cursor,
                pressed_button,
                &self.palette,
                self.cursor,
            ));
        }
        if scene.indicator.is_none()
            && display_mode != ModeId::window()
            && let Some((indicator, geometry)) = self.build_indicator(&display_mode)
        {
            scene.indicator = Some(indicator);
            dynamic.indicator = Some(geometry);
        }
        // Cursor-only scenes use a fixed screen-sized surface. Moving an
        // NSPanel on every pointer event trails macOS's hardware cursor under
        // fast motion; keeping the panel fixed lets only its lightweight
        // cursor and indicator subviews move, matching grid-mode behaviour.
        if scene.clip.is_none()
            && scene.backdrop.is_none()
            && scene.labels.is_empty()
            && scene.shapes.is_empty()
        {
            scene.clip =
                Screen::containing(&self.screens, &self.cursor).map(|screen| screen.bounds);
        }
        self.decorate_key_help(&mut scene);
        let trace_overlay = if cursor_only {
            self.settings.debug.motion
        } else {
            self.settings.debug.overlay
        };
        self.trace_lazy(trace_overlay, "overlay", || {
            format!(
                "present mode={} shapes={} labels={} indicator={} clip={:?}",
                self.registry.active,
                scene.shapes.len(),
                scene.labels.len(),
                scene
                    .indicator
                    .as_ref()
                    .map(|item| item.text.as_str())
                    .unwrap_or("<none>"),
                scene.clip
            )
        });

        // Skip identical frames: overlay presentation is the most expensive
        // thing we do, and modes redraw on every keystroke.
        let positions = OverlayPositions {
            cursor: dynamic.cursor.then_some(self.cursor),
            indicator: dynamic
                .indicator
                .map(|geometry| geometry.position(self.cursor, &self.screens)),
        };
        if self.overlay.visible
            && self
                .overlay
                .last_scene
                .as_deref()
                .is_some_and(|previous| previous == &scene)
        {
            self.overlay.dynamic = dynamic;
            self.overlay.positions = Some(positions);
            return Ok(());
        }
        let scene = Arc::new(scene);
        crate::support::perf_probe::mark("overlay_submitted");
        crate::support::perf_probe::mark("native_submitted");
        backend.present(Arc::clone(&scene))?;
        crate::support::perf_probe::mark("overlay_presented");
        self.trace(trace_overlay, "overlay", "backend present: ok");
        self.overlay.last_scene = Some(scene);
        self.overlay.visible = true;
        self.overlay.dynamic = dynamic;
        self.overlay.positions = Some(positions);
        if starts_visible_session {
            self.overlay.position_fast_path_disabled = false;
        }
        Ok(())
    }

    pub(super) fn hide_overlay(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        if self.overlay.command_batch_depth > 0 {
            self.overlay.pending = Some(PendingOverlay::Hide);
            return Ok(());
        }
        self.hide_overlay_now(backend)
    }

    fn hide_overlay_now(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        self.overlay.content = None;
        if self.registry.active != ModeId::idle() {
            return self.present_overlay(OverlayScene::new(), backend);
        }
        if self.overlay.visible {
            backend.dismiss()?;
        }
        self.overlay.last_scene = None;
        self.overlay.visible = false;
        self.overlay.dynamic = DynamicOverlayState::default();
        self.overlay.positions = None;
        self.overlay.position_fast_path_disabled = false;
        crate::support::perf_probe::mark("overlay_hidden");
        Ok(())
    }

    pub(super) fn refresh_overlay(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        if self.overlay.command_batch_depth > 0 {
            if matches!(self.overlay.pending, None | Some(PendingOverlay::Positions)) {
                self.overlay.pending = Some(PendingOverlay::Refresh);
            }
            return Ok(());
        }
        self.refresh_overlay_now(backend)
    }

    fn refresh_overlay_now(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        if self.registry.active == ModeId::idle() {
            return Ok(());
        }
        let scene = self.overlay.content.as_deref().cloned().unwrap_or_default();
        self.present_overlay(scene, backend)
    }

    /// Move only Engine-owned dynamic decorations. Static mode content and
    /// indicator text/style remain shared with the last complete scene.
    pub(super) fn refresh_overlay_positions(
        &mut self,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if self.registry.active == ModeId::idle() || !self.overlay.visible {
            return Ok(());
        }
        if self.overlay.key_help_visible
            && self.overlay.key_help_cache.as_ref().is_none_or(|cache| {
                Screen::containing(&self.screens, &self.cursor)
                    .is_none_or(|screen| !cache.matches_screen(screen))
            })
        {
            return self.refresh_overlay(backend);
        }
        if self.overlay.dynamic.follows_cursor_screen {
            let current_clip =
                Screen::containing(&self.screens, &self.cursor).map(|screen| screen.bounds);
            if self
                .overlay
                .last_scene
                .as_deref()
                .and_then(|scene| scene.clip)
                != current_clip
            {
                return self.refresh_overlay(backend);
            }
        }

        let positions = OverlayPositions {
            cursor: self.overlay.dynamic.cursor.then_some(self.cursor),
            indicator: self
                .overlay
                .dynamic
                .indicator
                .map(|geometry| geometry.position(self.cursor, &self.screens)),
        };
        if positions.cursor.is_none() && positions.indicator.is_none() {
            return Ok(());
        }
        if self.overlay.positions == Some(positions) {
            return Ok(());
        }
        if self.overlay.position_fast_path_disabled {
            return self.refresh_overlay(backend);
        }
        if self.overlay.command_batch_depth > 0 {
            if self.overlay.pending.is_none() {
                self.overlay.pending = Some(PendingOverlay::Positions);
            }
            return Ok(());
        }

        crate::support::perf_probe::mark("native_submitted");
        match backend.update_overlay_positions(positions.cursor, positions.indicator) {
            Ok(true) => {
                self.overlay.positions = Some(positions);
                Ok(())
            }
            Ok(false) => {
                self.overlay.position_fast_path_disabled = true;
                self.refresh_overlay_now(backend)
            }
            Err(error) => {
                self.overlay.position_fast_path_disabled = true;
                crate::support::logging::report_error(
                    "overlay",
                    format!(
                        "position-only overlay update failed; using complete frames until the overlay is dismissed: {error}"
                    ),
                );
                self.refresh_overlay_now(backend)
            }
        }
    }

    pub(super) fn flush_pending_overlay(
        &mut self,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        match self.overlay.pending.take() {
            Some(PendingOverlay::Refresh) => self.refresh_overlay_now(backend),
            Some(PendingOverlay::Positions) => self.refresh_overlay_positions(backend),
            Some(PendingOverlay::Show(mut scene)) => {
                Arc::make_mut(&mut scene).sort_in_place();
                self.show_shared_overlay_now(scene, backend)
            }
            Some(PendingOverlay::Hide) => self.hide_overlay_now(backend),
            None => Ok(()),
        }
    }

    pub(super) fn build_indicator(
        &self,
        display_mode: &ModeId,
    ) -> Option<(Indicator, IndicatorGeometry)> {
        let mode = self.registry.get(display_mode)?;
        let (text, ui) = self
            .settings
            .mode_indicator
            .for_mode_with(display_mode.as_str(), || mode.display_name())?;

        let background = mode
            .indicator_color(&self.palette)
            .unwrap_or_else(|| self.palette.surface_label());
        let held_text = mode
            .indicator_detail()
            .map(|value| HeldTargetsText {
                character_count: value.chars().count(),
                value,
            })
            .or_else(|| held_targets_text(&self.input.latched, self.overlay.speed_toggle));
        Some(crate::presentation::dynamic::indicator(
            text,
            &ui,
            background,
            held_text,
            &self.palette,
            self.cursor,
            &self.screens,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_target_text_uses_stable_order_and_exact_character_count() {
        let mut targets = LatchedTargets::default();
        targets.insert(InputTarget::Mouse(Button::Middle));
        targets.insert(InputTarget::Key(Key::new("left_shift").unwrap()));
        targets.insert(InputTarget::Mouse(Button::Right));
        targets.insert(InputTarget::Mouse(Button::Left));

        let held = held_targets_text(&targets, None).expect("held targets");
        assert_eq!(
            held.value,
            "● LEFT SHIFT · MOUSE LEFT · MOUSE RIGHT · MOUSE MIDDLE"
        );
        assert_eq!(held.character_count, held.value.chars().count());
    }

    #[test]
    fn held_target_text_preserves_non_ascii_key_names() {
        let mut targets = LatchedTargets::default();
        targets.insert(InputTarget::Key(Key::new("é").unwrap()));

        let held = held_targets_text(&targets, None).expect("held target");
        assert_eq!(held.value, "● é");
        assert_eq!(held.character_count, 3);
    }

    #[test]
    fn speed_toggle_uses_the_same_second_indicator_line() {
        let targets = LatchedTargets::default();
        let held = held_targets_text(&targets, Some(crate::api::binding::Speed::Fast))
            .expect("speed indicator");
        assert_eq!(held.value, "● FAST");
        assert_eq!(held.character_count, 6);
    }
}
