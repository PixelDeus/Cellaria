//! Проверка `GpuEngine` для v2-подмножества (сдвиги + запись в соседа,
//! реальный многораундовый арбитраж — см. `gpu::rule_table`'s
//! `needs_arbitration`/`GpuEngine`'s `Arbitrated`-пайплайн) против
//! CPU-эталона `engine::run_tick` — прямое продолжение
//! `tests/gpu_v1_correctness.rs`, но со сдвигами, которые v1 сознательно
//! исключал.
//!
//! Генератор строго ограничен документированным GPU-подмножеством (см.
//! `gpu::rule_table::GpuUnsupportedReason`): без `ChangeValue::Ref`, без
//! `OverflowAction::Write`/`WriteLiteral` — оба выхода намеренно НЕ
//! генерируются вовсе (не polyfill+skip, а структурно недостижимы), так что
//! почти каждый сгенерированный конфиг реально проверяется, а не
//! пропускается. Единственная причина пропуска — `TooManyRulesForArbitration`
//! (голова случайно набрала больше `MAX_MATCHES_PER_CELL` правил).
//!
//! Несколько тиков подряд, сверка ПОСЛЕ КАЖДОГО (не только в конце) — чтобы
//! расхождение локализовалось на конкретном тике, а не терялось за
//! накопленным дрейфом.

use std::collections::{HashMap, HashSet};

use proptest::prelude::*;

use cellaria::engine::run_tick;
use cellaria::gpu::GpuEngine;
use cellaria::types::{CamSearch, Cell, CellType, CellValue, ChangeValue, Direction, OverflowAction, Rule, ShiftSpec};
use cellaria::{Grid, VecStorage};

const CELL_ALPHABET: u8 = 3;
const SIDE: usize = 7;
const TICKS: usize = 4;

fn cell_type_strategy() -> impl Strategy<Value = u8> {
    1..=CELL_ALPHABET
}

fn pattern_strategy() -> impl Strategy<Value = (u8, Vec<(i8, i8, CellType)>)> {
    (
        cell_type_strategy(),
        prop::collection::vec((-1i8..=1, -1i8..=1, cell_type_strategy()), 0..=2),
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

fn direction_strategy() -> impl Strategy<Value = Direction> {
    prop_oneof![
        Just(Direction::Up),
        Just(Direction::Down),
        Just(Direction::Left),
        Just(Direction::Right),
    ]
}

/// v2-правило: 0..=2 независимых сдвигов (шаг фиксирован в 1 — держит
/// решётку `SIDE`×`SIDE` маленькой относительно возможных целей записи) и
/// 0..=3 `changes` на произвольном смещении в ±1 (не только self, в
/// отличие от v1-генератора) — фильтруется так, чтобы НЕ было одновременно
/// пустых shifts И changes (вне подмножества — `GpuUnsupportedReason::NoEffect`).
fn v2_rule_strategy() -> impl Strategy<Value = Rule> {
    (
        pattern_strategy(),
        prop::collection::vec(direction_strategy(), 0..=2),
        prop::collection::vec(
            (-1i32..=1, -1i32..=1, (1u8..=9).prop_map(ChangeValue::Literal)),
            0..=3,
        ),
        0u32..=3,
        0u64..=1,
        any::<bool>(),
    )
        .prop_map(|((head, pattern), shift_dirs, changes, priority, min_age, active_only)| {
            let shifts: Vec<Vec<ShiftSpec>> = shift_dirs.into_iter().map(|d| vec![ShiftSpec::new(d, 1)]).collect();
            Rule {
                id: vec![CellType(head)],
                pattern,
                shifts,
                changes,
                active_only,
                priority,
                min_age,
                overflow: OverflowAction::Discard,
                cam: None,
                tie_break: 0,
                starvation_after: None,
            }
        })
        .prop_filter("rule must have at least one shift or change", |r| {
            !(r.shifts.is_empty() && r.changes.is_empty())
        })
}

fn v2_rule_set_strategy() -> impl Strategy<Value = Vec<Rule>> {
    prop::collection::vec(v2_rule_strategy(), 1..=5)
}

fn make_rule_index(rules: &[Rule]) -> HashMap<CellType, Vec<Rule>> {
    let mut index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        if let Some(&head) = rule.id.first() {
            index.entry(head).or_default().push(rule.clone());
        }
    }
    // Тот же порядок, что и `config::load_config`/`RuleStore::get_index` —
    // критично для тай-брейка по `rule_idx`, см. аналогичный комментарий в
    // `gpu_v1_correctness.rs`.
    for group in index.values_mut() {
        group.sort_by_key(|r| std::cmp::Reverse(r.priority));
    }
    index
}

fn grid_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(0..=CELL_ALPHABET, SIDE * SIDE)
}

proptest! {
    // Каждый кейс строит настоящее GPU-устройство и гоняет TICKS тиков —
    // держим число кейсов умеренным (как и в gpu_v1_correctness.rs).
    #![proptest_config(ProptestConfig::with_cases(80))]

    #[test]
    fn test_gpu_v2_matches_cpu_property_random_shifting_rules(rules in v2_rule_set_strategy(), cells in grid_strategy()) {
        let rule_index = make_rule_index(&rules);

        let storage = VecStorage::new(SIDE, SIDE);
        let mut cpu_grid = Grid::new(storage, HashSet::new());
        let mut gpu_initial: Vec<(usize, usize, Cell)> = Vec::new();
        for (i, &v) in cells.iter().enumerate() {
            if v != 0 {
                let x = i % SIDE;
                let y = i / SIDE;
                let cell = Cell { value: CellValue(CellType(v)), born_at: 0 };
                cpu_grid.set_cell(x, y, cell);
                gpu_initial.push((x, y, cell));
            }
        }

        // Пропускаем конфиги вне GPU-подмножества (см. doc-комментарий
        // модуля — на практике почти всегда TooManyRulesForArbitration,
        // если у одной головы случайно оказалось больше MAX_MATCHES_PER_CELL
        // правил).
        let Ok(mut gpu_engine) = GpuEngine::new(SIDE, SIDE, &gpu_initial, &rule_index) else {
            return Ok(());
        };

        for tick in 0..TICKS {
            run_tick(&mut cpu_grid, &rule_index);
            gpu_engine.run_tick();
            let gpu_result = gpu_engine.read_grid();

            for y in 0..SIDE {
                for x in 0..SIDE {
                    let cpu_cell = cpu_grid.get_cell(x, y).copied().unwrap_or_default();
                    let gpu_cell = gpu_result[y * SIDE + x];
                    prop_assert_eq!(
                        cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                        "value mismatch at tick={} ({}, {})", tick, x, y
                    );
                    prop_assert_eq!(
                        cpu_cell.born_at, gpu_cell.born_at,
                        "born_at mismatch at tick={} ({}, {})", tick, x, y
                    );
                }
            }
        }
    }
}

/// Не-property, конкретный сценарий: две частицы движутся навстречу друг
/// другу и сталкиваются на одной клетке несколько тиков подряд (та же
/// природа конфликта, что и в `gpu::engine::tests::test_gpu_engine_arbitrated_write_conflict_all_or_nothing`,
/// но теперь сверяется с НАСТОЯЩИМ CPU-эталоном, а не вручную посчитанным
/// ожиданием).
#[test]
fn test_gpu_v2_matches_cpu_head_on_collision_scenario() {
    fn mover(id: u8, direction: Direction, priority: u32) -> Rule {
        Rule {
            id: vec![CellType(id)],
            pattern: vec![(0, 0, CellType(id))],
            shifts: vec![vec![ShiftSpec::new(direction, 1)]],
            changes: vec![],
            active_only: false,
            priority,
            min_age: 0,
            overflow: OverflowAction::Discard,
            cam: None,
            tie_break: 0,
            starvation_after: None,
        }
    }

    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(1), vec![mover(1, Direction::Right, 5)]);
    rule_index.insert(CellType(2), vec![mover(2, Direction::Left, 1)]);

    let n = 9usize;
    let storage = VecStorage::new(n, 1);
    let mut cpu_grid = Grid::new(storage, HashSet::new());
    cpu_grid.set_cell(0, 0, Cell { value: CellValue::new(1), born_at: 0 });
    cpu_grid.set_cell(8, 0, Cell { value: CellValue::new(2), born_at: 0 });

    let initial = vec![
        (0usize, 0usize, Cell { value: CellValue::new(1), born_at: 0 }),
        (8, 0, Cell { value: CellValue::new(2), born_at: 0 }),
    ];
    let mut gpu_engine = GpuEngine::new(n, 1, &initial, &rule_index).expect("two Discard movers are within the v2 subset");

    for tick in 0..6 {
        run_tick(&mut cpu_grid, &rule_index);
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();
        for x in 0..n {
            let cpu_cell = cpu_grid.get_cell(x, 0).copied().unwrap_or_default();
            let gpu_cell = gpu_result[x];
            assert_eq!(cpu_cell.value.0 .0, gpu_cell.value.0 .0, "value mismatch at tick={tick} x={x}");
            assert_eq!(cpu_cell.born_at, gpu_cell.born_at, "born_at mismatch at tick={tick} x={x}");
        }
    }
}

/// Прогоняет ГИБРИДНЫЙ (GPU + CPU-добор) арбитраж через настоящий
/// `GpuEngine`, а не синтетический `hybrid_check.rs`-прототип. Строит
/// цепочку конфликтов (см. `gpu::engine`'s doc-комментарий про
/// `chain_check.rs`-находку: n матчей в ряд, каждый пишет в себя И в
/// соседа справа [(0,0),(1,0)] — цепочка зависимых конфликтов длиной n
/// требует ~n/2 раундов на сходимость) ДЛИННЕЕ, чем `ROUNDS` (32) успевает
/// разрешить чистым GPU-путём — единственный способ, которым эта клетка
/// корректно обновится, это реальное срабатывание `cpu_fallback_resolve`.
/// Тай-брейк между соседями — по x (все клетки одного типа, один
/// приоритет, один возраст) — совпадает с построением
/// `hybrid_check.rs`/`chain_check.rs`'s цепочки.
#[test]
fn test_gpu_v2_hybrid_fallback_resolves_long_conflict_chain() {
    const CHAIN_LEN: usize = 100; // ~50 раундов нужно, ROUNDS=32 — не хватает чистому GPU
    let width = CHAIN_LEN + 5;

    let rule = Rule {
        id: vec![CellType(1)],
        pattern: vec![(0, 0, CellType(1))],
        shifts: Vec::new(),
        changes: vec![(0, 0, ChangeValue::Literal(1)), (1, 0, ChangeValue::Literal(1))],
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
    };
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(1), vec![rule]);

    let storage = VecStorage::new(width, 1);
    let mut cpu_grid = Grid::new(storage, HashSet::new());
    let mut initial = Vec::new();
    for x in 0..CHAIN_LEN {
        let cell = Cell { value: CellValue::new(1), born_at: 0 };
        cpu_grid.set_cell(x, 0, cell);
        initial.push((x, 0usize, cell));
    }

    let mut gpu_engine = GpuEngine::new(width, 1, &initial, &rule_index)
        .expect("self+neighbor change without shifts is within the v2 subset");

    // Несколько тиков подряд — не только проверяет, что CPU-добор
    // разрешает первый (самый длинный) хвост правильно, но и что
    // "залатанный" им буфер корректно используется как ВХОД для
    // следующего тика (detect_pass на GPU должен видеть именно то, что
    // записал CPU-fallback, а не устаревшее/непропатченное значение).
    for tick in 0..3 {
        run_tick(&mut cpu_grid, &rule_index);
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();

        for x in 0..width {
            let cpu_cell = cpu_grid.get_cell(x, 0).copied().unwrap_or_default();
            let gpu_cell = gpu_result[x];
            assert_eq!(cpu_cell.value.0 .0, gpu_cell.value.0 .0, "value mismatch at tick={tick} x={x} (chain len {CHAIN_LEN})");
            assert_eq!(cpu_cell.born_at, gpu_cell.born_at, "born_at mismatch at tick={tick} x={x} (chain len {CHAIN_LEN})");
        }
    }
}

/// CAM (`Rule::cam`, content-addressable поиск с ограниченным радиусом) на
/// GPU — сверка с CPU-эталоном на 2D-решётке (радиус — Chebyshev, значит
/// именно 2D, не 1D-случай остальных тестов этого файла). Два сценария в
/// одном: одиночный магнит без конфликта (клетка (0,0)) и два магнита,
/// реально претендующие на одну цель (клетки (6,0)/(6,6) обе в радиусе
/// цели (6,3)) — арбитраж должен дать ОДИНАКОВЫЙ результат на GPU и CPU,
/// включая тай-брейк по приоритету/позиции внутри `cam_search`'s поиска
/// ближайшей клетки при равных расстояниях.
#[test]
fn test_gpu_v2_cam_matches_cpu_single_and_conflicting_magnets() {
    const MAGNET: u8 = 10;
    const TARGET: u8 = 20;

    fn magnet_rule(radius: u8, priority: u32) -> Rule {
        Rule {
            id: vec![CellType(MAGNET)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![],
            active_only: false,
            priority,
            min_age: 0,
            overflow: OverflowAction::Discard,
            cam: Some(CamSearch { radius, target_type: CellType(TARGET) }),
            tie_break: 0,
            starvation_after: None,
        }
    }

    let width = 10;
    let height = 10;
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(MAGNET), vec![magnet_rule(4, 9), magnet_rule(4, 1)]);

    let cells = [
        (0usize, 0usize, MAGNET), // одиночный магнит, цель рядом
        (2, 0, TARGET),
        (6, 0, MAGNET), // два магнита...
        (6, 6, MAGNET), // ...претендующих на одну цель
        (6, 3, TARGET),
    ];

    let storage = VecStorage::new(width, height);
    let mut cpu_grid = Grid::new(storage, HashSet::new());
    let mut initial = Vec::new();
    for &(x, y, v) in &cells {
        let cell = Cell { value: CellValue::new(v), born_at: 0 };
        cpu_grid.set_cell(x, y, cell);
        initial.push((x, y, cell));
    }

    let mut gpu_engine = GpuEngine::new(width, height, &initial, &rule_index).expect("cam rule within MAX_CAM_RADIUS is supported");

    for tick in 0..3 {
        run_tick(&mut cpu_grid, &rule_index);
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();

        for y in 0..height {
            for x in 0..width {
                let cpu_cell = cpu_grid.get_cell(x, y).copied().unwrap_or_default();
                let gpu_cell = gpu_result[y * width + x];
                assert_eq!(cpu_cell.value.0 .0, gpu_cell.value.0 .0, "value mismatch at tick={tick} ({x},{y})");
                assert_eq!(cpu_cell.born_at, gpu_cell.born_at, "born_at mismatch at tick={tick} ({x},{y})");
            }
        }
    }
}

/// Модульный tie-break (`Rule::tie_break`, block F п.3) на GPU — та же
/// сцена, что и CPU-тест `test_tie_break_rotates_fairly_when_spaced_half_modulus_apart`
/// (два правила с равным priority, конкурирующие за одну и ту же клетку
/// каждый тик, tie_break расставлены на M/2 друг от друга), но сверяется
/// GPU против CPU-эталона побитово на каждом тике. `MODULUS` здесь —
/// намеренно захардкоженная копия `arbitrator::TIE_BREAK_MODULUS`/
/// `shader.wgsl::TIE_BREAK_MODULUS` (оба `pub(crate)`/шейдерная константа,
/// недоступны отсюда как внешнему интеграционному тесту) — если кто-то
/// поменяет модуль на одной стороне и забудет про другую, этот тест либо
/// поймает расхождение GPU/CPU напрямую (если стороны разъехались), либо
/// продолжит проверять корректный (но не факт что 50/50) сценарий — риск
/// приемлем, реальная защита от рассинхрона — побитовое сравнение ниже.
#[test]
fn test_gpu_v2_tie_break_matches_cpu_rotating_conflict() {
    const MODULUS: u32 = 16;
    const HEAD: u8 = 1;

    fn competing_rule(tie_break: u32, written_value: u8) -> Rule {
        Rule {
            id: vec![CellType(HEAD)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(1, 0, ChangeValue::Literal(written_value))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break,
            starvation_after: None,
        }
    }

    let width = 2;
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(HEAD), vec![competing_rule(0, 100), competing_rule(MODULUS / 2, 200)]);

    let initial = vec![(0usize, 0usize, Cell { value: CellValue::new(HEAD), born_at: 0 })];
    let storage = VecStorage::new(width, 1);
    let mut cpu_grid = Grid::new(storage, HashSet::new());
    cpu_grid.set_cell(0, 0, Cell { value: CellValue::new(HEAD), born_at: 0 });

    let mut gpu_engine = GpuEngine::new(width, 1, &initial, &rule_index).expect("two same-priority competing rules are within the v2 subset");

    let mut cpu_winners = Vec::new();
    for tick in 0..MODULUS {
        run_tick(&mut cpu_grid, &rule_index);
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();

        for x in 0..width {
            let cpu_cell = cpu_grid.get_cell(x, 0).copied().unwrap_or_default();
            let gpu_cell = gpu_result[x];
            assert_eq!(cpu_cell.value.0 .0, gpu_cell.value.0 .0, "value mismatch at tick={tick} x={x}");
            assert_eq!(cpu_cell.born_at, gpu_cell.born_at, "born_at mismatch at tick={tick} x={x}");
        }
        cpu_winners.push(cpu_grid.get_cell(1, 0).map(|c| c.value.0 .0));
    }

    let a_wins = cpu_winners.iter().filter(|&&w| w == Some(100)).count();
    let b_wins = cpu_winners.iter().filter(|&&w| w == Some(200)).count();
    assert_eq!(a_wins, (MODULUS / 2) as usize, "CPU-эталон сам должен чередовать 50/50 — иначе сцена не проверяет вращение");
    assert_eq!(b_wins, (MODULUS / 2) as usize, "CPU-эталон сам должен чередовать 50/50 — иначе сцена не проверяет вращение");
}
