use std::collections::{HashMap, HashSet};
use std::time::Instant;

use cellaria::engine::run_tick;
use cellaria::Grid;
use cellaria::ChunkStorage;
use cellaria::VecStorage;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Direction, Rule, RuleMatch, ShiftSpec};

// ============================================================================
// Helper: создать решётку с VecStorage
// ============================================================================

fn make_grid(width: usize, height: usize) -> Grid<VecStorage> {
    let storage = VecStorage::new(width, height);
    Grid::new(storage, HashSet::new())
}

/// Создать решётку с ChunkStorage (бесконечная).
fn make_grid_chunk() -> Grid<ChunkStorage> {
    let storage = ChunkStorage::new();
    Grid::new(storage, HashSet::new())
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
            pattern: vec![(0i8, 0i8, CellType(10)), (1i8, 0i8, CellType(2))],
            shifts: vec![vec![ShiftSpec {
                direction: Direction::Right,
                steps: 1,
            }]],
            changes: vec![(-1, 0, ChangeValue::Literal(1))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        },
        Rule {
            id: vec![CellType(10), CellType(1)],
            pattern: vec![(0i8, 0i8, CellType(10)), (1i8, 0i8, CellType(1))],
            shifts: vec![vec![ShiftSpec {
                direction: Direction::Right,
                steps: 1,
            }]],
            changes: vec![(-1, 0, ChangeValue::Literal(2))],
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
        let active: Vec<(usize, usize)> = grid.iter_active().collect();
        let (accepted, _) = run_tick(&mut grid, &rule_index, &active);
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
            pattern: vec![(0i8, 0i8, CellType(10)), (1i8, 0i8, CellType(1))],
            shifts: vec![vec![ShiftSpec {
                direction: Direction::Right,
                steps: 1,
            }]],
            changes: vec![(-1, 0, ChangeValue::Literal(0))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        },
        Rule {
            id: vec![CellType(10), CellType(2)],
            pattern: vec![(0i8, 0i8, CellType(10)), (1i8, 0i8, CellType(2))],
            shifts: vec![vec![ShiftSpec {
                direction: Direction::Right,
                steps: 1,
            }]],
            changes: vec![(-1, 0, ChangeValue::Literal(0))],
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
        let active: Vec<(usize, usize)> = grid.iter_active().collect();
        let (accepted, _) = run_tick(&mut grid, &rule_index, &active);
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
            pattern: vec![(0i8, 0i8, CellType(1)), (1i8, 0i8, CellType(2))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(5)), (1, 0, ChangeValue::Literal(5))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        },
        Rule {
            id: vec![CellType(3), CellType(4)],
            pattern: vec![(0i8, 0i8, CellType(3)), (1i8, 0i8, CellType(4))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(6)), (1, 0, ChangeValue::Literal(6))],
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
        let active: Vec<(usize, usize)> = grid.iter_active().collect();
        let (accepted, _) = run_tick(&mut grid, &rule_index, &active);
        if accepted.is_empty() {
            break;
        }
        ticks += 1;
    }

    ticks
}

// ============================================================================
// Бенчмарк 1: Worst-case arbitration
//
// Для M правил с id: [1, 0] и changes: [(0,0,Literal(...))] affected cells = [(0,0), (1,0)].
// Первое совпадение проверяет обе (принимается), остальные M-1 конфликтуют на (0,0).
// Итого проверок: 2 + (M-1)*1 = M + 1 (линейно, не квадратично).
// ============================================================================

/// Локальная копия arbitrate с дополнительным счётчиком used_cells.contains.
/// Используется ТОЛЬКО для benches/cellaria_bench.rs, не для production.
fn arbitrate_with_counter(
    all_matches: Vec<RuleMatch>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    _get_cell_age: impl Fn(usize, usize) -> u32,
    counter: &mut u64,
) -> Vec<RuleMatch> {
    use std::collections::HashSet;

    if all_matches.is_empty() {
        return Vec::new();
    }

    let mut accepted: Vec<RuleMatch> = Vec::new();
    let mut used_cells: HashSet<(u32, u32)> = HashSet::new();

    let mut sorted = all_matches;
    sorted.sort_by(|a, b| {
        let priority_a = get_priority_bench(&a.rule_id, rule_index);
        let priority_b = get_priority_bench(&b.rule_id, rule_index);
        priority_b.cmp(&priority_a).then_with(|| {
            let age_a = _get_cell_age(a.x as usize, a.y as usize);
            let age_b = _get_cell_age(b.x as usize, b.y as usize);
            age_b.cmp(&age_a)
        })
    });

    for m in sorted {
        let affected = get_match_affected_cells_bench(&m, rule_index);
        let mut conflict = false;

        for &(px, py) in &affected {
            if px >= 0 && py >= 0 {
                let coord = (px as u32, py as u32);
                *counter += 1;
                if used_cells.contains(&coord) {
                    conflict = true;
                    break;
                }
            }
        }

        if !conflict {
            for &(px, py) in &affected {
                if px >= 0 && py >= 0 {
                    used_cells.insert((px as u32, py as u32));
                }
            }
            accepted.push(m);
        }
    }

    accepted
}

fn get_priority_bench(rule_id: &[CellType], rule_index: &HashMap<CellType, Vec<Rule>>) -> u32 {
    if let Some(first) = rule_id.first() {
        if let Some(rules) = rule_index.get(first) {
            for rule in rules {
                if rule.id == rule_id {
                    return rule.priority;
                }
            }
        }
    }
    0
}

fn get_match_affected_cells_bench(
    m: &RuleMatch,
    rule_index: &HashMap<CellType, Vec<Rule>>,
) -> Vec<(i32, i32)> {
    use cellaria::conflict_analyzer::compute_affected_cells;
    let rule = find_rule_bench(&m.rule_id, rule_index);
    if let Some(rule) = rule {
        let relative = compute_affected_cells(rule);
        relative
            .iter()
            .map(|&(dx, dy)| (m.x as i32 + dx, m.y as i32 + dy))
            .collect()
    } else {
        let mut cells = Vec::new();
        for (i, _) in m.rule_id.iter().enumerate() {
            cells.push((m.x as i32 + i as i32, m.y as i32));
        }
        cells
    }
}

fn find_rule_bench<'a>(
    rule_id: &[CellType],
    rule_index: &'a HashMap<CellType, Vec<Rule>>,
) -> Option<&'a Rule> {
    if let Some(first) = rule_id.first() {
        if let Some(rules) = rule_index.get(first) {
            for rule in rules {
                if rule.id == rule_id {
                    return Some(rule);
                }
            }
        }
    }
    None
}

/// Создать M конфликтующих правил с уникальными id: [1, 0, i].
/// Все имеют одинаковый pattern [1, 0] и priority:10.
/// Каждое правило меняет ячейку (0,0) на своё уникальное значение,
/// поэтому все M совпадений конфликтуют за ячейку (0,0).
fn make_conflicting_rules(m: usize) -> Vec<Rule> {
    (0..m)
        .map(|i| Rule {
            id: vec![CellType(1), CellType(0), CellType(i as u8)],
            pattern: vec![(0i8, 0i8, CellType(1)), (1i8, 0i8, CellType(0))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(100 + i as u8))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        })
        .collect()
}

/// Запустить worst-case арбитраж и вернуть число проверок used_cells.contains.
fn worst_case_bench(m: usize) -> u64 {
    let mut grid = make_grid(2, 1);
    grid.set_cell(
        0,
        0,
        Cell {
            value: CellValue(CellType(1)),
            age: 0,
        },
    );
    grid.set_cell(
        1,
        0,
        Cell {
            value: CellValue(CellType(0)),
            age: 0,
        },
    );

    let rules = make_conflicting_rules(m);
    let rule_index = make_rule_index(rules);

    let active: Vec<(usize, usize)> = grid.iter_active().collect();
    let matches = cellaria::engine::detect_matches(&grid, &rule_index, &active);
    assert_eq!(matches.len(), m, "Должно быть M={} совпадений", m);

    let mut counter = 0u64;
    let _accepted = arbitrate_with_counter(matches, &rule_index, |_x, _y| 0u32, &mut counter);

    counter
}

// ============================================================================
// Бенчмарк 2: ChunkStorage vs VecStorage
//
// Гипотеза: ChunkStorage не добавляет накладных расходов на малых решётках
// по сравнению с VecStorage. Время ChunkStorage ≤ 2× время VecStorage.
// ============================================================================

/// Заполнить решётку чередованием [1, 2, 1, 2, ...] в каждой строке.
/// Это гарантирует, что правило id: [1, 2] матчится на каждой соседней паре.
fn fill_alternating<S: cellaria::GridStorage>(grid: &mut Grid<S>, w: usize, h: usize) {
    for y in 0..h {
        for x in 0..w {
            let val = if x % 2 == 0 { 1 } else { 2 };
            grid.set_cell(
                x,
                y,
                Cell {
                    value: CellValue(CellType(val)),
                    age: 0,
                },
            );
        }
    }
}

/// Правило id: [1, 2] меняет обе ячейки на 3 и 4.
fn storage_rule() -> Vec<Rule> {
    vec![Rule {
        id: vec![CellType(1), CellType(2)],
        pattern: vec![(0i8, 0i8, CellType(1)), (1i8, 0i8, CellType(2))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(3)), (1, 0, ChangeValue::Literal(4))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
    }]
}

/// Замерить время одного тика на решётке width×height.
fn storage_bench_vec(width: usize, height: usize) -> u128 {
    let mut grid = make_grid(width, height);
    fill_alternating(&mut grid, width, height);
    let rule_index = make_rule_index(storage_rule());

    let start = Instant::now();
    let active: Vec<(usize, usize)> = grid.iter_active().collect();
    let (accepted, _) = run_tick(&mut grid, &rule_index, &active);
    let elapsed = start.elapsed().as_micros();

    assert!(!accepted.is_empty(), "storage_bench_vec: тик не дал совпадений");
    elapsed
}

fn storage_bench_chunk(width: usize, height: usize) -> u128 {
    let mut grid = make_grid_chunk();
    fill_alternating(&mut grid, width, height);
    let rule_index = make_rule_index(storage_rule());

    let start = Instant::now();
    let active: Vec<(usize, usize)> = grid.iter_active().collect();
    let (accepted, _) = run_tick(&mut grid, &rule_index, &active);
    let elapsed = start.elapsed().as_micros();

    assert!(!accepted.is_empty(), "storage_bench_chunk: тик не дал совпадений");
    elapsed
}

// ============================================================================
// Бенчмарк 3: Рост решётки — O(активные ячейки)
//
// Гипотеза: время тика линейно по числу активных ячеек.
// ============================================================================

/// Правило для TM-головки: id: [10, 1] → сдвиг вправо, стирает head, пишет 2 слева.
fn grid_growth_rule() -> Vec<Rule> {
    vec![Rule {
        id: vec![CellType(10), CellType(1)],
        pattern: vec![(0i8, 0i8, CellType(10)), (1i8, 0i8, CellType(1))],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 1,
        }]],
        changes: vec![(-1, 0, ChangeValue::Literal(2))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
    }]
}

/// Замерить время одного тика на ленте длины N.
fn grid_growth_bench(n: usize) -> u128 {
    let width = n + 2;
    let mut grid = make_grid(width, 1);

    for i in 0..n {
        let val = if (i + n) % 2 == 0 { 1 } else { 2 };
        grid.set_cell(
            i + 1,
            0,
            Cell {
                value: CellValue(CellType(val)),
                age: 0,
            },
        );
    }

    grid.set_cell(
        0,
        0,
        Cell {
            value: CellValue(CellType(10)),
            age: 0,
        },
    );

    let rule_index = make_rule_index(grid_growth_rule());

    let start = Instant::now();
    let active: Vec<(usize, usize)> = grid.iter_active().collect();
    let (accepted, _) = run_tick(&mut grid, &rule_index, &active);
    let elapsed = start.elapsed().as_micros();

    assert!(!accepted.is_empty(), "grid_growth_bench: тик не дал совпадений для N={}", n);
    elapsed
}

// ============================================================================
// Бенчмарк 4: Множество правил — O(K)
//
// Гипотеза: поиск правила по head-типу не зависит от общего числа правил.
// ============================================================================

fn make_many_rules(k: usize) -> Vec<Rule> {
    (0..k)
        .map(|i| Rule {
            id: vec![CellType(i as u8 + 1)],
            pattern: vec![(0i8, 0i8, CellType(i as u8 + 1))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(99))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        })
        .collect()
}

fn rule_count_bench(k: usize) -> u128 {
    let mut grid = make_grid(k, 1);

    for i in 0..k {
        grid.set_cell(
            i,
            0,
            Cell {
                value: CellValue(CellType(i as u8 + 1)),
                age: 0,
            },
        );
    }

    let rules = make_many_rules(k);
    let rule_index = make_rule_index(rules);

    let start = Instant::now();
    let active: Vec<(usize, usize)> = grid.iter_active().collect();
    let (accepted, _) = run_tick(&mut grid, &rule_index, &active);
    let elapsed = start.elapsed().as_micros();

    assert_eq!(accepted.len(), k, "rule_count_bench: должны сматчиться все K={} правил", k);
    elapsed
}

// ============================================================================
// Бенчмарк 5: Саморепликация
//
// Гипотеза: total_time(len) ~ O(len²).
// ============================================================================

fn replication_rule() -> Vec<Rule> {
    vec![Rule {
        id: vec![CellType(10), CellType(0)],
        pattern: vec![(0i8, 0i8, CellType(10)), (1i8, 0i8, CellType(0))],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 1,
        }]],
        changes: vec![(-1, 0, ChangeValue::Literal(10))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
    }]
}

fn replication_bench(len: usize) -> u128 {
    let width = len + 10;
    let mut grid = make_grid(width, 1);

    grid.set_cell(
        0,
        0,
        Cell {
            value: CellValue(CellType(10)),
            age: 0,
        },
    );

    let rule_index = make_rule_index(replication_rule());

    let start = Instant::now();
    for _ in 0..len {
        let active: Vec<(usize, usize)> = grid.iter_active().collect();
        let (accepted, _) = run_tick(&mut grid, &rule_index, &active);
        if accepted.is_empty() {
            break;
        }
    }
    let elapsed = start.elapsed().as_micros();
    elapsed
}

// ============================================================================
// Тесты сложности (старые)
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

    println!("\nВсе оригинальные тесты сложности пройдены.");
}

// ============================================================================
// Тесты для новых бенчмарков
// ============================================================================

fn run_bench_tests() {
    // Бенчмарк 1: worst-case arbitration
    let count_10 = worst_case_bench(10);
    let count_20 = worst_case_bench(20);
    // affected = [(0,0), (1,0)]; первое проверяет обе, остальные конфликтуют на (0,0)
    // counter = 2 + (M-1)*1 = M + 1
    assert_eq!(count_10, 11, "Worst-case M=10: expected 11, got {}", count_10);
    assert_eq!(count_20, 21, "Worst-case M=20: expected 21, got {}", count_20);
    println!("Worst-case arbitration: M=10 → {} (exp 11), M=20 → {} (exp 21) OK", count_10, count_20);

    // Бенчмарк 2: storage
    let vec_time = storage_bench_vec(100, 100);
    let chunk_time = storage_bench_chunk(100, 100);
    assert!(vec_time < 100_000, "VecStorage 100×100: {}µs > 100ms", vec_time);
    assert!(chunk_time < 200_000, "ChunkStorage 100×100: {}µs > 200ms", chunk_time);
    assert!(chunk_time <= vec_time * 2 || vec_time < 1000,
        "Chunk {}µs > 2× Vec {}µs", chunk_time, vec_time);
    println!("Storage 100×100: Vec={}µs, Chunk={}µs OK", vec_time, chunk_time);

    // Бенчмарк 3: grid growth
    let time_100 = grid_growth_bench(100);
    let time_1000 = grid_growth_bench(1000);
    assert!(time_1000 <= time_100 * 10 || time_100 < 10,
        "Grid growth N=1000 {}µs > 10× N=100 {}µs", time_1000, time_100);
    println!("Grid growth: N=100 → {}µs, N=1000 → {}µs OK", time_100, time_1000);

    // Бенчмарк 4: rule count
    let time_10 = rule_count_bench(10);
    let time_200 = rule_count_bench(200);
    assert!(time_200 <= time_10 * 50 || time_10 < 10,
        "Rule count K=200 {}µs > 50× K=10 {}µs", time_200, time_10);
    println!("Rule count: K=10 → {}µs, K=200 → {}µs OK", time_10, time_200);

    // Бенчмарк 5: replication
    let repl_time_10 = replication_bench(10);
    let repl_time_100 = replication_bench(100);
    assert!(repl_time_100 <= repl_time_10 * 100 || repl_time_10 < 50,
        "Replication len=100 {}µs > 100× len=10 {}µs", repl_time_100, repl_time_10);
    println!("Replication: len=10 → {}µs, len=100 → {}µs OK", repl_time_10, repl_time_100);

    println!("\nВсе новые бенчмарк-тесты пройдены.");
}

// ============================================================================
// Criterion-бенчмарки
// ============================================================================

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

fn bench_worst_case(c: &mut criterion::Criterion) {
    let mut group = c.benchmark_group("worst_case_arbitration");
    for &m in &[10, 20, 50, 100] {
        group.bench_function(format!("M_{}", m), |b| {
            b.iter(|| {
                let _count = worst_case_bench(m);
            })
        });
    }
    group.finish();
}

fn bench_storage(c: &mut criterion::Criterion) {
    let mut group = c.benchmark_group("storage");
    for &(w, h) in &[(10, 10), (50, 50), (100, 100)] {
        group.bench_function(format!("vec_{}x{}", w, h), |b| {
            b.iter(|| {
                let _time = storage_bench_vec(w, h);
            })
        });
        group.bench_function(format!("chunk_{}x{}", w, h), |b| {
            b.iter(|| {
                let _time = storage_bench_chunk(w, h);
            })
        });
    }
    group.finish();
}

fn bench_grid_growth(c: &mut criterion::Criterion) {
    let mut group = c.benchmark_group("grid_growth");
    for &n in &[100, 500, 1000, 5000, 10000] {
        group.bench_function(format!("N_{}", n), |b| {
            b.iter(|| {
                let _time = grid_growth_bench(n);
            })
        });
    }
    group.finish();
}

fn bench_rule_count(c: &mut criterion::Criterion) {
    let mut group = c.benchmark_group("rule_count");
    for &k in &[10, 50, 100, 200] {
        group.bench_function(format!("K_{}", k), |b| {
            b.iter(|| {
                let _time = rule_count_bench(k);
            })
        });
    }
    group.finish();
}

fn bench_replication(c: &mut criterion::Criterion) {
    let mut group = c.benchmark_group("replication");
    for &len in &[10, 50, 100, 500] {
        group.bench_function(format!("len_{}", len), |b| {
            b.iter(|| {
                let _time = replication_bench(len);
            })
        });
    }
    group.finish();
}

// ============================================================================
// Точка входа
// ============================================================================

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--bench") {
        let mut criterion = criterion::Criterion::default()
            .configure_from_args();
        bench_turing(&mut criterion);
        bench_tag(&mut criterion);
        bench_conflict_free(&mut criterion);
        bench_worst_case(&mut criterion);
        bench_storage(&mut criterion);
        bench_grid_growth(&mut criterion);
        bench_rule_count(&mut criterion);
        bench_replication(&mut criterion);
        criterion.final_summary();
    } else {
        run_complexity_tests();
        println!("\n--- Новые бенчмарк-тесты ---");
        run_bench_tests();
    }
}