use std::collections::HashMap;

use cellaria::engine::run_tick;
use cellaria::Grid;
use cellaria::VecStorage;
use cellaria::types::{Cell, CellType, CellValue, Direction, Rule, ShiftSpec};

// ============================================================================
// Helper: создать решётку с VecStorage
// ============================================================================

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

// ============================================================================
// TM-симуляция
// ============================================================================

fn turing_rules() -> Vec<Rule> {
    vec![
        Rule {
            id: vec![CellType(10), CellType(2)],
            pattern: vec![vec![10, 2]],
            shifts: vec![vec![ShiftSpec {
                direction: Direction::Right,
                steps: 1,
            }]],
            changes: vec![(-1, 0, 1)],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        },
        Rule {
            id: vec![CellType(10), CellType(1)],
            pattern: vec![vec![10, 1]],
            shifts: vec![vec![ShiftSpec {
                direction: Direction::Right,
                steps: 1,
            }]],
            changes: vec![(-1, 0, 2)],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        },
    ]
}

/// Запустить TM-симуляцию и вернуть количество тиков до остановки.
#[allow(clippy::manual_is_multiple_of)]
fn tm_bench(len: usize) -> usize {
    let width = len + 2;
    let mut grid = make_grid(width, 1);

    for i in 0..len {
        let val = if (i + len) % 2 == 0 { 1 } else { 2 };
        grid.set_cell(i + 1, 0, Cell {
            value: CellValue(CellType(val)),
            age: 0,
        });
    }

    grid.set_cell(0, 0, Cell {
        value: CellValue(CellType(10)),
        age: 0,
    });

    let rule_index = make_rule_index(turing_rules());

    let mut ticks = 0;
    loop {
        let (accepted, _) = run_tick(&mut grid, &rule_index);
        if accepted.is_empty() {
            break;
        }
        ticks += 1;
    }

    ticks
}

// ============================================================================
// Tag system (однопроходный маркер)
// ============================================================================

fn tag_rules() -> Vec<Rule> {
    vec![
        Rule {
            id: vec![CellType(10), CellType(1)],
            pattern: vec![vec![10, 1]],
            shifts: vec![vec![ShiftSpec {
                direction: Direction::Right,
                steps: 1,
            }]],
            changes: vec![(-1, 0, 0)],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        },
        Rule {
            id: vec![CellType(10), CellType(2)],
            pattern: vec![vec![10, 2]],
            shifts: vec![vec![ShiftSpec {
                direction: Direction::Right,
                steps: 1,
            }]],
            changes: vec![(-1, 0, 0)],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        },
    ]
}

/// Запустить tag system симуляцию и вернуть количество тиков.
fn tag_bench(len: usize) -> usize {
    let width = len + 10;
    let mut grid = make_grid(width, 1);

    for i in 0..len {
        let val = if (i + 7).is_multiple_of(2) { 1 } else { 2 };
        grid.set_cell(i + 1, 0, Cell {
            value: CellValue(CellType(val)),
            age: 0,
        });
    }

    grid.set_cell(0, 0, Cell {
        value: CellValue(CellType(10)),
        age: 0,
    });

    let rule_index = make_rule_index(tag_rules());

    let mut ticks = 0;
    loop {
        let (accepted, _) = run_tick(&mut grid, &rule_index);
        if accepted.is_empty() {
            break;
        }
        ticks += 1;
    }

    ticks
}

// ============================================================================
// Conflict-free правила
// ============================================================================

fn conflict_free_rules() -> Vec<Rule> {
    vec![
        Rule {
            id: vec![CellType(1), CellType(2)],
            pattern: vec![vec![1, 2]],
            shifts: vec![],
            changes: vec![(0, 0, 5), (1, 0, 5)],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        },
        Rule {
            id: vec![CellType(3), CellType(4)],
            pattern: vec![vec![3, 4]],
            shifts: vec![],
            changes: vec![(0, 0, 6), (1, 0, 6)],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        },
    ]
}

/// Запустить симуляцию conflict-free правил и вернуть количество тиков.
#[allow(clippy::implicit_saturating_sub)]
fn conflict_free_bench(width: usize) -> usize {
    let mut grid = make_grid(width, 1);

    grid.set_cell(0, 0, Cell {
        value: CellValue(CellType(1)),
        age: 0,
    });
    grid.set_cell(1, 0, Cell {
        value: CellValue(CellType(2)),
        age: 0,
    });

    let p2_start = if width > 3 { width - 3 } else { 0 };
    grid.set_cell(p2_start, 0, Cell {
        value: CellValue(CellType(3)),
        age: 0,
    });
    grid.set_cell(p2_start + 1, 0, Cell {
        value: CellValue(CellType(4)),
        age: 0,
    });

    let rule_index = make_rule_index(conflict_free_rules());

    let mut ticks = 0;
    loop {
        let (accepted, _) = run_tick(&mut grid, &rule_index);
        if accepted.is_empty() {
            break;
        }
        ticks += 1;
    }

    ticks
}

// ============================================================================
// Тесты сложности
// ============================================================================

fn run_complexity_tests() {
    println!("\n--- TM-симуляция ---");
    let mut results: Vec<(usize, usize)> = Vec::new();

    for &len in &[10, 50, 100, 200] {
        let ticks = tm_bench(len);
        println!("len={}, ticks={}", len, ticks);
        results.push((len, ticks));
    }

    for &(len, ticks) in &results {
        assert!(
            ticks <= 3 * len,
            "TM: len={} ticks={} превышает 3*len={}",
            len,
            ticks,
            3 * len
        );
    }

    let ratios: Vec<f64> = results.iter().map(|&(l, t)| t as f64 / l as f64).collect();
    for i in 1..ratios.len() {
        assert!(
            ratios[i] <= ratios[i - 1] * 1.5,
            "TM: отношение ticks/len растёт слишком быстро: {} -> {}",
            ratios[i - 1],
            ratios[i]
        );
    }

    println!("\n--- Tag system ---");
    let mut results2: Vec<(usize, usize)> = Vec::new();

    for &len in &[5, 10, 20, 50] {
        let ticks = tag_bench(len);
        println!("len={}, ticks={}", len, ticks);
        results2.push((len, ticks));
    }

    for &(len, ticks) in &results2 {
        assert!(
            ticks <= 2 * len,
            "Tag: len={} ticks={} превышает 2*len={}",
            len,
            ticks,
            2 * len
        );
    }

    let ratios2: Vec<f64> = results2.iter().map(|&(l, t)| t as f64 / l as f64).collect();
    for i in 1..ratios2.len() {
        assert!(
            ratios2[i] <= ratios2[i - 1] * 1.5,
            "Tag: отношение ticks/len растёт слишком быстро: {} -> {}",
            ratios2[i - 1],
            ratios2[i]
        );
    }

    println!("\n--- Conflict-free ---");
    let mut results3: Vec<(usize, usize)> = Vec::new();

    for &width in &[8, 16, 32, 64] {
        let ticks = conflict_free_bench(width);
        println!("width={}, ticks={}", width, ticks);
        results3.push((width, ticks));
    }

    for &(width, ticks) in &results3 {
        assert!(
            ticks <= 5,
            "Conflict-free: width={} ticks={} превышает 5",
            width,
            ticks
        );
    }

    println!("\nВсе тесты сложности пройдены.");
}

// ============================================================================
// Точка входа
// ============================================================================

fn main() {
    // Если задан аргумент --bench, запускаем criterion (cargo bench).
    // Иначе (cargo test --bench или прямой запуск) — запускаем тесты сложности.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--bench") {
        // Запуск criterion
        let mut criterion = criterion::Criterion::default()
            .configure_from_args();
        bench_turing(&mut criterion);
        bench_tag(&mut criterion);
        bench_conflict_free(&mut criterion);
        criterion.final_summary();
    } else {
        run_complexity_tests();
    }
}

fn bench_turing(c: &mut criterion::Criterion) {
    c.bench_function("turing_len_100", |b| {
        b.iter(|| {
            let _ticks = tm_bench(100);
        })
    });
}

fn bench_tag(c: &mut criterion::Criterion) {
    c.bench_function("tag_len_20", |b| {
        b.iter(|| {
            let _ticks = tag_bench(20);
        })
    });
}

fn bench_conflict_free(c: &mut criterion::Criterion) {
    c.bench_function("conflict_free_width_32", |b| {
        b.iter(|| {
            let _ticks = conflict_free_bench(32);
        })
    });
}