//! Cellaria-CPU vs Cellaria-GPU (arbitrated-пайплайн) на СДВИГОВОЙ нагрузке —
//! в отличие от `flagship_gol.rs` (self-write-only, GPU идёт быстрым
//! однопроходным путём без арбитража вообще), здесь каждая клетка реально
//! ДВИГАЕТСЯ (`Rule::shifts`), а разные частицы регулярно сталкиваются на
//! одной целевой клетке — это и есть путь, для которого нужен полный
//! многораундовый claim/resolve арбитраж (`gpu::engine`'s `Arbitrated`-пайплайн,
//! см. её doc-комментарий и `tests/gpu_v2_correctness.rs`).
//!
//! Четыре типа клеток — частицы, движущиеся вправо/влево/вверх/вниз на 1
//! клетку за тик, `OverflowAction::Discard` (частица, ушедшая за край,
//! исчезает). При случайной плотности заполнения пути регулярно пересекаются
//! — конфликт возникает, когда 2+ частиц целятся в одну и ту же клетку в
//! одном тике, и решается ровно так же на CPU и на GPU (см. корректность
//! ниже) — не по совпадению, а потому что оба используют один и тот же
//! тотальный порядок тай-брейка (`arbitrator::arbitrate`'s
//! priority→age→id→x→y→rule_idx).
//!
//! В отличие от `flagship_gol.rs`, здесь нет "нативной" (голой Rust)
//! колонки — сравнение именно Cellaria-CPU против Cellaria-GPU, то есть про
//! то, сколько даёт перенос УЖЕ ЕСТЬ движка на GPU, а не про накладные
//! расходы паттерн-матчинга cellaria как таковые (это уже отдельно
//! отвечено в `flagship_gol.rs`).

use std::collections::HashMap;
use std::time::Instant;

use cellaria::engine::run_tick;
use cellaria::gpu::GpuEngine;
use cellaria::types::{Cell, CellType, CellValue, Direction, OverflowAction, Rule};
use cellaria::{Grid, VecStorage};

const RIGHT: u8 = 1;
const LEFT: u8 = 2;
const UP: u8 = 3;
const DOWN: u8 = 4;

fn mover_rule(id: u8, direction: Direction) -> Rule {
    Rule {
        id: vec![CellType(id)],
        pattern: vec![(0, 0, CellType(id))],
        shifts: vec![vec![cellaria::types::ShiftSpec::new(direction, 1)]],
        changes: vec![],
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None,
    }
}

fn build_rule_index() -> HashMap<CellType, Vec<Rule>> {
    let mut index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    index.insert(CellType(RIGHT), vec![mover_rule(RIGHT, Direction::Right)]);
    index.insert(CellType(LEFT), vec![mover_rule(LEFT, Direction::Left)]);
    index.insert(CellType(UP), vec![mover_rule(UP, Direction::Up)]);
    index.insert(CellType(DOWN), vec![mover_rule(DOWN, Direction::Down)]);
    index
}

fn seeded_fill(n: usize, density_percent: u64, seed: u64) -> Vec<u8> {
    let mut state: u64 = seed | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    (0..n * n)
        .map(|_| {
            if next() % 100 < density_percent {
                [RIGHT, LEFT, UP, DOWN][(next() % 4) as usize]
            } else {
                0
            }
        })
        .collect()
}

fn build_cpu_grid(n: usize, seed: &[u8]) -> Grid<VecStorage> {
    let storage = VecStorage::new(n, n);
    let mut grid = Grid::new(storage, Default::default());
    for y in 0..n {
        for x in 0..n {
            let v = seed[y * n + x];
            if v != 0 {
                grid.set_cell(x, y, Cell { value: CellValue(CellType(v)), born_at: 0 });
            }
        }
    }
    grid
}

fn build_gpu_engine(n: usize, seed: &[u8], rule_index: &HashMap<CellType, Vec<Rule>>) -> GpuEngine {
    let mut initial = Vec::new();
    for y in 0..n {
        for x in 0..n {
            let v = seed[y * n + x];
            if v != 0 {
                initial.push((x, y, Cell { value: CellValue(CellType(v)), born_at: 0 }));
            }
        }
    }
    GpuEngine::new(n, n, &initial, rule_index).expect("mover rules (single Discard shift each) are within the GPU subset")
}

fn grid_to_flat(grid: &Grid<VecStorage>, n: usize) -> Vec<u8> {
    let mut out = vec![0u8; n * n];
    for y in 0..n {
        for x in 0..n {
            out[y * n + x] = grid.get_cell(x, y).map_or(0, |c| c.value.0 .0);
        }
    }
    out
}

fn main() {
    println!("Движущиеся частицы (реальные сдвиги + конфликты) — Cellaria-CPU vs Cellaria-GPU\n");

    let rule_index = build_rule_index();

    // Корректность: несколько тиков подряд, несколько плотностей, сверка ПОСЛЕ КАЖДОГО тика.
    for &density in &[10u64, 30, 50] {
        let n = 24;
        let seed = seeded_fill(n, density, density * 7919 + 1);
        let mut cpu_grid = build_cpu_grid(n, &seed);
        let mut gpu_engine = build_gpu_engine(n, &seed, &rule_index);

        for tick in 0..8 {
            run_tick(&mut cpu_grid, &rule_index);
            gpu_engine.run_tick();
            let cpu_flat = grid_to_flat(&cpu_grid, n);
            let gpu_flat: Vec<u8> = gpu_engine.read_grid().iter().map(|c| c.value.0 .0).collect();
            assert_eq!(
                cpu_flat, gpu_flat,
                "CPU и GPU должны давать БИТ В БИТ одинаковый результат (плотность {density}%, тик {tick})"
            );
        }
    }
    println!("Корректность (N=24, плотности 10/30/50%, 8 тиков подряд): результаты идентичны побитово ✓\n");

    println!("{:>10} | {:>18} | {:>18} | {:>12}", "N (сторона)", "Cellaria CPU (кл/с)", "Cellaria GPU (кл/с)", "во сколько раз");
    println!("{}", "-".repeat(68));

    for &n in &[20usize, 50, 100, 200, 400] {
        let seed = seeded_fill(n, 20, 42);
        let cells = (n * n) as f64;

        let reps = if n <= 50 { 100 } else { 20 };

        // CPU: reps независимых замеров одного и того же первого тика
        // (та же логика, что и flagship_gol.rs).
        let mut cpu_total_ns = 0u128;
        for _ in 0..reps {
            let mut grid = build_cpu_grid(n, &seed);
            let t0 = Instant::now();
            run_tick(&mut grid, &rule_index);
            cpu_total_ns += t0.elapsed().as_nanos();
        }
        let cpu_per_sec = cells / (cpu_total_ns as f64 / reps as f64 / 1e9);

        // GPU: один движок, reps тиков подряд БЕЗ readback между ними (см.
        // GpuEngine::run_ticks) — решётка при этом эволюционирует, но
        // рабочая нагрузка (плотность частиц, характер конфликтов) остаётся
        // сравнимой на всём прогоне.
        let mut engine = build_gpu_engine(n, &seed, &rule_index);
        let t0 = Instant::now();
        engine.run_ticks(reps as u32);
        let _ = engine.read_grid(); // дождаться реального завершения всех тиков
        let gpu_total_ns = t0.elapsed().as_nanos();
        let gpu_per_sec = cells * reps as f64 / (gpu_total_ns as f64 / 1e9);

        let ratio = gpu_per_sec / cpu_per_sec;
        println!("{:>10} | {:>18.0} | {:>18.0} | {:>10.0}x", n, cpu_per_sec, gpu_per_sec, ratio);
    }
}
