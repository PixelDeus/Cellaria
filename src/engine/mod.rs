pub mod applicator;
pub mod arbitrator;
pub mod matcher;

use std::collections::HashMap;

use crate::conflict_analyzer::build_rule_data_cache;
use crate::grid::Grid;
use crate::storage::GridStorage;
use crate::types::{AffectedRegion, Cell, CellType, CellValue, Rule, RuleMatch};

pub use applicator::apply_matches;
pub use arbitrator::arbitrate;
pub use matcher::detect_matches;

/// Двигатель симуляции Cellaria.
pub struct Engine<S: GridStorage> {
    pub grid: Grid<S>,
    pub rule_index: HashMap<CellType, Vec<Rule>>,
    pub rule_cache: HashMap<(CellType, usize), crate::conflict_analyzer::RuleData>,
}

impl<S: GridStorage> Engine<S> {
    /// Создать новый двигатель с решёткой и индексом правил.
    pub fn new(
        grid: Grid<S>,
        rule_index: HashMap<CellType, Vec<Rule>>,
    ) -> Self {
        let rule_cache = build_rule_data_cache(&rule_index);
        Self {
            grid,
            rule_index,
            rule_cache,
        }
    }

    /// Получить ссылку на решётку.
    pub fn grid(&self) -> &Grid<S> {
        &self.grid
    }

    /// Получить мутабельную ссылку на решётку.
    pub fn grid_mut(&mut self) -> &mut Grid<S> {
        &mut self.grid
    }

    // ─── Обнаружение совпадений ───

    /// Обнаружить все совпадения на текущей решётке.
    pub fn detect_matches(&self) -> Vec<RuleMatch> {
        let coords: Vec<(usize, usize)> = self.grid.iter_active().collect();
        let search_coords = expand_neighborhood(&coords);
        detect_matches(&self.grid, &self.rule_index, &search_coords)
    }

    // ─── Арбитраж ───

    /// Выбрать непротиворечивый набор совпадений.
    pub fn arbitrate(&self, all_matches: Vec<RuleMatch>) -> Vec<RuleMatch> {
        arbitrate(
            all_matches,
            &self.rule_index,
            &self.rule_cache,
            (self.grid.width(), self.grid.height()),
            |x, y| self.grid.get_age(x, y) as u32,
        )
    }

    // ─── Применение ───

    /// Применить набор совпадений к решётке.
    pub fn apply_matches(
        &mut self,
        matches: Vec<RuleMatch>,
    ) -> (Vec<AffectedRegion>, Vec<(u32, Cell)>) {
        apply_matches(&mut self.grid, matches, &self.rule_index, &self.rule_cache)
    }

    // ─── IO ───

    /// Отправить значение на входной канал граничной ячейки.
    /// Ищет первый граничный буфер с direction == "input" и
    /// помещает значение в очередь указанного канала.
    pub fn push_input(&mut self, ch: u32, value: u8) {
        for (_, buf) in self.grid.iter_boundaries_mut() {
            if buf.direction == "input" {
                buf.enqueue(ch, Cell::new(value));
                return;
            }
        }
    }

    /// Извлечь выходные данные из граничных буферов.
    pub fn pop_output(&mut self) -> Vec<(u32, Cell)> {
        let mut outputs = Vec::new();
        let coords: Vec<(usize, usize)> = self.grid.boundary_coords().collect();
        for (x, y) in coords {
            if let Some(buf) = self.grid.get_boundary_mut(x, y) {
                let channels: Vec<u32> = buf.queues.keys().copied().collect();
                for ch in channels {
                    for cell in buf.dequeue(ch) {
                        outputs.push((x as u32, cell));
                    }
                }
            }
        }
        outputs
    }

    /// Применить данные из входных буферов к решётке.
    ///
    /// Каждый вызов потребляет ровно одно значение с фронта очереди каждого
    /// input-буфера (по первому непустому каналу) и продвигает очередь —
    /// иначе следующий тик увидел бы то же самое значение снова, а
    /// остальные когда-либо запушенные значения никогда бы не дошли до
    /// решётки.
    pub fn apply_input(&mut self) {
        let inputs: Vec<(usize, usize, u32, u8)> = {
            let mut v = Vec::new();
            for (&(x, y), buf) in self.grid.iter_boundaries() {
                if buf.direction == "input" {
                    for (&ch, queue) in &buf.queues {
                        if let Some(cell) = queue.front() {
                            v.push((x, y, ch, cell.value.0 .0));
                            break;
                        }
                    }
                }
            }
            v
        };
        let gen = self.grid.generation();
        for (x, y, ch, val) in inputs {
            self.grid.set_cell(
                x,
                y,
                Cell {
                    value: CellValue::new(val),
                    born_at: gen,
                },
            );
            // Потребляем значение — иначе оно будет применяться повторно
            // на каждом следующем тике, а очередь никогда не продвинется.
            if let Some(buf) = self.grid.get_boundary_mut(x, y) {
                if let Some(queue) = buf.queues.get_mut(&ch) {
                    queue.pop_front();
                }
            }
        }
    }

    /// Извлечь и очистить выходные буферы.
    pub fn drain_output(&mut self) -> Vec<(u32, Cell)> {
        let mut outputs = Vec::new();
        let coords: Vec<(usize, usize)> = self.grid.boundary_coords().collect();
        for (x, y) in coords {
            if let Some(buf) = self.grid.get_boundary_mut(x, y) {
                if buf.direction == "output" {
                    let channels: Vec<u32> = buf.queues.keys().copied().collect();
                    for ch in channels {
                        for cell in buf.dequeue(ch) {
                            outputs.push((x as u32, cell));
                        }
                    }
                }
            }
        }
        outputs
    }

    // ─── Age ───

    /// Увеличить возраст всех активных ячеек на 1.
    pub fn advance_age(&mut self) {
        self.grid.advance_age();
    }

    /// Сбросить возраст в affected regions.
    /// Устанавливает born_at = текущее поколение.
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

    /// Проверить, достигла ли система устойчивого состояния.
    pub fn detect_termination(&self, tick: u32) -> TerminationVerdict {
        let coords: Vec<(usize, usize)> = self.grid.iter_active().collect();
        let search_coords = expand_neighborhood(&coords);
        let matches = detect_matches(&self.grid, &self.rule_index, &search_coords);
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
            let coords: Vec<(usize, usize)> = self.grid.iter_active().collect();
            let search_coords = expand_neighborhood(&coords);
            let matches = detect_matches(&self.grid, &self.rule_index, &search_coords);
            if matches.is_empty() {
                break;
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

    // ─── Вспомогательные ───

    /// Получить приоритет правила.
    pub fn get_priority(&self, rule_id: &[CellType]) -> u32 {
        if let Some(first) = rule_id.first() {
            if let Some(rules) = self.rule_index.get(first) {
                for rule in rules {
                    if rule.id == rule_id {
                        return rule.priority;
                    }
                }
            }
        }
        0
    }

    /// Найти правило по ID.
    pub fn find_rule(&self, rule_id: &[CellType]) -> Option<&Rule> {
        if let Some(first) = rule_id.first() {
            if let Some(rules) = self.rule_index.get(first) {
                for rule in rules {
                    if rule.id == rule_id {
                        return Some(rule);
                    }
                }
            }
        }
        None
    }

    /// Выполнить один тик симуляции.
    pub fn run_tick(&mut self) -> (Vec<RuleMatch>, Vec<(u32, Cell)>) {
        run_tick(&mut self.grid, &self.rule_index)
    }
}

/// Выполнить один тик симуляции (свободная функция).
pub fn run_tick<S: GridStorage>(
    grid: &mut Grid<S>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
) -> (Vec<RuleMatch>, Vec<(u32, Cell)>) {
    let rule_cache = crate::conflict_analyzer::build_rule_data_cache(rule_index);
    let active: Vec<(usize, usize)> = grid.iter_active().collect();
    let search_coords = expand_neighborhood(&active);

    let matches = detect_matches(grid, rule_index, &search_coords);
    if matches.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // Арбитраж: выбираем непротиворечивый набор
    let accepted = arbitrate(matches, rule_index, &rule_cache, (grid.width(), grid.height()), |x, y| {
        grid.get_age(x, y) as u32
    });

    if accepted.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // Применение
    let (regions, outputs) = apply_matches(grid, accepted.clone(), rule_index, &rule_cache);

    // Старение
    grid.advance_age();

    // Сброс возраста для изменённых регионов
    reset_age_for_regions(grid, &regions);

    (accepted, outputs)
}

/// Расширить список координат на окрестность ±2.
/// Используется для обнаружения паттернов вокруг активных ячеек.
fn expand_neighborhood(coords: &[(usize, usize)]) -> Vec<(usize, usize)> {
    if coords.is_empty() {
        return Vec::new();
    }
    let mut set = std::collections::HashSet::new();
    for &(x, y) in coords {
        for dx in -2i32..=2i32 {
            for dy in -2i32..=2i32 {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx >= 0 && ny >= 0 {
                    set.insert((nx as usize, ny as usize));
                }
            }
        }
    }
    set.into_iter().collect()
}

fn reset_age_for_regions<S: GridStorage>(grid: &mut Grid<S>, regions: &[AffectedRegion]) {
    let gen = grid.generation();
    for region in regions {
        for y in region.y_start..region.y_end {
            for x in region.x_start..region.x_end {
                if let Some(cell) = grid.get_cell(x as usize, y as usize) {
                    grid.storage.set(
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

// ──────────────────────────────────────────────────────────────
// Тесты
// ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
