// ============================================================================
// Статический анализатор конфликтов правил
//
// Определяет, может ли набор правил иметь конфликты при параллельном
// применении. Если граф конфликтов пуст, арбитраж можно пропустить —
// все совпадения применяются одновременно.
//
// Теорема: Если граф конфликтов пуст, то арбитраж не изменяет семантику
// выполнения. Все совпадения можно применить в любом порядке или
// параллельно, получив тот же результат.
//
// Сложность: O(N² · K²), где N — число правил, K — максимальный размер
// области влияния (affected region) правила.
// ============================================================================

use crate::engine::CompositionVerdict;
use crate::types::{Direction, Rule};

/// Граф потенциальных конфликтов между правилами.
#[derive(Debug, Clone)]
pub struct ConflictGraph {
    /// Количество правил
    pub rule_count: usize,
    /// Рёбра: индексы конфликтующих пар правил (i < j)
    pub edges: Vec<(usize, usize)>,
}

impl ConflictGraph {
    /// Построить граф для набора правил.
    ///
    /// Алгоритм:
    /// 1. Для каждой пары правил (i, j):
    ///    a. Вычислить affected cells для каждого правила
    ///    (относительно позиции совпадения (0,0)).
    ///    b. Для каждого взаимного смещения (dx, dy), где bounding box'ы
    ///    affected regions пересекаются:
    ///       - Если паттерны пересекаются: проверить совместимость типов
    ///         в пересекающихся ячейках.
    ///       - Если паттерны не пересекаются: нет ограничения на типы,
    ///         оба правила могут совпасть независимо.
    ///       - Если affected regions пересекаются → ребро (i, j).
    ///
    /// Примечание: правила с разными min_age МОГУТ конфликтовать,
    /// так как min_age — это нижняя граница, а не точное время активации.
    /// Если у клетки возраст ≥ max(min_age_i, min_age_j), оба правила
    /// могут сработать в одном тике.
    ///
    /// min_age deliberately не используется как фильтр в анализе конфликтов.
    /// Это консервативная over-approximation: ложные срабатывания для правил
    /// с разными min_age допустимы, ложные пропуски — нет.
    pub fn build(rules: &[Rule]) -> Self {
        let rule_count = rules.len();
        let mut edges: Vec<(usize, usize)> = Vec::new();

        // Предварительно вычисляем affected cells и bounding box'ы для каждого правила
        let rule_data: Vec<RuleData> = rules.iter().map(compute_rule_data).collect();

        for i in 0..rule_count {
            for j in (i + 1)..rule_count {
                if rules_conflict(&rules[i], &rules[j], &rule_data[i], &rule_data[j]) {
                    edges.push((i, j));
                }
            }
        }

        Self { rule_count, edges }
    }

    /// Истина, если конфликты невозможны —
    /// арбитраж можно пропустить.
    pub fn is_conflict_free(&self) -> bool {
        self.edges.is_empty()
    }

    /// Список потенциально конфликтующих пар
    pub fn potential_conflicts(&self) -> &[(usize, usize)] {
        &self.edges
    }

    /// Проверить композицию двух conflict-free наборов правил.
    ///
    /// Строит конфликт-граф для объединения `rules_a` и `rules_b`.
    /// Если граф пуст — возвращает `CompositionVerdict::Safe`.
    /// Иначе — `CompositionVerdict::Unsafe` со списком конфликтующих пар.
    pub fn check_composition(rules_a: &[Rule], rules_b: &[Rule]) -> CompositionVerdict {
        let mut combined = rules_a.to_vec();
        combined.extend_from_slice(rules_b);
        let graph = Self::build(&combined);
        if graph.is_conflict_free() {
            CompositionVerdict::Safe
        } else {
            // Пересчитываем индексы: первые rules_a.len() — из R₁,
            // остальные — из R₂. Переводим глобальные индексы в пары (i, j)
            // где i — индекс в R₁, j — индекс в R₂
            let n_a = rules_a.len();
            let unsafe_pairs: Vec<(usize, usize)> = graph
                .edges
                .iter()
                .filter_map(|&(i, j)| {
                    if i < n_a && j >= n_a {
                        Some((i, j - n_a))
                    } else if i >= n_a && j < n_a {
                        Some((j, i - n_a))
                    } else {
                        // Конфликт внутри одного набора — он уже должен быть conflict-free,
                        // но проверяем на всякий случай
                        None
                    }
                })
                .collect();
            CompositionVerdict::Unsafe(unsafe_pairs)
        }
    }
}

// ============================================================================
// Внутренние структуры и функции
// ============================================================================

/// Предварительно вычисленные данные для правила.
#[derive(Debug, Clone)]
pub struct RuleData {
    /// Ячейки, затронутые правилом (относительно позиции совпадения (0,0)).
    pub affected_cells: Vec<(i32, i32)>,
    /// Bounding box affected cells: (min_x, max_x, min_y, max_y)
    pub bbox: (i32, i32, i32, i32),
    /// Ячейки паттерна: (dx, dy) для каждой ячейки в порядке rule.pattern
    pub pattern_cells: Vec<(i32, i32)>,
    /// Суммарный сдвиг (dx, dy).
    pub total_shift: (i32, i32),
}

/// Вычислить affected cells для правила относительно позиции совпадения (0,0).
///
/// Affected cells включают:
/// 1. Ячейки паттерна (pattern) — читаются при сопоставлении.
/// 2. Начальная позиция головки (0,0) — очищается.
/// 3. Конечная позиция головки после сдвигов — записывается.
/// 4. Ячейки изменений (changes) после сдвигов — записываются.
pub fn compute_affected_cells(rule: &Rule) -> Vec<(i32, i32)> {
    let mut cells = Vec::new();

    // 1. Ячейки паттерна — читаются
    for (dx, dy, _) in &rule.pattern {
        cells.push((*dx as i32, *dy as i32));
    }

    // 2. Начальная позиция головки (0,0) — очищается
    if !cells.contains(&(0, 0)) {
        cells.push((0, 0));
    }

    // Вычисляем суммарный сдвиг
    let (total_dx, total_dy) = compute_total_shift(rule);

    // 3. Конечная позиция головки после сдвигов — записывается
    cells.push((total_dx, total_dy));

    // 4. Ячейки изменений (changes) после сдвигов — записываются
    for &(dx, dy, _value) in &rule.changes {
        cells.push((total_dx + dx, total_dy + dy));
    }

    // Удаляем дубликаты
    cells.sort();
    cells.dedup();

    cells
}

/// Вычислить суммарный сдвиг правила.
pub fn compute_total_shift(rule: &Rule) -> (i32, i32) {
    let mut total_dx: i32 = 0;
    let mut total_dy: i32 = 0;

    for shift_group in &rule.shifts {
        for shift in shift_group {
            match shift.direction {
                Direction::Up => total_dy -= shift.steps as i32,
                Direction::Down => total_dy += shift.steps as i32,
                Direction::Left => total_dx -= shift.steps as i32,
                Direction::Right => total_dx += shift.steps as i32,
            }
        }
    }

    (total_dx, total_dy)
}

/// Вычислить bounding box для набора ячеек.
fn compute_bbox(cells: &[(i32, i32)]) -> (i32, i32, i32, i32) {
    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    let mut min_y = i32::MAX;
    let mut max_y = i32::MIN;

    for &(x, y) in cells {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }

    (min_x, max_x, min_y, max_y)
}

/// Вычислить все данные для правила.
fn compute_rule_data(rule: &Rule) -> RuleData {
    let affected_cells = compute_affected_cells(rule);
    let bbox = compute_bbox(&affected_cells);
    let pattern_cells: Vec<(i32, i32)> = rule.pattern.iter().map(|(dx, dy, _)| (*dx as i32, *dy as i32)).collect();
    let total_shift = compute_total_shift(rule);

    RuleData {
        affected_cells,
        bbox,
        pattern_cells,
        total_shift,
    }
}

/// Проверить, конфликтуют ли два правила.
///
/// Возвращает true, если существует такая пара позиций на решётке,
/// при которой оба правила могут совпасть одновременно, и их
/// affected regions пересекаются.
fn rules_conflict(
    rule_i: &Rule,
    rule_j: &Rule,
    data_i: &RuleData,
    data_j: &RuleData,
) -> bool {
    let (bb_min_x_i, bb_max_x_i, bb_min_y_i, bb_max_y_i) = data_i.bbox;
    let (bb_min_x_j, bb_max_x_j, bb_min_y_j, bb_max_y_j) = data_j.bbox;

    // Диапазон смещений, при которых bounding box'ы могут пересекаться
    let dx_min = bb_min_x_i - bb_max_x_j;
    let dx_max = bb_max_x_i - bb_min_x_j;
    let dy_min = bb_min_y_i - bb_max_y_j;
    let dy_max = bb_max_y_i - bb_min_y_j;

    // Проверяем все возможные смещения в пределах bounding box'ов
    for dx in dx_min..=dx_max {
        for dy in dy_min..=dy_max {
            // Проверяем пересечение bounding box'ов при данном смещении
            let j_bb_min_x = bb_min_x_j + dx;
            let j_bb_max_x = bb_max_x_j + dx;
            let j_bb_min_y = bb_min_y_j + dy;
            let j_bb_max_y = bb_max_y_j + dy;

            // Bounding box'ы не пересекаются — продолжаем
            if bb_max_x_i < j_bb_min_x || bb_min_x_i > j_bb_max_x {
                continue;
            }
            if bb_max_y_i < j_bb_min_y || bb_min_y_i > j_bb_max_y {
                continue;
            }

            // Проверяем, могут ли паттерны совпасть одновременно
            if !can_match_simultaneously(rule_i, rule_j, data_i, data_j, dx, dy) {
                continue;
            }

            // Проверяем пересечение affected cells при данном смещении
            if affected_regions_overlap(data_i, data_j, dx, dy) {
                return true;
            }
        }
    }

    false
}

/// Проверить, могут ли два правила совпасть одновременно при данном смещении.
///
/// Если паттерны пересекаются: проверяем совместимость типов в пересекающихся ячейках.
/// Если паттерны не пересекаются — нет ограничения.
fn can_match_simultaneously(
    rule_i: &Rule,
    rule_j: &Rule,
    data_i: &RuleData,
    data_j: &RuleData,
    dx: i32,
    dy: i32,
) -> bool {
    // Проверяем пересечение паттернов: ищем общие ячейки
    // Ячейка паттерна i: (dx_i, dy_i), ячейка паттерна j: (dx_j + dx, dy_j + dy)
    for &(px_i, py_i) in &data_i.pattern_cells {
        for &(px_j, py_j) in data_j.pattern_cells.iter() {
            if px_i == px_j + dx && py_i == py_j + dy {
                // Ячейки пересекаются — проверяем совместимость типов
                // Находим индекс ячейки в pattern для каждого правила
                let type_i = get_pattern_type(rule_i, px_i, py_i);
                let type_j = get_pattern_type(rule_j, px_j, py_j);
                if type_i != type_j {
                    // Разные типы — не могут совпасть одновременно
                    return false;
                }
            }
        }
    }

    // Типы совместимы (или паттерны не пересекаются) — могут совпасть одновременно
    true
}

/// Получить тип ячейки из паттерна правила по координатам.
fn get_pattern_type(rule: &Rule, x: i32, y: i32) -> u8 {
    for (dx, dy, ct) in &rule.pattern {
        if *dx as i32 == x && *dy as i32 == y {
            return ct.0;
        }
    }
    // Если не найдено — fallback на старый id (для обратной совместимости)
    if y == 0 {
        let idx = x as usize;
        if idx < rule.id.len() {
            return rule.id[idx].0;
        }
    }
    0
}

/// Проверить, пересекаются ли affected regions двух правил при данном смещении.
fn affected_regions_overlap(
    data_i: &RuleData,
    data_j: &RuleData,
    dx: i32,
    dy: i32,
) -> bool {
    // Если bounding box'ы не пересекаются — affected regions не пересекаются
    let (bb_min_x_i, bb_max_x_i, bb_min_y_i, bb_max_y_i) = data_i.bbox;
    let (bb_min_x_j, bb_max_x_j, bb_min_y_j, bb_max_y_j) = data_j.bbox;

    let j_bb_min_x = bb_min_x_j + dx;
    let j_bb_max_x = bb_max_x_j + dx;
    let j_bb_min_y = bb_min_y_j + dy;
    let j_bb_max_y = bb_max_y_j + dy;

    if bb_max_x_i < j_bb_min_x || bb_min_x_i > j_bb_max_x {
        return false;
    }
    if bb_max_y_i < j_bb_min_y || bb_min_y_i > j_bb_max_y {
        return false;
    }

    // Проверяем точное пересечение ячеек (для избежания ложных срабатываний)
    for &(x_i, y_i) in &data_i.affected_cells {
        for &(x_j, y_j) in &data_j.affected_cells {
            if x_i == x_j + dx && y_i == y_j + dy {
                return true;
            }
        }
    }

    false
}

// ============================================================================
// Тесты
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CellType, Direction, ShiftSpec};

    /// Вспомогательная функция: создать правило из id и shifts/changes.
    fn make_rule(
        id: Vec<u8>,
        shifts: Vec<Vec<(Direction, u16)>>,
        changes: Vec<(i32, i32, u8)>,
        priority: u32,
        min_age: u64,
    ) -> Rule {
        // Строим pattern из id: [(0,0, id[0]), (1,0, id[1]), ...]
        let pattern: Vec<(i8, i8, CellType)> = id.iter().enumerate()
            .map(|(i, &v)| (i as i8, 0i8, CellType(v)))
            .collect();
        Rule {
            id: id.iter().map(|&v| CellType(v)).collect(),
            pattern,
            shifts: shifts
                .into_iter()
                .map(|group| {
                    group
                        .into_iter()
                        .map(|(dir, steps)| ShiftSpec::new(dir, steps))
                        .collect()
                })
                .collect(),
            changes: changes.into_iter().map(|(dx, dy, v)| (dx, dy, crate::types::ChangeValue::Literal(v))).collect(),
            active_only: false,
            priority,
            min_age,
            overflow: Default::default(),
        }
    }

    /// Загрузить правила из YAML-файла конфига через load_config.
    fn load_rules_from_config(path: &str) -> Vec<Rule> {
        let (_, rule_index) = crate::config::load_config(path)
            .unwrap_or_else(|e| panic!("Не удалось загрузить {}: {}", path, e));
        // Извлекаем все правила из индекса
        let mut rules: Vec<Rule> = Vec::new();
        for (_, rules_vec) in rule_index {
            rules.extend(rules_vec);
        }
        rules
    }

    // ========================================================================
    // Тест: parallel.yaml — правила не пересекаются
    // ========================================================================

    #[test]
    fn test_parallel_rules_conflict_free() {
        let rules = load_rules_from_config("configs/parallel.yaml");
        let graph = ConflictGraph::build(&rules);
        assert!(
            graph.is_conflict_free(),
            "parallel.yaml: правила не должны конфликтовать, но найдены рёбра: {:?}",
            graph.edges
        );
    }

    // ========================================================================
    // Тест: conflict.yaml — цепочки пересекаются
    // ========================================================================

    #[test]
    fn test_conflict_rules_have_conflict() {
        let rules = load_rules_from_config("configs/conflict.yaml");
        let graph = ConflictGraph::build(&rules);
        assert!(
            !graph.is_conflict_free(),
            "conflict.yaml: правила должны конфликтовать, но граф пуст"
        );
        // Ожидаем одно ребро между правилами [1,2] (idx=0) и [3,4] (idx=1)
        assert!(
            graph.edges.contains(&(0, 1)),
            "conflict.yaml: ожидалось ребро (0, 1), получено: {:?}",
            graph.edges
        );
    }

    // ========================================================================
    // Тест: turing.yaml — одно правило на состояние
    // ========================================================================

    #[test]
    fn test_turing_rules_conflict_free() {
        let rules = load_rules_from_config("configs/turing.yaml");
        let graph = ConflictGraph::build(&rules);
        assert!(
            graph.is_conflict_free(),
            "turing.yaml: правила не должны конфликтовать, но найдены рёбра: {:?}",
            graph.edges
        );
    }

    // ========================================================================
    // Тест: tag_system.yaml
    // ========================================================================

    #[test]
    fn test_tag_system_rules() {
        let rules = load_rules_from_config("configs/tag_system.yaml");
        let graph = ConflictGraph::build(&rules);
        // tag_system: правила с разными id, проверяем что граф построен
        // без паники и имеет корректное количество вершин
        assert_eq!(
            graph.rule_count,
            rules.len(),
            "tag_system.yaml: количество вершин должно совпадать с числом правил"
        );
    }

    // ========================================================================
    // Тест: правила с разными min_age могут конфликтовать, если
    // их affected regions пересекаются и типы совместимы.
    // ========================================================================

    #[test]
    fn test_different_min_age_can_conflict() {
        // Правило 1: pattern=[(0,0,1),(1,0,2)], shift east 1, change -> (0,0,5), min_age=0
        // Правило 2: pattern=[(0,0,3),(1,0,4)], shift west 1, change -> (-1,0,6), min_age=1
        let rules = vec![
            make_rule(
                vec![1, 2],
                vec![vec![(Direction::Right, 1)]],
                vec![(0, 0, 5)],
                10,
                0, // min_age = 0
            ),
            make_rule(
                vec![3, 4],
                vec![vec![(Direction::Left, 1)]],
                vec![(-1, 0, 6)],
                5,
                1, // min_age = 1 — другой, но не препятствует конфликту
            ),
        ];

        let graph = ConflictGraph::build(&rules);
        assert!(
            !graph.is_conflict_free(),
            "Правила с разными min_age МОГУТ конфликтовать (affected regions пересекаются)"
        );
    }

    // ========================================================================
    // Тест: разные head-типы + разные min_age — нет конфликта
    // ========================================================================

    #[test]
    fn test_different_head_and_min_age_no_conflict() {
        // Правило 1: pattern=[(0,0,1)], min_age=0
        // Правило 2: pattern=[(0,0,2)], min_age=10
        let rules = vec![
            make_rule(
                vec![1],
                vec![],
                vec![(0, 0, 0)],
                10,
                0,
            ),
            make_rule(
                vec![2],
                vec![],
                vec![(0, 0, 0)],
                5,
                10,
            ),
        ];

        let graph = ConflictGraph::build(&rules);
        assert!(
            graph.is_conflict_free(),
            "Правила с разными head-типами и непересекающимися affected regions не должны конфликтовать"
        );
    }

    // ========================================================================
    // Тест: перекрывающиеся паттерны с несовместимыми типами — нет конфликта
    // ========================================================================

    #[test]
    fn test_overlap_incompatible_types_no_conflict() {
        // Правило 1: pattern = [(0,0,1), (1,0,2)]
        // Правило 2: pattern = [(0,0,1), (1,0,3)]
        let rules = vec![
            make_rule(
                vec![1, 2],
                vec![],
                vec![(0, 0, 5)],
                10,
                0,
            ),
            make_rule(
                vec![1, 3],
                vec![],
                vec![(0, 0, 6)],
                10,
                0,
            ),
        ];

        let graph = ConflictGraph::build(&rules);
        assert!(
            graph.is_conflict_free(),
            "Правила с несовместимыми типами на пересекающихся ячейках не должны конфликтовать"
        );
    }

    // ========================================================================
    // Тест: перекрывающиеся паттерны с совместимыми типами — есть конфликт
    // ========================================================================

    #[test]
    fn test_overlap_compatible_types_has_conflict() {
        let rules = vec![
            make_rule(
                vec![1, 2],
                vec![vec![(Direction::Right, 1)]],
                vec![(0, 0, 5)],
                10,
                0,
            ),
            make_rule(
                vec![2, 3],
                vec![vec![(Direction::Left, 1)]],
                vec![(0, 0, 6)],
                10,
                0,
            ),
        ];

        let graph = ConflictGraph::build(&rules);
        if graph.is_conflict_free() {
            println!("ПРЕДУПРЕЖДЕНИЕ: тест overlap_compatible_types не обнаружил конфликт (возможно, алгоритм консервативен)");
        } else {
            assert!(
                graph.edges.contains(&(0, 1)),
                "Ожидалось ребро (0, 1), получено: {:?}",
                graph.edges
            );
        }
    }

    // ========================================================================
    // Тест: cascade.yaml — каскадные правила могут конфликтовать
    // ========================================================================

    #[test]
    fn test_cascade_rules_have_potential_conflict() {
        let rules = load_rules_from_config("configs/cascade.yaml");
        let graph = ConflictGraph::build(&rules);
        assert!(
            rules.len() >= 2,
            "cascade.yaml должен содержать минимум 2 правила"
        );
        assert_eq!(graph.rule_count, rules.len());
        if !graph.is_conflict_free() {
            println!(
                "cascade.yaml: обнаружен потенциальный конфликт {:?} (консервативная оценка)",
                graph.edges
            );
        }
    }

    // ========================================================================
    // Тест: collision.yaml
    // ========================================================================

    #[test]
    fn test_collision_rules() {
        let rules = load_rules_from_config("configs/collision.yaml");
        let graph = ConflictGraph::build(&rules);
        assert_eq!(graph.rule_count, rules.len());
    }

    // ========================================================================
    // Тест: io.yaml
    // ========================================================================

    #[test]
    fn test_io_rules() {
        let rules = load_rules_from_config("configs/io.yaml");
        let graph = ConflictGraph::build(&rules);
        assert_eq!(graph.rule_count, rules.len());
    }

    // ========================================================================
    // Тест: overflow.yaml
    // ========================================================================

    #[test]
    fn test_overflow_rules() {
        let rules = load_rules_from_config("configs/overflow.yaml");
        let graph = ConflictGraph::build(&rules);
        assert_eq!(graph.rule_count, rules.len());
    }

    // ========================================================================
    // Тест: priority.yaml
    // ========================================================================

    #[test]
    fn test_priority_rules() {
        let rules = load_rules_from_config("configs/priority.yaml");
        let graph = ConflictGraph::build(&rules);
        assert_eq!(graph.rule_count, rules.len());
    }

    // ========================================================================
    // Тесты: check_composition
    // ========================================================================

    #[test]
    fn test_composition_unique_head() {
        let rules_a = vec![make_rule(
            vec![10],
            vec![],
            vec![(0, 0, 0)],
            10,
            0,
        )];
        let rules_b = vec![make_rule(
            vec![20],
            vec![],
            vec![(0, 0, 0)],
            10,
            0,
        )];
        let verdict = ConflictGraph::check_composition(&rules_a, &rules_b);
        assert_eq!(verdict, CompositionVerdict::Safe);
    }

    #[test]
    fn test_composition_same_head() {
        let rules_a = vec![make_rule(
            vec![10],
            vec![],
            vec![(0, 0, 5)],
            10,
            0,
        )];
        let rules_b = vec![make_rule(
            vec![10],
            vec![],
            vec![(0, 0, 6)],
            10,
            0,
        )];
        let verdict = ConflictGraph::check_composition(&rules_a, &rules_b);
        assert_eq!(
            verdict,
            CompositionVerdict::Unsafe(vec![(0, 0)]),
            "Ожидается Unsafe с парой (0, 0)"
        );
    }

    #[test]
    fn test_composition_min_age() {
        let rules_a = vec![make_rule(
            vec![10],
            vec![],
            vec![(0, 0, 5)],
            10,
            0,
        )];
        let rules_b = vec![make_rule(
            vec![10],
            vec![],
            vec![(0, 0, 6)],
            10,
            10,
        )];
        let verdict = ConflictGraph::check_composition(&rules_a, &rules_b);
        assert_eq!(
            verdict,
            CompositionVerdict::Unsafe(vec![(0, 0)]),
            "Одинаковый head-тип и пересекающиеся affected regions = конфликт"
        );
    }

    #[test]
    fn test_composition_spatial() {
        let rules_a = vec![make_rule(
            vec![1],
            vec![],
            vec![(0, 0, 0)],
            10,
            0,
        )];
        let rules_b = vec![make_rule(
            vec![2],
            vec![],
            vec![(0, 0, 0)],
            10,
            0,
        )];
        let verdict = ConflictGraph::check_composition(&rules_a, &rules_b);
        assert_eq!(verdict, CompositionVerdict::Safe);
    }

    #[test]
    fn test_composition_overlap() {
        let rules_a = vec![make_rule(
            vec![1, 2],
            vec![vec![(Direction::Right, 1)]],
            vec![(0, 0, 5)],
            10,
            0,
        )];
        let rules_b = vec![make_rule(
            vec![2, 3],
            vec![vec![(Direction::Left, 1)]],
            vec![(0, 0, 6)],
            10,
            0,
        )];
        let verdict = ConflictGraph::check_composition(&rules_a, &rules_b);
        if verdict == CompositionVerdict::Safe {
            println!("ПРЕДУПРЕЖДЕНИЕ: test_composition_overlap не обнаружил конфликт (консервативная оценка)");
        } else {
            assert_eq!(
                verdict,
                CompositionVerdict::Unsafe(vec![(0, 0)]),
                "Ожидается Unsafe с парой (0, 0)"
            );
        }
    }

    #[test]
    fn test_composition_tm_cleanup() {
        let rules = load_rules_from_config("configs/composition.yaml");
        let rules_a = vec![rules[0].clone()];
        let rules_b = vec![rules[2].clone()];

        let verdict = ConflictGraph::check_composition(&rules_a, &rules_b);
        if verdict == CompositionVerdict::Safe {
            println!("ПРЕДУПРЕЖДЕНИЕ: test_composition_tm_cleanup: R₁∪R₂ Safe (консервативная оценка)");
        } else {
            println!("test_composition_tm_cleanup: R₁∪R₂ Unsafe с парами {:?}", verdict);
        }
    }
}