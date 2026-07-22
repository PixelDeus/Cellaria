use criterion::{criterion_group, criterion_main, Criterion};

use cellaria::config::load_config;
use cellaria::engine::{apply_matches, arbitrate, detect_matches, run_tick};
use cellaria::Grid;
use cellaria::VecStorage;
use cellaria::types::{Cell, CellType, CellValue, OverflowAction, RuleId, AffectedRegion, RuleMatch, Direction};

fn bench_detect_matches(c: &mut Criterion) {
    // Загружаем реальный конфиг с несколькими правилами
    let (grid, rule_index) = load_config("configs/collision.yaml").expect("bench: load_config failed");

    c.bench_function("detect_matches", |b| {
        b.iter(|| {
            let _matches = detect_matches(&grid, &rule_index);
        })
    });
}

fn bench_arbitrate(c: &mut Criterion) {
    // Создаём 1000 RuleMatch'ей с разными приоритетами и конфликтующими координатами
    let mut matches = Vec::with_capacity(1000);
    for i in 0..1000 {
        matches.push(RuleMatch {
            rule_id: RuleId(i),
            priority: (i % 10) as u8,
            age: (i as u64) % 100,
            center: (i % 50, i / 50),
            affected_region: AffectedRegion::LocalGroup {
                group_cells: vec![(i % 50, i / 50)],
                result_cells: vec![CellValue(CellType(1))],
            },
        });
    }

    c.bench_function("arbitrate", |b| {
        b.iter(|| {
            let _accepted = arbitrate(matches.clone());
        })
    });
}

fn bench_apply_matches(c: &mut Criterion) {
    // Создаём решётку 10x10 и набор RuleMatch'ей
    let storage = VecStorage {
        cells: vec![Cell::default(); 100],
        width: 10,
        height: 10,
    };
    let mut grid = Grid::new(storage);

    let accepted = vec![
        RuleMatch {
            rule_id: RuleId(1),
            priority: 10,
            age: 0,
            center: (0, 0),
            affected_region: AffectedRegion::LocalGroup {
                group_cells: vec![(0, 0), (0, 1)],
                result_cells: vec![CellValue(CellType(1)), CellValue(CellType(2))],
            },
        },
        RuleMatch {
            rule_id: RuleId(2),
            priority: 5,
            age: 0,
            center: (5, 5),
            affected_region: AffectedRegion::Chain {
                group_cells: vec![(5, 5), (5, 6)],
                result_cells: vec![CellValue(CellType(3)), CellValue(CellType(4))],
                chain_cells: vec![(5, 5), (5, 6), (5, 7)],
                direction: Direction::SOUTH,
                fill_value: CellValue(CellType(0)),
                overflow_action: OverflowAction::Discard,
            },
        },
    ];

    c.bench_function("apply_matches", |b| {
        b.iter(|| {
            let mut g = Grid::new(VecStorage {
                cells: vec![Cell::default(); 100],
                width: 10,
                height: 10,
            });
            // Переносим accepted внутрь, т.к. apply_matches не модифицирует accepted
            let acc = accepted.clone();
            apply_matches(&mut g, &acc);
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