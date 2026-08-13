use std::time::Instant;
use std::collections::HashMap;

use criterion::Criterion;

use cellaria::engine::run_tick;
use cellaria::Grid;
use cellaria::{ChunkStorage, VecStorage};
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Direction, Rule, ShiftSpec};

use crate::helpers;

// Управляет длительностью окон в self-timed *_bench функциях ниже.
// Раньше `--quick` был распознан в main(), но никак не влиял на этот
// (не-Criterion) путь — окна оставались жёстко 100ms/1s независимо от
// флага. QUICK делит их на 10, отражая обещание "--quick: меньше итераций".
pub static QUICK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn window_micros(normal: u128) -> u128 {
    if QUICK.load(std::sync::atomic::Ordering::Relaxed) {
        normal / 10
    } else {
        normal
    }
}

// ============================================================================
// Setup (без замера) — переиспользуется и self-timed бенчами (для
// custom-репортера, run_phase1), и Criterion-обёртками ниже (для
// `cargo bench -- --bench`). Раньше Criterion-обёртки вызывали сами
// *_bench функции внутри b.iter() — те сами крутятся в цикле
// `while elapsed < 100ms..1s`, а Criterion поверх ЕЩЁ и сам хочет ~100
// семплов, итого до 100 секунд на один bench_function. Теперь setup
// делается один раз, а Criterion меряет ровно один run_tick за семпл —
// это его штатный режим работы.
// ============================================================================

/// Двухтактный осциллятор 1,2⇄3,4 — та же причина, что и у
/// `setup_single_cell` (см. её doc-комментарий): без обратного правила
/// решётка целиком конвертируется 1,2→3,4 на первом же тике, для типов 3/4
/// правил нет, и все последующие тики в измерительном окне пустые.
/// `max_throughput_no_shift` тогда мерил не устойчивый throughput, а
/// "количество совпадений одного-единственного реального тика, делённое на
/// window_micros" — число, которое зависит почти исключительно от N² и
/// почти не зависит от реальной скорости повторного тикания. Найдено при
/// сверке с Engine-путём: тот же паттерн, что уже был явно диагностирован
/// и исправлен для 1E, просто не перенесён сюда при добавлении 1A.
fn setup_no_shift(n: usize) -> (Grid<VecStorage>, HashMap<CellType, Vec<Rule>>) {
    let storage = VecStorage::new(n, n);
    let mut grid = Grid::from_storage(storage);

    for y in 0..n {
        for x in 0..n {
            let v = if (x + y) % 2 == 0 { 1 } else { 2 };
            grid.set_cell(
                x,
                y,
                Cell {
                    value: CellValue(CellType(v)),
                    born_at: 0,
                },
            );
        }
    }

    let forward = Rule {
        id: vec![CellType(1), CellType(2)],
        pattern: vec![(0i8, 0i8, CellType(1)), (1i8, 0i8, CellType(2))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(3)), (1, 0, ChangeValue::Literal(4))],
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
    };
    let backward = Rule {
        id: vec![CellType(3), CellType(4)],
        pattern: vec![(0i8, 0i8, CellType(3)), (1i8, 0i8, CellType(4))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(1)), (1, 0, ChangeValue::Literal(2))],
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
    };

    (grid, helpers::make_rule_index(vec![forward, backward]))
}

/// `ChunkStorage` (безграничная), а не `VecStorage` — сценарий "лента,
/// N ячеек сдвигаются вправо" на ОГРАНИЧЕННОЙ решётке шириной `n+2`
/// неизбежно упирается в правый край: клетки, доходящие до границы,
/// теряются (`OverflowAction::Discard` по умолчанию), фронт истончается и
/// гаснет — измерено: при N=100 последнее реальное совпадение на тике
/// ~136, дальше окно бенчмарка (100ms, тысячи тиков) меряет уже ничего не
/// делающие пустые тики, то есть в основном стену, а не устойчивый
/// throughput. На безграничной решётке той же лишний край просто
/// отсутствует — активность подтверждена сохраняющейся 2000+ тиков подряд.
fn setup_with_shift(n: usize) -> (Grid<ChunkStorage>, HashMap<CellType, Vec<Rule>>) {
    let mut grid = Grid::from_storage(ChunkStorage::new());
    for i in 0..n {
        grid.set_cell(
            i,
            0,
            Cell {
                value: CellValue(CellType(1)),
                born_at: 0,
            },
        );
    }

    let rules = (0..n)
        .map(|i| Rule {
            id: vec![CellType(1), CellType(i as u8 % 4)],
            pattern: vec![(0i8, 0i8, CellType(1)), (1i8, 0i8, CellType(i as u8 % 4))],
            shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
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
        })
        .collect();

    (grid, helpers::make_rule_index(rules))
}

fn setup_conflict(m: usize) -> (Grid<VecStorage>, HashMap<CellType, Vec<Rule>>) {
    let rules = helpers::priority_conflict_rules(m);
    let mut grid = helpers::make_grid(1, 1);
    grid.set_cell(
        0,
        0,
        Cell {
            value: CellValue(CellType(1)),
            born_at: 0,
        },
    );
    (grid, helpers::make_rule_index(rules))
}

/// Раньше здесь было одно правило `[1] → 2` без обратного `[2] → 1` —
/// после первого тика ячейка становилась типом 2, для которого правил нет,
/// и все последующие тики в окне были пустыми. `single_cell_max_tps` мерил
/// не устойчивый throughput, а то, сколько тиков впустую крутится ПОСЛЕ
/// единственного реального срабатывания — отсюда абсурдные ~1 секунда на
/// "тик" (весь бюджет окна на 1 принятое совпадение). Двухтактный
/// осциллятор 1⇄2 держит правило активным всё окно измерения.
fn setup_single_cell() -> (Grid<VecStorage>, HashMap<CellType, Vec<Rule>>) {
    let mut grid = helpers::make_grid(1, 1);
    grid.set_cell(
        0,
        0,
        Cell {
            value: CellValue(CellType(1)),
            born_at: 0,
        },
    );

    let rule_1_to_2 = Rule {
        id: vec![CellType(1)],
        pattern: vec![(0i8, 0i8, CellType(1))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(2))],
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
    };
    let rule_2_to_1 = Rule {
        id: vec![CellType(2)],
        pattern: vec![(0i8, 0i8, CellType(2))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(1))],
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
    };
    (grid, helpers::make_rule_index(vec![rule_1_to_2, rule_2_to_1]))
}

// setup_with_shift() уже описывает точно ту же решётку/правила, что нужны
// для 1F (длинная цепочка сдвигов) — это тот же сценарий, переиспользуем.

// ============================================================================
// 1A: Максимальный throughput без сдвига
// ============================================================================

/// N×N решётка, правило с паттерном [(0,0,1), (1,0,2)] → литерал, без сдвига.
pub fn max_throughput_no_shift(n: usize) -> (u128, usize) {
    let (mut grid, rule_index) = setup_no_shift(n);

    let start = Instant::now();
    let mut total_ticks = 0usize;
    while start.elapsed().as_micros() < window_micros(100_000) {
        let (accepted, _) = run_tick(&mut grid, &rule_index);
        total_ticks += accepted.len();
    }
    let elapsed = start.elapsed().as_micros();
    (elapsed, total_ticks)
}

// ============================================================================
// 1B: Максимальный throughput со сдвигом
// ============================================================================

/// TM-лента длины N: одно правило на ячейку.
pub fn max_throughput_with_shift(n: usize) -> (u128, usize) {
    let (mut grid, rule_index) = setup_with_shift(n);

    let start = Instant::now();
    let mut total_ticks = 0usize;
    while start.elapsed().as_micros() < window_micros(100_000) {
        let (accepted, _) = run_tick(&mut grid, &rule_index);
        total_ticks += accepted.len();
    }
    let elapsed = start.elapsed().as_micros();
    (elapsed, total_ticks)
}

// ============================================================================
// 1C: Конфликт (M правил на одной ячейке)
// ============================================================================

pub fn max_throughput_conflict(m: usize) -> (u128, usize) {
    let (mut grid, rule_index) = setup_conflict(m);

    let start = Instant::now();
    let mut total_accepted = 0usize;
    while start.elapsed().as_micros() < window_micros(100_000) {
        let (accepted, _) = run_tick(&mut grid, &rule_index);
        total_accepted += accepted.len();
    }
    let elapsed = start.elapsed().as_micros();
    (elapsed, total_accepted)
}

// ============================================================================
// 1D: Пустой тик (0 активных ячеек) — минимальный оверхед
// ============================================================================

/// Пустая решётка, 0 правил. Измеряем, сколько пустых тиков проходит за 1 секунду.
/// Ожидание: advance_age O(1) ≈ 1-5 ns.
pub fn empty_tick_bench() -> (u128, u128) {
    let mut grid: Grid<VecStorage> = Grid::default();
    let rule_index = helpers::make_rule_index(vec![]);

    // Прогрев
    run_tick(&mut grid, &rule_index);

    let start = Instant::now();
    let mut total_ticks = 0u128;
    while start.elapsed().as_micros() < window_micros(1_000_000) {
        run_tick(&mut grid, &rule_index);
        total_ticks += 1;
    }
    let elapsed_ns = start.elapsed().as_nanos();
    (elapsed_ns, total_ticks)
}

// ============================================================================
// 1E: Одна ячейка, одно правило — базовый latency
// ============================================================================

/// 1 ячейка, 1 правило без сдвига. Измеряем ticks за 1 секунду.
pub fn single_cell_max_tps() -> (u128, usize) {
    let (mut grid, rule_index) = setup_single_cell();

    let start = Instant::now();
    let mut total_ticks = 0usize;
    while start.elapsed().as_micros() < window_micros(1_000_000) {
        let (accepted, _) = run_tick(&mut grid, &rule_index);
        total_ticks += accepted.len();
    }
    let elapsed = start.elapsed().as_micros();
    (elapsed, total_ticks)
}

// ============================================================================
// 1F: Длинная цепочка сдвигов (N ячеек, каждая сдвигает соседа)
// ============================================================================

/// N ячеек на ленте, каждая имеет своё правило со сдвигом вправо.
pub fn long_shift_chain_bench(n: usize) -> (u128, usize) {
    let (mut grid, rule_index) = setup_with_shift(n);

    let start = Instant::now();
    let mut total_ticks = 0usize;
    while start.elapsed().as_micros() < window_micros(100_000) {
        let (accepted, _) = run_tick(&mut grid, &rule_index);
        total_ticks += accepted.len();
    }
    let elapsed = start.elapsed().as_micros();
    (elapsed, total_ticks)
}

// ============================================================================
// Criterion-бенчмарки для throughput
//
// Каждый делает setup один раз (см. setup_* выше) и меряет один run_tick()
// за Criterion-семпл — вместо того, чтобы звать self-timed *_bench функции
// (которые сами крутятся до 100ms-1s) внутри b.iter().
// ============================================================================

pub fn bench_throughput_no_shift(c: &mut Criterion) {
    let mut group = c.benchmark_group("throughput_no_shift");
    for &n in &[10, 50, 100] {
        let (mut grid, rule_index) = setup_no_shift(n);
        group.bench_function(format!("N_{}", n), |b| {
            b.iter(|| {
                let _ = run_tick(&mut grid, &rule_index);
            })
        });
    }
    group.finish();
}

pub fn bench_throughput_with_shift(c: &mut Criterion) {
    let mut group = c.benchmark_group("throughput_with_shift");
    for &n in &[10, 100, 500] {
        let (mut grid, rule_index) = setup_with_shift(n);
        group.bench_function(format!("N_{}", n), |b| {
            b.iter(|| {
                let _ = run_tick(&mut grid, &rule_index);
            })
        });
    }
    group.finish();
}

pub fn bench_throughput_conflict(c: &mut Criterion) {
    let mut group = c.benchmark_group("throughput_conflict");
    for &m in &[1, 10, 50] {
        let (mut grid, rule_index) = setup_conflict(m);
        group.bench_function(format!("M_{}", m), |b| {
            b.iter(|| {
                let _ = run_tick(&mut grid, &rule_index);
            })
        });
    }
    group.finish();
}

pub fn bench_empty_tick(c: &mut Criterion) {
    let mut grid: Grid<VecStorage> = Grid::default();
    let rule_index = helpers::make_rule_index(vec![]);

    let mut group = c.benchmark_group("empty_tick");
    group.bench_function("empty", |b| {
        b.iter(|| {
            let _ = run_tick(&mut grid, &rule_index);
        })
    });
    group.finish();
}

pub fn bench_single_cell(c: &mut Criterion) {
    let (mut grid, rule_index) = setup_single_cell();

    let mut group = c.benchmark_group("single_cell");
    group.bench_function("max_tps", |b| {
        b.iter(|| {
            let _ = run_tick(&mut grid, &rule_index);
        })
    });
    group.finish();
}

pub fn bench_long_shift_chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("long_shift_chain");
    for &n in &[10, 100, 500] {
        let (mut grid, rule_index) = setup_with_shift(n);
        group.bench_function(format!("N_{}", n), |b| {
            b.iter(|| {
                let _ = run_tick(&mut grid, &rule_index);
            })
        });
    }
    group.finish();
}

pub fn register_all(c: &mut Criterion) {
    bench_throughput_no_shift(c);
    bench_throughput_with_shift(c);
    bench_throughput_conflict(c);
    bench_empty_tick(c);
    bench_single_cell(c);
    bench_long_shift_chain(c);
}
