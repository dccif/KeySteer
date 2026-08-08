//! Unified execution of platform-independent commands.

use super::*;

impl Engine {
    /// Keep an absolute pointer target on a real display, including layouts
    /// with negative origins or gaps between monitors.
    pub(super) fn constrain_absolute_pointer(&self, requested: Point) -> Option<Point> {
        if !requested.x.is_finite() || !requested.y.is_finite() {
            return None;
        }
        if self
            .screens
            .iter()
            .any(|screen| screen.bounds.contains(&requested))
        {
            return Some(requested);
        }
        self.screens
            .iter()
            .map(|screen| clamp_to_screen(requested, screen))
            .min_by(|left, right| {
                requested
                    .distance_to(left)
                    .total_cmp(&requested.distance_to(right))
            })
    }

    /// Relative movement may cross directly into another display. If its target
    /// falls outside every display (an outer edge or a layout gap), clamp it to
    /// the current display without changing the held gesture or active mode.
    fn constrain_relative_pointer(&self, from: Point, requested: Point) -> Option<Point> {
        if !requested.x.is_finite() || !requested.y.is_finite() {
            return None;
        }
        if self
            .screens
            .iter()
            .any(|screen| screen.bounds.contains(&requested))
        {
            return Some(requested);
        }
        let current = self
            .screens
            .iter()
            .find(|screen| screen.bounds.contains(&from))
            .or_else(|| {
                self.screens.iter().min_by(|left, right| {
                    from.distance_to(&clamp_to_screen(from, left))
                        .total_cmp(&from.distance_to(&clamp_to_screen(from, right)))
                })
            })?;
        Some(clamp_to_screen(requested, current))
    }

    pub(super) fn execute(
        &mut self,
        commands: Vec<Command>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let owner = self.active.clone();
        self.execute_for(&owner, commands, backend)
    }

    pub(super) fn execute_for(
        &mut self,
        owner: &ModeId,
        commands: Vec<Command>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        self.command_batch_depth += 1;
        let result = self.execute_commands(owner, commands, backend);
        self.command_batch_depth -= 1;

        if result.is_err() {
            self.pending_overlay = None;
            return result;
        }
        if self.command_batch_depth == 0 {
            self.flush_pending_overlay(backend)?;
        }
        Ok(())
    }

    fn execute_commands(
        &mut self,
        owner: &ModeId,
        commands: Vec<Command>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        for command in commands {
            let trace_command = if matches!(&command, Command::MovePointer { .. }) {
                self.config.debug.motion
            } else {
                self.config.debug.actions
            };
            self.trace_lazy(trace_command, "command", || {
                format!("owner={owner} active={} command={command:?}", self.active)
            });
            match command {
                Command::DispatchActions(actions) => {
                    let input = crate::api::input::InputEvent {
                        key: Key::new("plugin_action")?,
                        state: KeyState::Down,
                        repeat: false,
                        injected: true,
                        timestamp_millis: 0,
                    };
                    let resolved = ResolvedBinding {
                        binding: Arc::new(Binding::Sequence(actions)),
                        owner: owner.clone(),
                    };
                    self.apply_binding(&resolved, &input, backend)?;
                }
                Command::MovePointer { dx, dy } => {
                    let requested = Point::new(self.cursor.x + dx, self.cursor.y + dy);
                    let Some(to) = self.constrain_relative_pointer(self.cursor, requested) else {
                        crate::report_warning!(
                            "pointer",
                            "ignoring non-finite or unavailable relative pointer target"
                        );
                        continue;
                    };
                    let actual_dx = to.x - self.cursor.x;
                    let actual_dy = to.y - self.cursor.y;
                    if actual_dx == 0.0 && actual_dy == 0.0 {
                        // Reaching an edge is not a gesture end. Keep the frame
                        // clock, pressed keys, acceleration and mode untouched;
                        // a later inward movement must work immediately.
                        continue;
                    }
                    if let Err(error) = backend.move_pointer(self.cursor, actual_dx, actual_dy) {
                        return Err(Self::recoverable_input_error("pointer movement", error));
                    }
                    self.recoverable_input_succeeded();
                    self.trace_lazy(self.config.debug.motion, "backend", || {
                        format!(
                            "move_pointer requested=({dx:.3},{dy:.3}) actual=({actual_dx:.3},{actual_dy:.3}): ok"
                        )
                    });
                    // Synthetic movement is not guaranteed to re-enter the
                    // input hook. Store the constrained position actually sent.
                    self.cursor = to;
                    self.refresh_overlay(backend)?;
                }
                Command::WarpPointer { x, y } => {
                    let Some(to) = self.constrain_absolute_pointer(Point::new(x, y)) else {
                        crate::report_warning!(
                            "pointer",
                            "ignoring non-finite or unavailable absolute pointer target"
                        );
                        continue;
                    };
                    if let Err(error) = backend.warp_pointer(to) {
                        return Err(Self::recoverable_input_error("pointer warp", error));
                    }
                    self.recoverable_input_succeeded();
                    self.trace_lazy(self.config.debug.motion, "backend", || {
                        format!("warp_pointer x={:.3} y={:.3}: ok", to.x, to.y)
                    });
                    self.cursor = to;
                    self.refresh_overlay(backend)?;
                }
                Command::MouseButton { button, action } => {
                    if let Err(error) = backend.mouse_button(button, action) {
                        return Err(Self::recoverable_input_error("mouse button action", error));
                    }
                    self.recoverable_input_succeeded();
                    if matches!(action, ButtonAction::Click | ButtonAction::DoubleClick) {
                        self.dispatch(ModeEvent::Clicked { button, action }, backend)?;
                    }
                }
                Command::FinishMode { cause } => {
                    self.dispatch(ModeEvent::FinishRequested { cause }, backend)?;
                }
                Command::RestartMode => self.restart_active(backend)?,
                Command::Scroll { dx, dy } => {
                    let (invert_horizontal, invert_vertical) =
                        self.config.effective_scroll_invert();
                    let dx = dx * if invert_horizontal { -1.0 } else { 1.0 };
                    let dy = dy * if invert_vertical { -1.0 } else { 1.0 };
                    if let Err(error) = backend.scroll(dx, dy) {
                        return Err(Self::recoverable_input_error("scroll", error));
                    }
                    self.recoverable_input_succeeded();
                    self.trace_lazy(self.config.debug.backend, "backend", || {
                        format!("scroll dx={dx:.3} dy={dy:.3}: ok")
                    });
                }
                Command::SetFrameClock(active) => {
                    self.frame_clock_owner = active.then(|| owner.clone());
                    if let Err(error) = backend.set_frame_clock(active) {
                        self.frame_clock_owner = None;
                        // A platform without a native display link retains
                        // keyboard-repeat movement as its compatibility path.
                        self.trace_lazy(self.config.debug.backend, "backend", || {
                            format!("set_frame_clock active={active}: {error}")
                        });
                    }
                }

                Command::ShowOverlay(scene) => self.show_overlay(scene, backend)?,
                Command::HideOverlay => self.hide_overlay(backend)?,

                Command::SendKey { key, state } => {
                    if let Err(error) = backend.send_key(&key, state) {
                        return Err(Self::recoverable_input_error("keyboard input", error));
                    }
                    self.recoverable_input_succeeded();
                }
                Command::SendChord { keys } => {
                    let events = keys
                        .iter()
                        .cloned()
                        .map(|key| (key, KeyState::Down))
                        .chain(keys.iter().rev().cloned().map(|key| (key, KeyState::Up)))
                        .collect::<Vec<_>>();
                    if let Err(error) = backend.send_keys(&events) {
                        self.latched.extend(keys.into_iter().map(InputTarget::Key));
                        return Err(Self::recoverable_input_error("keyboard chord", error));
                    }
                    self.recoverable_input_succeeded();
                }

                Command::ScanUi(request) => {
                    let bounds = request
                        .bounds
                        .unwrap_or_else(|| self.context().active_bounds());
                    let roles = if request.roles.is_empty() {
                        self.config.ui_hint.clickable_roles.clone()
                    } else {
                        request.roles
                    };
                    let request = UiScanRequest {
                        bounds: Some(bounds),
                        roles,
                        ..request
                    };
                    // A mode can only consume its latest scan generation. Drop
                    // superseded ownership before inserting the new request so
                    // stale/cancelled scans cannot grow this map indefinitely.
                    self.scan_owners
                        .retain(|_, existing_owner| existing_owner != owner);
                    self.scan_owners.insert(request.id, owner.clone());
                    backend.request_ui_scan(request)?;
                }

                Command::SwitchMode(id) => {
                    let previous = Some(self.active.clone());
                    self.modal_stack.clear();
                    self.activate(id, previous, backend)?;
                }
                Command::PushMode(id) => self.push_mode(id, backend)?,
                Command::PopMode => self.pop_mode(backend)?,
                Command::RetargetScreen { index, preserve } => {
                    let Some(screen) = self.screens.get(index).cloned() else {
                        crate::report_warning!(
                            "screen",
                            "screen {} does not exist ({} connected)",
                            index + 1,
                            self.screens.len()
                        );
                        continue;
                    };
                    self.dispatch(ModeEvent::ScreenRetargeted { screen, preserve }, backend)?;
                }

                Command::SetTimer {
                    id,
                    delay,
                    repeating,
                } => {
                    let now = Instant::now();
                    self.timers.insert(
                        id.clone(),
                        Timer {
                            fires_at: now + delay,
                            last_fired: now,
                            interval: repeating.then_some(delay),
                            owner: owner.clone(),
                        },
                    );
                    self.trace_lazy(self.config.debug.timers, "timer", || {
                        format!("set id={id:?} owner={owner} delay={delay:?} repeating={repeating}")
                    });
                }
                Command::CancelTimer { id } => {
                    self.timers.remove(&id);
                    self.trace_lazy(self.config.debug.timers, "timer", || {
                        format!("cancel id={id:?} owner={owner}")
                    });
                }

                Command::SetConfigValue { path, value } => {
                    let update = self
                        .config_store
                        .as_mut()
                        .ok_or_else(|| "no writable configuration source is attached".to_string())
                        .and_then(|store| store.set(&path, &value).map_err(|e| e.to_string()));
                    match update {
                        Ok(config) => {
                            self.apply_config(config)?;
                            self.notify_config_reloaded(backend)?;
                        }
                        Err(error) => {
                            return Err(format!(
                                "set_config {path} rejected; keeping the last valid configuration: {error}"
                            ));
                        }
                    }
                }
                Command::ReloadConfig => self.reload_config(backend)?,

                Command::Exec { program, args } => {
                    std::process::Command::new(&program)
                        .args(&args)
                        .spawn()
                        .map_err(|error| format!("cannot run {program}: {error}"))?;
                }

                Command::Quit => self.should_quit = true,
            }
        }
        Ok(())
    }
}

fn clamp_to_screen(point: Point, screen: &Screen) -> Point {
    let bounds = screen.bounds;
    let unit = if screen.scale.is_finite() && screen.scale > 0.0 {
        1.0 / screen.scale
    } else {
        1.0
    };
    let max_x = (bounds.right() - unit).max(bounds.left());
    let max_y = (bounds.bottom() - unit).max(bounds.top());
    Point::new(
        point.x.clamp(bounds.left(), max_x),
        point.y.clamp(bounds.top(), max_y),
    )
}
