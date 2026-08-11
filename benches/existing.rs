use criterion::Criterion;

use cellaria::engine::run_tick;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Rule};

use crate::helpers;

// Все бенчмарки в этом файле делают setup (решётка + правила) ОДИН раз, ДО
// b.iter(), и внутри b.iter() измеряют только один run_tick(). Раньше здесь
// напрямую вызывались helpers::*_bench(...) функции — они сами внутри себя
// крутятся в цикле `while elapsed < 1s`, а Criterion поверх этого ЕЩЁ и сам
// хочет вызвать замеряемую функцию ~100 раз для статистики — итого до
// 100+ секунд на один-единственный bench_function, что на практике вешало
// или обрывало `cargo bench -- --bench` (см. tm_100 ~100s, затем tag_20
// падал). Замер "сколько тиков влезет в окно" — это отдельная методология
// (у неё есть throughput::*_bench для custom-репортера), она не должна
// смешиваться с Criterion-семплированием одной и той же функции.

pub fn bench_tm(c: &mut Criterion) {
    let raw_rules = helpers::turing_rules(100);
    let rule_index = helpers::make_rule_index(raw_rules);
    let mut grid = helpers::make_grid(3, 1);
    grid.set_cell(1, 0, Cell { value: CellValue(CellType(0)), born_at: 0 });

    c.bench_function("tm_100", |b| {
        b.iter(|| {
            let _ = run_tick(&mut grid, &rule_index);
        })
    });
}

pub fn bench_tag(c: &mut Criterion) {
    let len = 20;
    let raw_rules = helpers::tag_rules(len);
    let rule_index = helpers::make_rule_index(raw_rules);
    let mut grid = helpers::make_grid(len + 2, 1);
    for i in 0..len {
        grid.set_cell(i, 0, Cell { value: CellValue(CellType(i as u8 % 4)), born_at: 0 });
    }

    c.bench_function("tag_20", |b| {
        b.iter(|| {
            let _ = run_tick(&mut grid, &rule_index);
        })
    });
}

pub fn bench_conflict_free(c: &mut Criterion) {
    let width = 32;
    let rule = Rule {
        id: vec![CellType(1)],
        pattern: vec![(0i8, 0i8, CellType(1))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(1))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None, cross_layer_reads: Vec::new(),
    };
    let rules = vec![rule; width];
    let rule_index = helpers::make_rule_index(rules);
    let mut grid = helpers::make_grid(width, 1);
    for x in 0..width {
        grid.set_cell(x, 0, Cell { value: CellValue(CellType(1)), born_at: 0 });
    }

    c.bench_function("conflict_free_32", |b| {
        b.iter(|| {
            let _ = run_tick(&mut grid, &rule_index);
        })
    });
}

pub fn bench_worst_case(c: &mut Criterion) {
    let mut group = c.benchmark_group("worst_case");
    for &m in &[1, 5, 10, 20] {
        let raw_rules = helpers::priority_conflict_rules(m);
        let rule_index = helpers::make_rule_index(raw_rules);
        let mut grid = helpers::make_grid(1, 1);
        grid.set_cell(0, 0, Cell { value: CellValue(CellType(1)), born_at: 0 });

        group.bench_function(format!("M_{}", m), |b| {
            b.iter(|| {
                let _ = run_tick(&mut grid, &rule_index);
            })
        });
    }
    group.finish();
}

pub fn bench_storage(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage");
    for &(w, h) in &[(10, 10), (100, 100), (500, 500)] {
        group.bench_function(format!("vec_{}x{}", w, h), |b| {
            b.iter(|| {
                let _time = helpers::storage_bench_vec(w, h);
            })
        });
        group.bench_function(format!("chunk_{}x{}", w, h), |b| {
            b.iter(|| {
                let _time = helpers::storage_bench_chunk(w, h);
            })
        });
    }
    group.finish();
}

pub fn bench_grid_growth(c: &mut Criterion) {
    let mut group = c.benchmark_group("grid_growth");
    for &n in &[10, 100, 500] {
        group.bench_function(format!("N_{}", n), |b| {
            b.iter(|| {
                let _time = helpers::grid_growth_bench(n);
            })
        });
    }
    group.finish();
}

pub fn bench_rule_count(c: &mut Criterion) {
    let mut group = c.benchmark_group("rule_count");
    for &k in &[1, 10, 50, 100] {
        group.bench_function(format!("K_{}", k), |b| {
            b.iter(|| {
                let _time = helpers::rule_count_bench(k);
            })
        });
    }
    group.finish();
}

pub fn bench_replication(c: &mut Criterion) {
    let mut group = c.benchmark_group("replication");
    for &len in &[1, 10, 50, 100] {
        group.bench_function(format!("len_{}", len), |b| {
            b.iter(|| {
                let _time = helpers::replication_bench(len);
            })
        });
    }
    group.finish();
}

pub fn register_all(c: &mut Criterion) {
    bench_tm(c);
    bench_tag(c);
    bench_conflict_free(c);
    bench_worst_case(c);
    bench_storage(c);
    bench_grid_growth(c);
    bench_rule_count(c);
    bench_replication(c);
}
