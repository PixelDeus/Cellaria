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

use crate::types::{CellType, Direction, Rule};
use std::collections::HashMap;

/// Эффективный паттерн правила: явный `rule.pattern`, либо (если он пуст —
/// правило описано только через `id`, обычное сокращение для сдвиговых
/// правил без явного паттерна) паттерн, построенный из `id` как
/// `(0,0,id[0]), (1,0,id[1]), ...` — РОВНО та же свёртка, что уже применяет
/// матчер (`matcher::build_group_data`'s `effective_patterns`) при реальном
/// сопоставлении. До этой функции здесь такого fallback не было: правило с
/// пустым `pattern` считалось анализатором не читающим вообще ничего (и,
/// значит, несовместимым по типу — не проверяемым в `can_match_simultaneously`
/// никогда), хотя матчер на самом деле требует конкретный тип в (0,0) (и
/// далее по `id`). Из-за этого рассинхрона анализатор терял единственную
/// причину признать два правила НЕ конфликтующими (несовпадение типов на
/// пересекающейся клетке) именно для самых обычных id-only правил (сдвиг без
/// паттерна) — и завышал число ложных конфликтов ровно для них.
fn effective_pattern(rule: &Rule) -> Vec<(i32, i32, CellType)> {
    if !rule.pattern.is_empty() {
        rule.pattern.iter().map(|&(dx, dy, ct)| (dx as i32, dy as i32, ct)).collect()
    } else {
        rule.id.iter().enumerate().map(|(i, &ct)| (i as i32, 0i32, ct)).collect()
    }
}

/// Контекст решётки, позволяющий анализатору точнее судить о правилах со
/// сдвигом и `OverflowAction::Write`/`WriteLiteral` — см. doc-комментарий
/// у `boundary_exempt`. Без него (`ConflictGraph::build`/`check_composition`)
/// анализ остаётся полностью относительным и максимально консервативным,
/// как и был.
pub struct GridContext<'a> {
    pub width: usize,
    pub height: usize,
    pub boundaries: &'a HashMap<(usize, usize), crate::types::BoundaryBuffer>,
}

/// Правило, чей ЛЮБОЙ сдвиг с overflow-записью доказуемо никогда не может
/// попасть в саму решётку — только в изолированную очередь `BoundaryBuffer`
/// (см. `apply_overflow_write` в `applicator.rs`: если на клэмпнутой позиции
/// настроен boundary, запись идёт в `buf.enqueue`, а не в `write_buffer`
/// решётки). Клэмпнутая позиция вдоль оси сдвига ФИКСИРОВАНА (например,
/// `x = width-1` для сдвига вправо), но позиция вдоль перпендикулярной оси
/// НЕИЗВЕСТНА в чисто относительном анализе (зависит от того, где реально
/// стоит совпавшая клетка на решётке, а не от структуры правила) — поэтому
/// единственное, что можно доказать статически: весь этот край решётки
/// целиком покрыт boundary-буферами, а не только какая-то одна точка на
/// нём. Именно поэтому `edge_fully_boundary_covered` проверяет ВСЕ клетки
/// края, а не одну.
fn boundary_exempt(rule: &Rule, grid: Option<&GridContext>) -> bool {
    let Some(grid) = grid else { return false };
    rule.shifts.iter().flatten().all(|spec| edge_fully_boundary_covered(spec.direction, grid))
}

fn edge_fully_boundary_covered(direction: Direction, grid: &GridContext) -> bool {
    let (w, h) = (grid.width, grid.height);
    match direction {
        Direction::Right => (0..h).all(|y| grid.boundaries.contains_key(&(w.saturating_sub(1), y))),
        Direction::Left => (0..h).all(|y| grid.boundaries.contains_key(&(0, y))),
        Direction::Down => (0..w).all(|x| grid.boundaries.contains_key(&(x, h.saturating_sub(1)))),
        Direction::Up => (0..w).all(|x| grid.boundaries.contains_key(&(x, 0))),
    }
}

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
    ///
    /// Важно: правило проверяется и САМО НА СЕБЯ. Одно и то же правило может
    /// сработать в двух разных (перекрывающихся) позициях одновременно —
    /// например, правило без сдвига с change на смещении -1 читает свою
    /// позицию паттерном и одновременно пишет change в позицию соседнего
    /// срабатывания того же правила. Раньше self-конфликт не проверялся
    /// вообще (цикл `j in (i+1)..rule_count` пропускает i==j), из-за чего
    /// любой набор из одного правила автоматически считался conflict-free,
    /// даже если оно конфликтует само с собой — что напрямую противоречит
    /// теореме "CF ⇒ арбитраж не нужен".
    pub fn build(rules: &[Rule]) -> Self {
        Self::build_impl(rules, None)
    }

    /// Как [`ConflictGraph::build`], но с контекстом решётки — правила-
    /// переносчики, чей overflow доказуемо всегда попадает в изолированную
    /// очередь `BoundaryBuffer` (см. `boundary_exempt`), больше не
    /// форсируются в конфликт с ЛЮБЫМ другим пишущим правилом; для них
    /// действует обычная (точная, по пересечению `write_cells`) проверка,
    /// как для любого не-overflow правила.
    pub fn build_with_grid(rules: &[Rule], grid: &GridContext) -> Self {
        Self::build_impl(rules, Some(grid))
    }

    fn build_impl(rules: &[Rule], grid: Option<&GridContext>) -> Self {
        let rule_count = rules.len();
        let mut edges: Vec<(usize, usize)> = Vec::new();

        // Предварительно вычисляем affected cells и bounding box'ы для каждого правила
        let rule_data: Vec<RuleData> = rules.iter().map(compute_rule_data).collect();

        // Правила со сдвигом и OverflowAction::Write: реальная точка записи при
        // выходе за границу клэмпится на край РЕШЁТКИ (см. `apply_shift_buffered`),
        // а этот анализатор работает в чисто относительных координатах и понятия
        // не имеет о размере решётки. Значит, для такого правила он не может ни
        // доказать, ни опровергнуть, что клэмпнутая позиция двух его срабатываний
        // (или срабатывания этого правила и другого, тоже пишущего) совпадёт на
        // каком-то реальном размере решётки — а раз не может доказать безопасность,
        // обязан консервативно считать конфликт возможным (иначе получится
        // ложноотрицательный CF-вердикт, что нарушает теорему "CF ⇒ арбитраж не
        // нужен": `prop_conflict_free_rules_accept_everything` ловит именно это
        // при совпадении shift-цели с overflow ровно на границе решётки).
        let has_overflow_write_shift = |r: &Rule| {
            !r.shifts.is_empty()
                && matches!(
                    r.overflow,
                    crate::types::OverflowAction::Write(_) | crate::types::OverflowAction::WriteLiteral(_)
                )
                && !boundary_exempt(r, grid)
        };

        for i in 0..rule_count {
            let force_i = has_overflow_write_shift(&rules[i]);
            if force_i || rule_self_conflicts(&rules[i], &rule_data[i]) {
                edges.push((i, i));
            }
            for j in (i + 1)..rule_count {
                let forced = (force_i || has_overflow_write_shift(&rules[j]))
                    && !rule_data[i].write_cells.is_empty()
                    && !rule_data[j].write_cells.is_empty();
                if forced || rules_conflict(&rules[i], &rules[j], &rule_data[i], &rule_data[j]) {
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
        Self::check_composition_impl(rules_a, rules_b, None)
    }

    /// Как [`ConflictGraph::check_composition`], но с контекстом решётки —
    /// см. [`ConflictGraph::build_with_grid`].
    pub fn check_composition_with_grid(rules_a: &[Rule], rules_b: &[Rule], grid: &GridContext) -> CompositionVerdict {
        Self::check_composition_impl(rules_a, rules_b, Some(grid))
    }

    fn check_composition_impl(rules_a: &[Rule], rules_b: &[Rule], grid: Option<&GridContext>) -> CompositionVerdict {
        let mut combined = rules_a.to_vec();
        combined.extend_from_slice(rules_b);
        let graph = Self::build_impl(&combined, grid);
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
// Вердикт композиции наборов правил
// ============================================================================

/// Результат проверки композиции двух conflict-free наборов правил.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompositionVerdict {
    /// Композиция безопасна — нет потенциальных конфликтов между правилами
    /// из разных наборов.
    Safe,
    /// Обнаружены потенциальные конфликты: список пар (i, j), где
    /// i — индекс правила в первом наборе, j — индекс во втором.
    Unsafe(Vec<(usize, usize)>),
}

// ============================================================================
// Кэш предвычисленных данных для правил (RuleDataCache)
// ============================================================================

/// Тип кэша: (голова id правила, позиция в rule_index[head]) → предвычисленные данные.
///
/// Ключ по одному `rule_id` недостаточен: несколько правил могут иметь
/// одинаковый `id` (паттерн недетерминированного выбора, см.
/// `test_nondeterministic_same_priority`), и тогда кэш по id молча вернул бы
/// данные первого из них для любого правила с этим id. `rule_idx` (позиция
/// правила в отсортированном по приоритету `Vec` для данной головы) делает
/// ключ однозначным.
pub type RuleDataCache = HashMap<(crate::types::CellType, usize), RuleData>;

/// Построить кэш RuleData из rule_index.
pub fn build_rule_data_cache(rule_index: &HashMap<crate::types::CellType, Vec<Rule>>) -> RuleDataCache {
    let mut cache = RuleDataCache::new();
    for (&head_type, rules) in rule_index {
        for (rule_idx, rule) in rules.iter().enumerate() {
            cache.insert((head_type, rule_idx), compute_rule_data(rule));
        }
    }
    cache
}

/// Получить RuleData из кэша по голове id правила и позиции в rule_index[head].
pub fn get_rule_data<'a>(
    cache: &'a RuleDataCache,
    head_type: crate::types::CellType,
    rule_idx: usize,
) -> Option<&'a RuleData> {
    cache.get(&(head_type, rule_idx))
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
    /// Суммарный сдвиг (dx, dy) — сумма дельт ВСЕХ сдвигов правила.
    /// Осмыслен только когда сдвиг ровно один; при 0 или 2+ сдвигах для
    /// позиционирования `changes` используется `shift_targets`, а не это
    /// поле (см. его комментарий).
    pub total_shift: (i32, i32),
    /// Целевая клетка КАЖДОГО отдельного сдвига правила (не суммарная).
    ///
    /// Правило с несколькими `ShiftSpec` (независимо от того, в одной они
    /// группе или в разных — вложенность в группы не влияет на применение,
    /// см. `apply_shift_buffered`) реплицирует значение головки в КАЖДУЮ
    /// цель независимо, а не двигает его по цепочке. Поэтому `changes`
    /// применяются один раз ОТНОСИТЕЛЬНО КАЖДОЙ такой цели, а не один раз
    /// относительно их суммы — раньше здесь ошибочно использовался
    /// `total_shift`, из-за чего при 2+ сдвигах `changes` попадали в
    /// точку, не совпадающую ни с одной реальной целью записи.
    pub shift_targets: Vec<(i32, i32)>,
    /// Ячейки, в которые правило реально ПИШЕТ (только запись, без чтения
    /// паттерна) — см. `compute_write_cells`. Используются для проверки
    /// конфликтов вместо `affected_cells`: две clетки могут одновременно
    /// ЧИТАТЬ одну и ту же клетку без конфликта (детекция всегда работает
    /// по состоянию решётки ДО тика, запись идёт в отдельный буфер и
    /// применяется атомарно после арбитража — см. `apply_matches`), конфликт
    /// возможен только когда ДВЕ записи целятся в одну клетку.
    pub write_cells: Vec<(i32, i32)>,
}

/// Вычислить все данные для правила.
pub fn compute_rule_data(rule: &Rule) -> RuleData {
    let affected_cells = compute_affected_cells(rule);
    let bbox = compute_bbox(&affected_cells);
    let pattern_cells: Vec<(i32, i32)> = effective_pattern(rule).iter().map(|&(dx, dy, _)| (dx, dy)).collect();
    let shift_targets = compute_shift_targets(rule);
    let total_shift = shift_targets.iter().fold((0, 0), |(ax, ay), &(dx, dy)| (ax + dx, ay + dy));
    let write_cells = compute_write_cells(rule, &shift_targets);

    RuleData {
        affected_cells,
        bbox,
        pattern_cells,
        total_shift,
        shift_targets,
        write_cells,
    }
}

/// Вычислить ячейки, в которые правило реально пишет — зеркалит ровно то,
/// что `apply_rule_buffered`/`apply_shift_buffered` кладут в write_buffer:
/// без сдвигов — только цели `changes` относительно (0,0); со сдвигами —
/// очищаемая исходная позиция (0,0) один раз плюс, для КАЖДОЙ цели сдвига,
/// сама цель и цели `changes` относительно неё (репликация, не цепочка —
/// см. `RuleData::shift_targets`). Паттерн (чтение) сюда намеренно не
/// входит — см. doc-комментарий `RuleData::write_cells`.
fn compute_write_cells(rule: &Rule, shift_targets: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut cells = Vec::new();

    if shift_targets.is_empty() {
        for &(dx, dy, _value) in &rule.changes {
            cells.push((dx, dy));
        }
    } else {
        cells.push((0, 0)); // очищается apply_shift_buffered при каждом сдвиге
        for &(sdx, sdy) in shift_targets {
            cells.push((sdx, sdy));
            for &(dx, dy, _value) in &rule.changes {
                cells.push((sdx + dx, sdy + dy));
            }
        }
    }

    cells.sort();
    cells.dedup();
    cells
}

/// Вычислить affected cells для правила относительно позиции совпадения (0,0).
///
/// Affected cells включают:
/// 1. Ячейки паттерна (pattern) — читаются при сопоставлении.
/// 2. Начальная позиция головки (0,0) — очищается.
/// 3. Целевая клетка КАЖДОГО сдвига — записывается.
/// 4. Ячейки изменений (changes), по одному разу ОТНОСИТЕЛЬНО КАЖДОЙ цели
///    сдвига — записываются. Если сдвигов нет — относительно исходной
///    позиции (0,0).
pub fn compute_affected_cells(rule: &Rule) -> Vec<(i32, i32)> {
    let mut cells = Vec::new();

    // 1. Ячейки паттерна — читаются (эффективный паттерн, с id-fallback —
    // см. `effective_pattern`).
    for (dx, dy, _) in effective_pattern(rule) {
        cells.push((dx, dy));
    }

    // 2. Начальная позиция головки (0,0) — очищается
    if !cells.contains(&(0, 0)) {
        cells.push((0, 0));
    }

    // 3 и 4. Каждый сдвиг — своя независимая цель записи и свои changes
    // относительно неё (репликация, не цепочка — см. shift_targets).
    let shift_targets = compute_shift_targets(rule);
    if shift_targets.is_empty() {
        for &(dx, dy, _value) in &rule.changes {
            cells.push((dx, dy));
        }
    } else {
        for &(sdx, sdy) in &shift_targets {
            cells.push((sdx, sdy));
            for &(dx, dy, _value) in &rule.changes {
                cells.push((sdx + dx, sdy + dy));
            }
        }
    }

    // Удаляем дубликаты
    cells.sort();
    cells.dedup();

    cells
}

/// Дельта одного сдвига: (dx, dy) для его направления и числа шагов.
fn shift_delta(shift: &crate::types::ShiftSpec) -> (i32, i32) {
    match shift.direction {
        Direction::Up => (0, -(shift.steps as i32)),
        Direction::Down => (0, shift.steps as i32),
        Direction::Left => (-(shift.steps as i32), 0),
        Direction::Right => (shift.steps as i32, 0),
    }
}

/// Целевая клетка каждого отдельного сдвига правила (не суммарная — см.
/// комментарий `RuleData::shift_targets`).
pub fn compute_shift_targets(rule: &Rule) -> Vec<(i32, i32)> {
    let mut targets = Vec::new();
    for shift_group in &rule.shifts {
        for shift in shift_group {
            targets.push(shift_delta(shift));
        }
    }
    targets
}

/// Вычислить суммарный сдвиг правила (сумма дельт всех сдвигов).
/// Осмыслен только когда сдвиг ровно один — см. `compute_shift_targets`.
pub fn compute_total_shift(rule: &Rule) -> (i32, i32) {
    compute_shift_targets(rule)
        .into_iter()
        .fold((0, 0), |(ax, ay), (dx, dy)| (ax + dx, ay + dy))
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

/// Проверить, конфликтует ли правило само с собой при срабатывании в двух
/// РАЗНЫХ (перекрывающихся) позициях одновременно.
///
/// Логика зеркальна `rules_conflict`, но смещение (0, 0) исключается: это
/// не два разных срабатывания, а одна и та же позиция матча.
fn rule_self_conflicts(rule: &Rule, data: &RuleData) -> bool {
    let (bb_min_x, bb_max_x, bb_min_y, bb_max_y) = data.bbox;

    let dx_min = bb_min_x - bb_max_x;
    let dx_max = bb_max_x - bb_min_x;
    let dy_min = bb_min_y - bb_max_y;
    let dy_max = bb_max_y - bb_min_y;

    for dx in dx_min..=dx_max {
        for dy in dy_min..=dy_max {
            if dx == 0 && dy == 0 {
                // Не два срабатывания, а одно и то же — не конфликт.
                continue;
            }

            let j_bb_min_x = bb_min_x + dx;
            let j_bb_max_x = bb_max_x + dx;
            let j_bb_min_y = bb_min_y + dy;
            let j_bb_max_y = bb_max_y + dy;

            if bb_max_x < j_bb_min_x || bb_min_x > j_bb_max_x {
                continue;
            }
            if bb_max_y < j_bb_min_y || bb_min_y > j_bb_max_y {
                continue;
            }

            if !can_match_simultaneously(rule, rule, data, data, dx, dy) {
                continue;
            }

            if affected_regions_overlap(data, data, dx, dy) {
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

/// Получить тип ячейки из паттерна правила по координатам. Fallback ниже
/// формулой идентичен `effective_pattern` — согласован с тем, что реально
/// лежит в `RuleData::pattern_cells` (которые эта функция и опрашивает из
/// `can_match_simultaneously`), так что здесь всегда находится валидный тип.
fn get_pattern_type(rule: &Rule, x: i32, y: i32) -> u8 {
    for (dx, dy, ct) in &rule.pattern {
        if *dx as i32 == x && *dy as i32 == y {
            return ct.0;
        }
    }
    // Если не найдено — fallback на id (правило описано без явного паттерна)
    if y == 0 {
        let idx = x as usize;
        if idx < rule.id.len() {
            return rule.id[idx].0;
        }
    }
    0
}

/// Проверить, пересекаются ли РЕАЛЬНЫЕ ЗАПИСИ (write_cells) двух правил при
/// данном смещении. `bbox` (посчитанный по affected_cells, т.е. включая
/// чтение) используется только как дешёвый консервативный предфильтр — он
/// шире реального множества записей, так что не может пропустить настоящее
/// пересечение записей, но может пропускать дальше пары без него (не беда,
/// точная проверка ниже по write_cells отбросит их).
///
/// Только запись-в-запись — настоящий конфликт: detect_matches всегда
/// читает состояние решётки ДО тика (см. `apply_matches`: запись идёт в
/// отдельный буфер и применяется атомарно после арбитража), поэтому
/// пересечение ЧТЕНИЙ (или чтения одного правила с записью другого) не
/// может вызвать гонку — оба видят одно и то же старое состояние
/// независимо от того, что решит арбитраж.
fn affected_regions_overlap(
    data_i: &RuleData,
    data_j: &RuleData,
    dx: i32,
    dy: i32,
) -> bool {
    // Если bounding box'ы (по affected_cells, надмножество write_cells) не
    // пересекаются — точно не пересекаются и write_cells.
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

    // Точное пересечение ЗАПИСЕЙ (не всех affected cells).
    for &(x_i, y_i) in &data_i.write_cells {
        for &(x_j, y_j) in &data_j.write_cells {
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
#[path = "conflict_analyzer_tests.rs"]
mod tests;
