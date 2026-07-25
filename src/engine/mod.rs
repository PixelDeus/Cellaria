pub mod applicator;
pub mod arbitrator;
pub mod matcher;

use std::collections::HashMap;

use crate::conflict_analyzer::compute_affected_cells;
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
}

impl<S: GridStorage> Engine<S> {
    /// Создать новый двигатель с решёткой и индексом правил.
    pub fn new(grid: Grid<S>, rule_index: HashMap<CellType, Vec<Rule>>) -> Self {
        Self { grid, rule_index }
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
        arbitrate(all_matches, &self.rule_index, |x, y| {
            self.grid
                .get_cell(x, y)
                .map(|c| c.age as u32)
                .unwrap_or(0)
        })
    }

    // ─── Применение ───

    /// Применить набор совпадений к решётке.
    pub fn apply_matches(
        &mut self,
        matches: Vec<RuleMatch>,
    ) -> (Vec<AffectedRegion>, Vec<(u32, Cell)>) {
        apply_matches(&mut self.grid, matches, &self.rule_index)
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
    pub fn apply_input(&mut self) {
        let inputs: Vec<(usize, usize, u32, u8)> = {
            let mut v = Vec::new();
            for (&(x, y), buf) in self.grid.iter_boundaries() {
                if buf.direction == "input" {
                    for (_ch, queue) in &buf.queues {
                        if let Some(cell) = queue.front() {
                            v.push((x, y, *_ch, cell.value.0 .0));
                            break;
                        }
                    }
                }
            }
            v
        };
        for (x, y, _ch, val) in inputs {
            self.grid.set_cell(
                x,
                y,
                Cell {
                    value: CellValue(CellType::new(val)),
                    age: 0,
                },
            );
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
        let coords: Vec<(usize, usize)> = self.grid.iter_active().collect();
        for &(x, y) in &coords {
            if let Some(cell) = self.grid.get_cell(x, y) {
                self.grid.set_cell(
                    x,
                    y,
                    Cell {
                        value: cell.value,
                        age: cell.age + 1,
                    },
                );
            }
        }
    }

    /// Сбросить возраст в affected regions до 0.
    /// Использует `storage.set()` напрямую, чтобы не затрагивать active_coords.
    pub fn reset_age(&mut self, regions: &[AffectedRegion]) {
        for region in regions {
            for y in region.y_start..region.y_end {
                for x in region.x_start..region.x_end {
                    if let Some(cell) = self.grid.get_cell(x as usize, y as usize) {
                        self.grid.storage.set(
                            x as usize,
                            y as usize,
                            Cell {
                                value: cell.value,
                                age: 0,
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
            for &(x, y) in &coords {
                if let Some(cell) = self.grid.get_cell(x, y) {
                    self.grid.set_cell(
                        x,
                        y,
                        Cell {
                            value: cell.value,
                            age: cell.age + 1,
                        },
                    );
                }
            }

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

    /// Вычислить affected cells для совпадения.
    pub fn get_match_affected_cells(&self, m: &RuleMatch) -> Vec<(i32, i32)> {
        let rule = self.find_rule(&m.rule_id);
        if let Some(rule) = rule {
            let relative = compute_affected_cells(rule);
            relative
                .iter()
                .map(|&(dx, dy)| (m.x as i32 + dx, m.y as i32 + dy))
                .collect()
        } else {
            let mut cells = Vec::new();
            for (i, _) in m.rule_id.iter().enumerate() {
                cells.push((m.x as i32 + i as i32, m.y as i32));
            }
            cells
        }
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
        let active: Vec<(usize, usize)> = self.grid.iter_active().collect();
        run_tick(&mut self.grid, &self.rule_index, &active)
    }
}

/// Выполнить один тик симуляции (свободная функция).
pub fn run_tick<S: GridStorage>(
    grid: &mut Grid<S>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    active: &[(usize, usize)],
) -> (Vec<RuleMatch>, Vec<(u32, Cell)>) {
    let search_coords = expand_neighborhood(active);

    let matches = detect_matches(grid, rule_index, &search_coords);
    if matches.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // Арбитраж: выбираем непротиворечивый набор
    let accepted = arbitrate(matches, rule_index, |x, y| {
        grid.get_cell(x, y).map(|c| c.age as u32).unwrap_or(0)
    });

    if accepted.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // Применение
    let (regions, outputs) = apply_matches(grid, accepted.clone(), rule_index);

    // Старение
    for &(x, y) in active {
        if let Some(cell) = grid.get_cell(x, y) {
            grid.set_cell(
                x,
                y,
                Cell {
                    value: cell.value,
                    age: cell.age + 1,
                },
            );
        }
    }

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
    for region in regions {
        for y in region.y_start..region.y_end {
            for x in region.x_start..region.x_end {
                if let Some(cell) = grid.get_cell(x as usize, y as usize) {
                    grid.storage.set(
                        x as usize,
                        y as usize,
                        Cell {
                            value: cell.value,
                            age: 0,
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
mod tests {
    use super::*;
    use crate::types::{
        Cell, CellType, CellValue, ChangeValue, Direction, OverflowAction, Rule, ShiftSpec,
    };
    use crate::BoundaryBuffer;
    use crate::VecStorage;
    use std::collections::HashSet;

    fn make_grid(w: usize, h: usize) -> Grid<VecStorage> {
        let storage = VecStorage::new(w, h);
        Grid::new(storage, HashSet::new())
    }

    fn make_rule_index(rules: Vec<Rule>) -> HashMap<CellType, Vec<Rule>> {
        let mut index: HashMap<CellType, Vec<Rule>> = HashMap::new();
        for rule in rules {
            if let Some(first) = rule.id.first() {
                index.entry(*first).or_default().push(rule);
            }
        }
        index
    }

    #[test]
    fn test_run_tick() {
        let mut grid = make_grid(2, 2);
        grid.set_cell(
            0,
            0,
            Cell {
                value: CellValue(CellType(5)),
                age: 0,
            },
        );

        let rule = Rule {
            id: vec![CellType(5)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(9))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        };

        let rule_index = make_rule_index(vec![rule]);
        let mut engine = Engine::new(grid, rule_index);

        let matches = engine.detect_matches();
        assert_eq!(matches.len(), 1);

        let accepted = engine.arbitrate(matches);
        assert_eq!(accepted.len(), 1);

        engine.apply_matches(accepted);

        assert_eq!(
            engine.grid.get_cell(0, 0).unwrap().value,
            CellValue(CellType(9))
        );
    }

    #[test]
    fn test_shift_right() {
        let mut grid = make_grid(3, 1);
        grid.set_cell(
            0,
            0,
            Cell {
                value: CellValue(CellType(5)),
                age: 0,
            },
        );

        let rule = Rule {
            id: vec![CellType(5)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec {
                direction: Direction::Right,
                steps: 1,
            }]],
            changes: vec![],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        };

        let rule_index = make_rule_index(vec![rule]);
        let mut engine = Engine::new(grid, rule_index);

        let matches = engine.detect_matches();
        let accepted = engine.arbitrate(matches);
        engine.apply_matches(accepted);

        assert_eq!(
            engine.grid.get_cell(0, 0).unwrap().value,
            CellValue(CellType(0))
        );
        assert_eq!(
            engine.grid.get_cell(1, 0).unwrap().value,
            CellValue(CellType(5))
        );
    }

    #[test]
    fn test_shift_with_change() {
        let mut grid = make_grid(3, 1);
        grid.set_cell(
            0,
            0,
            Cell {
                value: CellValue(CellType(5)),
                age: 0,
            },
        );
        grid.set_cell(
            1,
            0,
            Cell {
                value: CellValue(CellType(7)),
                age: 0,
            },
        );

        let rule = Rule {
            id: vec![CellType(5), CellType(7)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec {
                direction: Direction::Right,
                steps: 1,
            }]],
            changes: vec![
                (0, 0, ChangeValue::Literal(1)),
                (1, 0, ChangeValue::Literal(2)),
            ],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        };

        let rule_index = make_rule_index(vec![rule]);
        let mut engine = Engine::new(grid, rule_index);
        let matches = engine.detect_matches();
        assert_eq!(matches.len(), 1);

        let accepted = engine.arbitrate(matches);
        assert_eq!(accepted.len(), 1);

        // Применяем правило с возрастными эффектами
        let (regions, _) = engine.apply_matches(accepted);
        engine.advance_age();
        engine.reset_age(&regions);

        assert_eq!(
            engine.grid.get_cell(0, 0).unwrap().value,
            CellValue(CellType(0)),
            "original cell is cleared by shift"
        );
        assert_eq!(
            engine.grid.get_cell(1, 0).unwrap().value,
            CellValue(CellType(1)),
            "change (0,0) + total_dx=1 => (1,0) = 1"
        );
        assert_eq!(
            engine.grid.get_cell(2, 0).unwrap().value,
            CellValue(CellType(2)),
            "change (1,0) + total_dx=1 => (2,0) = 2"
        );
    }

    #[test]
    fn test_overflow_discard() {
        let mut grid = make_grid(1, 1);
        grid.set_cell(
            0,
            0,
            Cell {
                value: CellValue(CellType(5)),
                age: 0,
            },
        );

        let rule = Rule {
            id: vec![CellType(5)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec {
                direction: Direction::Right,
                steps: 1,
            }]],
            changes: vec![],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: OverflowAction::Discard,
        };

        let rule_index = make_rule_index(vec![rule]);
        let mut engine = Engine::new(grid, rule_index);
        let matches = engine.detect_matches();
        let accepted = engine.arbitrate(matches);
        engine.apply_matches(accepted);

        assert_eq!(
            engine.grid.get_cell(0, 0).unwrap().value,
            CellValue(CellType(0))
        );
    }

    #[test]
    fn test_overflow_write() {
        let mut grid = make_grid(1, 1);
        grid.set_cell(
            0,
            0,
            Cell {
                value: CellValue(CellType(42)),
                age: 0,
            },
        );

        let rule = Rule {
            id: vec![CellType(42)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec {
                direction: Direction::Right,
                steps: 1,
            }]],
            changes: vec![],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: OverflowAction::Write(99),
        };

        let rule_index = make_rule_index(vec![rule]);
        let mut engine = Engine::new(grid, rule_index);
        let matches = engine.detect_matches();
        let accepted = engine.arbitrate(matches);
        engine.apply_matches(accepted);

        assert_eq!(
            engine.grid.get_cell(0, 0).unwrap().value,
            CellValue(CellType(99))
        );
    }

    #[test]
    fn test_age_advancement() {
        let mut grid = make_grid(2, 2);
        grid.set_cell(
            0,
            0,
            Cell {
                value: CellValue(CellType(1)),
                age: 0,
            },
        );

        let rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
        let mut engine = Engine::new(grid, rule_index);
        engine.advance_age();

        assert_eq!(engine.grid().get_cell(0, 0).unwrap().age, 1);
    }

    #[test]
    fn test_reset_age() {
        let mut grid = make_grid(2, 2);
        grid.set_cell(
            0,
            0,
            Cell {
                value: CellValue(CellType(1)),
                age: 5,
            },
        );

        let rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
        let mut engine = Engine::new(grid, rule_index);

        let region = AffectedRegion {
            x_start: 0,
            x_end: 1,
            y_start: 0,
            y_end: 1,
            has_changes: true,
        };

        engine.reset_age(&[region]);

        assert_eq!(engine.grid().get_cell(0, 0).unwrap().age, 0);
    }

    #[test]
    fn test_detect_termination_stable() {
        let grid = make_grid(2, 2);
        let rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
        let engine = Engine::new(grid, rule_index);

        assert_eq!(
            engine.detect_termination(0),
            TerminationVerdict::Stable
        );
    }

    #[test]
    fn test_detect_termination_active() {
        let mut grid = make_grid(2, 2);
        grid.set_cell(
            0,
            0,
            Cell {
                value: CellValue(CellType(1)),
                age: 0,
            },
        );

        let rule = Rule {
            id: vec![CellType(1)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(2))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        };

        let rule_index = make_rule_index(vec![rule]);
        let engine = Engine::new(grid, rule_index);

        assert_eq!(engine.detect_termination(0), TerminationVerdict::Active);
    }

    #[test]
    fn test_apply_match() {
        let mut grid = make_grid(3, 3);
        grid.set_cell(
            1,
            1,
            Cell {
                value: CellValue(CellType(5)),
                age: 0,
            },
        );

        let rule = Rule {
            id: vec![CellType(5)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(9))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        };

        let rule_index = make_rule_index(vec![rule]);
        let mut engine = Engine::new(grid, rule_index);

        let matches = engine.detect_matches();
        assert_eq!(matches.len(), 1);
        let accepted = engine.arbitrate(matches);
        assert_eq!(accepted.len(), 1);

        engine.apply_matches(accepted);

        assert_eq!(
            engine.grid.get_cell(1, 1).unwrap().value,
            CellValue(CellType(9))
        );
    }

    #[test]
    fn test_apply_matches_empty() {
        let grid = make_grid(3, 3);
        let rule_index = make_rule_index(vec![]);
        let mut engine = Engine::new(grid, rule_index);
        let (regions, _) = engine.apply_matches(vec![]);
        assert!(regions.is_empty());
    }

    #[test]
    fn test_run_tick_simple() {
        let mut grid = make_grid(3, 3);
        grid.set_cell(
            1,
            1,
            Cell {
                value: CellValue(CellType(7)),
                age: 0,
            },
        );

        let rule = Rule {
            id: vec![CellType(7)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(3))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        };

        let rule_index = make_rule_index(vec![rule]);
        let active: Vec<(usize, usize)> = grid.iter_active().collect();
        let (accepted, _) = run_tick(&mut grid, &rule_index, &active);

        assert_eq!(accepted.len(), 1);
        assert_eq!(grid.get_cell(1, 1).unwrap().value, CellValue(CellType(3)));
    }

    #[test]
    fn test_run_tick_empty_grid() {
        let mut grid = make_grid(3, 3);
        let rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
        let active: Vec<(usize, usize)> = grid.iter_active().collect();
        let (accepted, _) = run_tick(&mut grid, &rule_index, &active);
        assert!(accepted.is_empty());
    }

    #[test]
    fn test_io_boundary() {
        let mut grid = make_grid(8, 1);
        let mut input_buf = BoundaryBuffer::new();
        input_buf.direction = "input".to_string();
        grid.set_boundary(0, 0, input_buf);

        let mut output_buf = BoundaryBuffer::new();
        output_buf.direction = "output".to_string();
        grid.set_boundary(7, 0, output_buf);

        let rule = Rule {
            id: vec![CellType(5)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec {
                direction: Direction::Right,
                steps: 1,
            }]],
            changes: vec![(0, 0, ChangeValue::Literal(0))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        };

        let rule_index = make_rule_index(vec![rule]);
        let mut engine = Engine::new(grid, rule_index);

        let matches = engine.detect_matches();
        assert!(matches.is_empty(), "No 5 on grid");

        let outputs = engine.pop_output();
        assert!(outputs.is_empty());
    }

    #[test]
    fn test_2d_pattern_match() {
        // Правило: pattern 3×3 L-образный
        // (0,0,1), (1,0,2), (0,1,3) → меняем на 4,5,6
        let rule = Rule {
            id: vec![CellType(1), CellType(2), CellType(3)],
            pattern: vec![
                (0i8, 0i8, CellType(1)),
                (1i8, 0i8, CellType(2)),
                (0i8, 1i8, CellType(3)),
            ],
            shifts: vec![],
            changes: vec![
                (0, 0, ChangeValue::Literal(4)),
                (1, 0, ChangeValue::Literal(5)),
                (0, 1, ChangeValue::Literal(6)),
            ],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        };

        let mut grid = make_grid(3, 3);
        grid.set_cell(0, 0, Cell { value: CellValue(CellType(1)), age: 0 });
        grid.set_cell(1, 0, Cell { value: CellValue(CellType(2)), age: 0 });
        grid.set_cell(0, 1, Cell { value: CellValue(CellType(3)), age: 0 });

        let rule_index = make_rule_index(vec![rule]);
        let mut engine = Engine::new(grid, rule_index);

        let matches = engine.detect_matches();
        assert_eq!(matches.len(), 1, "должно быть ровно одно совпадение");

        let accepted = engine.arbitrate(matches);
        assert_eq!(accepted.len(), 1, "арбитраж должен пропустить ровно одно");

        engine.apply_matches(accepted);

        // Проверяем изменения
        assert_eq!(
            engine.grid.get_cell(0, 0).unwrap().value,
            CellValue(CellType(4)),
            "ячейка (0,0) должна стать 4"
        );
        assert_eq!(
            engine.grid.get_cell(1, 0).unwrap().value,
            CellValue(CellType(5)),
            "ячейка (1,0) должна стать 5"
        );
        assert_eq!(
            engine.grid.get_cell(0, 1).unwrap().value,
            CellValue(CellType(6)),
            "ячейка (0,1) должна стать 6"
        );
    }

    #[test]
    fn test_nondeterministic_same_priority() {
        let mut grid = make_grid(8, 1);
        grid.set_cell(
            1,
            0,
            Cell {
                value: CellValue(CellType(1)),
                age: 0,
            },
        );
        grid.set_cell(
            2,
            0,
            Cell {
                value: CellValue(CellType(2)),
                age: 0,
            },
        );

        let rule_a = Rule {
            id: vec![CellType(1), CellType(2)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec {
                direction: Direction::Right,
                steps: 1,
            }]],
            changes: vec![
                (0, 0, ChangeValue::Literal(5)),
                (1, 0, ChangeValue::Literal(5)),
            ],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        };

        let rule_b = Rule {
            id: vec![CellType(1), CellType(2)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec {
                direction: Direction::Left,
                steps: 1,
            }]],
            changes: vec![
                (0, 0, ChangeValue::Literal(5)),
                (1, 0, ChangeValue::Literal(5)),
            ],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        };

        let rule_index = make_rule_index(vec![rule_a, rule_b]);
        let engine = Engine::new(grid, rule_index);

        let matches = engine.detect_matches();
        assert_eq!(matches.len(), 2, "two rules match the same cells");

        let accepted = engine.arbitrate(matches);
        assert_eq!(accepted.len(), 1, "only one should be accepted");
    }
}