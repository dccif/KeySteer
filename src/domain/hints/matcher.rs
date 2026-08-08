use super::Hint;

#[derive(Debug, Clone, PartialEq)]
pub enum Match<T> {
    Complete(T),
    Partial { remaining: usize },
    None,
}

pub fn match_input<T: Clone>(hints: &[Hint<T>], input: &str) -> Match<T> {
    if input.is_empty() {
        return Match::Partial {
            remaining: hints.len(),
        };
    }
    if let Some(hit) = hints.iter().find(|hint| hint.label == input) {
        return Match::Complete(hit.value.clone());
    }

    let remaining = hints
        .iter()
        .filter(|hint| hint.label.starts_with(input))
        .count();
    if remaining == 0 {
        Match::None
    } else {
        Match::Partial { remaining }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Rect;

    fn hint(label: &str, value: usize) -> Hint<usize> {
        Hint {
            label: label.into(),
            bounds: Rect::default(),
            value,
        }
    }

    #[test]
    fn reports_completion_prefixes_and_dead_ends() {
        let hints = [hint("a", 0), hint("fa", 1), hint("fs", 2)];
        assert_eq!(match_input(&hints, "a"), Match::Complete(0));
        assert_eq!(match_input(&hints, "f"), Match::Partial { remaining: 2 });
        assert_eq!(match_input(&hints, "fa"), Match::Complete(1));
        assert_eq!(match_input(&hints, "z"), Match::None);
    }
}
