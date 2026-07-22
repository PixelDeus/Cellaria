use crate::grid::Grid;
use crate::storage::GridStorage;
use crate::types::{
    AffectedRegion, CellType, CellValue, Direction, OverflowAction, Rule, RuleMatch,
};
use std::collections::{HashMap, HashSet};

// === Priority queue (BinaryHeap) for RuleMatch ===

impl Ord for RuleMatch {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority
            .cmp(&other.priority)
            .then_with(|| self.age.cmp(&other.age))
            .then_with(|| other.rule_id.0.cmp(&self.rule_id.0))
            .then_with(|| other.center.cmp(&self.center))
    }
}

impl PartialOrd for RuleMatch {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Фаза 1: перенос данных из входных буферов границ в ячейки.
///
/// Для каждой граничной ячейки: если во входной очереди есть значение,
/// оно записывается в `cell.value` через [`set_cell`](Grid::set_cell).
pub fn apply_input<S: GridStorage>(grid: &mut Grid<S>) {
    let boundaries: Vec<(usize, usize)> = grid.boundary_coords().collect();
    for (x, y) in boundaries {
        let val = grid
            .get_boundary(x, y)
            .and_then(|buf| buf.input_queue.front().copied());
        if let Some(val) = val {
            if let Some(cell) = grid.get_cell(x, y) {
                let mut new_cell = cell.clone();
                new_cell.value = val;
                grid.set_cell(x, y, new_cell);
            }
            // Удаляем из input_queue
            if let Some(buf) = grid.get_boundary_mut(x, y) {
                buf.input_queue.pop_front();
            }
        }
    }
}

/// Фаза 2: поиск совпадений паттернов правил на активных ячейках.
///
/// Проходит по всем активным ячейкам, для каждой проверяет все правила
/// с соответствующим типом центра. Если паттерн совпадает, создаёт
/// [`RuleMatch`] с указанием затронутой области ([`AffectedRegion`]).
pub fn detect_matches<S: GridStorage>(
    grid: &Grid<S>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
) -> Vec<RuleMatch> {
    let mut matches = Vec::new();

    for (x, y) in grid.iter_active() {
        let cell = grid
            .get_cell(x, y)
            .expect("detect_matches: cell must exist in active_cells");
        let center_type = cell.value.0;

        let rules = match rule_index.get(&center_type) {
            Some(rules) => rules,
            None => continue,
        };

        for rule in rules {
            // Аксиома 5: min_age
            if cell.age < rule.min_age {
                continue;
            }

            let mut group_cells = Vec::with_capacity(rule.pattern.len());
            let mut pattern_matched = true;

            for &(dx, dy, expected_type) in &rule.pattern {
                let nx = x as isize + dx as isize;
                let ny = y as isize + dy as isize;

                if nx < 0 || ny < 0 {
                    pattern_matched = false;
                    break;
                }

                let nx = nx as usize;
                let ny = ny as usize;

                if nx >= grid.width() || ny >= grid.height() {
                    pattern_matched = false;
                    break;
                }

                let neighbor = grid
                    .get_cell(nx, ny)
                    .expect("detect_matches: neighbor within bounds");
                if neighbor.value.0 != expected_type {
                    pattern_matched = false;
                    break;
                }

                group_cells.push((nx, ny));
            }

            if !pattern_matched {
                continue;
            }

            let affected_region = match &rule.shift {
                Some(shift_spec) => {
                    let Direction(ddx, ddy) = shift_spec.direction;
                    let mut chain_cells = Vec::with_capacity(shift_spec.chain_length as usize);
                    let mut chain_valid = true;

                    for step in 0..shift_spec.chain_length as isize {
                        let cx = x as isize + ddx as isize * step;
                        let cy = y as isize + ddy as isize * step;

                        if cx < 0 || cy < 0 {
                            chain_valid = false;
                            break;
                        }

                        let cx = cx as usize;
                        let cy = cy as usize;

                        if cx >= grid.width() || cy >= grid.height() {
                            chain_valid = false;
                            break;
                        }

                        chain_cells.push((cx, cy));
                    }

                    if !chain_valid {
                        continue;
                    }

                    AffectedRegion::Chain {
                        group_cells,
                        result_cells: rule.result_cells.clone(),
                        chain_cells,
                        direction: shift_spec.direction,
                        fill_value: shift_spec.fill_value,
                        overflow_action: shift_spec.overflow_action.clone(),
                    }
                }
                None => AffectedRegion::LocalGroup {
                    group_cells,
                    result_cells: rule.result_cells.clone(),
                },
            };

            matches.push(RuleMatch {
                rule_id: rule.id,
                center: (x, y),
                priority: rule.priority,
                age: cell.age,
                affected_region,
            });
        }
    }

    matches
}

/// Фаза 3: жадный арбитраж конфликтов между совпадениями с использованием
/// [`BinaryHeap`] для сортировки.
///
/// Сортировка по приоритету (выше → лучше), затем по возрасту
/// (старше → лучше), затем по ID правила (меньше → лучше),
/// затем по координате центра.
pub fn arbitrate(matches: Vec<RuleMatch>) -> Vec<RuleMatch> {
    use std::collections::BinaryHeap;

    let mut heap: BinaryHeap<RuleMatch> = BinaryHeap::with_capacity(matches.len());
    for m in matches {
        heap.push(m);
    }

    let mut reserved: HashSet<(usize, usize)> = HashSet::new();
    let mut accepted: Vec<RuleMatch> = Vec::with_capacity(heap.len());

    while let Some(m) = heap.pop() {
        let coords: Vec<(usize, usize)> = match &m.affected_region {
            AffectedRegion::LocalGroup { group_cells, .. } => group_cells.clone(),
            AffectedRegion::Chain {
                group_cells,
                chain_cells,
                ..
            } => {
                let mut all = group_cells.clone();
                all.extend(chain_cells.iter().copied());
                all
            }
        };

        if !coords.iter().any(|c| reserved.contains(c)) {
            for &c in &coords {
                reserved.insert(c);
            }
            accepted.push(m);
        }
    }

    accepted
}

/// Применить сдвиг цепочки ячеек.
///
/// Сдвигает значения от дальнего конца к ближнему, заполняет первую
/// ячейку `fill_value` и обрабатывает вытолкнутое значение согласно
/// `overflow_action`.
pub(crate) fn apply_shift<S: GridStorage>(
    grid: &mut Grid<S>,
    chain_cells: &[(usize, usize)],
    fill_value: CellValue,
    overflow_action: &OverflowAction,
) {
    if chain_cells.is_empty() {
        return;
    }

    let last_idx = chain_cells.len() - 1;
    let (lx, ly) = chain_cells[last_idx];
    let overflow_value = grid
        .get_cell(lx, ly)
        .expect("apply_shift: last chain cell must exist")
        .value;

    // Сдвиг от дальнего конца к ближнему
    for i in (1..chain_cells.len()).rev() {
        let (curr_x, curr_y) = chain_cells[i];
        let (prev_x, prev_y) = chain_cells[i - 1];
        let prev_value = grid
            .get_cell(prev_x, prev_y)
            .expect("apply_shift: prev cell must exist")
            .value;
        let mut new_cell = grid
            .get_cell(curr_x, curr_y)
            .expect("apply_shift: curr cell must exist")
            .clone();
        new_cell.value = prev_value;
        grid.set_cell(curr_x, curr_y, new_cell);
    }

    // fill_value в ближнюю ячейку
    let (first_x, first_y) = chain_cells[0];
    {
        let mut new_cell = grid
            .get_cell(first_x, first_y)
            .expect("apply_shift: first cell must exist")
            .clone();
        new_cell.value = fill_value;
        grid.set_cell(first_x, first_y, new_cell);
    }

    // Обработка вытолкнутого
    match overflow_action {
        OverflowAction::Discard => {}
        OverflowAction::WriteValue(v) => {
            let mut new_cell = grid
                .get_cell(lx, ly)
                .expect("apply_shift: last cell must exist for write_value")
                .clone();
            new_cell.value = *v;
            grid.set_cell(lx, ly, new_cell);
        }
        OverflowAction::OutputToChannel(channel_id) => {
            // Устанавливаем pending_output в граничном буфере
            if let Some(buf) = grid.get_boundary_mut(lx, ly) {
                if buf.channel == *channel_id {
                    buf.pending_output = Some(overflow_value);
                }
            }
        }
    }
}

/// Фаза 4: применение принятых правил к решётке.
///
/// Для каждого принятого совпадения:
/// - [`LocalGroup`](AffectedRegion::LocalGroup): запись `result_cells` в `group_cells`.
/// - [`Chain`](AffectedRegion::Chain): сначала сдвиг цепочки ([`apply_shift`]),
///   затем запись `result_cells` в `group_cells`.
///
/// После применения обновляет возраст ячеек: затронутые → 0, остальные +1.
///
/// **Важно:** если `result_cells.len() < group_cells.len()`, недостающие
/// ячейки заполняются [`CellValue::default()`].
pub fn apply_matches<S: GridStorage>(grid: &mut Grid<S>, accepted: &[RuleMatch]) {
    let mut affected: HashSet<(usize, usize)> = HashSet::new();

    for match_ in accepted {
        match &match_.affected_region {
            AffectedRegion::LocalGroup {
                group_cells,
                result_cells,
            } => {
                let count = group_cells.len().min(result_cells.len());
                for i in 0..count {
                    let (x, y) = group_cells[i];
                    if let Some(cell) = grid.get_cell(x, y) {
                        let mut new_cell = cell.clone();
                        new_cell.value = result_cells[i];
                        grid.set_cell(x, y, new_cell);
                        affected.insert((x, y));
                    }
                }
                // Если result_cells короче group_cells — заполняем дефолтом
                for &(x, y) in &group_cells[count..] {
                    if let Some(cell) = grid.get_cell(x, y) {
                        let mut new_cell = cell.clone();
                        new_cell.value = CellValue::default();
                        grid.set_cell(x, y, new_cell);
                        affected.insert((x, y));
                    }
                }
            }
            AffectedRegion::Chain {
                group_cells,
                result_cells,
                chain_cells,
                fill_value,
                overflow_action,
                ..
            } => {
                apply_shift(grid, chain_cells, *fill_value, overflow_action);

                let count = result_cells.len().min(group_cells.len());
                for i in 0..count {
                    let (x, y) = group_cells[i];
                    if let Some(cell) = grid.get_cell(x, y) {
                        let mut new_cell = cell.clone();
                        new_cell.value = result_cells[i];
                        grid.set_cell(x, y, new_cell);
                        affected.insert((x, y));
                    }
                }
                // Если result_cells короче group_cells — заполняем дефолтом
                for &(x, y) in &group_cells[count..] {
                    if let Some(cell) = grid.get_cell(x, y) {
                        let mut new_cell = cell.clone();
                        new_cell.value = CellValue::default();
                        grid.set_cell(x, y, new_cell);
                        affected.insert((x, y));
                    }
                }

                for &coord in chain_cells {
                    affected.insert(coord);
                }
            }
        }
    }

    // Обновляем возраст: затронутые → 0, остальные +1
    let active: Vec<_> = grid.iter_active().collect();
    for (x, y) in active {
        if let Some(cell) = grid.get_cell(x, y) {
            let mut new_cell = cell.clone();
            if affected.contains(&(x, y)) {
                new_cell.age = 0;
            } else {
                new_cell.age += 1;
            }
            grid.set_cell(x, y, new_cell);
        }
    }
}

/// Фаза 5: сброс `pending_output` в выходные очереди каналов.
///
/// Для каждой граничной ячейки: если есть ожидающий вывода данных
/// (`pending_output`), он перемещается в `output_queue`.
pub fn flush_output<S: GridStorage>(grid: &mut Grid<S>) {
    let coords: Vec<(usize, usize)> = grid.boundary_coords().collect();
    for (x, y) in coords {
        if let Some(buf) = grid.get_boundary_mut(x, y) {
            if let Some(val) = buf.pending_output.take() {
                buf.output_queue.push_back(val);
            }
        }
    }
}

/// Тип hook-а для отслеживания принятых RuleMatch в Engine.
pub type MatchHook = Option<Box<dyn Fn(&RuleMatch)>>;

/// Движок симуляции с опциональным hook-ом для отслеживания RuleMatch.
///
/// Hook вызывается для каждого принятого совпадения после арбитража,
/// но до применения правил.
pub struct Engine<S: GridStorage> {
    /// Опциональный hook, вызываемый для каждого принятого RuleMatch.
    pub on_match: MatchHook,
    _marker: std::marker::PhantomData<S>,
}

impl<S: GridStorage> Engine<S> {
    /// Создать новый движок без hook-а.
    pub fn new() -> Self {
        Self {
            on_match: None,
            _marker: std::marker::PhantomData,
        }
    }

    /// Создать движок с hook-ом.
    pub fn with_hook(on_match: Box<dyn Fn(&RuleMatch)>) -> Self {
        Self {
            on_match: Some(on_match),
            _marker: std::marker::PhantomData,
        }
    }

    /// Выполнить один тик симуляции.
    ///
    /// Последовательно выполняет пять фаз:
    /// 1. [`apply_input`] — ввод данных
    /// 2. [`detect_matches`] — обнаружение совпадений
    /// 3. [`arbitrate`] — арбитраж
    /// 4. [`apply_matches`] — применение правил
    /// 5. [`flush_output`] — вывод данных
    ///
    /// Если установлен `on_match`, вызывается для каждого принятого совпадения
    /// перед применением.
    ///
    /// Возвращает список принятых совпадений за этот тик.
    pub fn run_tick(
        &self,
        grid: &mut Grid<S>,
        rule_index: &HashMap<CellType, Vec<Rule>>,
    ) -> Vec<RuleMatch> {
        apply_input(grid);
        let matches = detect_matches(grid, rule_index);
        let accepted = arbitrate(matches);

        // Hook
        if let Some(ref hook) = self.on_match {
            for m in &accepted {
                hook(m);
            }
        }

        apply_matches(grid, &accepted);
        flush_output(grid);
        accepted
    }
}

impl<S: GridStorage> Default for Engine<S> {
    fn default() -> Self {
        Self::new()
    }
}

/// Выполнить один тик симуляции (free-function для обратной совместимости).
///
/// Делегирует вызов [`Engine::run_tick`] c пустым hook-ом.
pub fn run_tick<S: GridStorage>(
    grid: &mut Grid<S>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
) -> Vec<RuleMatch> {
    let engine = Engine::<S>::new();
    engine.run_tick(grid, rule_index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::VecStorage;
    use crate::types::{Cell, RuleId, ShiftSpec, BoundaryBuffer};
    use std::collections::VecDeque;

    // Helper: create a simple grid with given cells
    fn make_grid(width: usize, height: usize, cells: &[(usize, usize, u8)]) -> Grid<VecStorage> {
        let storage = VecStorage {
            cells: (0..height)
                .flat_map(|y| {
                    (0..width).map(move |x| {
                        let val = cells
                            .iter()
                            .find(|&&(cx, cy, _)| cx == x && cy == y)
                            .map(|&(_, _, t)| t)
                            .unwrap_or(0);
                        Cell {
                            value: CellValue(CellType(val)),
                            age: 0,
                        }
                    })
                })
                .collect(),
            width,
            height,
        };
        Grid::new(storage)
    }

    // Helper: create a rule index with one rule
    fn make_rule_index(rules: Vec<Rule>) -> HashMap<CellType, Vec<Rule>> {
        let mut index: HashMap<CellType, Vec<Rule>> = HashMap::new();
        for rule in rules {
            let center = rule
                .pattern
                .iter()
                .find(|(dx, dy, _)| *dx == 0 && *dy == 0)
                .map(|&(_, _, ct)| ct)
                .expect("make_rule_index: rule must have center (0,0)");
            index.entry(center).or_default().push(rule);
        }
        for rules in index.values_mut() {
            rules.sort_by(|a, b| b.priority.cmp(&a.priority));
        }
        index
    }

    // ---- min_age tests ----

    #[test]
    fn test_detect_min_age_not_met() {
        let grid = make_grid(2, 1, &[(0, 0, 1)]);
        let rule = Rule {
            id: RuleId(1),
            priority: 10,
            min_age: 5,
            pattern: vec![(0, 0, CellType(1))],
            result_cells: vec![CellValue(CellType(2))],
            shift: None,
        };
        let index = make_rule_index(vec![rule]);
        let matches = detect_matches(&grid, &index);
        assert_eq!(matches.len(), 0, "min_age=5, age=0 → no match");
    }

    #[test]
    fn test_detect_min_age_met() {
        let storage = VecStorage {
            cells: vec![Cell {
                value: CellValue(CellType(1)),
                age: 5,
            }],
            width: 1,
            height: 1,
        };
        let grid = Grid::new(storage);
        let rule = Rule {
            id: RuleId(1),
            priority: 10,
            min_age: 5,
            pattern: vec![(0, 0, CellType(1))],
            result_cells: vec![CellValue(CellType(2))],
            shift: None,
        };
        let index = make_rule_index(vec![rule]);
        let matches = detect_matches(&grid, &index);
        assert_eq!(matches.len(), 1, "min_age=5, age=5 → match");
    }

    #[test]
    fn test_run_tick_min_age_cleanup() {
        let mut grid = make_grid(1, 1, &[(0, 0, 1)]);
        let rule = Rule {
            id: RuleId(1),
            priority: 10,
            min_age: 3,
            pattern: vec![(0, 0, CellType(1))],
            result_cells: vec![CellValue(CellType(0))],
            shift: None,
        };
        let index = make_rule_index(vec![rule]);

        // Тики 1-3: age < 3 → ничего
        for _ in 0..3 {
            run_tick(&mut grid, &index);
            assert_eq!(grid.get_cell(0, 0).unwrap().value, CellValue(CellType(1)));
        }
        // Тик 4: age = 3 >= 3 → срабатывает
        run_tick(&mut grid, &index);
        assert_eq!(
            grid.get_cell(0, 0).unwrap().value,
            CellValue(CellType(0)),
            "min_age=3: cell should be cleared on tick 4"
        );
        assert_eq!(
            grid.get_cell(0, 0).unwrap().age,
            0,
            "Affected cell age reset to 0"
        );
    }

    // ---- detect_matches tests ----

    #[test]
    fn test_detect_simple_pattern() {
        let grid = make_grid(3, 1, &[(0, 0, 1), (1, 0, 2)]);
        let rule = Rule {
            id: RuleId(1),
            priority: 10,
            min_age: 0,
            pattern: vec![(0, 0, CellType(1)), (1, 0, CellType(2))],
            result_cells: vec![CellValue(CellType(3)), CellValue(CellType(4))],
            shift: None,
        };
        let index = make_rule_index(vec![rule]);
        let matches = detect_matches(&grid, &index);
        assert_eq!(matches.len(), 1, "Should detect one match");
        assert_eq!(matches[0].center, (0, 0));
    }

    #[test]
    fn test_detect_no_match() {
        let grid = make_grid(3, 1, &[(0, 0, 1), (1, 0, 3)]);
        let rule = Rule {
            id: RuleId(1),
            priority: 10,
            min_age: 0,
            pattern: vec![(0, 0, CellType(1)), (1, 0, CellType(2))],
            result_cells: vec![CellValue(CellType(3)), CellValue(CellType(4))],
            shift: None,
        };
        let index = make_rule_index(vec![rule]);
        let matches = detect_matches(&grid, &index);
        assert_eq!(matches.len(), 0, "Should not detect match (type mismatch)");
    }

    #[test]
    fn test_detect_out_of_bounds() {
        let grid = make_grid(1, 1, &[(0, 0, 1)]);
        let rule = Rule {
            id: RuleId(1),
            priority: 10,
            min_age: 0,
            pattern: vec![(0, 0, CellType(1)), (1, 0, CellType(2))],
            result_cells: vec![CellValue(CellType(3)), CellValue(CellType(4))],
            shift: None,
        };
        let index = make_rule_index(vec![rule]);
        let matches = detect_matches(&grid, &index);
        assert_eq!(matches.len(), 0, "Should not detect match (out of bounds)");
    }

    #[test]
    fn test_detect_empty_grid() {
        let grid = make_grid(0, 0, &[]);
        let index = HashMap::new();
        let matches = detect_matches(&grid, &index);
        assert_eq!(matches.len(), 0, "Empty grid should have no matches");
    }

    // ---- arbitrate tests ----

    #[test]
    fn test_arbitrate_priority() {
        let matches = vec![
            RuleMatch {
                rule_id: RuleId(2),
                priority: 5,
                age: 0,
                center: (0, 0),
                affected_region: AffectedRegion::LocalGroup {
                    group_cells: vec![(0, 0)],
                    result_cells: vec![CellValue(CellType(1))],
                },
            },
            RuleMatch {
                rule_id: RuleId(1),
                priority: 10,
                age: 0,
                center: (0, 0),
                affected_region: AffectedRegion::LocalGroup {
                    group_cells: vec![(0, 0)],
                    result_cells: vec![CellValue(CellType(2))],
                },
            },
        ];
        let accepted = arbitrate(matches);
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].rule_id, RuleId(1), "Higher priority should win");
    }

    #[test]
    fn test_arbitrate_no_conflict() {
        let matches = vec![
            RuleMatch {
                rule_id: RuleId(1),
                priority: 10,
                age: 0,
                center: (0, 0),
                affected_region: AffectedRegion::LocalGroup {
                    group_cells: vec![(0, 0)],
                    result_cells: vec![CellValue(CellType(1))],
                },
            },
            RuleMatch {
                rule_id: RuleId(2),
                priority: 5,
                age: 0,
                center: (2, 2),
                affected_region: AffectedRegion::LocalGroup {
                    group_cells: vec![(2, 2)],
                    result_cells: vec![CellValue(CellType(2))],
                },
            },
        ];
        let accepted = arbitrate(matches);
        assert_eq!(accepted.len(), 2, "No conflict, both should be accepted");
    }

    // ---- apply_matches tests ----

    #[test]
    fn test_apply_local_group() {
        let mut grid = make_grid(2, 1, &[(0, 0, 1), (1, 0, 2)]);
        let accepted = vec![RuleMatch {
            rule_id: RuleId(1),
            priority: 10,
            age: 0,
            center: (0, 0),
            affected_region: AffectedRegion::LocalGroup {
                group_cells: vec![(0, 0), (1, 0)],
                result_cells: vec![CellValue(CellType(3)), CellValue(CellType(4))],
            },
        }];
        apply_matches(&mut grid, &accepted);
        assert_eq!(grid.get_cell(0, 0).unwrap().value, CellValue(CellType(3)));
        assert_eq!(grid.get_cell(1, 0).unwrap().value, CellValue(CellType(4)));
    }

    #[test]
    fn test_apply_chain() {
        let mut grid = make_grid(3, 1, &[(0, 0, 1), (1, 0, 2), (2, 0, 5)]);
        let accepted = vec![RuleMatch {
            rule_id: RuleId(1),
            priority: 10,
            age: 0,
            center: (0, 0),
            affected_region: AffectedRegion::Chain {
                group_cells: vec![(0, 0), (1, 0)],
                result_cells: vec![CellValue(CellType(3)), CellValue(CellType(4))],
                chain_cells: vec![(0, 0), (1, 0), (2, 0)],
                direction: Direction::EAST,
                fill_value: CellValue(CellType(0)),
                overflow_action: OverflowAction::Discard,
            },
        }];
        apply_matches(&mut grid, &accepted);
        assert_eq!(
            grid.get_cell(0, 0).unwrap().value,
            CellValue(CellType(3)),
        );
        assert_eq!(
            grid.get_cell(1, 0).unwrap().value,
            CellValue(CellType(4)),
        );
        assert_eq!(
            grid.get_cell(2, 0).unwrap().value,
            CellValue(CellType(2)),
        );
    }

    #[test]
    fn test_apply_matches_empty() {
        let mut grid = make_grid(1, 1, &[(0, 0, 1)]);
        apply_matches(&mut grid, &[]);
        assert_eq!(
            grid.get_cell(0, 0).unwrap().value,
            CellValue(CellType(1)),
            "No changes should occur"
        );
    }

    // ---- apply_shift tests ----

    #[test]
    fn test_shift_discard() {
        let mut grid = make_grid(3, 1, &[(0, 0, 1), (1, 0, 2), (2, 0, 3)]);
        let chain = vec![(0, 0), (1, 0), (2, 0)];
        apply_shift(
            &mut grid,
            &chain,
            CellValue(CellType(0)),
            &OverflowAction::Discard,
        );
        assert_eq!(grid.get_cell(0, 0).unwrap().value, CellValue(CellType(0)));
        assert_eq!(grid.get_cell(1, 0).unwrap().value, CellValue(CellType(1)));
        assert_eq!(grid.get_cell(2, 0).unwrap().value, CellValue(CellType(2)));
    }

    #[test]
    fn test_shift_write_value() {
        let mut grid = make_grid(2, 1, &[(0, 0, 1), (1, 0, 2)]);
        let chain = vec![(0, 0), (1, 0)];
        apply_shift(
            &mut grid,
            &chain,
            CellValue(CellType(0)),
            &OverflowAction::WriteValue(CellValue(CellType(9))),
        );
        assert_eq!(grid.get_cell(0, 0).unwrap().value, CellValue(CellType(0)));
        assert_eq!(
            grid.get_cell(1, 0).unwrap().value,
            CellValue(CellType(9)),
        );
    }

    #[test]
    fn test_output_to_channel() {
        let mut grid = make_grid(3, 1, &[(0, 0, 1), (1, 0, 2), (2, 0, 3)]);
        // Устанавливаем граничный буфер через новый API
        grid.set_boundary(2, 0, BoundaryBuffer {
            channel: 0,
            input_queue: VecDeque::new(),
            output_queue: VecDeque::new(),
            pending_output: None,
            max_queue_depth: 16,
        });
        let chain = vec![(0, 0), (1, 0), (2, 0)];
        apply_shift(
            &mut grid,
            &chain,
            CellValue(CellType(0)),
            &OverflowAction::OutputToChannel(0),
        );
        assert_eq!(
            grid.get_boundary(2, 0).unwrap().pending_output,
            Some(CellValue(CellType(3)))
        );
    }

    // ---- run_tick integration test ----

    #[test]
    fn test_run_tick_simple() {
        let mut grid = make_grid(2, 1, &[(0, 0, 1)]);
        let rule = Rule {
            id: RuleId(1),
            priority: 10,
            min_age: 0,
            pattern: vec![(0, 0, CellType(1))],
            result_cells: vec![CellValue(CellType(3))],
            shift: None,
        };
        let index = make_rule_index(vec![rule]);
        let accepted = run_tick(&mut grid, &index);
        assert_eq!(accepted.len(), 1, "One rule should fire");
        assert_eq!(
            grid.get_cell(0, 0).unwrap().value,
            CellValue(CellType(3)),
        );
        assert_eq!(
            grid.get_cell(0, 0).unwrap().age,
            0,
        );
    }

    #[test]
    fn test_run_tick_age_increment() {
        let mut grid = make_grid(2, 1, &[(0, 0, 1)]);
        let rule = Rule {
            id: RuleId(1),
            priority: 10,
            min_age: 0,
            pattern: vec![(0, 0, CellType(2))],
            result_cells: vec![CellValue(CellType(3))],
            shift: None,
        };
        let index = make_rule_index(vec![rule]);
        run_tick(&mut grid, &index);
        assert_eq!(
            grid.get_cell(0, 0).unwrap().age,
            1,
        );
    }

    #[test]
    fn test_run_tick_overflow_write_greater_chain() {
        let mut grid = make_grid(4, 1, &[(0, 0, 1), (1, 0, 2)]);
        let rule = Rule {
            id: RuleId(1),
            priority: 10,
            min_age: 0,
            pattern: vec![(0, 0, CellType(1)), (1, 0, CellType(2))],
            result_cells: vec![CellValue(CellType(0)), CellValue(CellType(1))],
            shift: Some(ShiftSpec {
                direction: Direction::EAST,
                chain_length: 3,
                fill_value: CellValue(CellType(0)),
                overflow_action: OverflowAction::WriteValue(CellValue(CellType(6))),
            }),
        };
        let index = make_rule_index(vec![rule]);
        let _accepted = run_tick(&mut grid, &index);

        assert_eq!(grid.get_cell(0, 0).unwrap().value, CellValue(CellType(0)));
        assert_eq!(grid.get_cell(1, 0).unwrap().value, CellValue(CellType(1)));
        assert_eq!(grid.get_cell(2, 0).unwrap().value, CellValue(CellType(6)));
        assert_eq!(grid.get_cell(3, 0).unwrap().value, CellValue(CellType(0)));
    }

    // ---- boundary buffer tests ----

    /// Тест: ввод из граничного буфера записывается в ячейку.
    #[test]
    fn test_apply_input_from_boundary() {
        let mut grid = make_grid(3, 1, &[(0, 0, 0), (1, 0, 0), (2, 0, 0)]);
        // Устанавливаем граничный буфер на (0,0) с данными во входной очереди
        grid.set_boundary(0, 0, BoundaryBuffer {
            channel: 0,
            input_queue: vec![CellValue(CellType(42))].into(),
            output_queue: VecDeque::new(),
            pending_output: None,
            max_queue_depth: 16,
        });
        apply_input(&mut grid);
        // Ячейка (0,0) должна получить значение 42
        assert_eq!(
            grid.get_cell(0, 0).unwrap().value,
            CellValue(CellType(42)),
            "Input from boundary buffer should write to cell"
        );
        // input_queue должна быть пуста после применения
        assert!(
            grid.get_boundary(0, 0).unwrap().input_queue.is_empty(),
            "Input queue should be drained after apply_input"
        );
    }

    /// Тест: output_queue заполняется через pending_output после flush.
    #[test]
    fn test_flush_output_to_boundary() {
        let mut grid = make_grid(1, 1, &[(0, 0, 0)]);
        grid.set_boundary(0, 0, BoundaryBuffer {
            channel: 0,
            input_queue: VecDeque::new(),
            output_queue: VecDeque::new(),
            pending_output: Some(CellValue(CellType(99))),
            max_queue_depth: 16,
        });
        flush_output(&mut grid);
        assert_eq!(
            grid.get_boundary(0, 0).unwrap().output_queue.front(),
            Some(&CellValue(CellType(99))),
            "Flush should move pending_output to output_queue"
        );
        assert!(
            grid.get_boundary(0, 0).unwrap().pending_output.is_none(),
            "pending_output should be None after flush"
        );
    }

    /// Тест: overflow в канал устанавливает pending_output в граничном буфере.
    #[test]
    fn test_overflow_to_channel_sets_pending() {
        let mut grid = make_grid(3, 1, &[(0, 0, 1), (1, 0, 2), (2, 0, 3)]);
        grid.set_boundary(2, 0, BoundaryBuffer {
            channel: 0,
            input_queue: VecDeque::new(),
            output_queue: VecDeque::new(),
            pending_output: None,
            max_queue_depth: 16,
        });
        let chain = vec![(0, 0), (1, 0), (2, 0)];
        apply_shift(
            &mut grid,
            &chain,
            CellValue(CellType(0)),
            &OverflowAction::OutputToChannel(0),
        );
        // После сдвига pending_output должен содержать вытолкнутое значение
        assert_eq!(
            grid.get_boundary(2, 0).unwrap().pending_output,
            Some(CellValue(CellType(3))),
            "Overflow to channel should set pending_output"
        );
        // После flush pending_output переходит в output_queue
        flush_output(&mut grid);
        assert_eq!(
            grid.get_boundary(2, 0).unwrap().output_queue.front(),
            Some(&CellValue(CellType(3))),
            "Flush should move overflow value to output_queue"
        );
    }

    /// Тест: input_queue принимает несколько значений, все применяются по очереди.
    #[test]
    fn test_apply_input_multiple_values() {
        let mut grid = make_grid(1, 1, &[(0, 0, 5)]);
        grid.set_boundary(0, 0, BoundaryBuffer {
            channel: 0,
            input_queue: vec![
                CellValue(CellType(10)),
                CellValue(CellType(20)),
                CellValue(CellType(30)),
            ].into(),
            output_queue: VecDeque::new(),
            pending_output: None,
            max_queue_depth: 16,
        });
        // Первый тик — применяем 10
        apply_input(&mut grid);
        assert_eq!(grid.get_cell(0, 0).unwrap().value, CellValue(CellType(10)));
        // Второй тик — применяем 20
        apply_input(&mut grid);
        assert_eq!(grid.get_cell(0, 0).unwrap().value, CellValue(CellType(20)));
        // Третий тик — применяем 30
        apply_input(&mut grid);
        assert_eq!(grid.get_cell(0, 0).unwrap().value, CellValue(CellType(30)));
        // Четвёртый тик — больше данных нет, значение не меняется
        apply_input(&mut grid);
        assert_eq!(
            grid.get_cell(0, 0).unwrap().value,
            CellValue(CellType(30)),
            "No more input — cell should keep last value"
        );
    }

    /// Интеграционный тест детерминизма с ChunkStorage.
    #[test]
    fn test_run_tick_deterministic_chunk_storage() {
        use crate::storage::ChunkStorage;

        let make_chunk_grid = || -> Grid<ChunkStorage> {
            let mut grid = Grid::<ChunkStorage>::new(ChunkStorage::new());
            let init_cells = [(0usize, 0usize, 1u8), (1, 0, 2), (5, 0, 1), (6, 0, 2)];
            for &(x, y, t) in &init_cells {
                grid.set_cell(
                    x,
                    y,
                    Cell {
                        value: CellValue(CellType(t)),
                        age: 0,
                    },
                );
            }
            grid
        };

        let rule = Rule {
            id: RuleId(1),
            priority: 10,
            min_age: 0,
            pattern: vec![(0, 0, CellType(1)), (1, 0, CellType(2))],
            result_cells: vec![CellValue(CellType(3)), CellValue(CellType(4))],
            shift: None,
        };
        let index = make_rule_index(vec![rule]);

        let n_runs = 10;
        let mut snapshots: Vec<Vec<(usize, usize, u8)>> = Vec::new();

        for _ in 0..n_runs {
            let mut grid = make_chunk_grid();
            for _ in 0..3 {
                run_tick(&mut grid, &index);
            }
            let mut cells: Vec<_> = grid
                .iter_active()
                .filter_map(|(x, y)| {
                    let cell = grid.get_cell(x, y).unwrap();
                    if cell.value.0 .0 != 0 {
                        Some((x, y, cell.value.0 .0))
                    } else {
                        None
                    }
                })
                .collect();
            cells.sort();
            snapshots.push(cells);
        }

        for (i, snap) in snapshots.iter().enumerate() {
            assert_eq!(
                *snap, snapshots[0],
                "Run {} produced different state — not deterministic!",
                i
            );
        }
    }
}