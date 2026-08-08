use crate::api::geometry::Rect;
use crate::api::hint::LabelDirection;

/// A target with its assigned, prefix-free label.
#[derive(Debug, Clone, PartialEq)]
pub struct Hint<T> {
    pub label: String,
    pub bounds: Rect,
    pub value: T,
}

pub fn assign<T>(
    targets: impl IntoIterator<Item = (Rect, T)>,
    alphabet: &[char],
    direction: LabelDirection,
) -> Result<Vec<Hint<T>>, String> {
    let targets: Vec<(Rect, T)> = targets.into_iter().collect();
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    if alphabet.len() < 2 {
        return Err("hint alphabet needs at least 2 characters".into());
    }

    let labels = match direction {
        LabelDirection::Normal => labels_normal(targets.len(), alphabet),
        LabelDirection::Reverse => labels_reverse(targets.len(), alphabet),
    };

    Ok(targets
        .into_iter()
        .zip(labels)
        .map(|((bounds, value), label)| Hint {
            label,
            bounds,
            value,
        })
        .collect())
}

fn labels_normal(count: usize, alphabet: &[char]) -> Vec<String> {
    let n = alphabet.len();
    let mut reserved = 0usize;
    while n - reserved + reserved * n < count {
        reserved += 1;
        if reserved == n {
            return labels_normal_deep(count, alphabet);
        }
    }

    let mut labels = Vec::with_capacity(count);
    for &character in &alphabet[..n - reserved] {
        labels.push(character.to_string());
        if labels.len() == count {
            return labels;
        }
    }
    for &prefix in &alphabet[n - reserved..] {
        for &suffix in alphabet {
            labels.push(format!("{prefix}{suffix}"));
            if labels.len() == count {
                return labels;
            }
        }
    }
    labels
}

fn labels_normal_deep(count: usize, alphabet: &[char]) -> Vec<String> {
    let width = fixed_width_for(count, alphabet.len());
    (0..count)
        .map(|index| fixed_width_label(index, alphabet, width))
        .collect()
}

fn labels_reverse(count: usize, alphabet: &[char]) -> Vec<String> {
    let radix = alphabet.len();
    if count <= radix {
        return alphabet[..count].iter().map(char::to_string).collect();
    }
    let width = fixed_width_for(count, radix);
    (0..count)
        .map(|mut index| {
            let mut label = String::with_capacity(width);
            for _ in 0..width {
                label.push(alphabet[index % radix]);
                index /= radix;
            }
            label
        })
        .collect()
}

fn fixed_width_for(count: usize, radix: usize) -> usize {
    let mut width = 1usize;
    let mut capacity = radix;
    while capacity < count {
        width += 1;
        capacity = capacity.saturating_mul(radix);
    }
    width
}

fn fixed_width_label(mut index: usize, alphabet: &[char], width: usize) -> String {
    let radix = alphabet.len();
    let mut characters = vec![alphabet[0]; width];
    for slot in characters.iter_mut().rev() {
        *slot = alphabet[index % radix];
        index /= radix;
    }
    characters.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(value: &str) -> Vec<char> {
        value.chars().collect()
    }

    fn targets(count: usize) -> Vec<(Rect, usize)> {
        (0..count)
            .map(|index| (Rect::new(index as f64, 0.0, 1.0, 1.0), index))
            .collect()
    }

    fn labels_of<T>(hints: &[Hint<T>]) -> Vec<&str> {
        hints.iter().map(|hint| hint.label.as_str()).collect()
    }

    #[test]
    fn directions_match_documented_sequences() {
        let normal = assign(targets(5), &chars("asdf"), LabelDirection::Normal).unwrap();
        let reverse = assign(targets(5), &chars("asdf"), LabelDirection::Reverse).unwrap();
        assert_eq!(labels_of(&normal), ["a", "s", "d", "fa", "fs"]);
        assert_eq!(labels_of(&reverse), ["aa", "sa", "da", "fa", "as"]);
    }

    #[test]
    fn labels_are_unique_and_prefix_free() {
        for direction in [LabelDirection::Normal, LabelDirection::Reverse] {
            for count in [1, 2, 9, 40, 200, 900] {
                let hints = assign(targets(count), &chars("asdfghjkl"), direction).unwrap();
                let labels = labels_of(&hints);
                let unique: std::collections::BTreeSet<_> = labels.iter().collect();
                assert_eq!(unique.len(), count);
                for left in &labels {
                    for right in &labels {
                        assert!(left == right || !right.starts_with(left));
                    }
                }
            }
        }
    }

    #[test]
    fn validates_degenerate_inputs() {
        assert!(assign(targets(3), &chars("a"), LabelDirection::Normal).is_err());
        assert!(
            assign(
                Vec::<(Rect, usize)>::new(),
                &chars("asdf"),
                LabelDirection::Normal
            )
            .unwrap()
            .is_empty()
        );
    }
}
