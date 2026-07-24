use std::collections::{HashMap, HashSet};

use crate::conflict_analyzer::{compute_affected_cells, ConflictGraph};
use crate::grid::Grid;
use crate::storage::GridStorage;
use crate::types::{
    AffectedRegion, Cell, CellType, CellValue, Direction, OverflowAction, Rule,
    RuleMatch, ShiftSpec,
};

// ============================================================================
// Результат анализа завершимости
// ============================================================================

/// Результат анализа завершимости.
#[derive(Debug, PartialEq, Eq)]
pub enum TerminationVerdict {
    /// Доказано: симуляция завершится
    Terminates,
    /// Обнаружен цикл: состояние повторилось
    MayDiverge,
    /// Не удалось определить за отведённое время
    Unknown,
}

// ============================================================================
// Результат проверки композиции
// ============================================================================

/// Результат проверки композиции двух conflict-free наборов правил.
#[derive(Debug, PartialEq, Eq)]
pub enum CompositionVerdict {
    /// Композиция безопасна — конфликт-граф объединения пуст
    Safe,
    /// Композиция небезопасна — найдены конфликтующие пары
    Unsafe(Vec<(usize, usize)>),
}

// ============================================================================
// Движок симуляции
// ============================================================================

/// Структура движка, инкапсулирующая логику сопоставления и применения правил.
#[derive(Clone)]
pub struct Engine<S: GridStorage> {
    grid: Grid<S>,
    rule_index: HashMap<CellType, Vec<Rule>>,
    pending_boundary: Vec<(u32, Cell)>,
}

impl<S: GridStorage> Engine<S> {
    pub fn new(grid: Grid<S>, rule_index: HashMap<CellType, Vec<Rule>>) -> Self {
        Self {
            grid,
            rule_index,
            pending_boundary: Vec::new(),
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

    /// Построить граф конфликтов для текущего набора правил.
    ///
    /// Если граф пуст, арбитраж можно пропустить —
    /// все совпадения применяются одновременно.
    pub fn analyze_conflicts(&self) -> ConflictGraph {
        let rules: Vec<Rule> = self.rule_index.values().flatten().cloned().collect();
        ConflictGraph::build(&rules)
    }

    // ========================================================================
    // IO: ввод-вывод через граничные буферы
    // ========================================================================

    /// Поместить значение во входной буфер границы.
    pub fn push_input(&mut self, channel: u32, value: u8) {
        let cell = Cell::new(value);
        for (_, buf) in self.grid.iter_boundaries_mut() {
            if buf.direction == "input" {
                buf.enqueue(channel, cell);
                return;
            }
        }
    }

    /// Извлечь все данные из выходных буферов границ.
    pub fn pop_output(&mut self) -> Vec<(u32, Cell)> {
        let mut outputs: Vec<(u32, Cell)> = Vec::new();
        let coords: Vec<(usize, usize)> = self.grid.boundary_coords().collect();
        for (x, y) in coords {
            let is_output = self.grid.get_boundary(x, y)
                .map(|b| b.direction == "output")
                .unwrap_or(false);
            if !is_output {
                continue;
            }
            if let Some(buf) = self.grid.get_boundary_mut(x, y) {
                let channels: Vec<u32> = buf.queues.keys().copied().collect();
                for ch in channels {
                    let cells = buf.dequeue(ch);
                    for cell in cells {
                        outputs.push((ch, cell));
                    }
                }
            }
        }
        outputs
    }

    /// Применить входные данные из input-буферов границ.
    fn apply_input(&mut self) {
        let coords: Vec<(usize, usize)> = self.grid.boundary_coords().collect();
        for (x, y) in coords {
            let is_input = self.grid.get_boundary(x, y)
                .map(|b| b.direction == "input")
                .unwrap_or(false);
            if !is_input {
                continue;
            }
            let cells = self.grid.get_boundary_mut(x, y)
                .map(|b| b.dequeue(0))
                .unwrap_or_default();
            if let Some(cell) = cells.into_iter().last() {
                self.grid.set_cell(x, y, cell);
            }
        }
    }

    /// Извлечь данные из output-буферов границ во внутренний буфер.
    fn drain_output(&mut self) -> Vec<(u32, Cell)> {
        self.pop_output()
    }

    // ========================================================================
    // Фаза 1: Сопоставление (detect_matches)
    // ========================================================================

    /// Обнаружить все совпадения правил на решётке.
    pub fn detect_matches(&self) -> Vec<RuleMatch> {
        let mut matches: Vec<RuleMatch> = Vec::new();

        let w = self.grid.width();
        let h = self.grid.height();

        let cells_to_check: Vec<(usize, usize)> = {
            let mut has_active_only = false;
            for rules in self.rule_index.values() {
                for rule in rules {
                    if rule.active_only {
                        has_active_only = true;
                        break;
                    }
                }
                if has_active_only {
                    break;
                }
            }

            if has_active_only || self.grid.storage.bounds().is_none() {
                let mut coords = HashSet::new();
                for (x, y) in self.grid.iter_active() {
                    for dx in -2i32..=2i32 {
                        for dy in -2i32..=2i32 {
                            let nx = x as i32 + dx;
                            let ny = y as i32 + dy;
                            if nx >= 0 && ny >= 0 {
                                coords.insert((nx as usize, ny as usize));
                            }
                        }
                    }
                }
                coords.into_iter().collect()
            } else {
                let mut coords = Vec::with_capacity(w * h);
                for y in 0..h {
                    for x in 0..w {
                        coords.push((x, y));
                    }
                }
                coords
            }
        };

        for (cell_type, rules) in &self.rule_index {
            for rule in rules {
                for &(cx, cy) in &cells_to_check {
                    if let Some(center_cell) = self.grid.get_cell(cx, cy) {
                        if center_cell.value.0 != *cell_type {
                            continue;
                        }
                        if center_cell.age < rule.min_age {
                            continue;
                        }
                    } else {
                        continue;
                    }

                    if rule.active_only {
                        let center_cell = self.grid.get_cell(cx, cy)
                            .expect("center cell should exist after checking");
                        let default_cell = Cell::default();
                        if center_cell.value == default_cell.value && center_cell.age == 0 {
                            continue;
                        }
                    }

                    let rule_id: Vec<CellType> = rule.id.clone();
                    let mut pattern: Vec<Vec<u8>> = Vec::new();
                    let row: Vec<u8> = rule_id.iter().map(|ct| ct.0).collect();
                    pattern.push(row);

                    let mut matched = true;

                    for (i, expected_type) in rule_id.iter().enumerate() {
                        let px = cx as i32 + i as i32;
                        let py = cy as i32;

                        if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                            matched = false;
                            break;
                        }

                        let cell = self.grid.get_cell(px as usize, py as usize);
                        match cell {
                            Some(c) if c.value.0 == *expected_type => {}
                            _ => {
                                matched = false;
                                break;
                            }
                        }
                    }

                    if matched {
                        matches.push(RuleMatch {
                            x: cx as u32,
                            y: cy as u32,
                            pattern: pattern.clone(),
                            rule_id: rule_id.clone(),
                        });
                    }
                }
            }
        }

        matches
    }

    // ========================================================================
    // Фаза 2: Арбитраж
    // ========================================================================

    /// Выбрать непротиворечивый набор совпадений.
    ///
    /// Арбитраж проверяет пересечение ПОЛНЫХ affected regions (паттерн +
    /// позиция сдвига + изменения), а не только паттерна. Это гарантирует,
    /// что два совпадения не будут конфликтовать при применении, даже если
    /// их паттерны не пересекаются, но их изменения затрагивают одни и те же
    /// ячейки.
    pub fn arbitrate(&self, all_matches: Vec<RuleMatch>) -> Vec<RuleMatch> {
        if all_matches.is_empty() {
            return Vec::new();
        }

        let mut accepted: Vec<RuleMatch> = Vec::new();
        let mut used_cells: HashSet<(u32, u32)> = HashSet::new();

        let mut sorted = all_matches;
        sorted.sort_by(|a, b| {
            let priority_a = self.get_priority(&a.rule_id);
            let priority_b = self.get_priority(&b.rule_id);
            priority_b.cmp(&priority_a).then_with(|| {
                let age_a = self
                    .grid
                    .get_cell(a.x as usize, a.y as usize)
                    .map(|c| c.age)
                    .unwrap_or(0);
                let age_b = self
                    .grid
                    .get_cell(b.x as usize, b.y as usize)
                    .map(|c| c.age)
                    .unwrap_or(0);
                age_b.cmp(&age_a)
            })
        });

        for m in sorted {
            // Вычисляем полный набор affected cells для этого совпадения
            let affected = self.get_match_affected_cells(&m);
            let mut conflict = false;

            for &(px, py) in &affected {
                if px >= 0 && py >= 0 {
                    let coord = (px as u32, py as u32);
                    if used_cells.contains(&coord) {
                        conflict = true;
                        break;
                    }
                }
            }

            if !conflict {
                for &(px, py) in &affected {
                    if px >= 0 && py >= 0 {
                        used_cells.insert((px as u32, py as u32));
                    }
                }
                accepted.push(m);
            }
        }

        accepted
    }

    /// Вычислить полный набор affected cells для совпадения.
    ///
    /// Использует `compute_affected_cells` из conflict_analyzer для получения
    /// относительных координат, затем сдвигает их на позицию совпадения.
    fn get_match_affected_cells(&self, m: &RuleMatch) -> Vec<(i32, i32)> {
        let rule = self.find_rule(&m.rule_id);
        if let Some(rule) = rule {
            let relative = compute_affected_cells(rule);
            relative
                .iter()
                .map(|&(dx, dy)| (m.x as i32 + dx, m.y as i32 + dy))
                .collect()
        } else {
            // Если правило не найдено, используем только паттерн
            let mut cells = Vec::new();
            for (i, _) in m.rule_id.iter().enumerate() {
                cells.push((m.x as i32 + i as i32, m.y as i32));
            }
            cells
        }
    }

    fn get_priority(&self, rule_id: &[CellType]) -> u32 {
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

    // ========================================================================
    // Фаза 3: Применение (apply_matches)
    // ========================================================================

    /// Применить набор совпадений к решётке.
    pub fn apply_matches(
        &mut self,
        matches: Vec<RuleMatch>,
    ) -> (Vec<AffectedRegion>, Vec<(u32, Cell)>) {
        let mut regions: Vec<AffectedRegion> = Vec::new();
        let mut boundary_outputs: Vec<(u32, Cell)> = Vec::new();

        for m in matches {
            let rule = self.find_rule(&m.rule_id).cloned();
            if let Some(rule) = rule {
                let region = self.apply_rule(&m, &rule);
                regions.push(region);
            }
        }

        boundary_outputs.append(&mut self.pending_boundary);
        (regions, boundary_outputs)
    }

    fn find_rule(&self, rule_id: &[CellType]) -> Option<&Rule> {
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

    /// Применить одно правило к ячейке.
    fn apply_rule(&mut self, m: &RuleMatch, _rule: &Rule) -> AffectedRegion {
        let cx = m.x as i32;
        let cy = m.y as i32;

        let mut affected = AffectedRegion {
            x_start: m.x,
            x_end: m.x + m.rule_id.len() as u32,
            y_start: m.y,
            y_end: m.y + 1,
            has_changes: false,
        };

        let rule = self.find_rule(&m.rule_id).cloned();
        let rule = match rule {
            Some(r) => r,
            None => return affected,
        };

        // Фаза 1: сдвиги — накапливаем суммарное смещение
        let mut total_dx: i32 = 0;
        let mut total_dy: i32 = 0;
        for shift_group in &rule.shifts {
            for shift in shift_group {
                self.apply_shift(cx, cy, shift, &mut affected, &rule);
                match shift.direction {
                    Direction::Up => total_dy -= shift.steps as i32,
                    Direction::Down => total_dy += shift.steps as i32,
                    Direction::Left => total_dx -= shift.steps as i32,
                    Direction::Right => total_dx += shift.steps as i32,
                }
            }
        }

        // Фаза 2: изменения на ПОСЛЕ-СДВИГОВЫХ позициях
        if !rule.changes.is_empty() {
            affected.has_changes = true;

            for &(dx, dy, value) in &rule.changes {
                let nx = cx + total_dx + dx;
                let ny = cy + total_dy + dy;
                if nx >= 0 && ny >= 0 {
                    let ux = nx as usize;
                    let uy = ny as usize;
                    let w = self.grid.width() as i32;
                    let h = self.grid.height() as i32;

                    if nx < w && ny < h {
                        self.grid.set_cell(
                            ux,
                            uy,
                            Cell {
                                value: CellValue(CellType::new(value)),
                                age: 0,
                            },
                        );

                        affected.x_start = affected.x_start.min(ux as u32);
                        affected.x_end = affected.x_end.max(ux as u32 + 1);
                        affected.y_start = affected.y_start.min(uy as u32);
                        affected.y_end = affected.y_end.max(uy as u32 + 1);
                    }
                }
            }
        }

        affected
    }

    /// Применить цепочечный сдвиг — перемещает ТОЛЬКО первую ячейку паттерна (головку).
    fn apply_shift(
        &mut self,
        cx: i32,
        cy: i32,
        shift: &ShiftSpec,
        affected: &mut AffectedRegion,
        rule: &Rule,
    ) {
        let w = self.grid.width() as i32;
        let h = self.grid.height() as i32;

        let (dx, dy) = match shift.direction {
            Direction::Up => (0, -1),
            Direction::Down => (0, 1),
            Direction::Left => (-1, 0),
            Direction::Right => (1, 0),
        };
        let steps = shift.steps as i32;

        let ox = cx;
        let oy = cy;
        if ox < 0 || ox >= w || oy < 0 || oy >= h {
            return;
        }
        let head_cell = match self.grid.get_cell(ox as usize, oy as usize).copied() {
            Some(cell) => cell,
            None => return,
        };

        let nx = ox + dx * steps;
        let ny = oy + dy * steps;

        // Обновляем affected-region: включаем старую и новую позиции
        let ux = ox as usize;
        let uy = oy as usize;
        affected.x_start = affected.x_start.min(ux as u32);
        affected.x_end = affected.x_end.max(ux as u32 + 1);
        affected.y_start = affected.y_start.min(uy as u32);
        affected.y_end = affected.y_end.max(uy as u32 + 1);

        if nx >= 0 && nx < w && ny >= 0 && ny < h {
            let unx = nx as usize;
            let uny = ny as usize;
            self.grid.set_cell(unx, uny, head_cell);
            affected.x_start = affected.x_start.min(unx as u32);
            affected.x_end = affected.x_end.max(unx as u32 + 1);
            affected.y_start = affected.y_start.min(uny as u32);
            affected.y_end = affected.y_end.max(uny as u32 + 1);
        } else {
            // Обработка overflow в зависимости от действия правила
            match rule.overflow {
                OverflowAction::Discard => {}
                OverflowAction::Write(value) => {
                    let bx = nx.clamp(0, w - 1) as usize;
                    let by = ny.clamp(0, h - 1) as usize;
                    if let Some(buf) = self.grid.get_boundary_mut(bx, by) {
                        let output_cell = if value != 0 {
                            Cell {
                                value: CellValue(CellType::new(value)),
                                age: head_cell.age,
                            }
                        } else {
                            head_cell
                        };
                        buf.enqueue(0, output_cell);
                    }
                }
            }
        }

        self.grid.set_cell(ox as usize, oy as usize, Cell::default());
    }

    /// Увеличить возраст всех не-дефолтных ячеек на 1.
    pub fn advance_age(&mut self) {
        let coords: Vec<(usize, usize)> = self.grid.iter_active().collect();
        for (x, y) in coords {
            if let Some(cell) = self.grid.get_cell(x, y).copied() {
                let default = Cell::default();
                if cell.value != default.value || cell.age > 0 {
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
    }

    /// Сбросить возраст ячейки.
    pub fn reset_age(&mut self, x: usize, y: usize) {
        if let Some(cell) = self.grid.get_cell(x, y).copied() {
            self.grid.set_cell(x, y, Cell {
                value: cell.value,
                age: 0,
            });
        }
    }

    // ========================================================================
    // Полный цикл тика
    // ========================================================================

    /// Запустить один полный тик симуляции.
    pub fn run_tick(&mut self) -> (Vec<RuleMatch>, Vec<(u32, Cell)>) {
        // Фаза 0: ввод из input-буферов
        self.apply_input();

        // Фаза 1: обнаружение
        let all_matches = self.detect_matches();

        // Фаза 2: статический анализ конфликтов
        let conflict_graph = self.analyze_conflicts();
        let accepted = if conflict_graph.is_conflict_free() {
            all_matches
        } else {
            self.arbitrate(all_matches)
        };

        // Фаза 3: применение
        self.apply_matches(accepted.clone());

        // Фаза 4: вывод — сбор данных из output-буферов
        let outputs = self.drain_output();

        // Фаза 5: увеличение возраста
        self.advance_age();

        // Сбрасываем возраст для ячеек, затронутых правилами
        for m in &accepted {
            self.reset_age(m.x as usize, m.y as usize);
        }

        (accepted, outputs)
    }

    // ========================================================================
    // Анализ завершимости (detect_termination)
    // ========================================================================

    /// Проверяет, завершится ли симуляция, по счётчиковому потенциалу.
    ///
    /// Потенциал Φ = количество активных ячеек (не default).
    /// Если Φ не увеличивается за `observation_ticks` тиков подряд
    /// и хотя бы в одном тике строго убывает — `Terminates`.
    /// Если состояние повторилось — `MayDiverge`.
    /// Если ни то ни другое за max_ticks — `Unknown`.
    ///
    /// Работает на клоне движка, не изменяя оригинал.
    pub fn detect_termination(
        &mut self,
        max_ticks: usize,
        observation_ticks: usize,
    ) -> TerminationVerdict
    where
        S: Clone,
    {
        // Работаем на клоне, чтобы не менять состояние оригинального Engine
        let mut engine = self.clone();

        // Начальный снапшот: активные ячейки + их типы
        let initial_snapshot: Vec<(usize, usize, CellType)> = engine
            .grid
            .iter_active()
            .filter_map(|(x, y)| {
                let cell = engine.grid.get_cell(x, y)?;
                Some((x, y, cell.value.0))
            })
            .collect();

        let mut phi_history: Vec<usize> = Vec::new();

        for tick in 0..max_ticks {
            let (accepted, _) = engine.run_tick();

            let phi = engine.grid.iter_active().count();

            // Если активных ячеек нет — стабильное состояние
            if phi == 0 {
                return TerminationVerdict::Terminates;
            }

            // Если ни одно правило не сработало — симуляция стабилизировалась
            if accepted.is_empty() {
                return TerminationVerdict::Terminates;
            }

            phi_history.push(phi);

            // Проверка: не увеличивался последние observation_ticks и хотя бы раз убыл
            let history_len = phi_history.len();
            if history_len >= observation_ticks {
                let recent = &phi_history[history_len - observation_ticks..];
                let max_recent = *recent.iter().max().unwrap_or(&0);
                // Проверяем, что Φ не увеличивался — max не больше первого в окне
                if max_recent <= phi_history[history_len - observation_ticks] {
                    // Проверяем, что хотя бы раз строго убыл
                    let ever_decreased = phi_history
                        .windows(2)
                        .any(|w| w[1] < w[0]);
                    if ever_decreased {
                        return TerminationVerdict::Terminates;
                    }
                }
            }

            // Проверка: состояние повторилось (сравниваем с начальным)
            let current_snapshot: Vec<(usize, usize, CellType)> = engine
                .grid
                .iter_active()
                .filter_map(|(x, y)| {
                    let cell = engine.grid.get_cell(x, y)?;
                    Some((x, y, cell.value.0))
                })
                .collect();

            if current_snapshot == initial_snapshot && tick > 0 {
                return TerminationVerdict::MayDiverge;
            }
        }

        TerminationVerdict::Unknown
    }
}

// ============================================================================
// API для запуска тика (совместимость с run_tick)
// ============================================================================

/// Запустить один тик симуляции (упрощённый API без Engine).
pub fn run_tick<S: GridStorage + Default>(
    grid: &mut Grid<S>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
) -> (Vec<RuleMatch>, Vec<(u32, Cell)>) {
    let mut engine = Engine::new(
        std::mem::take(grid),
        rule_index.clone(),
    );
    let result = engine.run_tick();
    *grid = engine.grid;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Grid;
    use crate::storage::VecStorage;
    use crate::types::{BoundaryBuffer, Direction, ShiftSpec};

    fn make_grid(width: usize, height: usize) -> Grid<VecStorage> {
        let storage = VecStorage::new(width, height);
        Grid::new(storage)
    }

    fn make_rule_index(rules: Vec<Rule>) -> HashMap<CellType, Vec<Rule>> {
        let mut index: HashMap<CellType, Vec<Rule>> = HashMap::new();
        for rule in rules {
            if let Some(center) = rule.id.first() {
                index.entry(*center).or_default().push(rule);
            }
        }
        for rules in index.values_mut() {
            rules.sort_by_key(|b| std::cmp::Reverse(b.priority));
        }
        index
    }

    #[test]
    fn test_detect_simple_pattern() {
        let mut grid = make_grid(4, 1);
        grid.set_cell(
            1,
            0,
            Cell {
                value: CellValue(CellType(5)),
                age: 3,
            },
        );
        grid.set_cell(
            2,
            0,
            Cell {
                value: CellValue(CellType(6)),
                age: 2,
            },
        );

        let rule = Rule {
            id: vec![CellType(5), CellType(6)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, 7)],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        };

        let rule_index = make_rule_index(vec![rule]);
        let engine = Engine::new(grid, rule_index);
        let matches = engine.detect_matches();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].x, 1);
        assert_eq!(matches[0].y, 0);
    }

    #[test]
    fn test_detect_no_match() {
        let grid = make_grid(4, 1);
        let rule = Rule {
            id: vec![CellType(1), CellType(2)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        };
        let rule_index = make_rule_index(vec![rule]);
        let engine = Engine::new(grid, rule_index);
        let matches = engine.detect_matches();
        assert!(matches.is_empty());
    }

    #[test]
    fn test_arbitrate_no_conflict() {
        let grid = make_grid(10, 1);
        let matches = vec![
            RuleMatch {
                x: 1,
                y: 0,
                pattern: vec![vec![1]],
                rule_id: vec![CellType(1)],
            },
            RuleMatch {
                x: 5,
                y: 0,
                pattern: vec![vec![2]],
                rule_id: vec![CellType(2)],
            },
        ];

        let rule_index = make_rule_index(vec![
            Rule {
                id: vec![CellType(1)],
                pattern: vec![],
                shifts: vec![],
                changes: vec![],
                active_only: false,
                priority: 10,
                min_age: 0,
                overflow: Default::default(),
            },
            Rule {
                id: vec![CellType(2)],
                pattern: vec![],
                shifts: vec![],
                changes: vec![],
                active_only: false,
                priority: 10,
                min_age: 0,
                overflow: Default::default(),
            },
        ]);

        let engine = Engine::new(grid, rule_index);
        let accepted = engine.arbitrate(matches);

        assert_eq!(accepted.len(), 2);
    }

    #[test]
    fn test_arbitrate_conflict() {
        let grid = make_grid(10, 1);
        let matches = vec![
            RuleMatch {
                x: 1,
                y: 0,
                pattern: vec![vec![1, 2]],
                rule_id: vec![CellType(1), CellType(2)],
            },
            RuleMatch {
                x: 2,
                y: 0,
                pattern: vec![vec![2, 3]],
                rule_id: vec![CellType(2), CellType(3)],
            },
        ];

        let rule_index = make_rule_index(vec![
            Rule {
                id: vec![CellType(1), CellType(2)],
                pattern: vec![],
                shifts: vec![],
                changes: vec![],
                active_only: false,
                priority: 20,
                min_age: 0,
                overflow: Default::default(),
            },
            Rule {
                id: vec![CellType(2), CellType(3)],
                pattern: vec![],
                shifts: vec![],
                changes: vec![],
                active_only: false,
                priority: 10,
                min_age: 0,
                overflow: Default::default(),
            },
        ]);

        let engine = Engine::new(grid, rule_index);
        let accepted = engine.arbitrate(matches);

        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].rule_id, vec![CellType(1), CellType(2)]);
    }

    #[test]
    fn test_shift_right() {
        let mut grid = make_grid(4, 1);
        grid.set_cell(
            0,
            0,
            Cell {
                value: CellValue(CellType(1)),
                age: 5,
            },
        );
        grid.set_cell(
            1,
            0,
            Cell {
                value: CellValue(CellType(2)),
                age: 3,
            },
        );
        grid.set_cell(
            2,
            0,
            Cell {
                value: CellValue(CellType(3)),
                age: 1,
            },
        );

        let rule = Rule {
            id: vec![CellType(1)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec {
                direction: Direction::Right,
                steps: 1,
            }]],
            changes: vec![(0, 0, 0)],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        };

        let rule_index = make_rule_index(vec![rule]);
        let engine = Engine::new(grid, rule_index);
        let matches = engine.detect_matches();
        let accepted = engine.arbitrate(matches);
        assert!(!accepted.is_empty());
    }

    #[test]
    fn test_apply_local_changes() {
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
            changes: vec![(0, 0, 9)],
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
            changes: vec![(0, 0, 3)],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        };

        let rule_index = make_rule_index(vec![rule]);
        let (accepted, _) = run_tick(&mut grid, &rule_index);

        assert_eq!(accepted.len(), 1);
        assert_eq!(grid.get_cell(1, 1).unwrap().value, CellValue(CellType(3)));
    }

    #[test]
    fn test_run_tick_empty_grid() {
        let mut grid = make_grid(3, 3);
        let rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
        let (accepted, _) = run_tick(&mut grid, &rule_index);
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
            changes: vec![(0, 0, 0)],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        };

        let rule_index = make_rule_index(vec![rule]);
        let mut engine = Engine::new(grid, rule_index);

        let matches = engine.detect_matches();
        assert!(matches.is_empty(), "No 5 on grid");

        // Simulate output from boundary
        let outputs = engine.pop_output();
        assert!(outputs.is_empty());
    }
}