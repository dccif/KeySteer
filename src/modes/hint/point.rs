use super::*;
use crate::api::{
    Point,
    point_sample::{ColorFormat, PromptKeys, Request},
};

pub(super) const TIMER: &str = "ui_hint.point_sample";

#[derive(Debug, Clone, PartialEq)]
pub struct PointSettings {
    pub field_modes: [crate::api::point_sample::FieldMode; 4],
    pub keys: PromptKeys,
    pub formats: Vec<ColorFormat>,
    pub marker_color: Option<crate::api::theme::CompiledColor>,
    pub marker_radius: u16,
    pub marker_width: u16,
    pub movement: crate::modes::normal::Settings,
}

pub(super) struct Inspection {
    movement: crate::modes::NormalMode,
    pub point: Point,
    /// Selection identity and original geometry, retained only for this query.
    pub members: SmallVec<[(usize, Rect); 8]>,
    pub colors: SmallVec<[crate::api::point_sample::SampledColor; 8]>,
    colors_remaining: usize,
    pending_request: Option<Request>,
    pending_member: Option<usize>,
    refresh_member: usize,
    collect_member: usize,
    pub position: usize,
    pub target: Option<usize>,
    pub color: Option<Color>,
    pub format: usize,
    pub adjusting: bool,
    pub copy_pending: bool,
}

impl Inspection {
    fn new(movement: &crate::modes::normal::Settings) -> Self {
        Self {
            movement: crate::modes::NormalMode::new(movement.clone()),
            point: Point::new(0.0, 0.0),
            members: SmallVec::new(),
            colors: SmallVec::new(),
            colors_remaining: 0,
            pending_request: None,
            pending_member: None,
            refresh_member: 0,
            collect_member: 0,
            position: 0,
            target: None,
            color: None,
            format: 0,
            adjusting: false,
            copy_pending: false,
        }
    }
}

impl HintMode {
    pub(super) fn point_candidates(&self) -> &[CompactHint<usize>] {
        self.search_result_hints().map_or(&[], |(_, hints)| hints)
    }

    pub(super) fn point_members_match(&self, point: &Inspection) -> bool {
        let candidates = self.point_candidates();
        point.members.len() == candidates.len()
            && point
                .members
                .iter()
                .zip(candidates)
                .all(|((index, bounds), hint)| {
                    *index == hint.value
                        && self
                            .session
                            .scanned
                            .get(*index)
                            .is_some_and(|target| target.rect == *bounds)
                })
    }

    pub(super) fn point_center(&self, candidates: &[CompactHint<usize>]) -> Point {
        let count = candidates.len() as f64;
        candidates
            .iter()
            .fold(Point::new(0.0, 0.0), |mut center, hint| {
                let point = self.target_point(hint.value);
                center.x += point.x / count;
                center.y += point.y / count;
                center
            })
    }

    fn sample_point(&mut self) -> Command {
        self.request_sample(None)
    }

    fn request_sample(&mut self, member: Option<usize>) -> Command {
        let destination = self.inspection.as_ref().map(|point| {
            member.map_or(point.point, |index| {
                self.target_point(point.members[index].0)
            })
        });
        let Some(point) = self.inspection.as_mut() else {
            return Command::CancelPointSample;
        };
        self.sample_serial = self.sample_serial.wrapping_add(1);
        let request = Request {
            id: self.sample_serial,
            point: destination.unwrap_or(point.point),
        };
        point.pending_request = Some(request);
        point.pending_member = member;
        if let Some(index) = member {
            point.refresh_member = (index + 1) % point.members.len();
        }
        Command::SamplePoint(request)
    }

    fn next_color_sample(&mut self) -> Option<Command> {
        let point = self.inspection.as_mut()?;
        if point.pending_request.is_some() {
            return None;
        }
        // Each slot is visited once. Superseded member captures leave this
        // cursor in place and resume after the current point is sampled.
        while point.collect_member < point.colors.len() && point.colors[point.collect_member].ready
        {
            point.collect_member += 1;
        }
        let index = (point.collect_member < point.colors.len()).then_some(point.collect_member)?;
        Some(self.request_sample(Some(index)))
    }

    pub(super) fn refresh_point_hit(&mut self) -> bool {
        let Some(point) = self.inspection.as_mut() else {
            return false;
        };
        let before = point.target;
        // Search filtering never removes the original scan hit-test targets.
        point.target = self
            .session
            .scanned
            .iter()
            .enumerate()
            .filter(|(_, target)| target.rect.contains(&point.point))
            .min_by(|(_, a), (_, b)| {
                (a.rect.width * a.rect.height).total_cmp(&(b.rect.width * b.rect.height))
            })
            .map(|(index, _)| index);
        point.target != before
    }

    #[inline(never)]
    pub(super) fn sync_point(&mut self, ctx: &HostContext<'_>, out: &mut CommandBatch) {
        let available = !self.point_candidates().is_empty();
        if self
            .inspection
            .as_ref()
            .map_or(!available, |p| self.point_members_match(p))
        {
            return;
        }
        let was_available = self.inspection.is_some();
        let was_adjusting = self.inspection.as_ref().is_some_and(|p| p.adjusting);
        let mut inspection = self.inspection.take();
        if was_adjusting && let Some(point) = inspection.as_mut() {
            point.movement.handle(&ModeEvent::Deactivated, ctx);
        }
        if available {
            let mut point = inspection
                .unwrap_or_else(|| Box::new(Inspection::new(&self.config.search_point.movement)));
            let candidates = self.point_candidates();
            point.point = self.point_center(candidates);
            point.members.clear();
            point.members.extend(
                candidates
                    .iter()
                    .map(|hint| (hint.value, self.session.scanned[hint.value].rect)),
            );
            let color_count = if self.config.search_point.field_modes[3]
                == crate::api::point_sample::FieldMode::Concat
                && candidates.len() > 1
            {
                candidates.len()
            } else {
                0
            };
            point.colors.clear();
            point.colors.resize(color_count, Default::default());
            point.colors_remaining = color_count;
            point.pending_request = None;
            point.pending_member = None;
            point.refresh_member = 0;
            point.collect_member = 0;
            point.position = 0;
            point.target = (candidates.len() == 1).then(|| candidates[0].value);
            point.color = None;
            point.format = 0;
            point.adjusting = false;
            point.copy_pending = false;
            self.inspection = Some(point);
        }
        if self
            .inspection
            .as_ref()
            .is_some_and(|p| p.members.len() > 1)
        {
            self.refresh_point_hit();
        }
        if was_adjusting {
            out.push(Command::SetFrameClock(false));
        }
        if was_available != available || was_adjusting {
            out.push(Command::SetPointAdjustment {
                available,
                adjusting: false,
            });
        }
        self.redraw();
        if available {
            out.push(self.sample_point());
            // Selection changes sample immediately; reuse the active refresh timer.
            if !was_available {
                out.push(Command::SetTimer {
                    id: TIMER.into(),
                    delay: Duration::from_millis(100),
                    repeating: true,
                });
            }
        } else {
            out.push(Command::CancelPointSample);
            out.push(Command::CancelTimer { id: TIMER.into() });
        }
    }

    #[inline(never)]
    pub(super) fn point_event(
        &mut self,
        event: &ModeEvent,
        ctx: &HostContext<'_>,
    ) -> Option<CommandBatch> {
        let next_target = if matches!(event, ModeEvent::CyclePointTarget) {
            self.inspection
                .as_ref()
                .filter(|point| point.adjusting)
                .and_then(|point| {
                    let position = next_target_position(
                        point.position.checked_sub(1),
                        (!point.members.is_empty()).then_some(0),
                        |position| (position + 1) % point.members.len(),
                    )?;
                    Some((position, self.target_point(point.members[position].0)))
                })
        } else {
            None
        };
        let point = self.inspection.as_mut()?;
        match event {
            ModeEvent::TogglePointAdjustment => {
                point.adjusting = !point.adjusting;
                let adjusting = point.adjusting;
                point.movement.handle(&ModeEvent::Deactivated, ctx);
                let mut out = self.redraw();
                out.push(Command::SetFrameClock(false));
                out.push(Command::SetPointAdjustment {
                    available: true,
                    adjusting,
                });
                Some(out)
            }
            ModeEvent::CyclePointColor => {
                point.adjusting = true;
                point.format = (point.format + 1) % self.config.search_point.formats.len();
                let mut out = self.redraw();
                out.push(Command::SetPointAdjustment {
                    available: true,
                    adjusting: true,
                });
                Some(out)
            }
            ModeEvent::CyclePointTarget if point.adjusting => {
                if point.members.len() < 2 || point.copy_pending {
                    return Some(CommandBatch::new());
                }
                let (position, next_point) = next_target?;
                point.position = position + 1;
                let index = point.members[position].0;
                point.point = next_point;
                point.target = Some(index);
                point.color = None;
                point.movement.handle(&ModeEvent::Deactivated, ctx);
                self.refresh_point_hit();
                let mut out = self.redraw();
                out.push(Command::SetFrameClock(false));
                out.push(self.sample_point());
                Some(out)
            }
            ModeEvent::CopyTextField(3) => {
                point.movement.handle(&ModeEvent::Deactivated, ctx);
                let mut out = CommandBatch::one(Command::SetFrameClock(false));
                let concatenate = !point.colors.is_empty();
                let ready = if concatenate {
                    point.colors_remaining == 0
                } else {
                    point.color.is_some()
                };
                if ready {
                    // The visible color already belongs to this exact point.
                    // Copy the same value, never a placeholder or an extra
                    // capture that could fail after the panel was updated.
                    if let Some(info) = self.search_info() {
                        let text = info.copy_value(3);
                        if !text.is_empty() {
                            out.push(Command::CopyText(text));
                        }
                    }
                } else {
                    point.copy_pending = true;
                    if point.pending_request.is_none() {
                        out.push(
                            self.next_color_sample()
                                .unwrap_or_else(|| self.sample_point()),
                        );
                    }
                }
                Some(out)
            }
            ModeEvent::Timer { id, .. } if id == TIMER => {
                // A clipboard sample cannot be superseded by the refresh timer.
                Some(if point.pending_request.is_some() {
                    CommandBatch::new()
                } else {
                    let member = (!point.colors.is_empty()).then_some(point.refresh_member);
                    self.next_color_sample()
                        .unwrap_or_else(|| self.request_sample(member))
                        .into()
                })
            }
            ModeEvent::PointSampled(sample) => {
                if point.pending_request != Some(sample.request) {
                    return Some(CommandBatch::new());
                }
                point.pending_request = None;
                let member = point.pending_member.take();
                let mut changed = false;
                let cache = member.or_else(|| {
                    point.position.checked_sub(1).filter(|&index| {
                        !point.colors.is_empty()
                            && point.members[index].1.center() == sample.request.point
                    })
                });
                if let Some(index) = cache {
                    let captured = crate::api::point_sample::SampledColor {
                        color: sample.color,
                        ready: true,
                    };
                    if !point.colors[index].ready {
                        point.colors_remaining -= 1;
                    }
                    changed = point.colors[index] != captured;
                    point.colors[index] = captured;
                }
                if sample.request.point == point.point {
                    changed |= point.color != sample.color;
                    point.color = sample.color;
                }
                let complete = point.colors_remaining == 0;
                let copy = point.copy_pending && complete;
                if copy {
                    point.copy_pending = false;
                }
                let mut out = if changed {
                    self.redraw()
                } else {
                    CommandBatch::new()
                };
                if copy && let Some(info) = self.search_info() {
                    let text = info.copy_value(3);
                    if !text.is_empty() {
                        out.push(Command::CopyText(text));
                    }
                }
                if let Some(command) = self.next_color_sample() {
                    out.push(command);
                }
                Some(out)
            }
            ModeEvent::Binding { .. } | ModeEvent::Frame { .. }
                if point.adjusting && !point.copy_pending =>
            {
                let mut out = CommandBatch::new();
                let before = point.point;
                for command in point.movement.handle(event, ctx) {
                    match command {
                        Command::MovePointer { dx, dy } => {
                            let bounds = self
                                .session
                                .scan_bounds
                                .unwrap_or_else(|| ctx.active_bounds());
                            point.point.x = (point.point.x + dx)
                                .clamp(bounds.x, (bounds.right() - 1.0).max(bounds.x));
                            point.point.y = (point.point.y + dy)
                                .clamp(bounds.y, (bounds.bottom() - 1.0).max(bounds.y));
                        }
                        Command::SetFrameClock(_) => out.push(command),
                        _ => {}
                    }
                }
                if point.point != before {
                    point.color = None;
                    point.copy_pending = false;
                    self.refresh_point_hit();
                    out.extend(self.redraw());
                    out.push(self.sample_point());
                }
                Some(out)
            }
            _ => None,
        }
    }
}
