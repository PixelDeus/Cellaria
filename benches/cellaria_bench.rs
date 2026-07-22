use criterion::{criterion_group, criterion_main, Criterion};

use cellaria::config::load_config;
use cellaria::engine::{apply_matches, arbitrate, detect_matches, run_tick};
use cellaria::Grid;
use cellaria::VecStorage;
use cellaria::types::{Cell, CellType, CellValue, RuleMatch};

fn bench_detect_matches(c: &mut Criterion) {
    let (grid, rule_index) = load_config("configs/collision.yaml").expect("bench: load_config failed");

    c.bench_function("detect_matches", |b| {
        b.iter(|| {
            let _matches = detect_matches(&grid, &rule_index);
        })
    });
}

fn bench_arbitrate(c: &mut Criterion) {
    let mut matches = Vec::with_capacity(1000);
    for i in 0..1000 {
        matches.push(RuleMatch {
            x: (i % 50) as u32,
            y: (i / 50) as u32,
            pattern: vec![vec![(i % 10) as u8]],
            rule_id: vec![CellType((i % 10) as u8)],
        });
    }

    c.bench_function("arbitrate", |b| {
        b.iter(|| {
            let _accepted = arbitrate(matches.clone());
        })
    });
}

fn bench_apply_matches(c: &mut Criterion) {
    let storage = VecStorage {
        cells: vec![Cell::default(); 100],
        width: 10,
        height: 10,
    };
    let mut grid = Grid::new(storage);
    // Need a dummy rule_index for apply_matches
    let rule_index = std::collections::HashMap::new();

    let accepted = vec![
        RuleMatch {
            x: 0, y: 0,
            pattern: vec![vec![1]],
            rule_id: vec![CellType(1)],
        },
        RuleMatch {
            x: 5, y: 5,
            pattern: vec![vec![3]],
            rule_id: vec![CellType(3)],
        },
    ];

    c.bench_function("apply_matches", |b| {
        b.iter(|| {
            let mut g = Grid::new(VecStorage {
                cells: vec![Cell::default(); 100],
                width: 10,
                height: 10,
            });
            let acc = accepted.clone();
            apply_matches(&mut g, &acc, &rule_index);
        })
    });
}

fn bench_run_tick(c: &mut Criterion) {
    let (mut grid, rule_index) = load_config("configs/collision.yaml").expect("bench: load_config failed");

    c.bench_function("run_tick", |b| {
        b.iter(|| {
            run_tick(&mut grid, &rule_index);
        })
    });
}

criterion_group!(
    name = cellaria;
    config = Criterion::default();
    targets = bench_detect_matches, bench_arbitrate, bench_apply_matches, bench_run_tick
);
criterion_main!(cellaria);