use std::cell::Cell;
use std::ops::Range;

pub(crate) type Row = Option<String>;

#[derive(Debug, Clone, Default)]
pub(crate) struct ListState {
    selected: Option<String>,
    index: usize,
    offset: Cell<usize>,
}

impl ListState {
    pub(crate) fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub(crate) fn index(&self) -> usize {
        self.index
    }

    pub(crate) fn sync(&mut self, rows: &[Row]) {
        if let Some(position) = self
            .selected
            .as_deref()
            .and_then(|id| rows.iter().position(|row| row.as_deref() == Some(id)))
        {
            self.index = position;
            return;
        }
        let start = self.index.min(rows.len().saturating_sub(1));
        let found = rows
            .iter()
            .enumerate()
            .skip(start)
            .chain(rows.iter().enumerate().take(start).rev())
            .find(|(_, row)| row.is_some());
        if let Some((position, row)) = found {
            self.index = position;
            self.selected.clone_from(row);
        } else {
            self.index = 0;
            self.selected = None;
        }
    }

    pub(crate) fn step(&mut self, rows: &[Row], delta: isize) {
        self.sync(rows);
        let forward = delta > 0;
        for _ in 0..delta.unsigned_abs() {
            let next = if forward {
                rows.iter()
                    .enumerate()
                    .skip(self.index + 1)
                    .find(|(_, row)| row.is_some())
            } else {
                rows.iter()
                    .enumerate()
                    .take(self.index)
                    .rev()
                    .find(|(_, row)| row.is_some())
            };
            let Some((position, row)) = next else { break };
            self.index = position;
            self.selected.clone_from(row);
        }
    }

    pub(crate) fn top(&mut self, rows: &[Row]) {
        self.selected = None;
        self.index = 0;
        self.sync(rows);
    }

    pub(crate) fn bottom(&mut self, rows: &[Row]) {
        self.selected = None;
        self.index = rows.len().saturating_sub(1);
        self.sync(rows);
        if let Some((position, row)) = rows.iter().enumerate().rev().find(|(_, row)| row.is_some())
        {
            self.index = position;
            self.selected.clone_from(row);
        }
    }

    pub(crate) fn visible(&self, rows: &[Row], height: usize) -> Range<usize> {
        let mut offset = self.offset.get();
        let lead = usize::from(
            self.index
                .checked_sub(1)
                .and_then(|above| rows.get(above))
                .is_some_and(Option::is_none),
        );
        if self.index < offset + lead {
            offset = self.index.saturating_sub(lead);
        } else if self.index >= offset + height {
            offset = self.index + 1 - height;
        }
        offset = offset.min(rows.len().saturating_sub(height));
        self.offset.set(offset);
        offset..(offset + height).min(rows.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(ids: &[&str]) -> Vec<Row> {
        ids.iter()
            .map(|id| (!id.is_empty()).then(|| (*id).to_owned()))
            .collect()
    }

    #[test]
    fn selection_skips_headers() {
        let rows = rows(&["", "a", "b", "", "c"]);
        let mut state = ListState::default();
        state.sync(&rows);
        assert_eq!(state.selected(), Some("a"));
        state.step(&rows, 2);
        assert_eq!(state.selected(), Some("c"));
        state.step(&rows, 5);
        assert_eq!(state.selected(), Some("c"));
        state.step(&rows, -1);
        assert_eq!(state.selected(), Some("b"));
        state.top(&rows);
        assert_eq!(state.selected(), Some("a"));
        state.bottom(&rows);
        assert_eq!(state.selected(), Some("c"));
    }

    #[test]
    fn selection_follows_the_id_when_rows_move() {
        let mut state = ListState::default();
        state.sync(&rows(&["a", "b", "c"]));
        state.step(&rows(&["a", "b", "c"]), 1);
        let moved = rows(&["x", "a", "b", "c"]);
        state.sync(&moved);
        assert_eq!(state.selected(), Some("b"));
        assert_eq!(state.index(), 2);
    }

    #[test]
    fn selection_moves_to_a_neighbour_when_the_row_vanishes() {
        let mut state = ListState::default();
        let before = rows(&["a", "b", "c"]);
        state.sync(&before);
        state.step(&before, 1);
        state.sync(&rows(&["a", "c"]));
        assert_eq!(state.selected(), Some("c"));
        state.sync(&[]);
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn window_keeps_the_selection_and_its_header_visible() {
        let rows = rows(&["H", "a", "b", "c", "H2", "d", "e"]);
        let rows: Vec<Row> = rows
            .into_iter()
            .map(|r| r.filter(|id| !id.starts_with('H')))
            .collect();
        let mut state = ListState::default();
        state.sync(&rows);
        state.bottom(&rows);
        assert_eq!(state.visible(&rows, 3), 4..7);
        state.top(&rows);
        assert_eq!(state.visible(&rows, 3), 0..3);
        state.step(&rows, 3);
        let window = state.visible(&rows, 3);
        assert!(window.contains(&state.index()));
    }
}
