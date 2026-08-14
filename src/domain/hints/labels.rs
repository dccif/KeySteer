use crate::api::geometry::Rect;
use crate::api::hint::LabelDirection;
use smallvec::SmallVec;

/// A target with its assigned, prefix-free label.
#[derive(Debug, Clone, PartialEq)]
pub struct Hint<T> {
    pub label: String,
    pub bounds: Rect,
    pub value: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct HintCode(SmallVec<[u8; 8]>);

impl HintCode {
    pub(crate) fn as_str(&self) -> &str {
        // `push` is the only mutation path and appends complete encoded chars.
        // Keep the conversion safe even if that invariant changes later.
        std::str::from_utf8(&self.0).unwrap_or_default()
    }
}

impl std::ops::Deref for HintCode {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl PartialEq<String> for HintCode {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other
    }
}

pub(crate) struct CompactHint<T> {
    pub(crate) label: HintCode,
    pub(crate) bounds: Rect,
    pub(crate) value: T,
}

trait LabelBuffer {
    fn clear(&mut self);
    fn push(&mut self, value: char);
}

impl LabelBuffer for String {
    fn clear(&mut self) {
        String::clear(self);
    }

    fn push(&mut self, value: char) {
        String::push(self, value);
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

/// Assign into a reusable buffer, retaining every label String allocation.
pub fn assign_into<T, I>(
    output: &mut Vec<Hint<T>>,
    targets: I,
    alphabet: &[char],
    direction: LabelDirection,
) -> Result<(), String>
where
    I: Iterator<Item = (Rect, T)> + Clone,
{
    let count = targets.clone().count();
    if count != 0 && alphabet.len() < 2 {
        return Err("hint alphabet needs at least 2 characters".into());
    }
    output.truncate(count);
    output.reserve(count.saturating_sub(output.len()));
    for (index, (bounds, value)) in targets.enumerate() {
        if let Some(hint) = output.get_mut(index) {
            write_label(&mut hint.label, index, count, alphabet, direction);
            hint.bounds = bounds;
            hint.value = value;
        } else {
            let mut label = String::new();
            write_label(&mut label, index, count, alphabet, direction);
            output.push(Hint {
                label,
                bounds,
                value,
            });
        }
    }
    Ok(())
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
    let count = targets.clone().count();
    if count != 0 && alphabet.len() < 2 {
        return Err("hint alphabet needs at least 2 characters".into());
    }
    output.truncate(count);
    output.reserve(count.saturating_sub(output.len()));
    for (index, (bounds, value)) in targets.enumerate() {
        if let Some(hint) = output.get_mut(index) {
            write_label(&mut hint.label, index, count, alphabet, direction);
            hint.bounds = bounds;
            hint.value = value;
        } else {
            let mut label = HintCode::default();
            write_label(&mut label, index, count, alphabet, direction);
            output.push(CompactHint {
                label,
                bounds,
                value,
            });
        }
    }
    Ok(())
}

fn write_label<L: LabelBuffer>(
    label: &mut L,
    index: usize,
    count: usize,
    alphabet: &[char],
    direction: LabelDirection,
) {
    label.clear();
    let radix = alphabet.len();
    match direction {
        LabelDirection::Normal => {
            let mut reserved = 0usize;
            while radix - reserved + reserved * radix < count {
                reserved += 1;
                if reserved == radix {
                    let width = fixed_width_for(count, radix);
                    write_fixed_width(label, index, alphabet, width, false);
                    return;
                }
            }
            let singles = radix - reserved;
            if index < singles {
                label.push(alphabet[index]);
            } else {
                let pair = index - singles;
                label.push(alphabet[singles + pair / radix]);
                label.push(alphabet[pair % radix]);
            }
        }
        LabelDirection::Reverse if count <= radix => label.push(alphabet[index]),
        LabelDirection::Reverse => {
            let width = fixed_width_for(count, radix);
            write_fixed_width(label, index, alphabet, width, true);
        }
    }
}

fn write_fixed_width<L: LabelBuffer>(
    label: &mut L,
    mut index: usize,
    alphabet: &[char],
    width: usize,
    little_endian: bool,
) {
    let radix = alphabet.len();
    if little_endian {
        for _ in 0..width {
            label.push(alphabet[index % radix]);
            index /= radix;
        }
        return;
    }
    let mut divisor = radix.saturating_pow(width.saturating_sub(1) as u32);
    for _ in 0..width {
        label.push(alphabet[(index / divisor) % radix]);
        divisor = (divisor / radix).max(1);
    }
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

    #[test]
    fn assign_into_reuses_label_capacity_and_preserves_sequences() {
        let alphabet = chars("asdfghjkl");
        let source = targets(25);
        let mut output = Vec::new();
        assign_into(
            &mut output,
            source.iter().copied(),
            &alphabet,
            LabelDirection::Normal,
        )
        .unwrap();
        let capacities: Vec<_> = output.iter().map(|hint| hint.label.capacity()).collect();
        let expected = assign(source.iter().copied(), &alphabet, LabelDirection::Normal).unwrap();
        assert_eq!(output, expected);

        assign_into(
            &mut output,
            source.iter().copied(),
            &alphabet,
            LabelDirection::Normal,
        )
        .unwrap();
        assert_eq!(
            output
                .iter()
                .map(|hint| hint.label.capacity())
                .collect::<Vec<_>>(),
            capacities
        );
    }
}
