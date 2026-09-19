//! Host-owned hold/chord arbitration; no native APIs or periodic timers.
use super::*;
use crate::api::Rect;
use crate::api::input::InputEvent;
use crate::api::style::QuickSwitchPosition;
use std::collections::BTreeSet;

#[derive(Default)]
pub(super) struct QuickSwitcher {
    pub(super) pending: Option<Pending>,
    captured: BTreeSet<Key>,
}
pub(super) struct Pending {
    owner: ModeId,
    input: InputEvent,
    pub(super) deadline: Instant,
    pub(super) visible: bool,
    used: bool,
    candidates: Vec<ModeId>,
    window: Option<Rect>,
    rows: Vec<crate::api::overlay::OverlayText>,
    text_metrics: crate::presentation::quick_switch::CaptionMetrics,
}

impl Engine {
    pub(super) fn cancel_quick_switch(&mut self, capture_lost: bool) {
        self.quick_switch.pending = None;
        if capture_lost {
            self.quick_switch.captured.clear();
        }
    }
    pub(super) fn quick_switch_key(
        &mut self,
        input: &InputEvent,
        backend: &mut dyn Backend,
    ) -> Result<bool, String> {
        if input.injected {
            return Ok(false);
        }
        if !self.enabled || self.is_excluded_app() || self.window_presets.pending.is_some() {
            self.quick_switch.pending = None;
        }
        // The trigger belongs to normal input routing on both edges, even if
        // its immediate action switches modes. Only panel choices are captured.
        if input.state == KeyState::Up {
            if self
                .quick_switch
                .pending
                .as_ref()
                .is_some_and(|p| p.input.key == input.key)
            {
                self.quick_switch.pending = None;
                self.refresh_overlay(backend)?;
            }
            if self.quick_switch.captured.remove(&input.key) {
                self.input.pressed.remove(&input.key);
                self.input
                    .temporary_entry_keys
                    .retain(|key| key != &input.key);
                let outcome = self.complete_key_disposition(input, KeyOutcome::Consumed);
                self.dispose_input(input, outcome, false, backend)?;
                return Ok(true);
            }
            return Ok(false);
        }
        if input.state != KeyState::Down {
            return Ok(false);
        }
        if self.quick_switch.captured.contains(&input.key) {
            self.dispose_input(input, KeyOutcome::Consumed, false, backend)?;
            return Ok(true);
        }
        let starts = self.settings.quick_switch.enabled
            && self.enabled
            && !self.is_excluded_app()
            && self.registry.active != ModeId::idle()
            && self.window_presets.pending.is_none()
            && self.quick_switch.pending.is_none()
            && !input.repeat
            && input.key == self.settings.quick_switch.key
            && self.input.pressed.is_empty();
        if starts {
            self.quick_switch.pending = Some(Pending {
                owner: self.registry.active.clone(),
                input: input.clone(),
                deadline: Instant::now()
                    + Duration::from_millis(self.settings.quick_switch.hold_ms),
                visible: false,
                used: false,
                candidates: Vec::new(),
                window: None,
                rows: Vec::new(),
                text_metrics: Default::default(),
            });
            return Ok(false);
        }
        if self
            .quick_switch
            .pending
            .as_ref()
            .is_some_and(|p| p.input.key == input.key)
        {
            return Ok(false);
        }
        if self.quick_switch.pending.is_some()
            && !input.repeat
            && let Some(index) = input
                .key
                .as_char()
                .and_then(|ch| ch.to_digit(10))
                .filter(|n| *n > 0)
        {
            self.consume_switch_key(input, backend)?;
            self.open_quick_switch(backend, false)?;
            let target = self
                .quick_switch
                .pending
                .as_ref()
                .and_then(|p| p.candidates.get(index as usize - 1))
                .cloned();
            if let Some(target) = target {
                self.quick_switch.pending = None;
                if target != self.registry.active {
                    if let Ok(pointer) = backend.pointer()
                        && let Some(pointer) = self.constrain_absolute_pointer(pointer)
                    {
                        self.cursor = pointer;
                    }
                    self.activate(target, Some(self.registry.active.clone()), backend)?;
                }
                self.refresh_overlay(backend)?;
            } else if self
                .quick_switch
                .pending
                .as_ref()
                .is_some_and(|pending| pending.candidates.is_empty())
            {
                if let Some(pending) = &mut self.quick_switch.pending {
                    pending.used = true;
                }
            } else {
                self.open_quick_switch(backend, true)?;
            }
            return Ok(true);
        }
        if let Some(pending) = &mut self.quick_switch.pending {
            if pending.visible {
                let cancel = input.key.as_str() == "esc";
                self.consume_switch_key(input, backend)?;
                if cancel {
                    self.quick_switch.pending = None;
                    self.refresh_overlay(backend)?;
                }
                return Ok(true);
            }
            pending.used = true;
        }
        Ok(false)
    }

    fn consume_switch_key(
        &mut self,
        input: &InputEvent,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        self.quick_switch.captured.insert(input.key.clone());
        self.input.pressed.insert_ref(&input.key);
        let outcome = self.complete_key_disposition(input, KeyOutcome::Consumed);
        self.dispose_input(input, outcome, false, backend)
    }

    fn open_quick_switch(&mut self, backend: &mut dyn Backend, show: bool) -> Result<(), String> {
        let Some(pending) = &mut self.quick_switch.pending else {
            return Ok(());
        };
        if pending.visible {
            return Ok(());
        }
        let usage = self.window_presets.store.mode_usage();
        pending.candidates = self
            .registry
            .keys()
            .filter(|id| *id != &pending.owner)
            // Contextual plugin helpers are invoked by their own commands, not this panel.
            .filter(|id| !id.as_str().starts_with("plugin:"))
            .filter(|id| {
                !self
                    .settings
                    .quick_switch
                    .blacklist
                    .iter()
                    .any(|blocked| blocked == id.as_str())
            })
            .cloned()
            .collect();
        pending.candidates.sort_by(|a, b| {
            usage
                .get(b.as_str())
                .unwrap_or(&0)
                .cmp(usage.get(a.as_str()).unwrap_or(&0))
                .then_with(|| a.cmp(b))
        });
        pending.candidates.truncate(9);
        let rows = pending
            .candidates
            .iter()
            .map(|mode| crate::api::overlay::OverlayText::from(mode.as_str()))
            .collect::<Vec<_>>();
        pending.text_metrics = crate::presentation::quick_switch::CaptionMetrics::for_rows(&rows);
        pending.rows = rows;
        if !show {
            return Ok(());
        }
        pending.visible = true;
        if self.settings.quick_switch.position == QuickSwitchPosition::Window {
            pending.window = backend.focused_window_bounds().unwrap_or_else(|error| {
                crate::report_error!("quick-switch", "{error}");
                None
            });
        }
        self.refresh_overlay(backend)
    }

    pub(super) fn fire_quick_switch(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        let cancel = self.quick_switch.pending.as_ref().is_some_and(|p| p.used)
            || !self.enabled
            || self.is_excluded_app()
            || self.window_presets.pending.is_some();
        if cancel
            && self
                .quick_switch
                .pending
                .take()
                .is_some_and(|pending| pending.visible)
        {
            self.refresh_overlay(backend)?;
        }
        if self
            .quick_switch
            .pending
            .as_ref()
            .is_some_and(|p| !p.visible && Instant::now() >= p.deadline)
        {
            self.open_quick_switch(backend, true)?;
        }
        Ok(())
    }

    pub(super) fn decorate_quick_switch(&self, scene: &mut OverlayScene) {
        let Some(pending) = self.quick_switch.pending.as_ref().filter(|p| p.visible) else {
            return;
        };
        let Some(screen) = Screen::containing(&self.screens, &self.cursor) else {
            return;
        };
        let style = match self.palette.appearance {
            Appearance::Light => &self.settings.quick_switch.light,
            Appearance::Dark => &self.settings.quick_switch.dark,
        };
        crate::presentation::quick_switch::Panel {
            rows: &pending.rows,
            text_metrics: pending.text_metrics,
            styles: style,
            position: self.settings.quick_switch.position,
            screen: screen.bounds,
            scale: screen.scale,
            window: pending.window,
            cursor: self.cursor,
        }
        .append_to(scene);
    }
}
