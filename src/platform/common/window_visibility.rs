//! Match native visibility metadata without depending on platform handles.
use crate::api::Rect;

pub(crate) struct Visible {
    pub(crate) pid: i32,
    pub(crate) bounds: Rect,
    pub(crate) title: Option<String>,
    pub(crate) number: Option<isize>,
}

pub(crate) fn visible_candidates(
    candidates: &[(Rect, &str)],
    shown: &Visible,
    remaining: &[Visible],
) -> Vec<usize> {
    let matches: Vec<_> = candidates
        .iter()
        .enumerate()
        .filter(|(_, (bounds, _))| same_rect(*bounds, shown.bounds))
        .map(|(index, _)| index)
        .collect();
    if matches.len() <= 1 {
        return matches;
    }
    // Titles are asynchronous, optional metadata, not window identities. Use
    // them only to disambiguate coincident windows in this process.
    if let Some(title) = shown.title.as_deref().filter(|title| !title.is_empty()) {
        let named: Vec<_> = matches
            .iter()
            .copied()
            .filter(|index| candidates[*index].1 == title)
            .collect();
        if named.len() == 1 {
            return named;
        }
    }
    // Require enough on-screen records that each could describe every AX
    // candidate. Otherwise an indistinguishable window may be on another Space.
    let count = remaining
        .iter()
        .filter(|record| {
            record.pid == shown.pid
                && matches.iter().all(|index| {
                    let (bounds, _) = candidates[*index];
                    same_rect(bounds, record.bounds)
                })
        })
        .count();
    if count == matches.len() {
        matches
    } else {
        Vec::new()
    }
}

fn same_rect(a: Rect, b: Rect) -> bool {
    (a.x - b.x).abs() < 2.0
        && (a.y - b.y).abs() < 2.0
        && (a.width - b.width).abs() < 2.0
        && (a.height - b.height).abs() < 2.0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unique_geometry_does_not_require_synchronized_browser_titles() {
        let internal = Rect::new(0.0, 30.0, 1400.0, 900.0);
        let external = Rect::new(-1920.0, -200.0, 1920.0, 1080.0);
        let candidates = [
            (internal, "New tab - Google Chrome"),
            (external, "Document"),
        ];
        for title in [None, Some(""), Some("Old page"), Some("New tab")] {
            let shown = Visible {
                pid: 10,
                bounds: internal,
                title: title.map(str::to_owned),
                number: Some(42),
            };
            assert_eq!(shown.number, Some(42));
            assert_eq!(
                visible_candidates(&candidates, &shown, std::slice::from_ref(&shown)),
                vec![0]
            );
        }
        let shown = Visible {
            pid: 10,
            bounds: external,
            title: Some("Stale document".into()),
            number: None,
        };
        assert_eq!(
            visible_candidates(&candidates, &shown, std::slice::from_ref(&shown)),
            vec![1]
        );
    }

    #[test]
    fn title_mismatch_does_not_guess_between_spaces_or_processes() {
        let bounds = Rect::new(100.0, 100.0, 400.0, 300.0);
        let candidates = [(bounds, "A"), (bounds, "B")];
        let records = [10, 20].map(|pid| Visible {
            pid,
            bounds,
            title: Some("Stale title".into()),
            number: None,
        });
        assert!(visible_candidates(&candidates, &records[0], &records).is_empty());
        let other = [(Rect::new(900.0, 100.0, 400.0, 300.0), "Stale title")];
        assert!(visible_candidates(&other, &records[0], &records).is_empty());
    }

    #[test]
    fn coincident_windows_are_kept_only_when_the_entire_cohort_is_on_screen() {
        let bounds = Rect::new(100.0, 100.0, 400.0, 300.0);
        let candidates = [(bounds, "First"), (bounds, "Second")];
        let records = [0, 1].map(|number| Visible {
            pid: 10,
            bounds,
            title: None,
            number: Some(number),
        });
        assert_eq!(
            visible_candidates(&candidates, &records[0], &records),
            vec![0, 1]
        );
        assert!(visible_candidates(&candidates, &records[0], &records[..1]).is_empty());
        let named = Visible {
            pid: 10,
            bounds,
            title: Some("Second".into()),
            number: Some(1),
        };
        assert_eq!(visible_candidates(&candidates, &named, &records), vec![1]);
    }
}
