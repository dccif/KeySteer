//! Provisional OCR geometry for macOS Text Select sessions.
use super::PreviewFrame;
use crate::api::{
    Rect, SemanticRole, UiTarget,
    command::{UiScanResult, UiScanStatus},
    hint::CompactHint,
    text_select::Action,
};
use smallvec::SmallVec;
use std::{borrow::Cow, ops::Range, sync::Arc};

#[derive(Clone)]
struct Region {
    rows: Vec<UiTarget>,
    bounds: Vec<Rect>,
    complete: bool,
    incomplete: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Regions(Option<Vec<Option<Region>>>);

impl Regions {
    pub fn from_groups(groups: &[(Rect, SmallVec<[&UiTarget; 2]>)]) -> Self {
        Self(
            groups
                .iter()
                .any(|(_, lines)| lines.iter().any(|line| is_fast_preview(line)))
                .then(|| {
                    groups
                        .iter()
                        .map(|(_, lines)| {
                            lines
                                .iter()
                                .any(|line| is_fast_preview(line))
                                .then(|| Region {
                                    rows: lines.iter().map(|line| (*line).clone()).collect(),
                                    bounds: lines
                                        .iter()
                                        .filter(|line| is_fast_preview(line))
                                        .map(|line| line.rect)
                                        .collect(),
                                    complete: false,
                                    incomplete: false,
                                })
                        })
                        .collect()
                }),
        )
    }

    fn update(
        &mut self,
        text: &mut Arc<str>,
        units: &mut Vec<Range<usize>>,
        targets: &[UiTarget],
        retired: &[Rect],
    ) -> bool {
        let Some(previews) = &mut self.0 else {
            return false;
        };
        let mut changed = false;
        for region in previews.iter_mut().flatten() {
            let mut region_changed = false;
            let removed: Vec<_> = region
                .rows
                .iter()
                .filter(|row| retired.contains(&row.rect))
                .map(|row| row.rect)
                .collect();
            if !removed.is_empty() {
                region_changed = true;
                region.rows.retain(|row| !retired.contains(&row.rect));
            }
            for target in targets {
                if !is_fast_preview(target)
                    && removed
                        .iter()
                        .chain(&region.bounds)
                        .any(|rect| rect.intersect(&target.rect).is_some())
                    && !region.rows.contains(target)
                {
                    region.rows.push(target.clone());
                    region_changed = true;
                }
            }
            if region_changed {
                region.rows.sort_by(|a, b| {
                    a.rect
                        .y
                        .total_cmp(&b.rect.y)
                        .then(a.rect.x.total_cmp(&b.rect.x))
                });
                changed = true;
            }
        }
        if !changed {
            return false;
        }
        let mut rebuilt = String::new();
        let mut ranges = Vec::with_capacity(units.len());
        for (index, range) in units.iter().enumerate() {
            if !rebuilt.is_empty() {
                rebuilt.push_str("\n\n");
            }
            let start = rebuilt.len();
            if let Some(region) = &previews[index] {
                write_rows(&mut rebuilt, region.rows.iter());
            } else {
                rebuilt.push_str(&text[range.clone()]);
            }
            ranges.push(start..rebuilt.len());
        }
        *text = rebuilt.into();
        *units = ranges;
        true
    }

    fn finish(&mut self, success: bool) -> bool {
        let Some(previews) = &mut self.0 else {
            return false;
        };
        for region in previews.iter_mut().flatten() {
            region.complete = true;
            region.incomplete = !success
                || region.bounds.iter().any(|rect| {
                    !region
                        .rows
                        .iter()
                        .any(|row| !is_fast_preview(row) && rect.intersect(&row.rect).is_some())
                });
        }
        true
    }

    pub fn reconcile(
        &mut self,
        text: &mut Arc<str>,
        units: &mut Vec<Range<usize>>,
        result: &UiScanResult,
    ) -> bool {
        let mut changed = self.update(text, units, &result.targets, &result.retired);
        if result.status != UiScanStatus::Partial {
            changed |= self.finish(result.status == UiScanStatus::Success);
        }
        changed
    }

    pub fn waiting(&self, selected: &[bool]) -> bool {
        self.0.as_ref().is_some_and(|previews| {
            selected.iter().zip(previews).any(|(selected, region)| {
                *selected && region.as_ref().is_some_and(|region| !region.complete)
            })
        })
    }

    pub fn incomplete(&self, selected: &[bool]) -> bool {
        self.0.as_ref().is_some_and(|previews| {
            selected.iter().zip(previews).any(|(selected, region)| {
                *selected && region.as_ref().is_some_and(|region| region.incomplete)
            })
        })
    }

    pub fn write_deduplicated(
        &self,
        selected: &[bool],
        units: &[Range<usize>],
        text: &Arc<str>,
        output: &mut String,
    ) -> bool {
        let Some(previews) = &self.0 else {
            return false;
        };
        let mut seen = Vec::new();
        let mut duplicate = false;
        for (selected, region) in selected.iter().zip(previews) {
            if !*selected {
                continue;
            }
            if let Some(region) = region {
                for row in region.rows.iter().filter(|row| !is_fast_preview(row)) {
                    if seen.contains(&row) {
                        duplicate = true;
                        break;
                    }
                    seen.push(row);
                }
            }
        }
        if !duplicate {
            return false;
        }
        seen.clear();
        let mut scratch = String::new();
        for (index, (selected, region)) in selected.iter().zip(previews).enumerate() {
            if !*selected {
                continue;
            }
            scratch.clear();
            if let Some(region) = region {
                let mut unique = Vec::new();
                for row in region.rows.iter().filter(|row| !is_fast_preview(row)) {
                    if !seen.contains(&row) {
                        seen.push(row);
                        unique.push(row);
                    }
                }
                write_rows(&mut scratch, unique.into_iter());
            } else {
                scratch.push_str(&text[units[index].clone()]);
            }
            if !scratch.is_empty() {
                if !output.is_empty() {
                    output.push_str("\n\n");
                }
                output.push_str(&scratch);
            }
        }
        true
    }
}

fn write_rows<'a>(output: &mut String, rows: impl Iterator<Item = &'a UiTarget>) {
    let content = rows.flat_map(|line| {
        let source = if line.ocr_text().is_empty() {
            &line.name
        } else {
            line.ocr_text()
        };
        source.trim().chars().chain(std::iter::once(' '))
    });
    let _ = crate::api::presentation::write_ocr_text(output, content);
}

#[derive(Default)]
pub(crate) struct Actions {
    pending: Option<Action>,
}

impl Actions {
    pub fn clear(&mut self) {
        self.pending = None;
    }

    pub fn wait(
        &mut self,
        action: Action,
        waiting: bool,
        incomplete: bool,
        frozen: &mut bool,
    ) -> bool {
        if incomplete {
            return true;
        }
        if waiting {
            *frozen = true;
            self.pending = Some(action);
            return true;
        }
        false
    }

    pub fn resolve(
        &mut self,
        status: &UiScanStatus,
        waiting: bool,
        incomplete: bool,
    ) -> Option<Action> {
        if !matches!(status, UiScanStatus::Partial | UiScanStatus::Success) || incomplete {
            self.pending = None;
        } else if !waiting {
            return self.pending.take();
        }
        None
    }

    #[cfg(test)]
    pub fn pending(&self) -> Option<Action> {
        self.pending
    }
}

pub(crate) fn is_fast_preview(target: &UiTarget) -> bool {
    target.role == SemanticRole::Control && target.name.is_empty() && target.details.is_none()
}

pub(crate) fn holds_labels(
    frozen: bool,
    is_regions: bool,
    previews: &Regions,
    query: &str,
    hints: &[CompactHint<usize>],
) -> bool {
    if frozen || !is_regions || previews.0.is_none() {
        return false;
    }
    let trimmed = query.trim_end();
    if trimmed.len() != query.len() {
        return false;
    }
    let Some(term) = trimmed.split_whitespace().next_back() else {
        return false;
    };
    let code = term
        .strip_prefix('@')
        .or_else(|| term.strip_suffix('@'))
        .unwrap_or(term);
    let code = if code.chars().any(char::is_uppercase) {
        Cow::Owned(code.to_lowercase())
    } else {
        Cow::Borrowed(code)
    };
    (term == "@" || !code.is_empty())
        && hints
            .iter()
            .any(|hint| hint.label.as_str().starts_with(code.as_ref()))
}

pub(crate) fn reconcile<T: PreviewFrame>(
    frame: &mut T,
    history: &mut [T],
    result: &UiScanResult,
) -> bool {
    let changed = frame.reconcile_preview(result);
    for frame in history {
        frame.reconcile_preview(result);
    }
    changed
}
