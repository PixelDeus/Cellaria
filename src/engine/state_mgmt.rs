//! Возраст клеток (`advance_age`/`reset_age`) и терминация/композиция
//! (`detect_termination`/`compose_with`) + `TerminationVerdict`/
//! `CompositionVerdict`.

use super::*;
use super::matcher::CamPositions;

impl<S: GridStorage> Engine<S> {
    // ─── Age ───

    pub fn advance_age(&mut self) {
        self.grid.advance_age();
    }

    /// Устанавливает `born_at` = текущее поколение для всех РЕАЛЬНО
    /// записанных клеток (`region.written_cells`), не для прямоугольника
    /// `x_start..x_end`/`y_start..y_end` — тот шире реального множества
    /// записей (например, сдвиг на N>1 клеток захватывает и клетки МЕЖДУ
    /// исходной и целевой позицией, которые сдвиг не трогает вовсе), и
    /// сбрасывал бы возраст клеткам, которые в этом тике не менялись.
    /// Делегирует внутренней `reset_age_for_regions` — той же функции,
    /// что использует настоящий `run_tick` (единая логика, не две
    /// параллельные копии, из которых только одна была бы исправлена).
    pub fn reset_age(&mut self, regions: &[AffectedRegion]) {
        reset_age_for_regions(&mut self.grid, regions);
    }

    // ─── Терминация ───

    /// Не мутирует `self` (не тик), поэтому не может пройти через
    /// `Engine::run_tick` — арбитраж строится напрямую через
    /// `arbitrate_with_cam` с РЕАЛЬНЫМИ (не пустыми, в отличие от
    /// `Engine::arbitrate`/`raw_phases.rs`) `starvation_counters`/
    /// `feedback_counters`, читаемыми только на просмотр
    /// (`self.state.snapshot()`, ничего не пишется назад) — иначе вердикт
    /// для правил со `starvation_after`/`feedback` был бы неверным
    /// (тот же класс бага, что и в `compose_with`, ниже).
    ///
    /// Честное ограничение, оставшееся и после этого фикса: гейты
    /// `memory`/`max_activations` применяются как фильтр матчей ДО
    /// арбитража внутри `run_tick_with_cache`, не внутри самого
    /// `arbitrate_with_cam` — правило, которое реальный тик бы отфильтровал
    /// этим гейтом, здесь всё ещё виден как найденный матч, так что вердикт
    /// может ошибочно показать `Active` вместо `Stable` для таких правил.
    pub fn detect_termination(&self, tick: u32) -> TerminationVerdict {
        let search_coords = resolve_search_coords_peek(&self.grid, &self.search_radius_cache);
        let matches = detect_matches_with_group_data(&self.grid, &self.group_cache, &search_coords);
        if matches.is_empty() {
            TerminationVerdict::Stable
        } else if tick > 0 {
            let snapshot = self.state.snapshot();
            let (accepted, _) = arbitrate_with_cam(
                matches,
                &self.rule_index,
                &self.rule_cache,
                (self.grid.width(), self.grid.height()),
                &CamPositions::default(),
                self.grid.generation() as u32,
                snapshot.starvation_counters(),
                snapshot.feedback_counters(),
                &[],
                |x, y| self.grid.get_age(x, y) as u32,
            );
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
    ///
    /// Гоняет РЕАЛЬНЫЕ тики через [`Engine::run_tick`] (полный стейтфул-
    /// пайплайн), а не свои собственные detect/arbitrate/apply — раньше
    /// использовал "сырые" `Engine::arbitrate`/`Engine::apply_matches`,
    /// не хранящие состояние между вызовами (см. `raw_phases.rs`'s doc-
    /// комментарий), из-за чего `cam`/`starvation_after`/`feedback`/
    /// `memory`/`max_activations` были no-op для ЭТОЙ проверки — вердикт
    /// (`Bounded`/`Divergent`/`NonTerminating`) мог быть неверным для
    /// любого набора правил с этими расширениями (тот же класс бага, что
    /// был у `LayeredEngine`; подтверждено эмпирически:
    /// правило со `starvation_after` выигрывало 6/20 через `run_tick`,
    /// 0/20 через сырые методы).
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
            let (accepted, _outputs) = self.run_tick();
            if accepted.is_empty() {
                break;
            }
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
