use crate::api::geometry::Rect;
use crate::api::hint::LabelDirection;

pub(crate) use crate::api::hint::{CompactHint, HintCode};

trait LabelBuffer {
    fn clear(&mut self);
    fn push(&mut self, value: char);
}

#[derive(Clone, Copy)]
enum LabelPlan {
    Direct,
    NormalPairs { singles: usize },
    FixedForward { width: usize, divisor: usize },
    FixedReverse { width: usize },
}

impl LabelPlan {
    fn new(count: usize, alphabet_len: usize, direction: LabelDirection) -> Self {
        match direction {
            LabelDirection::Normal => {
                let mut reserved = 0usize;
                while alphabet_len - reserved + reserved * alphabet_len < count {
                    reserved += 1;
                    if reserved == alphabet_len {
                        let width = fixed_width_for(count, alphabet_len);
                        return Self::FixedForward {
                            width,
                            divisor: alphabet_len.saturating_pow(width.saturating_sub(1) as u32),
                        };
                    }
                }
                Self::NormalPairs {
                    singles: alphabet_len - reserved,
                }
            }
            LabelDirection::Reverse if count <= alphabet_len => Self::Direct,
            LabelDirection::Reverse => Self::FixedReverse {
                width: fixed_width_for(count, alphabet_len),
            },
        }
    }
}

impl LabelBuffer for HintCode {
    fn clear(&mut self) {
        self.0.clear();
    }

    fn push(&mut self, value: char) {
        let mut encoded = [0; 4];
        self.0
            .extend_from_slice(value.encode_utf8(&mut encoded).as_bytes());
    }
}

pub(crate) fn assign_compact_into<T, I>(
    output: &mut Vec<CompactHint<T>>,
    targets: I,
    alphabet: &[char],
    direction: LabelDirection,
) -> Result<(), String>
where
    I: Iterator<Item = (Rect, T)> + Clone,
{
    let count = iterator_count(&targets);
    if count == 0 {
        output.clear();
        return Ok(());
    }
    if alphabet.len() < 2 {
        return Err("hint alphabet needs at least 2 characters".into());
    }
    let plan = LabelPlan::new(count, alphabet.len(), direction);
    output.truncate(count);
    output.reserve(count.saturating_sub(output.len()));
    for (index, (bounds, value)) in targets.enumerate() {
        if let Some(hint) = output.get_mut(index) {
            write_label(&mut hint.label, index, alphabet, plan);
            hint.bounds = bounds;
            hint.value = value;
        } else {
            let mut label = HintCode::default();
            write_label(&mut label, index, alphabet, plan);
            output.push(CompactHint {
                label,
                bounds,
                value,
            });
        }
    }
    Ok(())
}

fn iterator_count<I: Iterator + Clone>(iterator: &I) -> usize {
    let (minimum, maximum) = iterator.size_hint();
    if maximum == Some(minimum) {
        minimum
    } else {
        iterator.clone().count()
    }
}

fn write_label<L: LabelBuffer>(label: &mut L, index: usize, alphabet: &[char], plan: LabelPlan) {
    label.clear();
    let radix = alphabet.len();
    match plan {
        LabelPlan::Direct => label.push(alphabet[index]),
        LabelPlan::NormalPairs { singles } => {
            if index < singles {
                label.push(alphabet[index]);
            } else {
                let pair = index - singles;
                label.push(alphabet[singles + pair / radix]);
                label.push(alphabet[pair % radix]);
            }
        }
        LabelPlan::FixedForward { width, divisor } => {
            write_fixed_width(label, index, alphabet, width, Some(divisor));
        }
        LabelPlan::FixedReverse { width } => {
            write_fixed_width(label, index, alphabet, width, None);
        }
    }
}

fn write_fixed_width<L: LabelBuffer>(
    label: &mut L,
    mut index: usize,
    alphabet: &[char],
    width: usize,
    mut divisor: Option<usize>,
) {
    let radix = alphabet.len();
    if divisor.is_none() {
        for _ in 0..width {
            label.push(alphabet[index % radix]);
            index /= radix;
        }
        return;
    }
    let mut divisor = divisor.take().unwrap_or(1);
    for _ in 0..width {
        label.push(alphabet[(index / divisor) % radix]);
        divisor = (divisor / radix).max(1);
    }
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

    fn assign_labels(
        count: usize,
        alphabet: &[char],
        direction: LabelDirection,
    ) -> Result<Vec<CompactHint<usize>>, String> {
        let mut hints = Vec::new();
        assign_compact_into(&mut hints, targets(count).into_iter(), alphabet, direction)?;
        Ok(hints)
    }

    fn labels_of<T>(hints: &[CompactHint<T>]) -> Vec<&str> {
        hints.iter().map(|hint| hint.label.as_str()).collect()
    }

    fn reference_label(
        mut index: usize,
        count: usize,
        alphabet: &[char],
        direction: LabelDirection,
    ) -> String {
        let radix = alphabet.len();
        match direction {
            LabelDirection::Normal => {
                let mut reserved = 0usize;
                while radix - reserved + reserved * radix < count {
                    reserved += 1;
                    if reserved == radix {
                        let width = fixed_width_for(count, radix);
                        let mut divisor = radix.saturating_pow(width.saturating_sub(1) as u32);
                        let mut label = String::new();
                        for _ in 0..width {
                            label.push(alphabet[(index / divisor) % radix]);
                            divisor = (divisor / radix).max(1);
                        }
                        return label;
                    }
                }
                let singles = radix - reserved;
                if index < singles {
                    alphabet[index].to_string()
                } else {
                    let pair = index - singles;
                    format!(
                        "{}{}",
                        alphabet[singles + pair / radix],
                        alphabet[pair % radix]
                    )
                }
            }
            LabelDirection::Reverse if count <= radix => alphabet[index].to_string(),
            LabelDirection::Reverse => {
                let width = fixed_width_for(count, radix);
                let mut label = String::new();
                for _ in 0..width {
                    label.push(alphabet[index % radix]);
                    index /= radix;
                }
                label
            }
        }
    }

    #[test]
    fn directions_match_documented_sequences() {
        let normal = assign_labels(5, &chars("asdf"), LabelDirection::Normal).unwrap();
        let reverse = assign_labels(5, &chars("asdf"), LabelDirection::Reverse).unwrap();
        assert_eq!(labels_of(&normal), ["a", "s", "d", "fa", "fs"]);
        assert_eq!(labels_of(&reverse), ["aa", "sa", "da", "fa", "as"]);
    }

    #[test]
    fn labels_are_unique_and_prefix_free() {
        for direction in [LabelDirection::Normal, LabelDirection::Reverse] {
            for count in [1, 2, 9, 40, 200, 900] {
                let hints = assign_labels(count, &chars("asdfghjkl"), direction).unwrap();
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
        assert!(assign_labels(3, &chars("a"), LabelDirection::Normal).is_err());
        assert!(
            assign_labels(0, &chars("asdf"), LabelDirection::Normal)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn compact_assignment_reuses_label_capacity_and_preserves_sequences() {
        let alphabet = chars("asdfghjkl");
        let source = targets(25);
        let mut output = Vec::new();
        assign_compact_into(
            &mut output,
            source.iter().copied(),
            &alphabet,
            LabelDirection::Normal,
        )
        .unwrap();
        let capacities: Vec<_> = output.iter().map(|hint| hint.label.0.capacity()).collect();
        let expected = labels_of(&output)
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();

        assign_compact_into(
            &mut output,
            source.iter().copied(),
            &alphabet,
            LabelDirection::Normal,
        )
        .unwrap();
        assert_eq!(
            output
                .iter()
                .map(|hint| hint.label.0.capacity())
                .collect::<Vec<_>>(),
            capacities
        );
        assert_eq!(labels_of(&output), expected);
    }

    #[test]
    fn precomputed_plans_preserve_labels_across_capacity_boundaries() {
        let alphabet = chars("arstneioqwfpjluy");
        for direction in [LabelDirection::Normal, LabelDirection::Reverse] {
            for count in [1, 16, 17, 128, 129, 256, 257, 500, 2_000] {
                let assigned = assign_labels(count, &alphabet, direction).unwrap();
                for (index, hint) in assigned.iter().enumerate() {
                    assert_eq!(
                        hint.label.as_str(),
                        reference_label(index, count, &alphabet, direction),
                        "count={count}, index={index}, direction={direction:?}"
                    );
                    assert_eq!(hint.value, index);
                    assert_eq!(hint.bounds, Rect::new(index as f64, 0.0, 1.0, 1.0));
                }
            }
        }
    }
}
