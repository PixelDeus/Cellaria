//! Property-based тесты для арбитража и матчинга.
//!
//! Проверяют инварианты, которые должны держаться на ЛЮБОМ наборе правил и
//! состоянии решётки, а не только на конкретных примерах из unit-тестов.
//! Решётка и паттерны — полноценно 2D (случайные width/height, паттерны с
//! ненулевым dx И dy одновременно, сдвиги по всем 4 направлениям) — раньше
//! генератор проверял только строку 1×N, как и сам движок в своё время
//! начинался с 1D, прежде чем перейти к 2D-паттернам.
//!
//! 1. `prop_arbitrate_never_overlaps` — safety-инвариант арбитра: набор
//!    принятых matches никогда не затрагивает одну и ту же клетку дважды.
//!    Это должно выполняться ВСЕГДА, для любых правил (конфликтных или нет).
//! 2. `prop_conflict_free_rules_accept_everything` — компьютерная проверка
//!    заявленной теоремы CF ⊂ CA: если статический анализатор признал набор
//!    правил conflict-free, то арбитраж не должен отклонять НИ ОДНОГО match'а
//!    ни при каком состоянии решётки.
//! 3. `prop_match_rule_idx_points_to_matching_rule` — внутренняя
//!    согласованность матчера: `rule_idx` в каждом RuleMatch должен указывать
//!    именно на то правило (в отсортированном по приоритету Vec для этого
//!    head-типа), чей id совпадает с `rule_id` матча. Это тот инвариант,
//!    нарушение которого раньше приводило к тихой подмене правила при
//!    совпадающих id.
//! 4. `prop_apply_matches_order_independent` — прямая, независимая от
//!    внутреннего представления affected_cells проверка того, что арбитраж
//!    действительно исключил ВСЕ реальные конфликты: применяем один и тот
//!    же принятый набор matches в прямом и в обратном порядке и сравниваем
//!    итоговые состояния решётки. Если арбитраж пропустил конфликт (например
//!    из-за клэмпинга при `OverflowAction::Write` — реальная запись при
//!    overflow идёт на границу решётки, а анализ affected_cells считает в
//!    неограниченных абстрактных координатах и про клэмпинг не знает),
//!    результат будет зависеть от порядка — это и есть наблюдаемый признак
//!    "тихого" конфликта, необнаружимого через одни только affected_cells.

use std::collections::{HashMap, HashSet};

use proptest::prelude::*;

use cellaria::conflict_analyzer::{build_rule_data_cache, get_rule_data, ConflictGraph};
use cellaria::engine::{apply_matches, arbitrate, detect_matches};
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Direction, OverflowAction, Rule, ShiftSpec};
use cellaria::{Grid, VecStorage};

const CELL_ALPHABET: u8 = 4; // типы клеток: 1..=CELL_ALPHABET (0 зарезервирован как "пусто")
const MIN_SIDE: usize = 4;
const MAX_SIDE: usize = 8;

fn cell_type_strategy() -> impl Strategy<Value = u8> {
    1..=CELL_ALPHABET
}

/// 2D-паттерн правила: голова всегда в (0,0), плюс 0..=2 дополнительных
/// клеток в окрестности ±2 по каждой оси. Раньше паттерн всегда строился
/// автоматически из `id` как плоская линия (dy≡0) — теперь генерируются
/// настоящие 2D-формы (уголки, кресты, диагонали).
fn pattern_strategy() -> impl Strategy<Value = (u8, Vec<(i8, i8, CellType)>)> {
    (
        cell_type_strategy(),
        prop::collection::vec((-2i8..=2, -2i8..=2, cell_type_strategy()), 0..=2),
    )
        .prop_map(|(head, extra)| {
            let mut seen: HashSet<(i8, i8)> = HashSet::new();
            seen.insert((0, 0));
            let mut cells = vec![(0i8, 0i8, CellType(head))];
            for (dx, dy, ct) in extra {
                if seen.insert((dx, dy)) {
                    cells.push((dx, dy, CellType(ct)));
                }
            }
            (head, cells)
        })
}

fn rule_strategy() -> impl Strategy<Value = Rule> {
    (
        pattern_strategy(),
        // 0..=2 независимых сдвигов (каждый — своя группа из одного
        // ShiftSpec). Правило с несколькими сдвигами реплицирует значение
        // головки в КАЖДУЮ цель независимо (не цепочка) — это то самое
        // поведение, из-за которого changes раньше позиционировались
        // неверно при 2+ сдвигах (см. RuleData::shift_targets).
        prop::collection::vec(
            (
                prop_oneof![
                    Just(Direction::Up),
                    Just(Direction::Down),
                    Just(Direction::Left),
                    Just(Direction::Right),
                ],
                1u16..=2,
            ),
            0..=2,
        ),
        // changes: (dx, dy, значение) — значение либо литерал, либо Ref(i)
        // (ссылка на i-ю клетку паттерна, включая иногда заведомо
        // невалидный индекс — проверяем fallback на CellValue::default()).
        prop::collection::vec(
            (
                -2i32..=2,
                -2i32..=2,
                prop_oneof![
                    (1u8..=9).prop_map(ChangeValue::Literal),
                    (0usize..=3).prop_map(ChangeValue::Ref),
                ],
            ),
            0..=2,
        ),
        0u32..=5, // priority
        0u64..=2, // min_age
        prop_oneof![
            Just(OverflowAction::Discard),
            cell_type_strategy().prop_map(OverflowAction::Write),
        ],
        // active_only — раньше вообще не генерировался (всегда false).
        any::<bool>(),
    )
        .prop_map(|((head, pattern), shift_specs, changes, priority, min_age, overflow, active_only)| {
            let shifts: Vec<Vec<ShiftSpec>> = shift_specs
                .into_iter()
                .map(|(dir, steps)| vec![ShiftSpec::new(dir, steps)])
                .collect();
            Rule {
                id: vec![CellType(head)],
                pattern,
                shifts,
                changes,
                active_only,
                priority,
                min_age,
                overflow,
            }
        })
}

fn rule_set_strategy() -> impl Strategy<Value = Vec<Rule>> {
    prop::collection::vec(rule_strategy(), 1..=4)
}

fn make_rule_index(rules: &[Rule]) -> HashMap<CellType, Vec<Rule>> {
    let mut index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        if let Some(&head) = rule.id.first() {
            index.entry(head).or_default().push(rule.clone());
        }
    }
    // Сортировка по приоритету — как это делает RuleStore::get_index в реальном движке.
    for group in index.values_mut() {
        group.sort_by_key(|r| std::cmp::Reverse(r.priority));
    }
    index
}

/// (width, height, содержимое width×height клеток, row-major).
fn grid_strategy() -> impl Strategy<Value = (usize, usize, Vec<u8>)> {
    (MIN_SIDE..=MAX_SIDE, MIN_SIDE..=MAX_SIDE).prop_flat_map(|(w, h)| {
        prop::collection::vec(0..=CELL_ALPHABET, w * h).prop_map(move |cells| (w, h, cells))
    })
}

fn build_grid(width: usize, height: usize, cells: &[u8]) -> Grid<VecStorage> {
    let storage = VecStorage::new(width, height);
    let mut grid = Grid::new(storage, HashSet::new());
    for (i, &v) in cells.iter().enumerate() {
        if v != 0 {
            let x = i % width;
            let y = i / width;
            grid.set_cell(x, y, Cell { value: CellValue(CellType(v)), born_at: 0 });
        }
    }
    grid
}

/// Абсолютная позиция affected cell с учётом клэмпинга — зеркалит логику
/// `arbitrator::get_match_affected_cells` (она приватна, поэтому дублируем
/// здесь как независимую проверку того же контракта). `shift_targets` —
/// цель КАЖДОГО отдельного сдвига правила (не суммарная), т.к. правило с
/// несколькими сдвигами реплицирует значение в каждую цель независимо.
fn affected_cell_abs(
    mx: i32,
    my: i32,
    dx: i32,
    dy: i32,
    shift_targets: &[(i32, i32)],
    overflow: OverflowAction,
    bounds: (usize, usize),
) -> (i32, i32) {
    let abs = (mx + dx, my + dy);
    let (w, h) = (bounds.0 as i32, bounds.1 as i32);
    if w > 0 && h > 0 && shift_targets.contains(&(dx, dy)) {
        if let OverflowAction::Write(_) = overflow {
            if abs.0 < 0 || abs.0 >= w || abs.1 < 0 || abs.1 >= h {
                return (abs.0.clamp(0, w - 1), abs.1.clamp(0, h - 1));
            }
        }
    }
    abs
}

proptest! {
    // CF (conflict-free) наборы редки при таком генераторе (маленький
    // алфавит типов, высокая вероятность пересечений) — без увеличения
    // лимита `prop_assume!` в prop_conflict_free_rules_accept_everything
    // будет отбрасывать почти все случаи и падать с "too many global
    // rejects", даже когда сам инвариант не нарушен.
    #![proptest_config(ProptestConfig { max_global_rejects: 200_000, cases: 128, ..ProptestConfig::default() })]

    /// Safety-инвариант: что бы арбитр ни принял, никакие два принятых
    /// match'а не должны затрагивать общую клетку. Держится ВСЕГДА,
    /// независимо от того, conflict-free набор правил или нет.
    #[test]
    fn prop_arbitrate_never_overlaps(
        rules in rule_set_strategy(),
        (width, height, cells) in grid_strategy(),
        advance_ticks in 0u32..=3,
    ) {
        let rule_index = make_rule_index(&rules);
        let mut grid = build_grid(width, height, &cells);
        for _ in 0..advance_ticks {
            grid.advance_age();
        }

        let active: Vec<(usize, usize)> = grid.iter_active().collect();
        let matches = detect_matches(&grid, &rule_index, &active);
        let rule_cache = build_rule_data_cache(&rule_index);
        let bounds = (grid.width(), grid.height());
        let accepted = arbitrate(matches, &rule_index, &rule_cache, bounds, |x, y| grid.get_age(x, y) as u32);

        let mut used: HashSet<(i32, i32)> = HashSet::new();
        for m in &accepted {
            let head = m.rule_id[0];
            let rd = get_rule_data(&rule_cache, head, m.rule_idx)
                .expect("rule_data должна быть в кэше для принятого match'а");
            let overflow = rule_index
                .get(&head)
                .and_then(|rules| rules.get(m.rule_idx))
                .expect("rule_idx должен указывать на реальное правило")
                .overflow;
            // Дедуп внутри ОДНОГО match'а: после клэмпинга цель сдвига может
            // совпасть с его же origin-клеткой (например, сдвиг вправо с
            // последней колонки при OverflowAction::Write клэмпится обратно
            // на ту же колонку) — это не конфликт МЕЖДУ матчами, а особенность
            // применения одного match'а самого по себе, и не должно
            // засчитываться как нарушение safety-инварианта арбитра.
            let own_cells: HashSet<(i32, i32)> = rd
                .affected_cells
                .iter()
                .map(|&(dx, dy)| affected_cell_abs(m.x as i32, m.y as i32, dx, dy, &rd.shift_targets, overflow, bounds))
                .collect();
            for coord in own_cells {
                prop_assert!(
                    used.insert(coord),
                    "клетка {:?} затронута двумя принятыми matches одновременно — safety-инвариант арбитра нарушен",
                    coord
                );
            }
        }
    }

    /// Компьютерная проверка теоремы CF ⊂ CA: если ConflictGraph признал
    /// набор правил conflict-free, арбитраж не должен отклонить НИ ОДНОГО
    /// match'а ни при каком состоянии решётки.
    #[test]
    fn prop_conflict_free_rules_accept_everything(
        rules in rule_set_strategy(),
        (width, height, cells) in grid_strategy(),
        advance_ticks in 0u32..=3,
    ) {
        prop_assume!(ConflictGraph::build(&rules).is_conflict_free());

        let rule_index = make_rule_index(&rules);
        let mut grid = build_grid(width, height, &cells);
        for _ in 0..advance_ticks {
            grid.advance_age();
        }

        let active: Vec<(usize, usize)> = grid.iter_active().collect();
        let matches = detect_matches(&grid, &rule_index, &active);
        let rule_cache = build_rule_data_cache(&rule_index);
        let accepted = arbitrate(matches.clone(), &rule_index, &rule_cache, (grid.width(), grid.height()), |x, y| grid.get_age(x, y) as u32);

        prop_assert_eq!(
            accepted.len(),
            matches.len(),
            "набор правил признан conflict-free, но арбитраж отклонил {} из {} matches",
            matches.len() - accepted.len(),
            matches.len()
        );
    }

    /// Внутренняя согласованность матчера: rule_idx каждого RuleMatch должен
    /// указывать на правило с тем же id в отсортированном по приоритету
    /// Vec для этого head-типа. Это ровно тот инвариант, чьё нарушение
    /// раньше приводило к тихой подмене правила при совпадающих id.
    #[test]
    fn prop_match_rule_idx_points_to_matching_rule(
        rules in rule_set_strategy(),
        (width, height, cells) in grid_strategy(),
        advance_ticks in 0u32..=3,
    ) {
        let rule_index = make_rule_index(&rules);
        let mut grid = build_grid(width, height, &cells);
        for _ in 0..advance_ticks {
            grid.advance_age();
        }

        let active: Vec<(usize, usize)> = grid.iter_active().collect();
        let matches = detect_matches(&grid, &rule_index, &active);

        for m in &matches {
            let head = m.rule_id[0];
            let group = rule_index.get(&head).expect("head-тип матча должен быть в rule_index");
            let resolved = group.get(m.rule_idx).expect("rule_idx должен быть валидным индексом в группе");
            prop_assert_eq!(
                &resolved.id, &m.rule_id,
                "rule_idx={} в матче на ({}, {}) указывает на правило с id={:?}, а сам матч несёт rule_id={:?}",
                m.rule_idx, m.x, m.y, resolved.id, m.rule_id
            );
        }
    }

    /// Применяем принятый набор matches в прямом и обратном порядке —
    /// результат обязан совпадать. Расхождение означает, что арбитраж
    /// пропустил реальный конфликт (например, из-за клэмпинга при
    /// OverflowAction::Write, который affected_cells не моделирует).
    #[test]
    fn prop_apply_matches_order_independent(
        rules in rule_set_strategy(),
        (width, height, cells) in grid_strategy(),
        advance_ticks in 0u32..=3,
    ) {
        let rule_index = make_rule_index(&rules);
        let mut grid = build_grid(width, height, &cells);
        for _ in 0..advance_ticks {
            grid.advance_age();
        }

        let active: Vec<(usize, usize)> = grid.iter_active().collect();
        let matches = detect_matches(&grid, &rule_index, &active);
        let rule_cache = build_rule_data_cache(&rule_index);
        let accepted = arbitrate(matches, &rule_index, &rule_cache, (grid.width(), grid.height()), |x, y| grid.get_age(x, y) as u32);

        let mut grid_forward = grid.clone();
        apply_matches(&mut grid_forward, accepted.clone(), &rule_index, &rule_cache);

        let mut reversed = accepted.clone();
        reversed.reverse();
        let mut grid_backward = grid.clone();
        apply_matches(&mut grid_backward, reversed, &rule_index, &rule_cache);

        for y in 0..height {
            for x in 0..width {
                let a = grid_forward.get_cell(x, y).map(|c| c.value);
                let b = grid_backward.get_cell(x, y).map(|c| c.value);
                prop_assert_eq!(
                    a, b,
                    "результат apply_matches на клетке ({}, {}) зависит от порядка matches в принятом наборе — арбитраж пропустил реальный конфликт",
                    x, y
                );
            }
        }
    }

    /// Корректность `ChangeValue::Ref`: записанное значение должно быть
    /// именно тем, которое было в i-й клетке паттерна на МОМЕНТ НАЧАЛА
    /// тика (до любых shifts/changes — своих или чужих matches), не после.
    /// Индекс вне диапазона паттерна — фолбэк на `CellValue::default()`.
    #[test]
    fn prop_change_ref_reads_pre_tick_value(
        rules in rule_set_strategy(),
        (width, height, cells) in grid_strategy(),
        advance_ticks in 0u32..=3,
    ) {
        let rule_index = make_rule_index(&rules);
        let mut grid = build_grid(width, height, &cells);
        for _ in 0..advance_ticks {
            grid.advance_age();
        }

        let active: Vec<(usize, usize)> = grid.iter_active().collect();
        let matches = detect_matches(&grid, &rule_index, &active);
        let rule_cache = build_rule_data_cache(&rule_index);
        let bounds = (grid.width(), grid.height());
        let accepted = arbitrate(matches, &rule_index, &rule_cache, bounds, |x, y| grid.get_age(x, y) as u32);

        // Ожидаемые значения на клетках, куда пишет Ref-change, — из
        // ИСХОДНОЙ (до apply_matches) решётки. HashMap, не Vec: если
        // несколько записей одного match'а метят одну клетку, побеждает
        // последняя по порядку rule.changes — так же, как write_buffer
        // внутри apply_rule_buffered (insert перезаписывает).
        let mut expected: HashMap<(u32, u32), CellValue> = HashMap::new();
        for m in &accepted {
            let head = m.rule_id[0];
            let rule = rule_index
                .get(&head)
                .and_then(|rs| rs.get(m.rule_idx))
                .expect("rule_idx должен указывать на реальное правило");
            if rule.changes.is_empty() {
                continue;
            }

            let pattern_vals: Vec<CellValue> = rule
                .pattern
                .iter()
                .map(|(dx, dy, _)| {
                    let px = m.x as i32 + *dx as i32;
                    let py = m.y as i32 + *dy as i32;
                    if px >= 0 && py >= 0 {
                        grid.get_cell(px as usize, py as usize)
                            .map(|c| c.value)
                            .unwrap_or_default()
                    } else {
                        CellValue::default()
                    }
                })
                .collect();

            let rd = get_rule_data(&rule_cache, head, m.rule_idx)
                .expect("rule_data должна быть в кэше");
            let apply_points: Vec<(i32, i32)> = if rd.shift_targets.is_empty() {
                vec![(0, 0)]
            } else {
                rd.shift_targets.clone()
            };

            // Обрабатываем ВСЕ changes по порядку (не только Ref) — иначе,
            // если два change'а одного правила метят одно и то же смещение
            // (один Ref, другой Literal), позже стоящий в rule.changes
            // молча перезаписывает более ранний и в реальном write_buffer, и
            // здесь — а мы должны ожидать именно то, что реально победило.
            for &(base_dx, base_dy) in &apply_points {
                for &(dx, dy, ref value) in &rule.changes {
                    let nx = m.x as i32 + base_dx + dx;
                    let ny = m.y as i32 + base_dy + dy;
                    if nx >= 0 && ny >= 0 && (nx as usize) < width && (ny as usize) < height {
                        let val = match *value {
                            ChangeValue::Literal(v) => CellValue(CellType::new(v)),
                            ChangeValue::Ref(i) => pattern_vals.get(i).copied().unwrap_or_default(),
                        };
                        expected.insert((nx as u32, ny as u32), val);
                    }
                }
            }
        }

        let mut result_grid = grid.clone();
        apply_matches(&mut result_grid, accepted, &rule_index, &rule_cache);

        for ((x, y), expected_val) in expected {
            let actual = result_grid.get_cell(x as usize, y as usize).map(|c| c.value);
            prop_assert_eq!(
                actual, Some(expected_val),
                "Ref-change на клетке ({}, {}) должен был записать значение из паттерна на момент начала тика",
                x, y
            );
        }
    }
}
