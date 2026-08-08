//! Overlay scene decoration, deduplication, and presentation.

use super::*;

impl Engine {
    pub(super) fn show_overlay(
        &mut self,
        scene: OverlayScene,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if self.command_batch_depth > 0 {
            self.pending_overlay = Some(PendingOverlay::Show(Box::new(scene)));
            return Ok(());
        }
        // Static primitives keep their order for the lifetime of the mode
        // scene. Sort once before sharing them instead of sorting (and
        // detaching copy-on-write storage) on every cursor refresh.
        self.show_shared_overlay_now(Arc::new(scene.sorted()), backend)
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
        let cursor_only = scene.clip.is_none()
            && scene.backdrop.is_none()
            && scene.labels.is_empty()
            && scene.shapes.is_empty()
            && scene.indicator.is_none();
        // The engine owns mode decoration. A temporary mode changes only this
        // display identity; the active mode keeps its underlying scene/state.
        let display_mode = self.display_mode();
        if let Some(cursor) = self
            .config
            .mode_indicator
            .cursor_for_mode(display_mode.as_str())
        {
            let pressed_button = [
                crate::api::binding::Button::Left,
                crate::api::binding::Button::Middle,
                crate::api::binding::Button::Right,
            ]
            .into_iter()
            .find(|button| self.latched.contains(&InputTarget::Mouse(*button)))
            .or_else(|| self.active_click_indicators.latest_button());
            let pressed_color = match pressed_button {
                Some(crate::api::binding::Button::Left) => cursor.left_pressed_color.as_ref(),
                Some(crate::api::binding::Button::Middle) => cursor.middle_pressed_color.as_ref(),
                Some(crate::api::binding::Button::Right) => cursor.right_pressed_color.as_ref(),
                None => None,
            }
            .and_then(|color| color.resolve(self.palette.appearance));
            let fill = pressed_color.map_or_else(
                || {
                    crate::config::style::resolve(
                        cursor.fill_color.as_ref(),
                        self.palette.appearance,
                        self.palette.accent.with_alpha(34),
                    )
                },
                |color| color.with_opacity(0.2),
            );
            let stroke = pressed_color.unwrap_or_else(|| {
                crate::config::style::resolve(
                    cursor.stroke_color.as_ref(),
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
        if scene.indicator.is_none() {
            scene.indicator = self.build_indicator(&display_mode);
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
        if self.overlay_visible
            && self
                .last_scene
                .as_deref()
                .is_some_and(|previous| previous == &scene)
        {
            return Ok(());
        }
        let scene = Arc::new(scene);
        backend.present(Arc::clone(&scene))?;
        self.trace(trace_overlay, "overlay", "backend present: ok");
        self.last_scene = Some(scene);
        self.overlay_visible = true;
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
        Ok(())
    }

    pub(super) fn refresh_overlay(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        if self.command_batch_depth > 0 {
            if self.pending_overlay.is_none() {
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

    pub(super) fn flush_pending_overlay(
        &mut self,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        match self.pending_overlay.take() {
            Some(PendingOverlay::Refresh) => self.refresh_overlay_now(backend),
            Some(PendingOverlay::Show(scene)) => {
                self.show_shared_overlay_now(Arc::new((*scene).sorted()), backend)
            }
            Some(PendingOverlay::Hide) => self.hide_overlay_now(backend),
            None => Ok(()),
        }
    }

    fn build_indicator(&self, display_mode: &ModeId) -> Option<Indicator> {
        let mode = self.modes.get(display_mode)?;
        let display = mode.display_name();
        let (text, ui) = self
            .config
            .mode_indicator
            .for_mode(display_mode.as_str(), &display)?;

        let background = mode
            .indicator_color(&self.palette)
            .unwrap_or_else(|| self.palette.surface_label());
        let style = ui.label.resolve(
            &self.palette,
            background,
            self.palette.readable_on(background),
            self.palette.accent,
        );
        let held_text = (!self.latched.is_empty()).then(|| {
            let targets = self
                .latched
                .iter()
                .map(|target| target.canonical().replace('_', " ").to_ascii_uppercase())
                .collect::<Vec<_>>()
                .join(" · ");
            format!("● {targets}")
        });
        let text_width = |value: &str| {
            (value.chars().count() as f64 * style.font_size * 0.75 + style.padding_x * 2.0)
                .max(style.font_size * 2.0)
                .ceil()
        };
        let width = held_text
            .as_deref()
            .map(text_width)
            .unwrap_or_default()
            .max(text_width(&text));
        let line_height = (style.font_size * 1.4 + style.padding_y * 2.0).ceil();
        let height = line_height + held_text.as_ref().map_or(0.0, |_| line_height + 4.0);
        // `position.x` is the shared right edge of both badges. Keeping the
        // anchor independent of the longest line prevents a wide held-input
        // badge from pushing the shorter mode badge away from the cursor.
        let mut position = Point::new(
            self.cursor.x + ui.indicator_x_offset as f64,
            self.cursor.y + ui.indicator_y_offset as f64,
        );
        if let Some(screen) = Screen::containing(&self.screens, &self.cursor) {
            position.x = position.x.clamp(
                (screen.bounds.x + width).min(screen.bounds.right()),
                screen.bounds.right(),
            );
            position.y = position.y.clamp(
                screen.bounds.y,
                (screen.bounds.bottom() - height).max(screen.bounds.y),
            );
        }
        Some(Indicator {
            text,
            held_text,
            position,
            style,
        })
    }
}
