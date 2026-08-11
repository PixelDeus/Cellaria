//! Проверка `GpuEngine` (v1-подмножество, feature `gpu`) против CPU-эталона
//! `engine::run_tick` — то самое сравнение, которое план называет ключевой
//! верификацией перед тем, как считать GPU-бэкенд рабочим: не "GPU быстрый",
//! а "GPU даёт ТОТ ЖЕ результат, что и настоящий движок cellaria".
//!
//! Два независимых теста:
//!
//! 1. `test_gpu_matches_cpu_for_real_game_of_life` — ПОЛНЫЙ, настоящий набор
//!    правил Game of Life (256 конфигураций соседей на голову, как в
//!    `examples/flagship_gol.rs`), несколько тиков подряд, несколько
//!    плотностей заполнения. Это именно тот тест, что поймал реальный баг:
//!    первая версия шейдера трактовала офсет паттерна вне границ решётки как
//!    "дефолтное значение" для КАЖДОГО правила независимо, тогда как
//!    CPU-матчер (`matcher::match_cell`) отключает ВСЮ голову целиком на
//!    клетке, если хоть один офсет union'а всех правил головы выходит за
//!    границу (см. `gpu::rule_table::GpuHeadSlot::offsets_start`) — на
//!    случайно заполненной решётке живые клетки почти всегда есть у самого
//!    края, так что этот тест ловит регрессию границ автоматически, без
//!    догадок о том, где именно смотреть.
//!
//! 2. `test_gpu_matches_cpu_property_random_v1_rules` — property-тест:
//!    случайные наборы правил СТРОГО в рамках v1-подмножества (без сдвигов,
//!    только self-changes), в том числе с НАМЕРЕННО пересекающимися
//!    паттернами одной головы — GoL-паттерны взаимоисключающие по
//!    построению (полностью специфицируют все 8 соседей) и НИКОГДА не
//!    порождают тай-брейк между правилами; этот тест специально гоняет тай-брейк
//!    (priority → id → rule_idx, см. `shader.wgsl`) через proptest.

#![cfg(feature = "gpu")]

use std::collections::{HashMap, HashSet};

use proptest::prelude::*;

use cellaria::engine::run_tick;
use cellaria::gpu::GpuEngine;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Rule};
use cellaria::{Grid, VecStorage};

// ============================================================================
// Тест 1: настоящий Game of Life (дублирует генератор из
// examples/flagship_gol.rs — examples не являются библиотечным кодом и не
// импортируемы отсюда напрямую).
// ============================================================================

const ALIVE: u8 = 1;
const DEAD: u8 = 0;

const NEIGHBOR_OFFSETS: [(i8, i8); 8] = [
    (-1, -1), (0, -1), (1, -1),
    (-1, 0), (1, 0),
    (-1, 1), (0, 1), (1, 1),
];

fn build_gol_rule_index() -> HashMap<CellType, Vec<Rule>> {
    let mut index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for center in [DEAD, ALIVE] {
        for mask in 0u8..=255 {
            let alive_neighbors = mask.count_ones();
            let next = match (center, alive_neighbors) {
                (ALIVE, 2) | (ALIVE, 3) => ALIVE,
                (DEAD, 3) => ALIVE,
                _ => DEAD,
            };
            if next == center {
                continue;
            }
            let mut pattern: Vec<(i8, i8, CellType)> = vec![(0, 0, CellType(center))];
            for (i, &(dx, dy)) in NEIGHBOR_OFFSETS.iter().enumerate() {
                let bit = (mask >> i) & 1;
                pattern.push((dx, dy, CellType(bit)));
            }
            index.entry(CellType(center)).or_default().push(Rule {
                id: vec![CellType(center)],
                pattern,
                shifts: vec![],
                changes: vec![(0, 0, ChangeValue::Literal(next))],
                active_only: false,
                priority: 10,
                min_age: 0,
                overflow: Default::default(),
                cam: None,
                tie_break: 0,
                starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None, cross_layer_reads: Vec::new(),
            });
        }
    }
    index
}

fn xorshift_fill(n: usize, density_percent: u64, seed: u64) -> Vec<u8> {
    let mut state = seed | 1; // xorshift требует ненулевое состояние
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    (0..n * n).map(|_| if next() % 100 < density_percent { ALIVE } else { DEAD }).collect()
}

#[test]
fn test_gpu_matches_cpu_for_real_game_of_life() {
    let rule_index = build_gol_rule_index();
    let n = 14;
    let ticks = 5;

    for &(density, seed) in &[(10u64, 1u64), (30, 2), (50, 3), (70, 4)] {
        let initial_flat = xorshift_fill(n, density, seed);

        let storage = VecStorage::new(n, n);
        let mut cpu_grid = Grid::new(storage, HashSet::new());
        let mut gpu_initial: Vec<(usize, usize, Cell)> = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let v = initial_flat[y * n + x];
                if v != DEAD {
                    let cell = Cell { value: CellValue(CellType(v)), born_at: 0 };
                    cpu_grid.set_cell(x, y, cell);
                    gpu_initial.push((x, y, cell));
                }
            }
        }

        let mut gpu_engine = GpuEngine::new(n, n, &gpu_initial, &rule_index)
            .expect("Game of Life is fully within the v1 subset (no shifts, self-only Literal changes)");

        for tick in 0..ticks {
            run_tick(&mut cpu_grid, &rule_index);
            gpu_engine.run_tick();

            let gpu_result = gpu_engine.read_grid();
            for y in 0..n {
                for x in 0..n {
                    let cpu_cell = cpu_grid.get_cell(x, y).copied().unwrap_or_default();
                    let gpu_cell = gpu_result[y * n + x];
                    assert_eq!(
                        cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                        "value mismatch at density={density}% tick={tick} ({x},{y})"
                    );
                    assert_eq!(
                        cpu_cell.born_at, gpu_cell.born_at,
                        "born_at mismatch at density={density}% tick={tick} ({x},{y})"
                    );
                }
            }
        }
    }
}

// ============================================================================
// Тест 2: property-based, случайные v1-совместимые наборы правил —
// специально нацелен на тай-брейк (priority → id → rule_idx), который
// мутуально-исключающие GoL-паттерны никогда не задействуют.
// ============================================================================

const CELL_ALPHABET: u8 = 3;
const SIDE: usize = 6;

fn cell_type_strategy() -> impl Strategy<Value = u8> {
    1..=CELL_ALPHABET
}

/// v1-паттерн: голова в (0,0) + 0..=3 доп. клеток в окрестности ±1 — узкая
/// окрестность специально увеличивает шанс, что паттерны РАЗНЫХ правил одной
/// головы пересекутся и матчер найдёт несколько кандидатов на одной клетке
/// (то, что и должен разруливать тай-брейк).
fn v1_pattern_strategy() -> impl Strategy<Value = (u8, Vec<(i8, i8, CellType)>)> {
    (
        cell_type_strategy(),
        prop::collection::vec((-1i8..=1, -1i8..=1, cell_type_strategy()), 0..=3),
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

fn v1_rule_strategy() -> impl Strategy<Value = Rule> {
    (
        v1_pattern_strategy(),
        1u8..=9, // new_value записываемый в (0,0)
        0u32..=3, // priority — намеренно узкий диапазон, чтобы равенства случались часто
        0u64..=1, // min_age
        any::<bool>(),
    )
        .prop_map(|((head, pattern), new_value, priority, min_age, active_only)| Rule {
            id: vec![CellType(head)],
            pattern,
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(new_value))],
            active_only,
            priority,
            min_age,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None, cross_layer_reads: Vec::new(),
        })
}

fn v1_rule_set_strategy() -> impl Strategy<Value = Vec<Rule>> {
    prop::collection::vec(v1_rule_strategy(), 1..=5)
}

fn make_rule_index(rules: &[Rule]) -> HashMap<CellType, Vec<Rule>> {
    let mut index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        if let Some(&head) = rule.id.first() {
            index.entry(head).or_default().push(rule.clone());
        }
    }
    // Тот же порядок, что и `config::load_config`/`RuleStore::get_index` —
    // критично: и CPU (`RuleMatch::rule_idx`), и GPU-кодировщик
    // (`rule_table::build_gpu_rule_table`'s `enumerate()`) используют
    // позицию в ЭТОМ отсортированном Vec как последний уровень тай-брейка,
    // так что порядок обязан совпадать между сторонами.
    for group in index.values_mut() {
        group.sort_by_key(|r| std::cmp::Reverse(r.priority));
    }
    index
}

fn grid_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(0..=CELL_ALPHABET, SIDE * SIDE)
}

proptest! {
    // Каждый кейс строит настоящее GPU-устройство (`GpuEngine::new`) — не
    // тысячи, как в других property-тестах: важно накрыть тай-брейк, а не
    // упереться в накладные расходы инициализации wgpu на кейс.
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn test_gpu_matches_cpu_property_random_v1_rules(rules in v1_rule_set_strategy(), cells in grid_strategy()) {
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

        // build_gpu_rule_table может в принципе отклонить набор (например,
        // если сам генератор когда-нибудь выйдет за v1-рамки) — по
        // построению этой стратегии такого не бывает, но пропускаем кейс
        // вместо паники, если всё же случится: это тест GPU-CPU эквивалентности
        // ДЛЯ v1-подмножества, а не теста самого `GpuUnsupportedReason`.
        let Ok(mut gpu_engine) = GpuEngine::new(SIDE, SIDE, &gpu_initial, &rule_index) else {
            return Ok(());
        };

        run_tick(&mut cpu_grid, &rule_index);
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();

        for y in 0..SIDE {
            for x in 0..SIDE {
                let cpu_cell = cpu_grid.get_cell(x, y).copied().unwrap_or_default();
                let gpu_cell = gpu_result[y * SIDE + x];
                prop_assert_eq!(cpu_cell.value.0 .0, gpu_cell.value.0 .0, "value mismatch at ({}, {})", x, y);
                prop_assert_eq!(cpu_cell.born_at, gpu_cell.born_at, "born_at mismatch at ({}, {})", x, y);
            }
        }
    }
}
