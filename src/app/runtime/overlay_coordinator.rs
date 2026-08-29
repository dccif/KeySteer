//! Overlay scene decoration, deduplication, and presentation.

use super::*;

struct HeldTargetsText {
    value: String,
    character_count: usize,
}

fn held_targets_text(targets: &LatchedTargets) -> Option<HeldTargetsText> {
    if targets.is_empty() {
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
    let mut text = String::with_capacity(
        PREFIX.len() + names_len + SEPARATOR.len() * target_count.saturating_sub(1),
    );
    text.push_str(PREFIX);
    for (index, target) in targets.iter().enumerate() {
        if index != 0 {
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
            + SEPARATOR.chars().count() * target_count.saturating_sub(1),
    })
}

impl Engine {
    pub(super) fn show_overlay(
        &mut self,
        mut scene: Arc<OverlayScene>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if self.command_batch_depth > 0 {
            self.pending_overlay = Some(PendingOverlay::Show(scene));
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
        self.overlay_content = Some(Arc::clone(&scene));
        self.present_overlay(scene.as_ref().clone(), backend)
    }

    fn present_overlay(
        &mut self,
        mut scene: OverlayScene,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let starts_visible_session = !self.overlay_visible;
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
            .config
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
            .find(|button| self.latched.contains(&InputTarget::Mouse(*button)))
            .or_else(|| self.active_click_indicators.latest_button());
            let pressed_color = match pressed_button {
                Some(crate::api::binding::Button::Left) => cursor.left_pressed_color,
                Some(crate::api::binding::Button::Middle) => cursor.middle_pressed_color,
                Some(crate::api::binding::Button::Right) => cursor.right_pressed_color,
                None => None,
            }
            .and_then(|color| color.resolve(self.palette.appearance));
            let fill = pressed_color.map_or_else(
                || {
                    crate::config::style::resolve(
                        cursor.fill_color,
                        self.palette.appearance,
                        self.palette.accent.with_alpha(34),
                    )
                },
                |color| color.with_opacity(0.2),
            );
            let stroke = pressed_color.unwrap_or_else(|| {
                crate::config::style::resolve(
                    cursor.stroke_color,
                    self.palette.appearance,
                    self.palette.accent_alt.with_alpha(210),
                )
            });
            scene.cursor_marker = Some(CursorMarker {
                center: self.cursor,
                radius: cursor.radius.max(1) as f64,
                fill,
                stroke,
                stroke_width: cursor.stroke_width.max(0) as f64,
            });
        }
        if scene.indicator.is_none()
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
        let trace_overlay = if cursor_only {
            self.config.debug.motion
        } else {
            self.config.debug.overlay
        };
        self.trace_lazy(trace_overlay, "overlay", || {
            format!(
                "present mode={} shapes={} labels={} indicator={} clip={:?}",
                self.active,
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
        if self.overlay_visible
            && self
                .last_scene
                .as_deref()
                .is_some_and(|previous| previous == &scene)
        {
            self.overlay_dynamic = dynamic;
            self.overlay_positions = Some(positions);
            return Ok(());
        }
        let scene = Arc::new(scene);
        crate::app::perf_probe::mark("overlay_submitted");
        crate::app::perf_probe::mark("native_submitted");
        backend.present(Arc::clone(&scene))?;
        crate::app::perf_probe::mark("overlay_presented");
        self.trace(trace_overlay, "overlay", "backend present: ok");
        self.last_scene = Some(scene);
        self.overlay_visible = true;
        self.overlay_dynamic = dynamic;
        self.overlay_positions = Some(positions);
        if starts_visible_session {
            self.overlay_position_fast_path_disabled = false;
        }
        Ok(())
    }

    pub(super) fn hide_overlay(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        if self.command_batch_depth > 0 {
            self.pending_overlay = Some(PendingOverlay::Hide);
            return Ok(());
        }
        self.hide_overlay_now(backend)
    }

    fn hide_overlay_now(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        self.overlay_content = None;
        if self.active != ModeId::idle() {
            return self.present_overlay(OverlayScene::new(), backend);
        }
        if self.overlay_visible {
            backend.dismiss()?;
        }
        self.last_scene = None;
        self.overlay_visible = false;
        self.overlay_dynamic = DynamicOverlayState::default();
        self.overlay_positions = None;
        self.overlay_position_fast_path_disabled = false;
        crate::app::perf_probe::mark("overlay_hidden");
        Ok(())
    }

    pub(super) fn refresh_overlay(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        if self.command_batch_depth > 0 {
            if matches!(self.pending_overlay, None | Some(PendingOverlay::Positions)) {
                self.pending_overlay = Some(PendingOverlay::Refresh);
            }
            return Ok(());
        }
        self.refresh_overlay_now(backend)
    }

    fn refresh_overlay_now(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        if self.active == ModeId::idle() {
            return Ok(());
        }
        let scene = self.overlay_content.as_deref().cloned().unwrap_or_default();
        self.present_overlay(scene, backend)
    }

    /// Move only Engine-owned dynamic decorations. Static mode content and
    /// indicator text/style remain shared with the last complete scene.
    pub(super) fn refresh_overlay_positions(
        &mut self,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if self.active == ModeId::idle() || !self.overlay_visible {
            return Ok(());
        }
        if self.overlay_dynamic.follows_cursor_screen {
            let current_clip =
                Screen::containing(&self.screens, &self.cursor).map(|screen| screen.bounds);
            if self.last_scene.as_deref().and_then(|scene| scene.clip) != current_clip {
                return self.refresh_overlay(backend);
            }
        }

        let positions = OverlayPositions {
            cursor: self.overlay_dynamic.cursor.then_some(self.cursor),
            indicator: self
                .overlay_dynamic
                .indicator
                .map(|geometry| geometry.position(self.cursor, &self.screens)),
        };
        if positions.cursor.is_none() && positions.indicator.is_none() {
            return Ok(());
        }
        if self.overlay_positions == Some(positions) {
            return Ok(());
        }
        if self.overlay_position_fast_path_disabled {
            return self.refresh_overlay(backend);
        }
        if self.command_batch_depth > 0 {
            if self.pending_overlay.is_none() {
                self.pending_overlay = Some(PendingOverlay::Positions);
            }
            return Ok(());
        }

        crate::app::perf_probe::mark("native_submitted");
        match backend.update_overlay_positions(positions.cursor, positions.indicator) {
            Ok(true) => {
                self.overlay_positions = Some(positions);
                Ok(())
            }
            Ok(false) => {
                self.overlay_position_fast_path_disabled = true;
                self.refresh_overlay_now(backend)
            }
            Err(error) => {
                self.overlay_position_fast_path_disabled = true;
                crate::app::logging::report_error(
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
        match self.pending_overlay.take() {
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

    fn build_indicator(&self, display_mode: &ModeId) -> Option<(Indicator, IndicatorGeometry)> {
        let mode = self.modes.get(display_mode)?;
        let (text, ui) = self
            .config
            .mode_indicator
            .for_mode_with(display_mode.as_str(), || mode.display_name())?;

        let background = mode
            .indicator_color(&self.palette)
            .unwrap_or_else(|| self.palette.surface_label());
        let style = ui.label.resolve(
            &self.palette,
            background,
            self.palette.readable_on(background),
            self.palette.accent,
        );
        let held_text = held_targets_text(&self.latched);
        let text_width = |character_count: usize| {
            (character_count as f64 * style.font_size * 0.75 + style.padding_x * 2.0)
                .max(style.font_size * 2.0)
                .ceil()
        };
        let width = held_text
            .as_ref()
            .map(|held| text_width(held.character_count))
            .unwrap_or_default()
            .max(text_width(text.chars().count()));
        let line_height = (style.font_size * 1.4 + style.padding_y * 2.0).ceil();
        let height = line_height + held_text.as_ref().map_or(0.0, |_| line_height + 4.0);
        // `position.x` is the shared right edge of both badges. Keeping the
        // anchor independent of the longest line prevents a wide held-input
        // badge from pushing the shorter mode badge away from the cursor.
        let geometry = IndicatorGeometry {
            width,
            height,
            x_offset: ui.indicator_x_offset as f64,
            y_offset: ui.indicator_y_offset as f64,
        };
        Some((
            Indicator {
                text,
                held_text: held_text.map(|held| held.value),
                position: geometry.position(self.cursor, &self.screens),
                style,
            },
            geometry,
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

        let held = held_targets_text(&targets).expect("held targets");
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

        let held = held_targets_text(&targets).expect("held target");
        assert_eq!(held.value, "● é");
        assert_eq!(held.character_count, 3);
    }
}
