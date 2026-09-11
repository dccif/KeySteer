//! Continuous number composition. Native acknowledgements serialize queued input.
use super::*;
use crate::api::window_tabs::{TabGroupId, TabOperation, TabState, WindowTarget};
use std::collections::VecDeque;

#[derive(Clone, Copy)]
pub(super) enum Input {
    Digit(char),
    Action(W),
}

#[derive(Default)]
pub(super) struct Interaction {
    pub state: TabState,
    pub queue: VecDeque<Input>,
    pub in_flight: Option<u64>,
    pub groups: NumberIndex,
    pub restore: Option<Restore>,
    pub restoring: Option<u64>,
}
pub(super) struct Restore {
    pub template: crate::api::window_presets::TabTemplate,
    pub count: usize,
    pub members: Vec<WindowId>,
}

impl WindowSession {
    pub(super) fn tab_request(&mut self, operation: TabOperation, out: &mut CommandBatch) {
        self.request(WindowOperation::Tabs(operation), out);
        self.tabs.in_flight = Some(self.request);
    }
    pub(super) fn enter_tabs(&mut self, out: &mut CommandBatch) {
        self.tabs.groups = NumberIndex::new(
            self.tabs
                .state
                .groups
                .iter()
                .filter(|g| self.visible.contains(&g.active))
                .map(|g| g.id.0),
        );
        if self.tabs.restore.is_some() {
            self.status = None;
            self.tab_request(TabOperation::EndGroup, out);
            return;
        }
        self.status = Some("Grouping matching application windows…".into());
        self.tab_request(
            TabOperation::Enter {
                screen: self.screen,
            },
            out,
        );
    }
    pub(super) fn tab_result(&mut self, id: u64, ctx: &HostContext<'_>, out: &mut CommandBatch) {
        self.tabs.groups = NumberIndex::new(
            self.tabs
                .state
                .groups
                .iter()
                .filter(|g| self.visible.contains(&g.active))
                .map(|g| g.id.0),
        );
        if self.tabs.restoring == Some(id) {
            self.tabs.restoring = None;
            if self.status.is_none() {
                self.tabs.restore = None;
                self.status = Some("Tab template restored".into());
            }
        }
        if self.tabs.in_flight.is_some_and(|request| id >= request) {
            self.tabs.in_flight = None;
            if self.status.as_deref() == Some("Grouping matching application windows…") {
                self.status = None;
            }
        }
        while self.tabs.in_flight.is_none() {
            let Some(input) = self.tabs.queue.pop_front() else {
                break;
            };
            self.apply_tab_input(input, ctx, out);
        }
    }
    pub(super) fn tab_input(
        &mut self,
        input: Input,
        ctx: &HostContext<'_>,
        out: &mut CommandBatch,
    ) {
        if self.tabs.in_flight.is_some() || self.enter_pending {
            if self.tabs.queue.len() < 64 {
                self.tabs.queue.push_back(input);
            } else {
                self.status =
                    Some("Grouping input is full · wait for the current operation".into());
            }
            return;
        }
        self.apply_tab_input(input, ctx, out);
    }
    fn tab_choose_number(&mut self, group: bool, number: u32, out: &mut CommandBatch) {
        let target = if group {
            self.tabs
                .state
                .group(TabGroupId(number))
                .map(|_| WindowTarget::Group(TabGroupId(number)))
        } else {
            self.visible
                .iter()
                .find(|id| self.numbers.get(id) == Some(&number))
                .map(|id| WindowTarget::Window(*id))
        };
        if let Some(restore) = &mut self.tabs.restore {
            let Some(WindowTarget::Window(id)) = target else {
                self.status = Some("Choose an individual window number for this template".into());
                return;
            };
            if restore.members.contains(&id) || restore.members.len() >= restore.count {
                self.status =
                    Some("Window already selected · remove a selection to change it".into());
                return;
            }
            restore.members.push(id);
            if restore.members.len() == restore.count {
                let operation = TabOperation::Restore {
                    members: restore.members.clone(),
                    region: restore.template.region,
                    screen: self.screen,
                    active: restore.template.active,
                };
                self.tab_request(operation, out);
                self.tabs.restoring = self.tabs.in_flight;
            }
            return;
        }
        if let Some(target) = target {
            self.tab_request(TabOperation::Choose(target), out);
        } else {
            self.status = Some(format!(
                "{}{} is unavailable",
                if group { "Group ~" } else { "Window " },
                number
            ));
        }
    }
    fn finish_tab_number(&mut self, out: &mut CommandBatch) -> bool {
        let pending = self.number.pending();
        let result = self.number.finish(&self.window_index, &self.tabs.groups);
        out.push(Command::CancelTimer {
            id: NUMBER_TIMER.into(),
        });
        if let Some((group, number)) = result {
            self.tab_choose_number(group, number, out);
        } else if pending {
            self.status = Some("Invalid number · the current group is unchanged".into());
        }
        result.is_some() || !pending
    }
    fn apply_tab_input(&mut self, input: Input, ctx: &HostContext<'_>, out: &mut CommandBatch) {
        match input {
            Input::Digit(digit) => {
                self.status = None;
                let completed = self
                    .number
                    .digit(digit, &self.window_index, &self.tabs.groups);
                if completed.is_empty() && !self.number.pending() {
                    self.status = Some("Invalid number · the current group is unchanged".into());
                }
                for (group, number) in completed {
                    self.tab_choose_number(group, number, out);
                }
                if self
                    .number
                    .needs_timer(&self.window_index, &self.tabs.groups)
                {
                    out.push(Command::SetTimer {
                        id: NUMBER_TIMER.into(),
                        delay: Duration::from_millis(self.settings.number_timeout_ms),
                        repeating: false,
                    });
                } else {
                    out.push(Command::CancelTimer {
                        id: NUMBER_TIMER.into(),
                    });
                }
            }
            Input::Action(action) => {
                if let Some(restore) = &mut self.tabs.restore {
                    match action {
                        W::TabRemove | W::Undo => {
                            restore.members.pop();
                            self.status = None;
                            return;
                        }
                        W::TabDissolve => {
                            self.tabs.restore = None;
                            self.cancel_number(out);
                            self.status = None;
                            return;
                        }
                        W::TabEnd => {
                            restore.members.clear();
                            self.cancel_number(out);
                            self.status = None;
                            return;
                        }
                        W::TabPrefix => {
                            self.status =
                                Some("Choose individual window numbers for this template".into());
                            return;
                        }
                        W::TabSeparator => {}
                        _ => {
                            self.status = Some("Select the template's windows first".into());
                            return;
                        }
                    }
                }
                if matches!(action, W::TabEnd | W::TabPrefix | W::TabSeparator) {
                    if !self.finish_tab_number(out) {
                        return;
                    }
                    if self.tabs.in_flight.is_some() && action != W::TabSeparator {
                        self.tabs.queue.push_front(Input::Action(action));
                        return;
                    }
                } else {
                    self.cancel_number(out);
                }
                let operation = match action {
                    W::TabEnd => {
                        self.number.cancel();
                        Some(TabOperation::EndGroup)
                    }
                    W::TabPrefix => {
                        self.number.begin_slot();
                        None
                    }
                    W::TabSeparator => None,
                    W::TabRemove => Some(TabOperation::RemoveActive),
                    W::TabDissolve => Some(TabOperation::Dissolve),
                    W::TabNext => Some(TabOperation::Cycle { backwards: false }),
                    W::TabPrevious => Some(TabOperation::Cycle { backwards: true }),
                    W::TabMoveLeft => Some(TabOperation::Reorder { backwards: true }),
                    W::TabMoveRight => Some(TabOperation::Reorder { backwards: false }),
                    W::Undo => Some(TabOperation::Undo),
                    W::Redo => Some(TabOperation::Redo),
                    W::SaveLayout => {
                        self.save_tab_layout(ctx, out);
                        None
                    }
                    _ => None,
                };
                if let Some(operation) = operation {
                    self.tab_request(operation, out);
                }
            }
        }
    }
    pub(super) fn tab_detail(&self) -> String {
        if let Some(restore) = &self.tabs.restore {
            let members = restore
                .members
                .iter()
                .filter_map(|id| self.numbers.get(id))
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            let mut text = format!(
                "Restore Tabs · {} / {} windows\nSelected: {members}\nSelect window numbers in tab order · applies when complete",
                restore.members.len(),
                restore.count
            );
            if !self.number.display.is_empty() {
                text.push_str(&format!("\nInput: {}", self.number.display));
            }
            if let Some(status) = &self.status {
                text.push_str(&format!("\n{status}"));
            }
            return text;
        }
        let mut text = match self.tabs.state.target {
            Some(WindowTarget::Group(id)) => format!("Tabs · composing ~{}", id.0),
            Some(WindowTarget::Window(id)) => format!(
                "Tabs · start {}",
                self.numbers.get(&id).copied().unwrap_or(0)
            ),
            None => "Tabs · type window numbers to start a group".into(),
        };
        if let Some(WindowTarget::Group(id)) = self.tabs.state.target
            && let Some(group) = self.tabs.state.group(id)
        {
            text.push_str("\nMembers: ");
            text.push_str(
                &group
                    .members
                    .iter()
                    .filter_map(|id| self.numbers.get(id))
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        text.push_str("\nContinue to add · end group to start the next");
        if !self.number.display.is_empty() {
            text.push_str("\nInput: ");
            text.push_str(&self.number.display.replace('`', "~"));
        }
        if self.number.slot {
            text.push_str(" · choose a tab group");
        }
        if let Some(status) = &self.status {
            text.push('\n');
            text.extend(status.chars().take(180));
        }
        text
    }
    fn save_tab_layout(&mut self, ctx: &HostContext<'_>, out: &mut CommandBatch) {
        let group = match self.tabs.state.target {
            Some(WindowTarget::Group(id)) => self.tabs.state.group(id),
            Some(WindowTarget::Window(id)) => self.tabs.state.containing(id),
            None => self
                .tabs
                .state
                .active
                .and_then(|id| self.tabs.state.containing(id)),
        };
        let Some(group) = group else {
            self.status = Some("Select a tab group to save".into());
            return;
        };
        let Some(window) = self.inventory.get(&group.active) else {
            return;
        };
        let Some(screen) = ctx.screens.get(window.screen) else {
            return;
        };
        let Some(bounds) = window.bounds.intersect(&screen.work_area) else {
            self.status = Some("The group is outside the display work area".into());
            return;
        };
        let region = Rect::new(
            (bounds.x - screen.work_area.x) / screen.work_area.width,
            (bounds.y - screen.work_area.y) / screen.work_area.height,
            bounds.width / screen.work_area.width,
            bounds.height / screen.work_area.height,
        );
        let template = crate::api::window_presets::TabTemplate {
            region,
            active: group
                .members
                .iter()
                .position(|id| *id == group.active)
                .unwrap_or(0),
        };
        self.save_pending = true;
        self.status = Some("Save preset · enter an optional name".into());
        out.push(Command::WindowPresets(Box::new(
            crate::api::window_presets::PresetLibraryRequest {
                session: self.session,
                operation: crate::api::window_presets::PresetLibraryOperation::Save {
                    template: crate::api::window_presets::WindowTemplate::Tabs(template),
                    window_count: group.members.len(),
                },
            },
        )));
    }
}
