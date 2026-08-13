use std::time::Instant;
use std::collections::HashMap;

use cellaria::engine::run_tick;
use cellaria::Grid;
use cellaria::VecStorage;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Direction, Rule, ShiftSpec};

// ============================================================================
// Хелперы: создание структур
// ============================================================================

/// Создать пустую решётку w×h с активными клетками (через Grid<VecStorage>).
pub fn make_grid(w: usize, h: usize) -> Grid<VecStorage> {
    let storage = VecStorage::new(w, h);
    let mut grid = Grid::from_storage(storage);
    for y in 0..h {
        for x in 0..w {
            grid.set_cell(
                x,
                y,
                Cell {
                    value: CellValue(CellType(0)),
                    born_at: 0,
                },
            );
        }
    }
    grid
}

/// Создать RuleIndex (HashMap<CellType, Vec<Rule>>) из Vec<Rule>. Тонкая
/// обёртка над `cellaria::build_rule_index` (не собственная копия —
/// найдено при подготовке модели к 1.0, что этот бенчмарк был одним из
/// 20+ мест в кодовой базе, независимо переизобретавших эту логику).
pub fn make_rule_index(rules: Vec<Rule>) -> HashMap<CellType, Vec<Rule>> {
    cellaria::build_rule_index(rules)
}

/// Создать одну группу сдвига вправо на 1 шаг.
pub fn shift_right_one() -> Vec<ShiftSpec> {
    vec![ShiftSpec::new(Direction::Right, 1)]
}

/// Создать одну группу сдвига вправо на 2 шага.
pub fn shift_right_two() -> Vec<ShiftSpec> {
    vec![ShiftSpec::new(Direction::Right, 2)]
}

// ============================================================================
// Хелперы: TM-правила
// ============================================================================

/// Построить TM-правила для ленты длины n.
pub fn turing_rules(n: usize) -> Vec<Rule> {
    let mut state = 0usize;
    let mut rules = Vec::new();
    while state < n {
        let q = state as u8;
        rules.push(Rule {
            id: vec![CellType(1), CellType(2), CellType(q)],
            pattern: vec![
                (0i8, 0i8, CellType(1)),
                (2i8, 0i8, CellType(2)),
                (1i8, 0i8, CellType(q)),
            ],
            shifts: vec![shift_right_one()],
            changes: vec![(1, 0, ChangeValue::Literal(1))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        });
        state += 1;
    }
    rules
}

/// Построить TM-правила для ленты длины n (упрощённые).
pub fn simple_turing_rules(n: usize) -> Vec<Rule> {
    (0..n)
        .map(|i| Rule {
            id: vec![CellType(i as u8 % 4)],
            pattern: vec![(0i8, 0i8, CellType(i as u8 % 4))],
            shifts: vec![shift_right_one()],
            changes: vec![(0, 0, ChangeValue::Literal(i as u8 + 1))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        })
        .collect()
}

// ============================================================================
// Хелперы: tag system
// ============================================================================

/// Построить tag system rules длины len.
pub fn tag_rules(len: usize) -> Vec<Rule> {
    (0..len)
        .map(|i| Rule {
            id: vec![CellType(i as u8 % 4)],
            pattern: vec![(0i8, 0i8, CellType(i as u8 % 4))],
            shifts: vec![shift_right_two()],
            changes: vec![(0, 0, ChangeValue::Literal(0))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        })
        .collect()
}

// ============================================================================
// Хелперы: конфликтные правила
// ============================================================================

/// Построить M конфликтующих правил с разными приоритетами, плюс одно
/// возвратное правило от типа-победителя (наивысший приоритет — `m-1`)
/// обратно к типу 1 — без него побеждает ровно ОДНО правило на первом же
/// тике (приоритет `m-1`), клетка становится типом `m-1`, для которого
/// правил (кроме m=2, где `m-1 == 1` — тривиальный самоцикл) больше нет, и
/// все последующие тики в измерительном окне бенчмарка пусты. Найдено при
/// сверке `max_throughput_conflict`: `tps` был практически одинаков для
/// ЛЮБОГО M (10, 50, 100, 200) — верный признак, что реальный M-way
/// конфликт разрешается только один раз, а всё остальное окно меряет
/// пустые тики. С возвратным правилом M-конфликт (и настоящий арбитраж
/// между M кандидатами) происходит на КАЖДОМ "прямом" такте оscillator'а,
/// а не один раз за весь прогон.
pub fn priority_conflict_rules(m: usize) -> Vec<Rule> {
    let winner_type = (m.saturating_sub(1)) as u8;
    let mut rules: Vec<Rule> = (0..m)
        .map(|i| Rule {
            id: vec![CellType(1)],
            pattern: vec![(0i8, 0i8, CellType(1))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(i as u8))],
            active_only: false,
            priority: i as u32,
            min_age: 0,
            overflow: Default::default(),
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
    rules.push(Rule {
        id: vec![CellType(winner_type)],
        pattern: vec![(0i8, 0i8, CellType(winner_type))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(1))],
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    });
    rules
}

// ============================================================================
// Хелперы: Storage benchmarks
// ============================================================================

/// Замерить время итерации по VecStorage w×h.
pub fn storage_bench_vec(w: usize, h: usize) -> u128 {
    let grid = make_grid(w, h);
    let start = Instant::now();
    let mut count = 0usize;
    for _ in 0..100 {
        for (x, y) in grid.iter_active() {
            let _cell = grid.get_cell(x, y);
            count += 1;
        }
    }
    let _ = count;
    start.elapsed().as_micros() / 100
}

/// Замерить время итерации по ChunkStorage w×h.
pub fn storage_bench_chunk(w: usize, h: usize) -> u128 {
    let storage = cellaria::ChunkStorage::new();
    let mut grid = Grid::from_storage(storage);
    for y in 0..h {
        for x in 0..w {
            grid.set_cell(
                x,
                y,
                Cell {
                    value: CellValue(CellType(0)),
                    born_at: 0,
                },
            );
        }
    }
    let start = Instant::now();
    let mut count = 0usize;
    for _ in 0..100 {
        for (x, y) in grid.iter_active() {
            let _cell = grid.get_cell(x, y);
            count += 1;
        }
    }
    let _ = count;
    start.elapsed().as_micros() / 100
}

// ============================================================================
// Хелперы: Grid growth benchmark
// ============================================================================

/// Замерить время итерации по сетке N×N.
pub fn grid_growth_bench(n: usize) -> u128 {
    let grid = make_grid(n, n);
    let start = Instant::now();
    let mut count = 0usize;
    for (_x, _y) in grid.iter_active() {
        count += 1;
    }
    let _ = count;
    start.elapsed().as_micros()
}

// ============================================================================
// Хелперы: Rule count benchmark
// ============================================================================

/// Создать K правил и замерить время тика.
pub fn rule_count_bench(k: usize) -> u128 {
    let rules = simple_turing_rules(k);
    let rule_index = make_rule_index(rules);

    let mut grid = make_grid(k + 2, 1);
    for x in 0..k {
        grid.set_cell(
            x,
            0,
            Cell {
                value: CellValue(CellType(x as u8 % 4)),
                born_at: 0,
            },
        );
    }

    let start = Instant::now();
    let (_, _) = run_tick(&mut grid, &rule_index);
    start.elapsed().as_micros()
}

// ============================================================================
// Хелперы: Find rule benchmark
// ============================================================================

/// Найти подходящее правило по id в rule_index (используется в profile_find_rule).
pub fn find_rule_bench<'a>(id: &'a [CellType], rule_index: &'a HashMap<CellType, Vec<Rule>>) -> Option<&'a Rule> {
    let first = *id.first()?;
    let rules = rule_index.get(&first)?;
    rules.iter().find(|r| r.id == id)
}

// ============================================================================
// Хелперы: Replication benchmark
// ============================================================================

/// Создать цепь правил длины len (каждое правило активирует следующее).
pub fn replication_rules(len: usize) -> Vec<Rule> {
    (0..len)
        .map(|i| Rule {
            id: vec![CellType(i as u8 % 10)],
            pattern: vec![(0i8, 0i8, CellType(i as u8 % 10))],
            shifts: vec![shift_right_one()],
            changes: vec![(0, 0, ChangeValue::Literal((i + 1) as u8 % 10))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        })
        .collect()
}

/// Замерить время цепной реакции длины len.
pub fn replication_bench(len: usize) -> u128 {
    let rules = replication_rules(len);
    let rule_index = make_rule_index(rules);

    let mut grid = make_grid(len + 1, 1);
    grid.set_cell(
        0,
        0,
        Cell {
            value: CellValue(CellType(0)),
            born_at: 0,
        },
    );

    let start = Instant::now();
    for _ in 0..len {
        let (_, _) = run_tick(&mut grid, &rule_index);
    }
    start.elapsed().as_micros()
}
