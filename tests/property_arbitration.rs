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
use cellaria::engine::{apply_matches, arbitrate, arbitrate_spatial, detect_matches};
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Direction, OverflowAction, Rule, RuleMatch, ShiftSpec};
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
        .prop_map(
            |((head, pattern), shift_specs, changes, priority, min_age, overflow, active_only)| {
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
                    cam: None,
                    tie_break: 0,
                    starvation_after: None,
                    feedback: None,
                    recursion: None,
                    memory: None,
                    max_activations: None,
                    cross_layer_reads: Vec::new(),
                }
            },
        )
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
    (MIN_SIDE..=MAX_SIDE, MIN_SIDE..=MAX_SIDE)
        .prop_flat_map(|(w, h)| prop::collection::vec(0..=CELL_ALPHABET, w * h).prop_map(move |cells| (w, h, cells)))
}

fn build_grid(width: usize, height: usize, cells: &[u8]) -> Grid<VecStorage> {
    let storage = VecStorage::new(width, height);
    let mut grid = Grid::new(storage, HashSet::new());
    for (i, &v) in cells.iter().enumerate() {
        if v != 0 {
            let x = i % width;
            let y = i / width;
            grid.set_cell(
                x,
                y,
                Cell {
                    value: CellValue(CellType(v)),
                    born_at: 0,
                },
            );
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

/// Независимый (не переиспользующий `applicator::resolve_change_value`)
/// эталон вычисления `ChangeValue` -- та же рекурсивная семантика
/// (`wrapping_add`/`wrapping_sub`, `Ref` вне длины паттерна -> дефолт), но
/// написана отдельно, чтобы этот тест реально сверял ДВЕ независимые
/// реализации, а не одну и ту же логику саму с собой.
fn resolve_change_value_reference(value: &ChangeValue, pattern_vals: &[CellValue]) -> CellValue {
    match value {
        ChangeValue::Literal(v) => CellValue::new(*v),
        ChangeValue::Ref(i) => pattern_vals.get(*i).copied().unwrap_or_default(),
        ChangeValue::Add(a, b) => {
            let av = resolve_change_value_reference(a, pattern_vals).0 .0;
            let bv = resolve_change_value_reference(b, pattern_vals).0 .0;
            CellValue::new(av.wrapping_add(bv))
        }
        ChangeValue::Sub(a, b) => {
            let av = resolve_change_value_reference(a, pattern_vals).0 .0;
            let bv = resolve_change_value_reference(b, pattern_vals).0 .0;
            CellValue::new(av.wrapping_sub(bv))
        }
    }
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
            let head = m.head;
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
            //
            // write_cells, не affected_cells: арбитр (arbitrator.rs) гарантирует
            // отсутствие пересечения только на реальных ЗАПИСЯХ — два match'а
            // могут легитимно читать общую клетку паттерна одновременно
            // (detect_matches всегда смотрит на состояние решётки до тика),
            // это не safety-нарушение и не должно тут проверяться.
            let own_cells: HashSet<(i32, i32)> = rd
                .write_cells
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
            let head = m.head;
            let group = rule_index.get(&head).expect("head-тип матча должен быть в rule_index");
            let resolved = group.get(m.rule_idx).expect("rule_idx должен быть валидным индексом в группе");
            // `RuleMatch` больше не несёт клон полного `id` (см. её doc-
            // комментарий) — но инвариант "голова совпадения совпадает с
            // головой правила, на которое указывает rule_idx" всё ещё стоит
            // проверять: он гарантирован построением `rule_index` (группировка
            // по `id.first()`), и регрессия здесь означала бы, что
            // `detect_matches` кладёт совпадение не в ту головную группу.
            prop_assert_eq!(
                resolved.id.first().copied(), Some(m.head),
                "rule_idx={} в матче на ({}, {}) указывает на правило с id={:?}, но голова матча head={:?}",
                m.rule_idx, m.x, m.y, resolved.id, m.head
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
            let head = m.head;
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
                        let val = resolve_change_value_reference(value, &pattern_vals);
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

    /// `engine::run_tick` использует инкрементальный скан: вместо полного
    /// пересмотра всех активных клеток каждый тик он берёт только клетки,
    /// изменившиеся с прошлого тика (`Grid::dirty_coords`), расширенные на
    /// радиус паттернов, плюс клетки типов с `min_age > 0` (см. doc-комментарии
    /// `resolve_search_coords_advance`/`min_age_gated_types` в `engine::mod`).
    /// Это отдельный, независимый от `prop_arbitrate_never_overlaps` риск:
    /// та проверка берёт `active_coords` напрямую и не касается
    /// dirty-tracking вообще.
    ///
    /// Сравниваем N тиков реального (инкрементального) run_tick с N тиками
    /// заведомо консервативного эталона: полный скан всех активных клеток,
    /// расширенный ФИКСИРОВАННЫМ радиусом 2 (генератор паттернов/changes
    /// этого файла ограничен offset'ами -2..=2, так что радиус 2 гарантированно
    /// достаточен для эталона — независимо от того, как настоящий движок
    /// вычисляет свой радиус). Если инкрементальный скан хоть раз пропустит
    /// реальное совпадение (или наоборот, лишний раз что-то заденет), решётки
    /// разойдутся на том же тике, где это произошло.
    #[test]
    fn prop_incremental_run_tick_matches_full_scan(
        rules in rule_set_strategy(),
        (width, height, cells) in grid_strategy(),
        num_ticks in 1u32..=6,
    ) {
        let rule_index = make_rule_index(&rules);

        let mut incremental = build_grid(width, height, &cells);
        let mut reference = build_grid(width, height, &cells);

        for tick in 0..num_ticks {
            let _ = cellaria::engine::run_tick(&mut incremental, &rule_index);
            reference_full_scan_tick(&mut reference, &rule_index);

            for y in 0..height {
                for x in 0..width {
                    let a = incremental.get_cell(x, y).copied().unwrap_or_default();
                    let b = reference.get_cell(x, y).copied().unwrap_or_default();
                    prop_assert_eq!(
                        a, b,
                        "клетка ({}, {}) разошлась на тике {} между инкрементальным run_tick и полным сканом",
                        x, y, tick
                    );
                }
            }
        }
    }
}

/// Эталонный тик: ВСЕГДА полный скан всех активных клеток, расширенный
/// фиксированным радиусом 2, без какого-либо dirty-tracking. Независимая от
/// оптимизированной реализации (`engine::mod::resolve_search_coords_advance`)
/// проверка того же контракта — намеренно НЕ переиспользует её код.
fn reference_full_scan_tick(grid: &mut Grid<VecStorage>, rule_index: &HashMap<CellType, Vec<Rule>>) {
    const RADIUS: i32 = 2;

    let active: Vec<(usize, usize)> = grid.iter_active().collect();
    let mut expanded: HashSet<(usize, usize)> = HashSet::new();
    for &(x, y) in &active {
        for dx in -RADIUS..=RADIUS {
            for dy in -RADIUS..=RADIUS {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx >= 0 && ny >= 0 {
                    expanded.insert((nx as usize, ny as usize));
                }
            }
        }
    }
    let search_coords: Vec<(usize, usize)> = expanded.into_iter().collect();

    // run_tick ВСЕГДА вызывает advance_age() ровно один раз за тик, даже если
    // совпадений не нашлось или все отклонены арбитражем — иначе время
    // (generation, а с ним и возраст любой клетки) замирает навсегда, как
    // только на решётке не осталось ничего, кроме `min_age`-клетки,
    // ожидающей своего часа: она никогда бы его не дождалась.
    let matches = detect_matches(grid, rule_index, &search_coords);
    if matches.is_empty() {
        grid.advance_age();
        return;
    }
    let rule_cache = build_rule_data_cache(rule_index);
    let accepted = arbitrate(
        matches,
        rule_index,
        &rule_cache,
        (grid.width(), grid.height()),
        |x, y| grid.get_age(x, y) as u32,
    );
    if accepted.is_empty() {
        grid.advance_age();
        return;
    }
    let (regions, _outputs) = apply_matches(grid, accepted, rule_index, &rule_cache);
    grid.advance_age();

    // written_cells, не bbox: прямоугольник между исходной и целевой позицией
    // сдвига на N>1 клеток включает клетки, которые сдвиг не трогает вовсе —
    // см. doc-комментарий `AffectedRegion::written_cells` и
    // `reset_age_for_regions` в engine/mod.rs.
    let gen = grid.generation();
    for region in &regions {
        for &(x, y) in &region.written_cells {
            if let Some(cell) = grid.get_cell(x as usize, y as usize) {
                grid.set_cell(
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

// ============================================================================
// arbitrate_spatial (Theorem 6, paper2.md §6.2) — крупномасштабная проверка
// ============================================================================
//
// `arbitrate_spatial` ниже `SPATIAL_THRESHOLD` (4096 матчей) просто зовёт
// `arbitrate` напрямую — существующие property-тесты выше (решётки 4×4..8×8)
// никогда не производят столько матчей и НИКОГДА не задевают реальное
// разбиение на полосы. Эта проверка — единственная, что реально прогоняет
// код полос: тысячи матчей, множество независимых "кластеров" конфликта
// (M конкурирующих правил на одной клетке — как `configs`'ный сценарий
// приоритетного конфликта), разнесённых далеко друг от друга по x.

fn conflict_cluster_rule_index(m: usize) -> (HashMap<CellType, Vec<Rule>>, cellaria::conflict_analyzer::RuleDataCache) {
    // M правил, все читают (0,0)=1 И (1,0)=2 (radius=1, чтобы `reach` не был
    // тривиальным нулём), конкурируют по приоритету за клетку (0,0).
    let rules: Vec<Rule> = (0..m)
        .map(|i| Rule {
            id: vec![CellType(1)],
            pattern: vec![(0, 0, CellType(1)), (1, 0, CellType(2))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal((i % 250 + 1) as u8))],
            active_only: false,
            priority: i as u32,
            min_age: 0,
            overflow: OverflowAction::Discard,
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        })
        .collect();
    let rule_index = make_rule_index(&rules);
    let rule_cache = build_rule_data_cache(&rule_index);
    (rule_index, rule_cache)
}

#[test]
fn test_arbitrate_spatial_matches_centralized_many_isolated_clusters() {
    let (rule_index, rule_cache) = conflict_cluster_rule_index(5);
    const N_CLUSTERS: u32 = 1500; // 1500 * 5 = 7500 матчей > SPATIAL_THRESHOLD
    const SPACING: u32 = 1000; // >> 2*reach — кластеры гарантированно независимы
    const REACH: i32 = 1;

    let mut matches: Vec<RuleMatch> = Vec::new();
    for cluster in 0..N_CLUSTERS {
        let x = cluster * SPACING;
        for rule_idx in 0..5usize {
            matches.push(RuleMatch {
                x,
                y: 0,
                head: CellType(1),
                rule_idx,
            });
        }
    }

    let get_age = |x: usize, _y: usize| (x as u32).wrapping_mul(2654435761) % 7; // произвольный, но детерминированный "возраст"

    let centralized = arbitrate(
        matches.clone(),
        &rule_index,
        &rule_cache,
        (usize::MAX, usize::MAX),
        get_age,
    );
    let spatial = arbitrate_spatial(
        matches,
        &rule_index,
        &rule_cache,
        (usize::MAX, usize::MAX),
        REACH,
        get_age,
    );

    let centralized_set: HashSet<(u32, u32, usize)> = centralized.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();
    let spatial_set: HashSet<(u32, u32, usize)> = spatial.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();

    assert_eq!(
        centralized.len(),
        spatial.len(),
        "разное число принятых матчей: {} vs {}",
        centralized.len(),
        spatial.len()
    );
    assert_eq!(
        centralized_set, spatial_set,
        "разбиение на полосы должно принимать РОВНО ТЕ ЖЕ матчи, что централизованный арбитраж"
    );

    // Ровно один победитель на кластер (M конкурирующих правил за одну клетку).
    assert_eq!(
        spatial.len() as u32,
        N_CLUSTERS,
        "должен победить ровно один матч на кластер"
    );
}

#[test]
fn test_arbitrate_spatial_matches_centralized_dense_adjacent_clusters() {
    // То же самое, но кластеры расположены ПЛОТНО (spacing=2, сравнимо с
    // 2*reach=2) — многие матчи должны попасть в boundary-класс, а не core,
    // проверяя именно пограничный, последовательный путь.
    let (rule_index, rule_cache) = conflict_cluster_rule_index(3);
    const N_CLUSTERS: u32 = 3000;
    const SPACING: u32 = 2;
    const REACH: i32 = 1;

    let mut matches: Vec<RuleMatch> = Vec::new();
    for cluster in 0..N_CLUSTERS {
        let x = cluster * SPACING;
        for rule_idx in 0..3usize {
            matches.push(RuleMatch {
                x,
                y: 0,
                head: CellType(1),
                rule_idx,
            });
        }
    }

    let get_age = |x: usize, _y: usize| (x as u32).wrapping_mul(40503) % 5;

    let centralized = arbitrate(
        matches.clone(),
        &rule_index,
        &rule_cache,
        (usize::MAX, usize::MAX),
        get_age,
    );
    let spatial = arbitrate_spatial(
        matches,
        &rule_index,
        &rule_cache,
        (usize::MAX, usize::MAX),
        REACH,
        get_age,
    );

    let centralized_set: HashSet<(u32, u32, usize)> = centralized.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();
    let spatial_set: HashSet<(u32, u32, usize)> = spatial.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();

    assert_eq!(centralized.len(), spatial.len());
    assert_eq!(
        centralized_set, spatial_set,
        "плотная упаковка (много boundary-матчей) не должна расходиться с централизованным арбитражем"
    );
}

/// Lemma 6 (спатиальная декомпозиция арбитража) доказана для модели БЕЗ
/// `recursion` — эта дыра не была закрыта эмпирически ни разу: обе функции
/// выше используют `pattern`-only правила, `recursion: None` жёстко зашит
/// (см. `conflict_cluster_rule_index`). `recursion` расширяет РЕАЛЬНУЮ
/// зону записи матча за пределы одного affected-региона (каскад
/// `k=1..=max_depth`), а `arbitrate_spatial`'s `margin = 2*reach` обязан
/// это учитывать — теоретически ЧЕРЕЗ `RuleData::write_cells` (union дисков
/// всех уровней каскада, см. `compute_rule_data`), практически — никогда не
/// проверено на большом (>SPATIAL_THRESHOLD) наборе матчей, где boundary-путь
/// вообще запускается. `reach` здесь — НЕ константа "1", а вычислен из
/// РЕАЛЬНОГО `RuleData::bbox` (та же формула, что `Engine::max_affected_radius`
/// в mod.rs) — тест проверяет саму АЛГОРИТМИЧЕСКУЮ корректность band-split
/// при данном (корректном) reach, не корректность вычисления reach как
/// такового.
fn recursion_cluster_rule_index(
    m: usize,
    max_depth: u8,
) -> (HashMap<CellType, Vec<Rule>>, cellaria::conflict_analyzer::RuleDataCache) {
    let rules: Vec<Rule> = (0..m)
        .map(|i| Rule {
            id: vec![CellType(1)],
            pattern: vec![(0, 0, CellType(1)), (1, 0, CellType(2))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal((i % 250 + 1) as u8))],
            active_only: false,
            priority: i as u32,
            min_age: 0,
            overflow: OverflowAction::Discard,
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: Some(cellaria::types::RecursionSpec {
                max_depth,
                direction: Direction::Right,
            }),
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        })
        .collect();
    let rule_index = make_rule_index(&rules);
    let rule_cache = build_rule_data_cache(&rule_index);
    (rule_index, rule_cache)
}

/// Реальный `reach` из `RuleData::bbox` — та же формула, что
/// `compute_conflict_partners` в `mod.rs` использует для
/// `Engine::max_affected_radius`, воспроизведена здесь напрямую (тест не
/// имеет доступа к `pub(crate)`-функции), чтобы не полагаться на угаданное
/// число.
fn max_reach_from_cache(rule_cache: &cellaria::conflict_analyzer::RuleDataCache) -> i32 {
    rule_cache
        .iter()
        .filter_map(|opt| opt.as_ref())
        .flat_map(|rules| rules.iter())
        .map(|data| {
            let (min_x, max_x, min_y, max_y) = data.bbox;
            min_x
                .unsigned_abs()
                .max(max_x.unsigned_abs())
                .max(min_y.unsigned_abs())
                .max(max_y.unsigned_abs()) as i32
        })
        .max()
        .unwrap_or(0)
}

#[test]
fn test_arbitrate_spatial_matches_centralized_recursion_isolated_clusters() {
    const MAX_DEPTH: u8 = 3;
    let (rule_index, rule_cache) = recursion_cluster_rule_index(5, MAX_DEPTH);
    let reach = max_reach_from_cache(&rule_cache);
    assert!(reach > 1, "recursion must genuinely widen the reach beyond the pattern's own 1 -- otherwise this test isn't exercising recursion's extra margin at all");

    const N_CLUSTERS: u32 = 1500;
    let spacing = (reach as u32) * 1000; // намного больше 2*reach -- кластеры гарантированно независимы
    let mut matches: Vec<RuleMatch> = Vec::new();
    for cluster in 0..N_CLUSTERS {
        let x = cluster * spacing;
        for rule_idx in 0..5usize {
            matches.push(RuleMatch {
                x,
                y: 0,
                head: CellType(1),
                rule_idx,
            });
        }
    }

    let get_age = |x: usize, _y: usize| (x as u32).wrapping_mul(2654435761) % 7;

    let centralized = arbitrate(
        matches.clone(),
        &rule_index,
        &rule_cache,
        (usize::MAX, usize::MAX),
        get_age,
    );
    let spatial = arbitrate_spatial(
        matches,
        &rule_index,
        &rule_cache,
        (usize::MAX, usize::MAX),
        reach,
        get_age,
    );

    let centralized_set: HashSet<(u32, u32, usize)> = centralized.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();
    let spatial_set: HashSet<(u32, u32, usize)> = spatial.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();

    assert_eq!(
        centralized.len(),
        spatial.len(),
        "разное число принятых матчей: {} vs {}",
        centralized.len(),
        spatial.len()
    );
    assert_eq!(centralized_set, spatial_set, "разбиение на полосы с recursion-расширенным reach должно принимать РОВНО ТЕ ЖЕ матчи, что централизованный арбитраж");
    assert_eq!(
        spatial.len() as u32,
        N_CLUSTERS,
        "должен победить ровно один матч на кластер"
    );
}

#[test]
fn test_arbitrate_spatial_matches_centralized_recursion_dense_overlapping_writes() {
    // В отличие от изолированных кластеров выше (spacing >> reach — соседние
    // кластеры НИКОГДА физически не пересекаются, каким бы ни был margin, а
    // значит и не проверяют его корректность вообще — обнаружено эмпирически
    // при попытке сабо­тажа margin, который ни isolated, ни первоначальный
    // dense (spacing=2*reach, тот же недостаток) вариант не ловили): здесь
    // анкоры стоят ВПЛОТНУЮ (spacing=1) вдоль всей линии — recursion-запись
    // каждого анкора (расширенная на `reach` клеток) физически ПЕРЕСЕКАЕТСЯ
    // с записью нескольких соседних анкоров. Это создаёт РЕАЛЬНУЮ
    // конкуренцию через границы будущих полос band-split, а не только
    // формальное попадание в boundary-класс без физических последствий —
    // именно тот сценарий, где заниженный/неверный margin реально дал бы
    // spatial != centralized. Полностью аналогично методологии, уже
    // применённой в этой сессии к CA-"пробке" (сплошной, не изолированный
    // конфликт).
    const MAX_DEPTH: u8 = 3;
    let (rule_index, rule_cache) = recursion_cluster_rule_index(3, MAX_DEPTH);
    let reach = max_reach_from_cache(&rule_cache);
    assert!(
        reach > 1,
        "recursion must genuinely widen the reach beyond the pattern's own 1"
    );

    const N_ANCHORS: u32 = 3000;
    let mut matches: Vec<RuleMatch> = Vec::new();
    for x in 0..N_ANCHORS {
        for rule_idx in 0..3usize {
            matches.push(RuleMatch {
                x,
                y: 0,
                head: CellType(1),
                rule_idx,
            });
        }
    }

    let get_age = |x: usize, _y: usize| (x as u32).wrapping_mul(40503) % 5;

    let centralized = arbitrate(
        matches.clone(),
        &rule_index,
        &rule_cache,
        (usize::MAX, usize::MAX),
        get_age,
    );
    let spatial = arbitrate_spatial(
        matches,
        &rule_index,
        &rule_cache,
        (usize::MAX, usize::MAX),
        reach,
        get_age,
    );

    let centralized_set: HashSet<(u32, u32, usize)> = centralized.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();
    let spatial_set: HashSet<(u32, u32, usize)> = spatial.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();

    assert_eq!(
        centralized.len(),
        spatial.len(),
        "разное число принятых матчей: {} vs {}",
        centralized.len(),
        spatial.len()
    );
    assert_eq!(
        centralized_set, spatial_set,
        "плотная упаковка recursion-матчей не должна расходиться с централизованным арбитражем"
    );
}

/// Тот же класс стресс-теста, что нашёл реальный баг boundary-vs-core (см.
/// CHANGELOG `[0.7.0] / Fixed`), но для ОБЫЧНОГО сдвига (`shifts`, без
/// `recursion`, без `cam`) — САМОГО распространённого случая, который
/// НИ РАЗУ не проверялся так до сегодня: `conflict_cluster_rule_index`
/// (тесты выше) всегда использовал `shifts: vec![]`. CA-"пробка"
/// (`300×300`, сплошной сдвиг вправо), которую эта сессия гоняла весь день
/// ради замеров производительности (`Engine::run_tick_profiled`), легко
/// превышает `SPATIAL_THRESHOLD=4096` матчей за тик — но НИ РАЗУ не
/// сверялась с централизованным эталоном, только замерялось время. Этот
/// тест — прямая, целенаправленная проверка именно этого пробела.
#[test]
fn test_arbitrate_spatial_matches_centralized_plain_shift_dense_overlapping_writes() {
    // Анкоры вплотную (spacing=1), каждый сдвигается на 1 клетку вправо —
    // write_cells = {source(0,0), target(1,0)}, bbox даёт reach=1. Anchor
    // x пишет {x,x+1}, anchor x+1 пишет {x+1,x+2} -- гарантированное
    // пересечение на x+1 для КАЖДОЙ соседней пары, ровно та же плотность
    // конкуренции, что и в реальной CA-"пробке".
    let rules: Vec<Rule> = (0..3)
        .map(|i| Rule {
            id: vec![CellType(1)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
            changes: vec![],
            active_only: false,
            priority: i as u32,
            min_age: 0,
            overflow: OverflowAction::Discard,
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        })
        .collect();
    let rule_index = make_rule_index(&rules);
    let rule_cache = build_rule_data_cache(&rule_index);
    let reach = max_reach_from_cache(&rule_cache);
    assert_eq!(reach, 1, "простой сдвиг на 1 клетку -- reach обязан быть ровно 1");

    const N_ANCHORS: u32 = 3000; // × 3 rule_idx = 9000 > SPATIAL_THRESHOLD=4096
    let mut matches: Vec<RuleMatch> = Vec::new();
    for x in 0..N_ANCHORS {
        for rule_idx in 0..3usize {
            matches.push(RuleMatch {
                x,
                y: 0,
                head: CellType(1),
                rule_idx,
            });
        }
    }

    let get_age = |x: usize, _y: usize| (x as u32).wrapping_mul(40503) % 5;

    let centralized = arbitrate(
        matches.clone(),
        &rule_index,
        &rule_cache,
        (usize::MAX, usize::MAX),
        get_age,
    );
    let spatial = arbitrate_spatial(
        matches,
        &rule_index,
        &rule_cache,
        (usize::MAX, usize::MAX),
        reach,
        get_age,
    );

    let centralized_set: HashSet<(u32, u32, usize)> = centralized.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();
    let spatial_set: HashSet<(u32, u32, usize)> = spatial.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();

    assert_eq!(
        centralized.len(),
        spatial.len(),
        "разное число принятых матчей: {} vs {}",
        centralized.len(),
        spatial.len()
    );
    assert_eq!(centralized_set, spatial_set, "плотная упаковка ОБЫЧНЫХ сдвигов не должна расходиться с централизованным арбитражем -- самый частый случай, впервые проверен только сегодня");
}

/// РЕАЛЬНЫЙ баг, найден 2026-08-11 при замере плотной производительности на
/// масштабе ~1M клеток (для проекта городской симуляции): "RefCell already
/// borrowed" panic. `arbitrator.rs`'s `KEYED_BUF.par_sort_unstable_by`
/// раньше вызывался ВНУТРИ заимствования `thread_local!`-буфера --
/// rayon'овский work-stealing может увести ТОТ ЖЕ OS-поток на ДРУГОЙ вызов
/// `arbitrate_with_cam` (соседняя полоса, работающая с ТЕМ ЖЕ thread_local),
/// пока первый ещё не вернулся из сортировки (ждёт свои подзадачи через
/// `join`) -- второй вызов пытается занять тот же `RefCell` и падает.
/// Никогда не ловилось раньше: нужен per-полосный размер >=
/// `PARALLEL_SORT_THRESHOLD=1024`, чего ни один тест до сегодняшнего замера
/// на 1M клеток не давал. Этот тест — дешёвая (не 1M клеток) прямая
/// проверка того же условия: много полос (`rayon::current_num_threads()`),
/// каждая гарантированно > 1024 матчей.
#[test]
fn test_arbitrate_spatial_no_refcell_reentrancy_panic_with_many_matches_per_band() {
    let num_bands_hint = rayon::current_num_threads().max(1);
    // С запасом x2, чтобы per-полосный размер надёжно перевалил за
    // PARALLEL_SORT_THRESHOLD=1024 независимо от точного деления на core/
    // boundary.
    let per_band = 2500usize;
    let total_anchors = num_bands_hint * per_band;

    let rules: Vec<Rule> = (0..2)
        .map(|i| Rule {
            id: vec![CellType(1)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
            changes: vec![],
            active_only: false,
            priority: i as u32,
            min_age: 0,
            overflow: OverflowAction::Discard,
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        })
        .collect();
    let rule_index = make_rule_index(&rules);
    let rule_cache = build_rule_data_cache(&rule_index);
    let reach = max_reach_from_cache(&rule_cache);

    let mut matches: Vec<RuleMatch> = Vec::new();
    for x in 0..total_anchors as u32 {
        for rule_idx in 0..2usize {
            matches.push(RuleMatch {
                x,
                y: 0,
                head: CellType(1),
                rule_idx,
            });
        }
    }

    let get_age = |x: usize, _y: usize| (x as u32).wrapping_mul(2654435761) % 7;

    // Несколько прогонов подряд -- гонка timing-зависимая, один прогон
    // может случайно не задеть её (см. сессию: понадобилось 3 тика на
    // 1M клеток, чтобы проявилось первый раз).
    for _ in 0..5 {
        let centralized = arbitrate(
            matches.clone(),
            &rule_index,
            &rule_cache,
            (usize::MAX, usize::MAX),
            get_age,
        );
        let spatial = arbitrate_spatial(
            matches.clone(),
            &rule_index,
            &rule_cache,
            (usize::MAX, usize::MAX),
            reach,
            get_age,
        );
        assert_eq!(
            centralized.len(),
            spatial.len(),
            "разное число принятых матчей: {} vs {}",
            centralized.len(),
            spatial.len()
        );
    }
}

/// Целенаправленная (не property, не случайная) проверка остаточного
/// теоретического риска, задокументированного в `specs/architecture.md`
/// §11 п.5 при фиксе бага boundary-vs-core: `at_risk`-победитель полосы,
/// проигравший boundary-конкуренту в финальном merge-проходе, не даёт
/// шанса более слабому кандидату СВОЕЙ ЖЕ полосы, отвергнутому ещё на
/// этапе локального арбитража полосы -- хотя истинный централизованный
/// порядок мог бы отдать клетку именно ему (греди-арбитраж: отклонение
/// сильного кандидата ОСВОБОЖДАЕТ клетки, которые он бы забрал, и это
/// может открыть путь более слабому конкуренту, если тот претендовал
/// ТОЛЬКО на эти клетки, а не на всю область сильного).
///
/// Конструкция трёх конкурентов на одну "клетку C" вплотную к границе
/// полосы (аналитически выведено, не угадано):
/// - L (rule_idx 0, priority 0) -- пишет ТОЛЬКО {C}.
/// - W (rule_idx 1, priority 1) -- пишет {C, C+2} (reach=2 от этого правила).
/// - B (rule_idx 2, priority 2, анкор в C+2) -- пишет ТОЛЬКО {C+2}.
///
/// L и W стоят В ОДНОЙ точке C, оба в ГЛУБИНЕ полосы (core), но C
/// намеренно поставлена так, что dist_right == margin (core, но НЕ safe --
/// значит W после локальной победы попадёт в `at_risk_accepted`, а не в
/// `safe_accepted`). B стоит в C+2, где dist_right < margin -- значит B
/// классифицируется boundary СРАЗУ, в общем проходе.
///
/// Локальный арбитраж полосы видит только {L, W}: W побеждает (выше
/// priority), L отклонён НАВСЕГДА -- в `accepted` полосы остаётся только W.
/// Финальный merge видит {W (at_risk), B (boundary), ...}: B побеждает W
/// (выше priority, C+2 общая) -- W отклонён ЦЕЛИКОМ (греди — не частично).
/// Итог spatial: принят только B, клетка C остаётся НИЧЬЕЙ, хотя L её
/// хотел и был единственным реальным претендентом после того, как W выбыл.
///
/// Централизованный арбитраж той же тройки (тот же тотальный порядок:
/// B > W > L): B принят (C+2 свободна) -> W отклонён (C+2 занята B) -> L
/// проверяется НЕЗАВИСИМО (C всё ещё свободна -- W её так и не занял,
/// будучи отклонён целиком) -> L ПРИНЯТ. Централизованный итог: {B, L}.
///
/// `L` и `B` НАМЕРЕННО не пересекаются друг с другом (L хочет только C, B
/// только C+2) -- это и есть условие, при котором расхождение возможно:
/// если бы B и L пересекались, B заблокировал бы L и централизованно тоже.
#[test]
fn test_arbitrate_spatial_matches_centralized_at_risk_loser_frees_locally_rejected_weaker_candidate() {
    let rules: Vec<Rule> = vec![
        // L (rule_idx 0) -- пишет только свою клетку.
        Rule {
            id: vec![CellType(1)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(1))],
            active_only: false,
            priority: 0,
            min_age: 0,
            overflow: OverflowAction::Discard,
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        },
        // W (rule_idx 1) -- пишет свою клетку И клетку на dx=+2 (даёт reach=2).
        Rule {
            id: vec![CellType(1)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(2)), (2, 0, ChangeValue::Literal(3))],
            active_only: false,
            priority: 1,
            min_age: 0,
            overflow: OverflowAction::Discard,
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        },
        // B (rule_idx 2) -- пишет только свою клетку, но выше приоритетом всех.
        Rule {
            id: vec![CellType(1)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(4))],
            active_only: false,
            priority: 2,
            min_age: 0,
            overflow: OverflowAction::Discard,
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        },
    ];
    let rule_index = make_rule_index(&rules);
    let rule_cache = build_rule_data_cache(&rule_index);
    let reach = max_reach_from_cache(&rule_cache);
    assert_eq!(reach, 2, "W's second write at dx=+2 must be what drives reach here");

    let num_threads = rayon::current_num_threads();
    assert!(num_threads >= 2, "this test needs a multi-threaded rayon pool to exercise band-split at all -- on a single-threaded pool arbitrate_spatial_with_cam falls back to arbitrate_with_cam directly and this test would pass vacuously without proving anything");

    let margin = (2 * reach) as u32; // 4
    let safe_margin = margin + reach as u32; // 6

    // Наполнитель: широко разнесённые изолированные кластеры (та же схема,
    // что в `test_arbitrate_spatial_matches_centralized_many_isolated_clusters`)
    // -- нужен только чтобы (а) перевалить SPATIAL_THRESHOLD и реально
    // включить band-split, и (б) дать разброс x, достаточный, чтобы число
    // полос было ограничено числом потоков, а не разбросом (см. ниже).
    const N_FILLER: u32 = 2000;
    const SPACING: u32 = 1000;
    let mut matches: Vec<RuleMatch> = Vec::new();
    for cluster in 0..N_FILLER {
        let x = cluster * SPACING;
        for rule_idx in 0..3usize {
            matches.push(RuleMatch {
                x,
                y: 0,
                head: CellType(1),
                rule_idx,
            });
        }
    }

    // Разброс наполнителя УЖЕ настолько велик (spread/(margin*2) на 3
    // порядка больше любого реалистичного числа потоков), что число полос
    // определяется ИСКЛЮЧИТЕЛЬНО `rayon::current_num_threads()`, не
    // разбросом -- воспроизводим ТУ ЖЕ формулу, что и сама
    // `arbitrate_spatial_with_cam`, чтобы точно знать границы полосы 0 и
    // разместить тройку L/W/B ровно на нужном расстоянии от них.
    let min_x = 0u32;
    let max_x = (N_FILLER - 1) * SPACING;
    let spread = max_x - min_x;
    let max_bands_by_spread = ((spread / (margin * 2)) as usize).max(1);
    let num_bands = num_threads.min(max_bands_by_spread).max(1);
    assert!(num_bands >= 2, "filler spread must be wide enough for band count to be thread-limited, not spread-limited -- got {num_bands} bands");
    let band_width = (spread / num_bands as u32).max(1);
    // Полоса 0 -- НЕ последняя (num_bands >= 2), значит её правая граница
    // считается обычной формулой, не спецслучаем "max_x + 1" последней полосы.
    let band0_end = min_x + band_width;
    assert!(
        band_width > 20,
        "band 0 must be wide enough that dist_left(x_pocket) is trivially >= margin -- got band_width={band_width}"
    );

    // C стоит так, что dist_right(C) == margin (core, но не safe: margin <=
    // dist_right < safe_margin). C+2 (B) стоит так, что dist_right(C+2) <
    // margin (boundary сразу).
    let x_c = band0_end - 1 - margin; // dist_right(x_c) = (band0_end-1) - x_c = margin
    let x_b = x_c + 2;

    let dist_right_c = (band0_end - 1).saturating_sub(x_c);
    let dist_right_b = (band0_end - 1).saturating_sub(x_b);
    assert!(dist_right_c >= margin && dist_right_c < safe_margin, "C must be core but at-risk (not safe) -- got dist_right={dist_right_c}, margin={margin}, safe_margin={safe_margin}");
    assert!(
        dist_right_b < margin,
        "C+2 must be classified boundary outright -- got dist_right={dist_right_b}, margin={margin}"
    );

    // Защита от случайного совпадения с наполнителем (SPACING=1000 -- при
    // band_width, не кратном 1000 практически невозможно, но проверяем
    // явно, а не полагаемся на "практически").
    for cluster in 0..N_FILLER {
        let fx = cluster * SPACING;
        assert!(
            fx.abs_diff(x_c) > 20 && fx.abs_diff(x_b) > 20,
            "filler cluster at x={fx} collides with the hand-placed L/W/B pocket at x_c={x_c}/x_b={x_b} -- adjust SPACING/N_FILLER"
        );
    }

    // ВНИМАНИЕ: `make_rule_index` (как и реальный `RuleStore::get_index`)
    // сортирует правила по приоритету УБЫВАЮЩЕ -- значит порядковый индекс
    // в `rules` (0=L,1=W,2=B по приоритету 0,1,2) НЕ совпадает с итоговым
    // `rule_idx` после сортировки: убывающий порядок даёт [B(pri2)->idx0,
    // W(pri1)->idx1, L(pri0)->idx2]. Используем ИТОГОВЫЕ индексы, не
    // индексы вставки -- иначе тест молча проверяет совсем другую тройку
    // ролей (обнаружено эмпирически: первая версия этого теста проходила,
    // но выяснилось, что она из-за этой путаницы вообще не строила
    // задуманный сценарий).
    matches.push(RuleMatch {
        x: x_c,
        y: 0,
        head: CellType(1),
        rule_idx: 2,
    }); // L (priority 0 -> index 2 after sort)
    matches.push(RuleMatch {
        x: x_c,
        y: 0,
        head: CellType(1),
        rule_idx: 1,
    }); // W (priority 1 -> index 1 after sort)
    matches.push(RuleMatch {
        x: x_b,
        y: 0,
        head: CellType(1),
        rule_idx: 0,
    }); // B (priority 2 -> index 0 after sort)

    // Все priority различны (0/1/2) -- возраст не участвует в разрешении,
    // берём константу, чтобы не вносить лишнюю степень свободы.
    let get_age = |_x: usize, _y: usize| 0u32;

    let centralized = arbitrate(
        matches.clone(),
        &rule_index,
        &rule_cache,
        (usize::MAX, usize::MAX),
        get_age,
    );
    let spatial = arbitrate_spatial(
        matches,
        &rule_index,
        &rule_cache,
        (usize::MAX, usize::MAX),
        reach,
        get_age,
    );

    let centralized_set: HashSet<(u32, u32, usize)> = centralized.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();
    let spatial_set: HashSet<(u32, u32, usize)> = spatial.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();

    assert!(centralized_set.contains(&(x_c, 0, 2)), "sanity check on the reference: centralized arbitration must accept L at C -- B only ever contests C+2, not C, so once W is rejected wholesale, C is free for L");
    assert_eq!(
        centralized_set, spatial_set,
        "spatial band-split must accept exactly what centralized arbitration accepts -- if this diverges, spatial is missing L at C={x_c}: centralized={centralized_set:?}, spatial={spatial_set:?}"
    );
}

/// Проверка, ни разу не сделанная прямо: НЕСКОЛЬКО независимых `Engine`,
/// работающих ОДНОВРЕМЕННО с разных OS-потоков, делят ОДИН И ТОТ ЖЕ
/// глобальный rayon-пул (`arbitrate_spatial_with_cam` внутри каждого
/// диспетчерит на него свои полосы) -- ровно тот же класс риска, что и
/// найденная и исправленная в этой сессии реентерабельность `KEYED_BUF`
/// (work-stealing может увести ОДИН OS-поток на ЧУЖОЙ вызов
/// `arbitrate_with_cam`, будь то соседняя полоса ТОГО ЖЕ движка или
/// ВООБЩЕ ДРУГОЙ движок на другом потоке, делящий тот же пул). Тот фикс
/// был точечным (одна конкретная функция) -- этот тест целится в сам
/// класс проблемы на уровне двух ПОЛНОСТЬЮ независимых `Engine`, не одной
/// функции: если где-то ещё осталось похожее удержание `thread_local!`
/// заимствования поперёк вложенного rayon-вызова, это должно проявиться
/// именно здесь (панике reentrancy или, хуже, тихой порче состояния одного
/// движка данными другого).
///
/// Дизайн: N движков, каждый со СВОИМ dense CA-сценарием (сплошной сдвиг
/// вправо, >SPATIAL_THRESHOLD матчей за тик -- гарантированно включает
/// band-split), запущены параллельно с разных потоков на много тиков.
/// Каждый сверяется с ОДИНАКОВО построенным, но прогнанным
/// ПОСЛЕДОВАТЕЛЬНО (до старта потоков) эталоном -- если разделяемый
/// rayon-пул хоть как-то путает состояние между движками, конкретный
/// движок разойдётся со своим же эталоном (тем же самым, что дал бы,
/// будучи прогнан в одиночку).
#[test]
fn test_multiple_engines_run_concurrently_on_shared_rayon_pool_without_cross_contamination() {
    fn build_dense_engine(side: usize) -> cellaria::engine::Engine<VecStorage> {
        let rule = Rule {
            id: vec![CellType(1)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
            changes: vec![],
            active_only: false,
            priority: 1,
            min_age: 0,
            overflow: OverflowAction::Discard,
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        };
        let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
        rule_index.insert(CellType(1), vec![rule]);
        let mut grid = Grid::new(VecStorage::new(side, side), HashSet::new());
        for y in 0..side {
            for x in 0..side {
                grid.set_cell(
                    x,
                    y,
                    Cell {
                        value: CellValue(CellType(1)),
                        born_at: 0,
                    },
                );
            }
        }
        cellaria::engine::Engine::new(grid, rule_index)
    }

    fn dump(engine: &cellaria::engine::Engine<VecStorage>, side: usize) -> Vec<Cell> {
        (0..side)
            .flat_map(|y| (0..side).map(move |x| (x, y)))
            .map(|(x, y)| engine.grid().get_cell(x, y).copied().unwrap_or_default())
            .collect()
    }

    const SIDE: usize = 90; // 8100 клеток > SPATIAL_THRESHOLD=4096 -- band-split реально включается
    const TICKS: u32 = 15;
    const N_ENGINES: usize = 6;

    // Эталон: один движок, прогнанный ОДИНОКО, ПОСЛЕДОВАТЕЛЬНО, до старта
    // конкурентных потоков.
    let mut reference = build_dense_engine(SIDE);
    for _ in 0..TICKS {
        reference.run_tick();
    }
    let reference_dump = dump(&reference, SIDE);

    let handles: Vec<_> = (0..N_ENGINES)
        .map(|_| {
            std::thread::spawn(move || {
                let mut engine = build_dense_engine(SIDE);
                for _ in 0..TICKS {
                    engine.run_tick();
                }
                dump(&engine, SIDE)
            })
        })
        .collect();

    for (i, handle) in handles.into_iter().enumerate() {
        let result = handle
            .join()
            .unwrap_or_else(|e| panic!("engine {i} thread panicked (likely reentrancy): {e:?}"));
        assert_eq!(result, reference_dump, "engine {i}, run concurrently with {} others on the shared rayon pool, diverged from the same scenario run alone -- cross-instance interference", N_ENGINES - 1);
    }
}
