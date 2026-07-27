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
    let mut grid = Grid::new(storage, std::collections::HashSet::new());
    for y in 0..h {
        for x in 0..w {
            grid.set_cell(x, y, Cell {
                value: CellValue(CellType(0)),
                born_at: 0,
            });
        }
    }
    grid
}

/// Создать RuleIndex (HashMap<CellType, Vec<Rule>>) из Vec<Rule>.
pub fn make_rule_index(rules: Vec<Rule>) -> HashMap<CellType, Vec<Rule>> {
    let mut index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        if let Some(first) = rule.id.first() {
            index.entry(*first).or_default().push(rule);
        }
    }
    index
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
        })
        .collect()
}

// ============================================================================
// Хелперы: конфликтные правила
// ============================================================================

/// Построить M конфликтующих правил с разными приоритетами
pub fn priority_conflict_rules(m: usize) -> Vec<Rule> {
    (0..m)
        .map(|i| Rule {
            id: vec![CellType(1)],
            pattern: vec![(0i8, 0i8, CellType(1))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(i as u8))],
            active_only: false,
            priority: i as u32,
            min_age: 0,
            overflow: Default::default(),
        })
        .collect()
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
    let mut grid = Grid::new(storage, std::collections::HashSet::new());
    for y in 0..h {
        for x in 0..w {
            grid.set_cell(x, y, Cell {
                value: CellValue(CellType(0)),
                born_at: 0,
            });
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
        grid.set_cell(x, 0, Cell {
            value: CellValue(CellType(x as u8 % 4)),
            born_at: 0,
        });
    }

    let start = Instant::now();
    let (_, _) = run_tick(&mut grid, &rule_index);
    start.elapsed().as_micros()
}

// ============================================================================
// Хелперы: Find rule benchmark
// ============================================================================

/// Найти подходящее правило по id в rule_index (используется в profile_find_rule).
pub fn find_rule_bench<'a>(
    id: &'a [CellType],
    rule_index: &'a HashMap<CellType, Vec<Rule>>,
) -> Option<&'a Rule> {
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
        })
        .collect()
}

/// Замерить время цепной реакции длины len.
pub fn replication_bench(len: usize) -> u128 {
    let rules = replication_rules(len);
    let rule_index = make_rule_index(rules);

    let mut grid = make_grid(len + 1, 1);
    grid.set_cell(0, 0, Cell {
        value: CellValue(CellType(0)),
        born_at: 0,
    });

    let start = Instant::now();
    for _ in 0..len {
        let (_, _) = run_tick(&mut grid, &rule_index);
    }
    start.elapsed().as_micros()
}