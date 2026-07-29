//! Честное сравнение: один шаг Conway's Game of Life через Cellaria против
//! прямой (стандартной) реализации на чистом Rust — подсчёт соседей +
//! запись в отдельный буфер, без всякого паттерн-матчинга.
//!
//! Существующие demo-конфиги (configs/gol_*.yaml) НЕ являются полной
//! реализацией GoL — там жёстко закодированы 1-2 конкретных случая
//! (см. gol_glider.yaml, который даёт 0 совпадений даже на своей же
//! начальной решётке). Здесь строится ПОЛНЫЙ, корректный набор правил:
//! перечисляются все возможные конфигурации 8 соседей и по стандартному
//! правилу GoL генерируются правила только для клеток, которые РЕАЛЬНО
//! меняют состояние (рождение/смерть) — "не меняется" не требует правила.
//!
//! Особенность движка: если хоть один сосед паттерна выходит за границу
//! решётки, совпадение просто не ищется для этой клетки (см.
//! `matcher.rs`'s `cache_valid`) — клетки на границе (расстояние < 1 от
//! края) НИКОГДА не матчатся ни одним правилом и остаются неизменными.
//! Нативная реализация ниже сознательно воспроизводит то же самое
//! (не обновляет граничное кольцо), чтобы сравнение было честным —
//! иначе это была бы не менее быстрая, а просто ДРУГАЯ функция.

use std::collections::HashMap;
use std::time::Instant;

use cellaria::engine::run_tick;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Rule};
use cellaria::{Grid, VecStorage};

const ALIVE: u8 = 1;
const DEAD: u8 = 0;

// ============================================================================
// Полный набор правил GoL для Cellaria
// ============================================================================

/// (dx, dy) всех 8 соседей в фиксированном порядке — важно для построения
/// битовой маски конфигурации.
const NEIGHBOR_OFFSETS: [(i8, i8); 8] = [
    (-1, -1), (0, -1), (1, -1),
    (-1, 0), (1, 0),
    (-1, 1), (0, 1), (1, 1),
];

/// Построить полный набор правил GoL: по одному правилу на каждую
/// конфигурацию 8 соседей (256 вариантов), для которой стандартное
/// правило GoL меняет состояние центра (рождение или смерть). Конфигурации
/// без изменения состояния не требуют правила вовсе.
fn build_gol_rules() -> Vec<Rule> {
    let mut rules = Vec::new();
    for center in [DEAD, ALIVE] {
        for mask in 0u8..=255 {
            let alive_neighbors = mask.count_ones();
            let next = match (center, alive_neighbors) {
                (ALIVE, 2) | (ALIVE, 3) => ALIVE,
                (DEAD, 3) => ALIVE,
                _ => DEAD,
            };
            if next == center {
                continue; // без изменений — правило не нужно
            }
            let mut pattern: Vec<(i8, i8, CellType)> = vec![(0, 0, CellType(center))];
            for (i, &(dx, dy)) in NEIGHBOR_OFFSETS.iter().enumerate() {
                let bit = (mask >> i) & 1;
                pattern.push((dx, dy, CellType(bit)));
            }
            rules.push(Rule {
                id: vec![CellType(center)],
                pattern,
                shifts: vec![],
                changes: vec![(0, 0, ChangeValue::Literal(next))],
                active_only: false,
                priority: 10,
                min_age: 0,
                overflow: Default::default(),
            });
        }
    }
    rules
}

fn build_rule_index(rules: Vec<Rule>) -> HashMap<CellType, Vec<Rule>> {
    let mut index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        index.entry(rule.id[0]).or_default().push(rule);
    }
    index
}

// ============================================================================
// Детерминированный псевдослучайный засев (без внешних crate) — один и тот
// же паттерн для Cellaria и для нативной версии.
// ============================================================================

fn seeded_fill(n: usize, density_percent: u64) -> Vec<u8> {
    let mut state: u64 = 0x2545F4914F6CDD1D;
    let mut next = || {
        // xorshift64
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    (0..n * n).map(|_| if next() % 100 < density_percent { ALIVE } else { DEAD }).collect()
}

// ============================================================================
// Сторона Cellaria
// ============================================================================

fn run_cellaria_gol_step(n: usize, seed: &[u8]) -> (u128, Vec<u8>) {
    let storage = VecStorage::new(n, n);
    let mut grid = Grid::new(storage, Default::default());
    for y in 0..n {
        for x in 0..n {
            let v = seed[y * n + x];
            if v != DEAD {
                grid.set_cell(x, y, Cell { value: CellValue(CellType(v)), born_at: 0 });
            }
        }
    }
    let rule_index = build_rule_index(build_gol_rules());

    let t0 = Instant::now();
    run_tick(&mut grid, &rule_index);
    let elapsed = t0.elapsed().as_nanos() as u128;

    let mut result = vec![DEAD; n * n];
    for y in 0..n {
        for x in 0..n {
            result[y * n + x] = grid.get_cell(x, y).map_or(DEAD, |c| c.value.0.0);
        }
    }
    (elapsed, result)
}

// ============================================================================
// Сторона "прямая реализация" — подсчёт соседей, отдельный выходной буфер.
// Граничное кольцо (как и в Cellaria) не обновляется — см. комментарий
// в начале файла.
// ============================================================================

fn run_native_gol_step(n: usize, seed: &[u8]) -> (u128, Vec<u8>) {
    let mut result = seed.to_vec();
    let t0 = Instant::now();
    for y in 1..n.saturating_sub(1) {
        for x in 1..n.saturating_sub(1) {
            let mut alive_neighbors = 0u32;
            for &(dx, dy) in &NEIGHBOR_OFFSETS {
                let nx = (x as i32 + dx as i32) as usize;
                let ny = (y as i32 + dy as i32) as usize;
                if seed[ny * n + nx] == ALIVE {
                    alive_neighbors += 1;
                }
            }
            let center = seed[y * n + x];
            let next = match (center, alive_neighbors) {
                (ALIVE, 2) | (ALIVE, 3) => ALIVE,
                (DEAD, 3) => ALIVE,
                _ => DEAD,
            };
            result[y * n + x] = next;
        }
    }
    (t0.elapsed().as_nanos() as u128, result)
}

fn main() {
    println!("Game of Life — один шаг поколения — Cellaria vs прямая реализация\n");

    // Раньше здесь стоял ассерт, который гарантированно падал: арбитраж путал
    // пересечение ЧТЕНИЯ паттерна (соседи, которых GoL-правило только читает)
    // с конфликтом ЗАПИСИ, из-за чего массово (57-83% в плотных случаях)
    // отбрасывал легитимные одновременные переходы. Это было исправлено на
    // уровне движка (conflict_analyzer.rs: RuleData::write_cells отделена от
    // affected_cells; арбитраж теперь сравнивает только реальные записи).
    // Ниже — проверка корректности на нескольких плотностях, затем честная
    // throughput-таблица, раз результаты теперь побитово идентичны.
    for &density in &[10u64, 30, 50] {
        let n0 = 20;
        let seed0 = seeded_fill(n0, density);
        let (_, c_res) = run_cellaria_gol_step(n0, &seed0);
        let (_, n_res) = run_native_gol_step(n0, &seed0);
        assert_eq!(c_res, n_res, "Cellaria и нативная реализация должны давать БИТ В БИТ одинаковый результат (плотность {}%)", density);
    }
    println!("Корректность (N=20, плотности 10/30/50%): результаты идентичны побитово ✓\n");

    println!("{:>10} | {:>18} | {:>18} | {:>12}", "N (сторона)", "Cellaria (кл/с)", "Native (кл/с)", "во сколько раз");
    println!("{}", "-".repeat(68));

    for &n in &[10usize, 30, 50, 100, 200] {
        let seed = seeded_fill(n, 30);
        let cells = (n * n) as f64;

        let mut c_total_ns = 0u128;
        let mut n_total_ns = 0u128;
        let reps = if n <= 30 { 200 } else { 20 };
        for _ in 0..reps {
            let (c_ns, _) = run_cellaria_gol_step(n, &seed);
            let (n_ns, _) = run_native_gol_step(n, &seed);
            c_total_ns += c_ns;
            n_total_ns += n_ns;
        }
        let c_per_sec = cells / (c_total_ns as f64 / reps as f64 / 1e9);
        let n_per_sec = cells / (n_total_ns as f64 / reps as f64 / 1e9);
        let ratio = n_per_sec / c_per_sec;

        println!(
            "{:>10} | {:>18.0} | {:>18.0} | {:>10.0}x",
            n, c_per_sec, n_per_sec, ratio
        );
    }
}
