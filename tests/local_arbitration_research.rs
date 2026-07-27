//! Research-тест: можно ли заменить ГЛОБАЛЬНЫЙ арбитраж (сортировка всех
//! matches решётки целиком и жадный проход) на ЛОКАЛЬНОЕ разрешение
//! конфликтов, где каждая спорная клетка сама решает победителя, глядя
//! только на матчи, которые её реально затрагивают — без глобальной
//! сортировки. Это прямая проверка вопроса "можно ли эту модель перенести
//! в железо", где нет способа "посмотреть на всю решётку сразу".
//!
//! `local_arbitrate` реализует это через раундовое итеративное исключение
//! (похоже на распределённые алгоритмы maximal independent set):
//! - на каждом раунде для каждой ещё спорной клетки среди ЖИВЫХ кандидатов
//!   выбирается локальный победитель (priority, потом age, потом
//!   детерминированный tie-break по позиции и rule_idx — все три критерия
//!   читаются из самого match'а и его непосредственного окружения, без
//!   обращения к остальной решётке);
//! - любой ЖИВОЙ match, который не победил хотя бы в одной из своих клеток,
//!   умирает;
//! - повторяем, пока раунд не пройдёт без изменений.
//!
//! Это НЕ то же самое, что глобальный жадный проход по приоритету — тот
//! может "выбрать по цепочке" (A побеждает B за клетку X, значит клетка Y,
//! за которую B конкурировал с C, достаётся C автоматически), а локальный
//! резолвер такой цепочки не видит. Вопрос теста — насколько часто и в
//! каких случаях это расхождение реально проявляется.

use std::collections::{HashMap, HashSet};

use proptest::prelude::*;

use cellaria::conflict_analyzer::{build_rule_data_cache, get_rule_data, RuleDataCache};
use cellaria::engine::{arbitrate, detect_matches};
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Direction, OverflowAction, Rule, RuleMatch, ShiftSpec};
use cellaria::{Grid, VecStorage};

const CELL_ALPHABET: u8 = 4;
const MIN_SIDE: usize = 4;
const MAX_SIDE: usize = 8;

fn cell_type_strategy() -> impl Strategy<Value = u8> {
    1..=CELL_ALPHABET
}

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
        prop::collection::vec((-2i32..=2, -2i32..=2, 1u8..=9), 0..=2),
        0u32..=5,
        0u64..=2,
        prop_oneof![
            Just(OverflowAction::Discard),
            cell_type_strategy().prop_map(OverflowAction::Write),
        ],
    )
        .prop_map(|((head, pattern), shift_specs, changes, priority, min_age, overflow)| {
            let shifts: Vec<Vec<ShiftSpec>> = shift_specs
                .into_iter()
                .map(|(dir, steps)| vec![ShiftSpec::new(dir, steps)])
                .collect();
            Rule {
                id: vec![CellType(head)],
                pattern,
                shifts,
                changes: changes
                    .into_iter()
                    .map(|(dx, dy, v)| (dx, dy, ChangeValue::Literal(v)))
                    .collect(),
                active_only: false,
                priority,
                min_age,
                overflow,
            }
        })
}

fn rule_set_strategy() -> impl Strategy<Value = Vec<Rule>> {
    prop::collection::vec(rule_strategy(), 1..=6)
}

fn make_rule_index(rules: &[Rule]) -> HashMap<CellType, Vec<Rule>> {
    let mut index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        if let Some(&head) = rule.id.first() {
            index.entry(head).or_default().push(rule.clone());
        }
    }
    for group in index.values_mut() {
        group.sort_by_key(|r| std::cmp::Reverse(r.priority));
    }
    index
}

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

fn full_affected_cells(
    m: &RuleMatch,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &RuleDataCache,
    bounds: (usize, usize),
) -> Vec<(i32, i32)> {
    let head = m.rule_id[0];
    let rd = get_rule_data(rule_cache, head, m.rule_idx).expect("rule_data должна быть в кэше");
    let overflow = rule_index
        .get(&head)
        .and_then(|rules| rules.get(m.rule_idx))
        .expect("rule_idx должен быть валиден")
        .overflow;
    rd.affected_cells
        .iter()
        .map(|&(dx, dy)| affected_cell_abs(m.x as i32, m.y as i32, dx, dy, &rd.shift_targets, overflow, bounds))
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Alive,
    Confirmed,
    Eliminated,
}

/// Локальный резолвер конфликтов — многораундовая версия.
///
/// В отличие от первой (наивной) версии, где за один раунд "убивался" любой
/// живой кандидат, проигравший ХОТЬ ГДЕ-ТО (что оказалось слишком
/// агрессивно на цепочках конфликтов — см. историю обсуждения), здесь за
/// раунд ПОДТВЕРЖДАЕТСЯ только тот, кто побеждает НА ВСЕХ своих клетках
/// среди ещё живых кандидатов. У подтверждённых забираются их клетки, и
/// только ТОГДА выбывают остальные живые претенденты на эти клетки.
/// Гарантированный прогресс: кандидат с глобально максимальным ключом среди
/// живых всегда побеждает везде (по построению порядка), так что раунд
/// всегда подтверждает хотя бы одного, и цикл не может зависнуть.
///
/// Total order — (priority, age, rule_id, x, y, rule_idx), по формулировке
/// из обсуждения. `rule_idx` — обязательный последний тай-брейк: два РАЗНЫХ
/// правила могут иметь одинаковый `rule_id` (недетерминированный выбор) и
/// сработать в одной и той же позиции — тогда priority/age/rule_id/x/y все
/// совпадут, и только `rule_idx` (сам по себе локальный — это свойство
/// match'а) остаётся однозначным различителем.
fn local_arbitrate(
    matches: Vec<RuleMatch>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &RuleDataCache,
    bounds: (usize, usize),
    get_cell_age: impl Fn(usize, usize) -> u32,
) -> Vec<RuleMatch> {
    struct Candidate {
        m: RuleMatch,
        priority: u32,
        age: u32,
        rule_id_key: Vec<u8>,
        cells: Vec<(i32, i32)>,
        status: Status,
    }

    let mut candidates: Vec<Candidate> = matches
        .into_iter()
        .map(|m| {
            let head = m.rule_id[0];
            let priority = rule_index
                .get(&head)
                .and_then(|rules| rules.get(m.rule_idx))
                .map_or(0, |r| r.priority);
            let age = get_cell_age(m.x as usize, m.y as usize);
            let rule_id_key: Vec<u8> = m.rule_id.iter().map(|ct| ct.0).collect();
            let cells: Vec<(i32, i32)> = full_affected_cells(&m, rule_index, rule_cache, bounds)
                .into_iter()
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            Candidate { m, priority, age, rule_id_key, cells, status: Status::Alive }
        })
        .collect();

    let key = |c: &Candidate| -> (u32, u32, Vec<u8>, u32, u32, usize) {
        (c.priority, c.age, c.rule_id_key.clone(), c.m.x, c.m.y, c.m.rule_idx)
    };

    let mut rounds = 0usize;
    loop {
        rounds += 1;
        // Клетка -> индексы ещё живых (не решённых) кандидатов, которые её затрагивают.
        let mut cell_map: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, c) in candidates.iter().enumerate() {
            if c.status != Status::Alive {
                continue;
            }
            for &cell in &c.cells {
                cell_map.entry(cell).or_default().push(i);
            }
        }

        let mut newly_confirmed: Vec<usize> = Vec::new();
        for (i, c) in candidates.iter().enumerate() {
            if c.status != Status::Alive {
                continue;
            }
            let my_key = key(c);
            let wins_everywhere = c.cells.iter().all(|cell| {
                cell_map[cell]
                    .iter()
                    .all(|&j| j == i || my_key > key(&candidates[j]))
            });
            if wins_everywhere {
                newly_confirmed.push(i);
            }
        }

        if newly_confirmed.is_empty() {
            break;
        }

        for &i in &newly_confirmed {
            candidates[i].status = Status::Confirmed;
        }
        let mut to_eliminate: HashSet<usize> = HashSet::new();
        for &i in &newly_confirmed {
            for cell in &candidates[i].cells {
                for &j in &cell_map[cell] {
                    if j != i && candidates[j].status == Status::Alive {
                        to_eliminate.insert(j);
                    }
                }
            }
        }
        for j in to_eliminate {
            candidates[j].status = Status::Eliminated;
        }
    }

    if std::env::var_os("PRINT_ROUNDS").is_some() {
        eprintln!("ROUNDS {}", rounds);
    }

    candidates
        .into_iter()
        .filter(|c| c.status == Status::Confirmed)
        .map(|c| c.m)
        .collect()
}

/// Референс: тот же жадный последовательный проход, что и продакшн
/// `arbitrate()`, но с ЯВНЫМ, детерминированным тай-брейком (priority, age,
/// rule_id, x, y, rule_idx) вместо "как получилось" (реальный `arbitrate()`
/// при равенстве priority+age просто сохраняет порядок, в котором нашлись
/// матчи — implementation-defined, не осмысленный тай-брейк). Нужен, чтобы
/// отделить вопрос "работает ли локальный МЕХАНИЗМ" от вопроса "совпадает
/// ли тай-брейк с тем, что сейчас случайно получается в коде".
fn sequential_greedy_reference(
    matches: Vec<RuleMatch>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &RuleDataCache,
    bounds: (usize, usize),
    get_cell_age: impl Fn(usize, usize) -> u32,
) -> Vec<RuleMatch> {
    struct Scored {
        m: RuleMatch,
        key: (u32, u32, Vec<u8>, u32, u32, usize),
        cells: Vec<(i32, i32)>,
    }

    let mut scored: Vec<Scored> = matches
        .into_iter()
        .map(|m| {
            let head = m.rule_id[0];
            let priority = rule_index
                .get(&head)
                .and_then(|rules| rules.get(m.rule_idx))
                .map_or(0, |r| r.priority);
            let age = get_cell_age(m.x as usize, m.y as usize);
            let rule_id_key: Vec<u8> = m.rule_id.iter().map(|ct| ct.0).collect();
            let cells = full_affected_cells(&m, rule_index, rule_cache, bounds);
            let key = (priority, age, rule_id_key, m.x, m.y, m.rule_idx);
            Scored { m, key, cells }
        })
        .collect();

    // Максимум первым — тот же порядок сравнения, что и в local_arbitrate.
    scored.sort_by(|a, b| b.key.cmp(&a.key));

    let mut used: HashSet<(i32, i32)> = HashSet::new();
    let mut accepted = Vec::new();
    for s in scored {
        if s.cells.iter().any(|c| used.contains(c)) {
            continue;
        }
        used.extend(s.cells.iter().copied());
        accepted.push(s.m);
    }
    accepted
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// Safety локального резолвера сама по себе: он не должен пропускать
    /// конфликты — это должно выполняться по построению алгоритма, но
    /// проверяем отдельно, а не только сравнением с глобальным.
    #[test]
    fn prop_local_arbitrate_never_overlaps(
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
        let accepted = local_arbitrate(matches, &rule_index, &rule_cache, bounds, |x, y| grid.get_age(x, y) as u32);

        let mut used: HashSet<(i32, i32)> = HashSet::new();
        for m in &accepted {
            // Дедуп внутри ОДНОГО match'а: после клэмпинга цель сдвига может
            // совпасть с его же origin-клеткой — см. аналогичный комментарий
            // в property_arbitration.rs. Это не конфликт МЕЖДУ matches.
            let own_cells: HashSet<(i32, i32)> = full_affected_cells(m, &rule_index, &rule_cache, bounds).into_iter().collect();
            for coord in own_cells {
                prop_assert!(
                    used.insert(coord),
                    "локальный резолвер пропустил конфликт на клетке {:?}",
                    coord
                );
            }
        }
    }

    /// Сравнение с ПРОДАКШН `arbitrate()`. Раньше (до перехода на явный
    /// тай-брейк priority→age→rule_id→coords→rule_idx в самом `arbitrate()`)
    /// здесь допускалось расхождение по размеру набора — implementation-
    /// defined порядок обнаружения матчей делал жадный проход по цепочкам
    /// конфликтов непредсказуемым относительно локального резолвера.
    /// Теперь оба используют один и тот же тай-брейк — требуем побитового
    /// совпадения множества принятых matches, не только размера.
    #[test]
    fn prop_local_vs_global_arbitrate_count(
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

        let global = arbitrate(matches.clone(), &rule_index, &rule_cache, bounds, |x, y| grid.get_age(x, y) as u32);
        let local = local_arbitrate(matches, &rule_index, &rule_cache, bounds, |x, y| grid.get_age(x, y) as u32);

        let mut global_ids: Vec<(u32, u32, usize)> = global.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();
        let mut local_ids: Vec<(u32, u32, usize)> = local.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();
        global_ids.sort();
        local_ids.sort();

        prop_assert_eq!(
            local_ids, global_ids,
            "продакшн arbitrate() и локальный резолвер разошлись при одинаковом тай-брейке (решётка {}x{})",
            width, height
        );
    }

    /// ГЛАВНОЕ сравнение: локальный резолвер против последовательного
    /// жадного прохода с ТЕМ ЖЕ САМЫМ явным тай-брейком (не продакшн
    /// arbitrate(), а sequential_greedy_reference). Если это совпадает
    /// побитово всегда — доказывает, что локальный МЕХАНИЗМ (раундовое
    /// подтверждение только полных победителей) корректно воспроизводит
    /// жадный по total-order проход, и расхождение с продакшном в
    /// предыдущем тесте — целиком заслуга разных тай-брейков, а не
    /// принципиальной невозможности локального решения.
    #[test]
    fn prop_local_matches_reference_exactly(
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

        let reference = sequential_greedy_reference(matches.clone(), &rule_index, &rule_cache, bounds, |x, y| grid.get_age(x, y) as u32);
        let local = local_arbitrate(matches, &rule_index, &rule_cache, bounds, |x, y| grid.get_age(x, y) as u32);

        let mut ref_ids: Vec<(u32, u32, usize)> = reference.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();
        let mut local_ids: Vec<(u32, u32, usize)> = local.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();
        ref_ids.sort();
        local_ids.sort();

        prop_assert_eq!(
            local_ids, ref_ids,
            "локальный резолвер разошёлся с референсным жадным проходом при ОДИНАКОВОМ тай-брейке — это уже был бы баг в механизме, не в выборе тай-брейка"
        );
    }
}
