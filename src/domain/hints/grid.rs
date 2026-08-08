use crate::api::geometry::Rect;

pub fn grid_labels(
    rows: usize,
    columns: usize,
    row_characters: &[char],
    column_characters: &[char],
) -> Result<Vec<String>, String> {
    if row_characters.len() < rows {
        return Err(format!(
            "need {rows} row characters but only {} available",
            row_characters.len()
        ));
    }
    if column_characters.len() < columns {
        return Err(format!(
            "need {columns} column characters but only {} available",
            column_characters.len()
        ));
    }

    Ok((0..rows)
        .flat_map(|row_index| {
            let row = row_characters[row_index];
            (0..columns)
                .map(move |column_index| format!("{row}{}", column_characters[column_index]))
        })
        .collect())
}

pub fn fit_grid(bounds: Rect, max_cells: usize, target_aspect: f64) -> (usize, usize) {
    let max_cells = max_cells.max(1);
    let target_aspect = target_aspect.max(0.1);
    let mut best = (1usize, 1usize, f64::INFINITY);
    for rows in 1..=max_cells {
        for columns in 1..=(max_cells / rows) {
            let cell_aspect =
                (bounds.width / columns as f64) / (bounds.height / rows as f64).max(1.0);
            let aspect_error = (cell_aspect / target_aspect).ln().abs();
            let waste = (max_cells - rows * columns) as f64 / max_cells as f64 * 0.2;
            let score = aspect_error + waste;
            if score < best.2 {
                best = (rows, columns, score);
            }
        }
    }
    (best.0, best.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(value: &str) -> Vec<char> {
        value.chars().collect()
    }

    #[test]
    fn labels_are_row_major_pairs() {
        assert_eq!(
            grid_labels(2, 3, &chars("ab"), &chars("xyz")).unwrap(),
            ["ax", "ay", "az", "bx", "by", "bz"]
        );
        assert!(grid_labels(4, 3, &chars("ab"), &chars("xyz")).is_err());
    }

    #[test]
    fn fit_respects_capacity_and_aspect() {
        let (rows, columns) = fit_grid(Rect::new(0.0, 0.0, 1920.0, 1080.0), 30, 1.5);
        assert!(rows * columns <= 30);
        assert!(columns >= rows);
    }
}
