//! Возраст клеток (`advance_age`/`reset_age`) и терминация/композиция
//! (`detect_termination`/`compose_with`) + `TerminationVerdict`/
//! `CompositionVerdict`.

use super::*;

impl<S: GridStorage> Engine<S> {
    // ─── Age ───

    pub fn advance_age(&mut self) {
        self.grid.advance_age();
    }

    /// Устанавливает `born_at` = текущее поколение для всех клеток в regions.
    pub fn reset_age(&mut self, regions: &[AffectedRegion]) {
        let gen = self.grid.generation();
        for region in regions {
            for y in region.y_start..region.y_end {
                for x in region.x_start..region.x_end {
                    if let Some(cell) = self.grid.get_cell(x as usize, y as usize) {
                        self.grid.storage.set(
                            x as usize,
                            y as usize,
                            Cell {
                                value: cell.value,
                                born_at: gen,
                            },
                        );
                    }
                }
            }
        }
    }

    // ─── Терминация ───

    pub fn detect_termination(&self, tick: u32) -> TerminationVerdict {
        let search_coords = resolve_search_coords_peek(&self.grid, &self.search_radius_cache);
        let matches = detect_matches_with_group_data(&self.grid, &self.group_cache, &search_coords);
        if matches.is_empty() {
            TerminationVerdict::Stable
        } else if tick > 0 {
            let accepted = self.arbitrate(matches);
            if accepted.is_empty() {
                TerminationVerdict::Stable
            } else {
                TerminationVerdict::Active
            }
        } else {
            TerminationVerdict::Active
        }
    }

    /// Проверить возможность композиции.
    pub fn compose_with(&mut self) -> CompositionVerdict {
        let initial_cells: Vec<(usize, usize, Cell)> = {
            let mut v = Vec::new();
            for (x, y) in self.grid.iter_active() {
                if let Some(cell) = self.grid.get_cell(x, y) {
                    v.push((x, y, *cell));
                }
            }
            v
        };
        let initial_count = initial_cells.len();

        let mut tick = 0u32;
        loop {
            if tick > 1000 {
                return CompositionVerdict::NonTerminating;
            }
            let search_coords = resolve_search_coords_advance(&mut self.grid, &self.search_radius_cache);
            let matches = detect_matches_with_group_data(&self.grid, &self.group_cache, &search_coords);
            if matches.is_empty() {
                break;
            }
            // См. комментарий в свободной функции run_tick: помечаем
            // ВСЕ найденные совпадения, не только принятые — проигравшее
            // арбитраж совпадение остаётся актуальным условием и должно
            // переоцениваться на следующем тике.
            for m in &matches {
                self.grid.mark_dirty(m.x as usize, m.y as usize);
            }
            let accepted = self.arbitrate(matches);
            if accepted.is_empty() {
                break;
            }
            let (regions, _) = self.apply_matches(accepted);

            // Старение
            self.grid.advance_age();
            self.reset_age(&regions);
            tick += 1;
        }

        let final_cells: Vec<(usize, usize, Cell)> = {
            let mut v = Vec::new();
            for (x, y) in self.grid.iter_active() {
                if let Some(cell) = self.grid.get_cell(x, y) {
                    v.push((x, y, *cell));
                }
            }
            v
        };
        let final_count = final_cells.len();

        if final_count > initial_count * 2 {
            CompositionVerdict::Divergent
        } else if final_count < initial_count / 2 && initial_count > 0 {
            CompositionVerdict::Shrinking
        } else {
            CompositionVerdict::Bounded(final_count)
        }
    }
}

/// Вердикт терминации.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminationVerdict {
    /// Система активна — есть совпадения.
    Active,
    /// Система стабильна — нет совпадений или арбитраж отклонил все.
    Stable,
}

/// Вердикт композиции.
#[derive(Debug, Clone, PartialEq)]
pub enum CompositionVerdict {
    /// Композиция ограничена.
    Bounded(usize),
    /// Композиция расходится.
    Divergent,
    /// Композиция сжимается.
    Shrinking,
    /// Композиция не завершается за лимит тиков.
    NonTerminating,
    /// Композиция conflict-free (статический анализ).
    Safe,
    /// Композиция с потенциальными конфликтами (статический анализ).
    Unsafe(Vec<(usize, usize)>),
}
