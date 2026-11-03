//! Command interpretation and overlay batch commit. State remains owned by Engine.
use super::*;

impl Engine {
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
            // A suspended window continues receiving asynchronous results, but
            // must not repaint over the picker or replay an old native pointer.
            if *owner == ModeId::window()
                && self.registry.modal_stack.contains(owner)
                && matches!(
                    command,
                    Command::ShowOverlay(_) | Command::HideOverlay | Command::WarpPointer { .. }
                )
            {
                continue;
            }
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
                Command::SetPointAdjustment {
                    available,
                    adjusting,
                } => {
                    if self.scheduler.point_input.available != available
                        || self.scheduler.point_input.adjusting != adjusting
                    {
                        self.scheduler.point_input = PointInput {
                            available,
                            adjusting,
                            ..PointInput::default()
                        };
                    }
                }
                Command::SegmentText(mut request) => {
                    // Submit the waiting scene before starting native work;
                    // batch coalescing would otherwise defer it until afterward.
                    self.flush_pending_overlay(backend)?;
                    self.scheduler.text_segmentation_serial =
                        self.scheduler.text_segmentation_serial.wrapping_add(1);
                    let id = self.scheduler.text_segmentation_serial;
                    let original = request.id;
                    self.scheduler.text_segmentation = Some((owner.clone(), id, original));
                    request.id = id;
                    if !backend.request_text_segmentation(request) {
                        self.scheduler.text_segmentation = None;
                        self.dispatch_to(
                            owner,
                            ModeEvent::TextSegmented(crate::api::text_select::Segmented {
                                id: original,
                                words: Vec::new(),
                            }),
                            backend,
                        )?;
                    }
                }
                Command::CancelTextSegmentation => {
                    self.scheduler.text_segmentation = None;
                    backend.cancel_text_segmentation();
                }
                Command::SamplePoint(request) => {
                    self.flush_pending_overlay(backend)?;
                    self.scheduler.point_sample_serial =
                        self.scheduler.point_sample_serial.wrapping_add(1);
                    let id = self.scheduler.point_sample_serial;
                    self.scheduler.point_sample = Some((owner.clone(), id, request));
                    if !backend
                        .request_point_sample(crate::api::point_sample::Request { id, ..request })
                    {
                        self.dispatch_to(
                            owner,
                            ModeEvent::PointSampled(crate::api::point_sample::Sample {
                                request,
                                color: None,
                            }),
                            backend,
                        )?;
                    }
                }
                Command::CancelPointSample => {
                    self.scheduler.point_sample = None;
                    backend.cancel_point_sample();
                }
                Command::RequestModeList => {
                    let usage = self.window_presets.store.mode_usage();
                    let mut modes: Vec<_> = self.registry.keys().cloned().collect();
                    modes.sort_by(|a, b| {
                        usage
                            .get(b.as_str())
                            .unwrap_or(&0)
                            .cmp(usage.get(a.as_str()).unwrap_or(&0))
                            .then_with(|| a.cmp(b))
                    });
                    self.overlay.key_help_cache = None;
                    self.dispatch_to(owner, ModeEvent::ModeList(modes), backend)?;
                }
                Command::RequestPanelWindowBounds(request) => {
                    self.flush_pending_overlay(backend)?;
                    if let Some(process) = self.focused_app.as_ref().map(|app| app.process_id) {
                        self.scheduler.panel_geometry_serial =
                            self.scheduler.panel_geometry_serial.wrapping_add(1);
                        let id = self.scheduler.panel_geometry_serial;
                        self.scheduler
                            .panel_geometry
                            .retain(|_, (candidate, _, _)| candidate != owner);
                        self.scheduler
                            .panel_geometry
                            .insert(id, (owner.clone(), request, process));
                        if let Err(error) = backend.request_focused_window_bounds(id, process) {
                            self.scheduler.panel_geometry.remove(&id);
                            crate::report_error!("panel", "geometry submission: {error}");
                        }
                    }
                }
                Command::OpenTextPrompt(mut prompt) => {
                    self.clear_point_input(backend);
                    self.scheduler.text_prompt_serial =
                        self.scheduler.text_prompt_serial.wrapping_add(1);
                    prompt.id = self.scheduler.text_prompt_serial | (1 << 63);
                    backend.set_text_capture(false);
                    if let Some((_, old)) = self.scheduler.text_prompt.take() {
                        backend.cancel_text_prompt(old.id);
                    }
                    if prompt.live_style.is_some() {
                        self.scheduler.text_prompt = Some((owner.clone(), *prompt));
                        backend.set_text_capture(true);
                        continue;
                    }
                    self.scheduler.text_prompt = Some((owner.clone(), (*prompt).clone()));
                    if let Err(error) = backend.request_text_prompt(*prompt) {
                        self.scheduler.text_prompt = None;
                        self.report_action_error(error, backend);
                        self.dispatch_to(owner, ModeEvent::TextSubmitted(None), backend)?;
                    }
                }
                Command::CloseTextPrompt => {
                    self.clear_point_input(backend);
                    backend.set_text_capture(false);
                    if let Some((_, prompt)) = self.scheduler.text_prompt.take() {
                        self.scheduler.text_prompt_returning_focus = prompt.live_style.is_none();
                        backend.cancel_text_prompt(prompt.id);
                    }
                }
                Command::ReleaseTextPrompt => {
                    self.clear_point_input(backend);
                    self.scheduler.text_prompt = None;
                    backend.set_text_capture(false);
                    backend.release_text_prompt();
                }
                Command::CopyText(text) => {
                    if let Err(error) = backend.copy_text(&text) {
                        self.report_action_error(error, backend);
                    } else {
                        self.dispatch_to(owner, ModeEvent::TextCopied, backend)?;
                    }
                }
                Command::ReadClipboard => match backend.read_clipboard() {
                    Ok(text) => self.dispatch_to(owner, ModeEvent::TextPasted(text), backend)?,
                    Err(error) => self.report_action_error(error, backend),
                },
                Command::CopyInputText { text, cut } => {
                    if let Err(error) = backend.copy_text(&text) {
                        self.report_action_error(error, backend);
                    } else if cut {
                        self.dispatch_to(
                            owner,
                            ModeEvent::TextEdit(crate::api::text_edit::EditAction::Delete),
                            backend,
                        )?;
                    }
                }
                Command::WindowPresets(request) => {
                    self.request_window_presets(owner, *request, backend)?
                }
                Command::AudioRequest(request) => {
                    self.scheduler
                        .audio_sessions
                        .insert(request.session, owner.clone());
                    let (session, id) = (request.session, request.id);
                    if let Err(error) = backend.request_audio(*request) {
                        crate::support::logging::report_error_context(
                            "audio",
                            &error,
                            format_args!("operation=submit session={session} request={id}"),
                        );
                        self.dispatch_to(
                            owner,
                            ModeEvent::AudioResult(Box::new(crate::api::audio::AudioResult {
                                session,
                                id,
                                outcome: Err(error),
                            })),
                            backend,
                        )?;
                    }
                }
                Command::CancelAudioSession(session) => {
                    self.scheduler.audio_sessions.remove(&session);
                    backend.cancel_audio_session(session);
                }
                Command::WindowRequest(request) => {
                    self.scheduler
                        .window_sessions
                        .insert(request.session, owner.clone());
                    let (session, id) = (request.session, request.id);
                    let edit = match &request.operation {
                        crate::api::window::WindowOperation::BeginEdit { transaction, .. }
                        | crate::api::window::WindowOperation::ApplyLayout {
                            transaction, ..
                        }
                        | crate::api::window::WindowOperation::EndEdit { transaction, .. } => {
                            Some(Box::new(crate::api::window::WindowEditResult::Ended {
                                transaction: *transaction,
                                committed: false,
                            }))
                        }
                        _ => None,
                    };
                    if let Err(error) = backend.request_window(*request) {
                        if edit.is_some() {
                            backend.cancel_window_session(session);
                        }
                        crate::support::logging::report_error_context(
                            "window",
                            &error,
                            format_args!("operation=submit session={session} request={id}"),
                        );
                        self.dispatch_to(
                            owner,
                            ModeEvent::WindowResult(Box::new(crate::api::window::WindowResult {
                                tabs: None,
                                closed: Vec::new(),
                                session,
                                id,
                                target: None,
                                windows: Some(Vec::new()),
                                pointer: None,
                                changed: 0,
                                skipped: 0,
                                message: Some(error),
                                edit,
                            })),
                            backend,
                        )?;
                    }
                }
                Command::CancelWindowSession(session) => {
                    if self
                        .window_presets
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.session == session)
                        && let Some(prompt) = self.window_presets.pending.take()
                    {
                        backend.set_text_capture(false);
                        backend.cancel_text_prompt(prompt.id);
                    }
                    self.scheduler.window_sessions.remove(&session);
                    backend.cancel_window_session(session);
                }
                Command::DispatchActions(actions) => {
                    let input = crate::api::input::InputEvent {
                        character: None,
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
                    let window_suspended = self.registry.modal_stack.contains(&ModeId::window());
                    // Normal does not consume pointer notifications. A suspended
                    // Window can change the active mode while receiving one.
                    let previous_bounds = (window_suspended || self.active_wants_pointer_events())
                        .then(|| self.context().active_bounds());
                    self.cursor = to;
                    if window_suspended {
                        self.dispatch_to(&ModeId::window(), ModeEvent::PointerMoved(to), backend)?;
                    }
                    if previous_bounds
                        .is_some_and(|bounds| bounds != self.context().active_bounds())
                        && self.active_wants_pointer_events()
                    {
                        self.dispatch(ModeEvent::PointerMoved(to), backend)?;
                    }
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
                    let window_suspended = self.registry.modal_stack.contains(&ModeId::window());
                    // Normal does not consume pointer notifications. A suspended
                    // Window can change the active mode while receiving one.
                    let previous_bounds = (window_suspended || self.active_wants_pointer_events())
                        .then(|| self.context().active_bounds());
                    self.cursor = to;
                    if window_suspended {
                        self.dispatch_to(&ModeId::window(), ModeEvent::PointerMoved(to), backend)?;
                    }
                    // Respect the same subscription as physical pointer events.
                    // Window selection warps must not become MoveTo requests;
                    // explicit modal targeting is delivered separately above.
                    if previous_bounds
                        .is_some_and(|bounds| bounds != self.context().active_bounds())
                        && self.active_wants_pointer_events()
                    {
                        self.dispatch(ModeEvent::PointerMoved(to), backend)?;
                    }
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
                Command::ScrollFrame(mut frame) => {
                    let (horizontal, vertical) = self.settings.invert_scroll;
                    if horizontal {
                        frame.dx = -frame.dx;
                    }
                    if vertical {
                        frame.dy = -frame.dy;
                    }
                    if let Err(error) = backend.scroll_frame(frame) {
                        return Err(self.recoverable_input_error("continuous scroll", error));
                    }
                    self.recoverable_input_succeeded();
                }
                Command::SetFrameClock(active) => {
                    self.scheduler.frame_clock_owner = active.then(|| owner.clone());
                    if let Err(error) = backend.set_frame_clock(active) {
                        self.scheduler.frame_clock_owner = None;
                        // A platform without a native display link retains
                        // keyboard-repeat movement/scrolling as its compatibility path.
                        self.trace_lazy(self.settings.debug.backend, "backend", || {
                            format!("set_frame_clock active={active}: {error}")
                        });
                    }
                }

                Command::SetSpeedToggle { speed, active } => {
                    let selected = active.then_some(speed);
                    if self.overlay.speed_toggle != selected {
                        self.overlay.speed_toggle = selected;
                        self.refresh_overlay(backend)?;
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
                    self.scheduler.scan_activation = None;
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
                    if !self.registry.modal_stack.contains(&ModeId::window()) {
                        self.registry.modal_stack.clear();
                    }
                    self.activate(id, previous, backend)?;
                }
                Command::PushMode(id) => self.push_mode(id, backend)?,
                Command::PushModeInRegion { mode, bounds } => {
                    self.push_mode_in_region(mode, Some(bounds), backend)?
                }
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
                    // A grid retains ownership of its selection while borrowing
                    // Normal shortcuts. Retarget it before the pointer crosses
                    // screens, otherwise PointerMoved resets the retained path.
                    // Passthrough shells still target their temporary mode.
                    let recipient = if self.registry.active.is_grid() {
                        self.registry.active.clone()
                    } else {
                        self.display_mode()
                    };
                    self.dispatch_to(
                        &recipient,
                        ModeEvent::ScreenRetargeted { screen, preserve },
                        backend,
                    )?;
                }

                Command::SetTimer {
                    id,
                    delay,
                    repeating,
                } => {
                    let now = Instant::now();
                    self.trace_lazy(self.settings.debug.timers, "timer", || {
                        format!("set id={id:?} owner={owner} delay={delay:?} repeating={repeating}")
                    });
                    self.scheduler.timers.insert(
                        id,
                        Timer {
                            fires_at: now + delay,
                            last_fired: now,
                            interval: repeating.then_some(delay),
                            owner: owner.clone(),
                        },
                    );
                }
                Command::CancelTimer { id } => {
                    self.scheduler.timers.remove(&id);
                    self.trace_lazy(self.settings.debug.timers, "timer", || {
                        format!("cancel id={id:?} owner={owner}")
                    });
                }

                Command::SetConfigValue { path, value } => {
                    self.request_configuration(
                        configuration_work::Operation::Set { path, value },
                        backend,
                    )?;
                }
                Command::ReloadConfig => self.reload_config(backend)?,

                Command::Exec { program, args } => {
                    std::process::Command::new(&program)
                        .args(&args)
                        .spawn()
                        .map_err(|error| format!("cannot run {program}: {error}"))?;
                }

                Command::Quit => self.should_quit = true,
                Command::MoveWindowFromToScreen { source, target } => {
                    if let Some(pointer) = backend.move_window_from_to_screen(source, target)? {
                        self.execute_for(owner, [Command::warp_to(pointer)], backend)?;
                    }
                }
                Command::MoveWindowToScreen(target) => {
                    if let Some(pointer) = backend.move_window_to_screen(target)? {
                        // Reuse the normal warp path so the authoritative
                        // cursor, overlay and drag state move together.
                        self.execute_for(owner, [Command::warp_to(pointer)], backend)?;
                    }
                }
                Command::CycleWindow { backwards }
                | Command::CycleOverlappingWindow { backwards } => {
                    let operation = if matches!(command, Command::CycleOverlappingWindow { .. }) {
                        if !self.settings.window_overlap_enabled {
                            continue;
                        }
                        crate::api::window::WindowOperation::CycleOverlapping { backwards }
                    } else {
                        crate::api::window::WindowOperation::CycleActive { backwards }
                    };
                    backend.request_window(crate::api::window::WindowRequest {
                        scope: None,
                        session: 0,
                        id: 0,
                        operation,
                    })?;
                }
            }
        }
        Ok(())
    }
}
