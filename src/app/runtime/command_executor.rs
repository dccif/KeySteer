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
        commands: impl IntoIterator<Item = Command>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let owner = self.registry.active.clone();
        self.execute_for(&owner, commands, backend)
    }

    pub(super) fn execute_for(
        &mut self,
        owner: &ModeId,
        commands: impl IntoIterator<Item = Command>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        self.overlay.command_batch_depth += 1;
        let result = self.execute_commands(owner, commands, backend);
        self.overlay.command_batch_depth -= 1;

        if result.is_err() {
            self.overlay.pending = None;
            return result;
        }
        if self.overlay.command_batch_depth == 0 {
            self.flush_pending_overlay(backend)?;
        }
        Ok(())
    }

    fn execute_commands(
        &mut self,
        owner: &ModeId,
        commands: impl IntoIterator<Item = Command>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        for command in commands {
            let trace_command = if matches!(&command, Command::MovePointer { .. }) {
                self.settings.debug.motion
            } else {
                self.settings.debug.actions
            };
            self.trace_lazy(trace_command, "command", || {
                format!(
                    "owner={owner} active={} command={command:?}",
                    self.registry.active
                )
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
                    self.apply_binding(resolved, &input, backend)?;
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
                        return Err(self.recoverable_input_error("pointer movement", error));
                    }
                    self.recoverable_input_succeeded();
                    self.trace_lazy(self.settings.debug.motion, "backend", || {
                        format!(
                            "move_pointer requested=({dx:.3},{dy:.3}) actual=({actual_dx:.3},{actual_dy:.3}): ok"
                        )
                    });
                    // Synthetic movement is not guaranteed to re-enter the
                    // input hook. Store the constrained position actually sent.
                    self.cursor = to;
                    self.note_drag_pointer_moved();
                    self.refresh_overlay_positions(backend)?;
                }
                Command::WarpPointer { x, y } => {
                    let Some(to) = self.constrain_absolute_pointer(Point::new(x, y)) else {
                        crate::report_warning!(
                            "pointer",
                            "ignoring non-finite or unavailable absolute pointer target"
                        );
                        continue;
                    };
                    let changed = self.cursor != to;
                    if let Err(error) = backend.warp_pointer(to) {
                        return Err(self.recoverable_input_error("pointer warp", error));
                    }
                    self.recoverable_input_succeeded();
                    self.trace_lazy(self.settings.debug.motion, "backend", || {
                        format!("warp_pointer x={:.3} y={:.3}: ok", to.x, to.y)
                    });
                    self.cursor = to;
                    if changed {
                        self.note_drag_pointer_moved();
                    }
                    self.refresh_overlay_positions(backend)?;
                }
                Command::MouseButton { button, action } => {
                    self.inject_mouse_button(button, action, backend)?;
                    if matches!(action, ButtonAction::Click | ButtonAction::DoubleClick) {
                        self.dispatch(ModeEvent::Clicked { button, action }, backend)?;
                    }
                }
                Command::FinishMode { cause } => {
                    self.cancel_scans_for_owner(owner, backend)?;
                    self.dispatch(ModeEvent::FinishRequested { cause }, backend)?;
                }
                Command::RestartMode => self.restart_active(backend)?,
                Command::Scroll { dx, dy } => {
                    let (invert_horizontal, invert_vertical) = self.settings.invert_scroll;
                    let dx = dx * if invert_horizontal { -1.0 } else { 1.0 };
                    let dy = dy * if invert_vertical { -1.0 } else { 1.0 };
                    if let Err(error) = backend.scroll(dx, dy) {
                        return Err(self.recoverable_input_error("scroll", error));
                    }
                    self.recoverable_input_succeeded();
                    self.trace_lazy(self.settings.debug.backend, "backend", || {
                        format!("scroll dx={dx:.3} dy={dy:.3}: ok")
                    });
                }
                Command::SetFrameClock(active) => {
                    self.scheduler.frame_clock_owner = active.then(|| owner.clone());
                    if let Err(error) = backend.set_frame_clock(active) {
                        self.scheduler.frame_clock_owner = None;
                        // A platform without a native display link retains
                        // keyboard-repeat movement as its compatibility path.
                        self.trace_lazy(self.settings.debug.backend, "backend", || {
                            format!("set_frame_clock active={active}: {error}")
                        });
                    }
                }

                Command::ShowOverlay(scene) => self.show_overlay(scene, backend)?,
                Command::HideOverlay => self.hide_overlay(backend)?,

                Command::SendKey { key, state } => {
                    if let Err(error) = backend.send_key(&key, state) {
                        return Err(self.recoverable_input_error("keyboard input", error));
                    }
                    crate::support::perf_probe::mark("injection_executed");
                    self.recoverable_input_succeeded();
                }
                Command::SendChord { keys } => {
                    if let Err(error) = backend.send_chord(&keys) {
                        self.input
                            .latched
                            .extend(keys.into_iter().map(InputTarget::Key));
                        return Err(self.recoverable_input_error("keyboard chord", error));
                    }
                    crate::support::perf_probe::mark("injection_executed");
                    self.recoverable_input_succeeded();
                }

                Command::ScanUi(request) => {
                    let request = *request;
                    let bounds = request
                        .bounds
                        .unwrap_or_else(|| self.context().active_bounds());
                    let roles = if request.roles.is_empty() {
                        self.settings.default_scan_roles.clone()
                    } else {
                        request.roles
                    };
                    let request = UiScanRequest {
                        bounds: Some(bounds),
                        roles,
                        ..request
                    };
                    let request_id = request.id;
                    crate::support::perf_probe::mark_value(
                        "scan_requested",
                        isize::try_from(request_id).unwrap_or(isize::MAX),
                    );
                    // A mode can only consume its latest scan generation.
                    // Cancel the superseded native job before publishing the
                    // new owner so providers cannot retain stale work.
                    self.cancel_scans_for_owner(owner, backend)?;
                    self.scan_owners.insert(request.id, owner.clone());
                    if let Err(error) = backend.request_ui_scan(request) {
                        self.scan_owners.remove(&request_id);
                        return Err(error);
                    }
                }

                Command::SwitchMode(id) => {
                    let previous = Some(self.registry.active.clone());
                    self.registry.modal_stack.clear();
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
                    self.scheduler.timers.insert(
                        id.clone(),
                        Timer {
                            fires_at: now + delay,
                            last_fired: now,
                            interval: repeating.then_some(delay),
                            owner: owner.clone(),
                        },
                    );
                    self.trace_lazy(self.settings.debug.timers, "timer", || {
                        format!("set id={id:?} owner={owner} delay={delay:?} repeating={repeating}")
                    });
                }
                Command::CancelTimer { id } => {
                    self.scheduler.timers.remove(&id);
                    self.trace_lazy(self.settings.debug.timers, "timer", || {
                        format!("cancel id={id:?} owner={owner}")
                    });
                }

                Command::SetConfigValue { path, value } => {
                    let update = self
                        .configuration
                        .as_ref()
                        .ok_or_else(|| "no writable configuration source is attached".to_string())
                        .and_then(|repository| repository.set_candidate(&path, &value));
                    match update {
                        Ok(candidate) => {
                            let super::ConfigurationCandidate {
                                plan,
                                repository,
                                source_path: _,
                            } = candidate;
                            self.apply_runtime_plan(plan, backend)?;
                            self.configuration = Some(repository);
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

    pub(super) fn inject_mouse_button(
        &mut self,
        button: MouseButton,
        action: ButtonAction,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if let Err(error) = backend.mouse_button(button, action) {
            return Err(
                self.recoverable_input_error(&format!("mouse button {button:?} {action:?}"), error)
            );
        }
        crate::support::perf_probe::mark("injection_executed");
        self.recoverable_input_succeeded();
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

impl Engine {
    pub(super) fn flatten_sequence(&self, actions: &[Binding]) -> Vec<Binding> {
        fn append(binding: &Binding, flattened: &mut Vec<Binding>) {
            match binding {
                Binding::Sequence(nested) => {
                    for action in nested {
                        append(action, flattened);
                    }
                }
                action => flattened.push(action.clone()),
            }
        }

        let mut flattened = Vec::new();
        for action in actions {
            append(action, &mut flattened);
        }

        // Two identical key sends are a double tap. Give the focused app the
        // same default interval as an explicit `wait`/`wait 0`, while leaving
        // an explicitly configured wait untouched.
        let mut expanded = Vec::with_capacity(flattened.len());
        for action in flattened {
            if matches!(
                (expanded.last(), &action),
                (Some(Binding::Send(previous)), Binding::Send(current)) if previous == current
            ) {
                expanded.push(Binding::Wait {
                    min_ms: DEFAULT_WAIT_MS,
                    max_ms: DEFAULT_WAIT_MS,
                });
            }
            expanded.push(action);
        }
        expanded
    }

    pub(super) fn continue_sequence(
        &mut self,
        mut actions: VecDeque<Binding>,
        owner: ModeId,
        mut input: crate::api::input::InputEvent,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        input.repeat = false;
        while let Some(action) = actions.pop_front() {
            if let Binding::Wait { min_ms, max_ms } = action {
                if actions.is_empty() {
                    return Ok(());
                }
                const MAX_PENDING_SEQUENCES: usize = 256;
                if self.scheduler.sequences.len() >= MAX_PENDING_SEQUENCES {
                    return Err("too many action sequences are waiting".into());
                }
                let delay = random_wait_ms(min_ms, max_ms);
                let pending = PendingSequence {
                    fires_at: Instant::now() + Duration::from_millis(delay),
                    actions,
                    owner,
                    input,
                };
                let index = self
                    .scheduler
                    .sequences
                    .partition_point(|current| current.fires_at > pending.fires_at);
                self.scheduler.sequences.insert(index, pending);
                return Ok(());
            }
            let nested = ResolvedBinding {
                binding: Arc::new(action),
                owner: owner.clone(),
            };
            self.apply_binding(nested, &input, backend)?;
        }
        Ok(())
    }

    /// Act on a resolved binding.
    ///
    /// Returns whether the key was consumed. Host-level verbs are executed
    /// here; everything else is forwarded to the mode as a
    /// [`ModeEvent::Binding`], which is what a plugin sees too.
    pub(super) fn apply_binding(
        &mut self,
        resolved: ResolvedBinding,
        input: &crate::api::input::InputEvent,
        backend: &mut dyn Backend,
    ) -> Result<bool, String> {
        let binding = resolved.binding.as_ref();
        let is_press = input.state == KeyState::Down;
        self.trace_lazy(
            self.settings.debug.actions && (!input.repeat || self.settings.debug.motion),
            "action",
            || {
                format!(
                    "phase={:?} owner={} active={} action={binding:?}",
                    input.state, resolved.owner, self.registry.active
                )
            },
        );

        // Held bindings need both edges; the rest act on the press only.
        if !is_press && !binding.is_held() {
            // Still consume the release so the app never sees half a gesture.
            return Ok(true);
        }
        // Auto-repeat must not re-trigger a discrete action.
        if input.repeat && !binding.is_held() {
            return Ok(true);
        }

        // Stateful gestures and mode-specific discrete actions are owned by
        // the receiving mode. Transfer the resolved binding's Arc directly;
        // a held key already stored the one clone needed for its release edge.
        if matches!(
            binding,
            Binding::Move(_)
                | Binding::Scroll(..)
                | Binding::Speed(_)
                | Binding::ToggleCursorFollowSelection
                | Binding::RescanUi
        ) {
            return self
                .dispatch_to(
                    &resolved.owner,
                    ModeEvent::Binding {
                        binding: resolved.binding,
                        state: input.state,
                        key: input.key.clone(),
                    },
                    backend,
                )
                .map(|_| true);
        }

        match binding {
            Binding::Sequence(actions) => {
                let actions = self.flatten_sequence(actions);
                let has_held = actions.iter().any(Binding::is_held);
                if has_held
                    && actions
                        .iter()
                        .any(|action| matches!(action, Binding::Wait { .. }))
                {
                    return Err(
                        "`wait` cannot be combined with held movement, scroll, or speed actions"
                            .into(),
                    );
                }
                if has_held
                    && actions.iter().any(|action| {
                        matches!(
                            action,
                            Binding::Mode(_)
                                | Binding::Invoke { .. }
                                | Binding::FinishMode
                                | Binding::RestartMode
                                | Binding::Escape
                                | Binding::Quit
                        )
                    })
                {
                    return Err("held movement, scroll, or speed actions cannot be combined with mode-changing actions".into());
                }
                if is_press {
                    let actions = if input.repeat {
                        actions.into_iter().filter(Binding::is_held).collect()
                    } else {
                        actions
                    };
                    self.continue_sequence(
                        actions.into(),
                        resolved.owner.clone(),
                        input.clone(),
                        backend,
                    )?;
                } else {
                    // Stateful movement/scroll bindings still receive their
                    // release immediately; waits only order discrete actions.
                    for action in actions.into_iter().filter(Binding::is_held) {
                        let nested = ResolvedBinding {
                            binding: Arc::new(action),
                            owner: resolved.owner.clone(),
                        };
                        self.apply_binding(nested, input, backend)?;
                    }
                }
                Ok(true)
            }

            Binding::Mode(id) => {
                if !is_press {
                    return Ok(true);
                }
                if !self.registry.contains_key(id) {
                    crate::report_warning!(
                        "binding",
                        "binding targets unknown mode {:?}; is the plugin registered?",
                        id.as_str()
                    );
                    return Ok(true);
                }
                // Pressing a mode's own key while it is active leaves it.
                let next = if *id == self.registry.active {
                    ModeId::idle()
                } else {
                    id.clone()
                };
                // A coalesced pointer event can still be pending when the mode
                // hotkey arrives. Query the OS once so normal and every
                // targeting mode activate against the display actually under
                // the mouse rather than the last reported display.
                if let Ok(pointer) = backend.pointer()
                    && let Some(pointer) = self.constrain_absolute_pointer(pointer)
                {
                    self.cursor = pointer;
                }
                self.activate(next, Some(self.registry.active.clone()), backend)?;
                Ok(true)
            }

            Binding::Invoke { verb, args } => {
                if !is_press {
                    return Ok(true);
                }
                let Some(owner) = self.registry.plugin_verbs.get(verb).cloned() else {
                    crate::report_warning!("plugin", "no plugin exports verb {verb:?}");
                    return Ok(true);
                };
                self.dispatch_to(
                    &owner,
                    ModeEvent::Invoked {
                        verb: verb.clone(),
                        args: args.clone(),
                    },
                    backend,
                )?;
                Ok(true)
            }

            Binding::Escape => {
                if is_press {
                    self.scheduler.sequences.clear();
                    // `press`/`toggle` are explicit engine-wide latches, not
                    // mode-owned gestures. Escape changes mode but must not
                    // synthesize an Up edge for them.
                    self.activate(ModeId::idle(), Some(self.registry.active.clone()), backend)?;
                }
                Ok(true)
            }

            Binding::Quit => {
                self.should_quit = true;
                Ok(true)
            }

            Binding::Send(chord) => {
                self.send_chord(chord, backend)?;
                Ok(true)
            }

            Binding::Warp { x, y } => {
                self.execute(
                    [Command::WarpPointer {
                        x: *x as f64,
                        y: *y as f64,
                    }],
                    backend,
                )?;
                Ok(true)
            }

            Binding::Exec { program, args } => {
                std::process::Command::new(program)
                    .args(args)
                    .spawn()
                    .map_err(|error| format!("cannot run {program}: {error}"))?;
                Ok(true)
            }

            Binding::ReloadConfig => {
                self.reload_config(backend)?;
                Ok(true)
            }
            Binding::FinishMode => {
                self.execute(
                    [Command::FinishMode {
                        cause: FinishCause::Explicit,
                    }],
                    backend,
                )?;
                Ok(true)
            }
            Binding::RestartMode => {
                self.execute([Command::RestartMode], backend)?;
                Ok(true)
            }
            Binding::SetConfig { path, value } => {
                self.execute(
                    [Command::SetConfigValue {
                        path: path.clone(),
                        value: value.clone(),
                    }],
                    backend,
                )?;
                Ok(true)
            }

            Binding::Click(button) => {
                self.execute([Command::click(map_button(*button))], backend)?;
                self.activate_click_indicator(input, *button, backend)?;
                Ok(true)
            }
            Binding::DoubleClick(button) => {
                self.execute(
                    [Command::MouseButton {
                        button: map_button(*button),
                        action: ButtonAction::DoubleClick,
                    }],
                    backend,
                )?;
                self.activate_click_indicator(input, *button, backend)?;
                Ok(true)
            }
            Binding::Press(targets) => {
                self.transfer_pending_long_press_targets(targets);
                self.press_targets(targets, backend)?;
                self.refresh_overlay(backend)?;
                Ok(true)
            }
            Binding::Release(targets) => {
                self.transfer_pending_long_press_targets(targets);
                self.release_targets(targets, true, backend)?;
                self.refresh_overlay(backend)?;
                Ok(true)
            }
            Binding::Toggle(targets) => {
                if targets.is_empty() {
                    let inferred = self.pressed_toggle_targets(&input.key);
                    let used = !inferred.is_empty();
                    if used {
                        self.transfer_pending_long_press_targets(&inferred);
                        self.press_targets(&inferred, backend)?;
                        self.refresh_overlay(backend)?;
                    }
                    if self.input.pressed.contains(&input.key) {
                        self.input
                            .active_default_toggles
                            .insert(input.key.clone(), used);
                    }
                } else {
                    let toggle = self.unprimed_toggle_targets(targets);
                    self.toggle_targets(&toggle, backend)?;
                    self.refresh_overlay(backend)?;
                }
                Ok(true)
            }
            Binding::Wait { .. } => Ok(true),

            Binding::Move(_)
            | Binding::Scroll(..)
            | Binding::Speed(_)
            | Binding::ToggleCursorFollowSelection
            | Binding::RescanUi => {
                Err("stateful binding reached the stateless runtime dispatch boundary".into())
            }

            // Filtered out when the table was built.
            Binding::Disabled => Ok(false),
        }
    }
}
