//! Честное сравнение: машина Тьюринга через Cellaria (редукция TM→локальная
//! редукция, см. tm_translator.rs) против прямого интерпретатора той же ТМ
//! на чистом Rust. Обе стороны выполняют РОВНО ОДНУ И ТУ ЖЕ машину
//! (инвертирование бит на ленте, как в configs/turing.yaml) — одна и та же
//! δ-функция, одинаковая начальная лента. Метрика — "шагов ТМ в секунду".

use std::collections::HashMap;
use std::time::Instant;

use cellaria::engine::Engine;
use cellaria::tm_translator::translate_tm;
use cellaria::types::{Cell, CellType, CellValue, Rule};
use cellaria::{Grid, VecStorage};

// ============================================================================
// Сторона Cellaria
// ============================================================================

fn build_cellaria_tape(n: usize) -> Grid<VecStorage> {
    let storage = VecStorage::new(n + 2, 1);
    let mut grid = Grid::new(storage, Default::default());
    // Головка (тип 10) в состоянии Q0 на позиции 0.
    grid.set_cell(
        0,
        0,
        Cell {
            value: CellValue(CellType(10)),
            born_at: 0,
        },
    );
    // Лента: чередующиеся биты 1/2 (символы "1"/"0"), длина n.
    for i in 0..n {
        let bit = if i % 2 == 0 { 1u8 } else { 2u8 };
        grid.set_cell(
            i + 1,
            0,
            Cell {
                value: CellValue(CellType(bit)),
                born_at: 0,
            },
        );
    }
    grid
}

fn cellaria_tm_rules() -> Vec<Rule> {
    // states = [Q0], symbols = [бит "1" = тип1, бит "0" = тип2].
    // δ(Q0,1) = (Q0, 2, R) — инвертируем 1→0, вправо.
    // δ(Q0,2) = (Q0, 1, R) — инвертируем 0→1, вправо.
    // Финальных состояний нет: остановка — когда головка доходит до
    // пустой (тип 0) клетки и ни одно правило не совпадает (ровно как в
    // configs/turing.yaml).
    translate_tm(&[10], &[1, 2], &[(0, 0, 0, 1, 'R'), (0, 1, 0, 0, 'R')], 0, &[])
}

fn run_cellaria_tm(n: usize) -> (u128, u64) {
    let grid = build_cellaria_tape(n);
    let rules = cellaria_tm_rules();
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        rule_index.entry(rule.id[0]).or_default().push(rule);
    }
    // Engine, а не свободная функция run_tick(): та пересобирает
    // RuleDataCache заново на КАЖДЫЙ вызов (см. её doc-комментарий) — за
    // тысячи тиков в цикле это чистые накладные расходы, которых Engine
    // избегает, построив кэш один раз в Engine::new. Логика тика (детект →
    // арбитраж → применение) идентична — обе делегируют в один и тот же
    // run_tick_with_cache, так что это не смена алгоритма, а просто выбор
    // уже существующего быстрого пути.
    let mut engine = Engine::new(grid, rule_index);

    let mut steps = 0u64;
    let t0 = Instant::now();
    loop {
        let (accepted, _) = engine.run_tick();
        if accepted.is_empty() {
            break; // головка упёрлась в пустую клетку — остановка ТМ
        }
        steps += 1;
        if steps > n as u64 + 10 {
            panic!("TM не остановилась за разумное число шагов — бага в тесте");
        }
    }
    (t0.elapsed().as_nanos() as u128, steps)
}

// ============================================================================
// Сторона "прямой интерпретатор" — та же δ-функция, без Cellaria вообще.
// ============================================================================

fn run_native_tm_once(n: usize) -> (u128, u64) {
    // Лента как обычный Vec<u8>: 1 = бит "1", 2 = бит "0", 0 = пусто.
    let mut tape: Vec<u8> = vec![0u8; n + 2];
    for i in 0..n {
        tape[i] = if i % 2 == 0 { 1 } else { 2 };
    }
    let mut head = 0usize;
    let mut steps = 0u64;

    let t0 = Instant::now();
    loop {
        let symbol = tape[head];
        let new_symbol = match symbol {
            1 => 2,
            2 => 1,
            _ => break, // пустая клетка — остановка (та же δ, что и в Cellaria-версии)
        };
        tape[head] = new_symbol;
        head += 1;
        steps += 1;
        if steps > n as u64 + 10 {
            panic!("TM не остановилась за разумное число шагов — бага в тесте");
        }
    }
    (t0.elapsed().as_nanos() as u128, steps)
}

/// Для маленьких N один прогон занимает суб-микросекунды — таймер не
/// различает. Повторяем прогон, пока суммарное время не станет измеримым
/// (10мс), суммируя шаги — итоговая скорость (шаг/с) от этого не меняется,
/// просто числитель и знаменатель растут вместе.
fn run_native_tm(n: usize) -> (u128, u64) {
    let mut total_ns = 0u128;
    let mut total_steps = 0u64;
    let mut reps = 0u64;
    while total_ns < 10_000_000 && reps < 1_000_000 {
        let (ns, steps) = run_native_tm_once(n);
        total_ns += ns;
        total_steps += steps;
        reps += 1;
    }
    (total_ns.max(1), total_steps)
}

fn main() {
    println!("Машина Тьюринга (инвертирование бит) — Cellaria vs прямой интерпретатор\n");
    println!(
        "{:>10} | {:>18} | {:>18} | {:>12}",
        "N (длина)", "Cellaria (шаг/с)", "Native (шаг/с)", "во сколько раз"
    );
    println!("{}", "-".repeat(68));

    for &n in &[10usize, 100, 1_000, 10_000, 100_000] {
        let (c_ns, c_steps) = run_cellaria_tm(n);
        // Валидация эквивалентности — ОДИН прогон, до усреднения повторами.
        let (_, n_steps_once) = run_native_tm_once(n);
        assert_eq!(
            c_steps, n_steps_once,
            "число шагов должно совпадать — иначе машины не эквивалентны"
        );

        let (n_ns, n_steps) = run_native_tm(n);

        let c_per_sec = (c_steps as f64) / (c_ns as f64 / 1e9);
        let n_per_sec = (n_steps as f64) / (n_ns as f64 / 1e9);
        let ratio = n_per_sec / c_per_sec;

        println!(
            "{:>10} | {:>18.0} | {:>18.0} | {:>10.0}x",
            n, c_per_sec, n_per_sec, ratio
        );
    }
}
