use std::time::Instant;

use criterion::Criterion;

use cellaria::engine::run_tick;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Rule};

use crate::helpers;

// ============================================================================
// ФАЗА 4: Сложность правил — pattern size, rules per head
// ============================================================================

/// Правило с pattern_size клеток в ряд.
pub fn make_pattern_rule(pattern_size: usize) -> Rule {
    let id: Vec<CellType> = (0..pattern_size).map(|i| CellType(i as u8 + 1)).collect();
    let pattern: Vec<(i8, i8, CellType)> = (0..pattern_size as i8)
        .map(|i| (i, 0i8, CellType(i as u8 + 1)))
        .collect();
    let changes: Vec<(i32, i32, ChangeValue)> = (0..pattern_size as i32)
        .map(|i| (i, 0, ChangeValue::Literal(99)))
        .collect();
    Rule {
        id,
        pattern,
        shifts: vec![],
        changes,
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
    }
}

/// Замерить время тика от размера паттерна.
pub fn pattern_size_bench(size: usize) -> u128 {
    let w = size + 2;
    let mut grid = helpers::make_grid(w, 1);
    for i in 0..size {
        grid.set_cell(i, 0, Cell {
            value: CellValue(CellType(i as u8 + 1)),
            born_at: 0,
        });
    }
    let rule = make_pattern_rule(size);
    let rule_index = helpers::make_rule_index(vec![rule]);

    let start = Instant::now();
    let (accepted, _) = run_tick(&mut grid, &rule_index);
    let elapsed = start.elapsed().as_micros();
    assert!(!accepted.is_empty(), "pattern_size_bench: тик не дал совпадений");
    elapsed
}

/// Создать K правил с одинаковым head-типом, разными pattern.
pub fn rules_per_head(k: usize) -> Vec<Rule> {
    (0..k)
        .map(|i| Rule {
            id: vec![CellType(1), CellType(i as u8 + 2)],
            pattern: vec![(0i8, 0i8, CellType(1)), (1i8, 0i8, CellType(i as u8 + 2))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(100 + i as u8))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
        })
        .collect()
}

/// Замерить время поиска правила среди K с одним head-типом.
pub fn rules_per_head_bench(k: usize) -> u128 {
    let mut grid = helpers::make_grid(k + 2, 1);
    grid.set_cell(0, 0, Cell {
        value: CellValue(CellType(1)),
        born_at: 0,
    });
    grid.set_cell(1, 0, Cell {
        value: CellValue(CellType(2)), // подходит под первое правило
        born_at: 0,
    });

    let rules = rules_per_head(k);
    let rule_index = helpers::make_rule_index(rules);

    let start = Instant::now();
    let (accepted, _) = run_tick(&mut grid, &rule_index);
    let elapsed = start.elapsed().as_micros();
    assert_eq!(accepted.len(), 1, "Должно быть ровно одно совпадение");
    elapsed
}

// ============================================================================
// ФАЗА 5: Profiling — горячие точки
// ============================================================================

/// Профилирование find_rule: замерить среднее время поиска правила
/// при разных K (количество правил на один head-тип).
pub fn profile_find_rule(k: usize) -> (u128, usize) {
    let rules = rules_per_head(k);
    let rule_index = helpers::make_rule_index(rules);

    let rule_id = vec![CellType(1), CellType(2)];
    let start = Instant::now();
    let mut found = 0usize;
    for _ in 0..10000 {
        if helpers::find_rule_bench(&rule_id, &rule_index).is_some() {
            found += 1;
        }
    }
    let elapsed = start.elapsed().as_nanos() as u128;
    (elapsed / 10000, found) // среднее наносекунд на один поиск
}

// ============================================================================
// Criterion-бенчмарки
// ============================================================================

pub fn bench_pattern_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("pattern_size");
    for &size in &[1, 2, 4, 9] {
        group.bench_function(format!("size_{}", size), |b| {
            b.iter(|| {
                let _elapsed = pattern_size_bench(size);
            })
        });
    }
    group.finish();
}

pub fn bench_rules_per_head(c: &mut Criterion) {
    let mut group = c.benchmark_group("rules_per_head");
    for &k in &[1, 10, 50, 100] {
        group.bench_function(format!("K_{}", k), |b| {
            b.iter(|| {
                let _elapsed = rules_per_head_bench(k);
            })
        });
    }
    group.finish();
}

pub fn bench_find_rule(c: &mut Criterion) {
    let mut group = c.benchmark_group("find_rule");
    for &k in &[1, 10, 50, 100, 200] {
        group.bench_function(format!("K_{}", k), |b| {
            let rules = rules_per_head(k);
            let rule_index = helpers::make_rule_index(rules);
            let rule_id = vec![CellType(1), CellType(2)];
            b.iter(|| {
                let _found = helpers::find_rule_bench(&rule_id, &rule_index);
            })
        });
    }
    group.finish();
}

pub fn register_all(c: &mut Criterion) {
    bench_pattern_size(c);
    bench_rules_per_head(c);
    bench_find_rule(c);
}