//! Честное сравнение: "найти и исправить первую инверсию" (примитив из
//! configs/sorting.yaml — маркер идёт слева направо, на первой паре (1,0)
//! меняет местами и останавливается) через Cellaria против прямого
//! линейного скана на чистом Rust. Один полный пузырьковый сорт — это N
//! повторений этого примитива с обеих сторон, так что множитель здесь и
//! есть множитель полного сорта.

use std::collections::HashMap;
use std::time::Instant;

use cellaria::engine::Engine;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Direction, Rule, ShiftSpec};
use cellaria::{Grid, VecStorage};

// ============================================================================
// Сторона Cellaria
// ============================================================================

fn build_cellaria_array(n: usize) -> Grid<VecStorage> {
    // Ширина n+1 (не n+2): позиции 0..=n, маркер на 0, данные на 1..=n.
    // Последняя реальная пара — (data[n-2], data[n-1]) на позициях (n-1, n).
    // На следующем тике маркер (позиция n) читал бы позицию n+1 — она уже
    // ВНЕ решётки, а не дефолтная клетка внутри — detect честно отбрасывает
    // такой матч (cache_valid=false), и тик останавливается ровно после
    // n-1 сравнений, как и у прямого скана. С шириной n+2 здесь была бы
    // лишняя дефолтная клетка внутри решётки, дающая один лишний тик.
    let storage = VecStorage::new(n + 1, 1);
    let mut grid = Grid::new(storage, Default::default());
    grid.set_cell(0, 0, Cell { value: CellValue(CellType(10)), born_at: 0 });
    // Массив без единой инверсии (все нули) — худший случай: маркер
    // проходит ВЕСЬ массив, ни разу не найдя пару (1,0).
    for i in 0..n {
        grid.set_cell(i + 1, 0, Cell { value: CellValue(CellType(0)), born_at: 0 });
    }
    grid
}

fn cellaria_sort_pass_rules() -> Vec<Rule> {
    let mk = |id: Vec<u8>, changes: Vec<(i32, i32, ChangeValue)>| Rule {
        id: id.into_iter().map(CellType).collect(),
        pattern: vec![],
        shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
        changes,
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
    };
    vec![
        mk(vec![10, 1, 0], vec![(0, 0, ChangeValue::Literal(0)), (1, 0, ChangeValue::Literal(1))]),
        mk(vec![10, 0, 1], vec![(0, 0, ChangeValue::Literal(10))]),
        mk(vec![10, 0, 0], vec![(0, 0, ChangeValue::Literal(10))]),
        mk(vec![10, 1, 1], vec![(0, 0, ChangeValue::Literal(10))]),
    ]
}

fn run_cellaria_pass_once(n: usize) -> (u128, u64) {
    let grid = build_cellaria_array(n);
    let rules = cellaria_sort_pass_rules();
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        rule_index.entry(rule.id[0]).or_default().push(rule);
    }
    // Engine, а не свободная функция run_tick(): та пересобирает RuleDataCache
    // и GroupCache заново на КАЖДЫЙ вызов (см. их doc-комментарии) — за тысячи
    // тиков в цикле это чистые накладные расходы, которых Engine избегает,
    // построив оба кэша один раз в Engine::new.
    let mut engine = Engine::new(grid, rule_index);

    let mut steps = 0u64;
    let t0 = Instant::now();
    loop {
        let (accepted, _) = engine.run_tick();
        if accepted.is_empty() {
            break;
        }
        steps += 1;
        if steps > n as u64 + 10 {
            panic!("проход не остановился за разумное число шагов — бага в тесте");
        }
    }
    (t0.elapsed().as_nanos() as u128, steps)
}

/// Для маленьких N суб-микросекунды — повторяем, пока не накопится
/// измеримое суммарное время.
fn run_cellaria_pass(n: usize) -> (u128, u64) {
    let mut total_ns = 0u128;
    let mut total_steps = 0u64;
    let mut reps = 0u64;
    while total_ns < 10_000_000 && reps < 1_000_000 {
        let (ns, steps) = run_cellaria_pass_once(n);
        total_ns += ns;
        total_steps += steps;
        reps += 1;
    }
    (total_ns.max(1), total_steps)
}

// ============================================================================
// Сторона "прямой скан" — тот же алгоритм, без Cellaria.
// ============================================================================

fn run_native_pass_once(n: usize) -> (u128, u64) {
    // Массив без единой инверсии (все нули) — худший случай для скана:
    // маркер/указатель проходит ВЕСЬ массив, ни разу не найдя (1,0).
    let arr: Vec<u8> = vec![0u8; n];
    let mut steps = 0u64;
    let t0 = Instant::now();
    let mut i = 0usize;
    while i + 1 < arr.len() {
        steps += 1;
        if arr[i] == 1 && arr[i + 1] == 0 {
            break; // нашли инверсию — в реальном свопе здесь был бы swap
        }
        i += 1;
    }
    (t0.elapsed().as_nanos() as u128, steps)
}

fn run_native_pass(n: usize) -> (u128, u64) {
    let mut total_ns = 0u128;
    let mut total_steps = 0u64;
    let mut reps = 0u64;
    while total_ns < 10_000_000 && reps < 1_000_000 {
        let (ns, steps) = run_native_pass_once(n);
        total_ns += ns;
        total_steps += steps;
        reps += 1;
    }
    (total_ns.max(1), total_steps)
}

fn main() {
    println!("Пузырьковая сортировка: 'найти первую инверсию' — Cellaria vs прямой скан\n");
    println!("{:>10} | {:>18} | {:>18} | {:>12}", "N (элем.)", "Cellaria (шаг/с)", "Native (шаг/с)", "во сколько раз");
    println!("{}", "-".repeat(68));

    for &n in &[10usize, 100, 1_000, 10_000, 100_000] {
        let (_, c_steps_once) = run_cellaria_pass_once(n);
        let (_, n_steps_once) = run_native_pass_once(n);
        assert_eq!(c_steps_once, n_steps_once, "число сравнений должно совпадать — иначе алгоритмы не эквивалентны");

        let (c_ns, c_steps) = run_cellaria_pass(n);
        let (n_ns, n_steps) = run_native_pass(n);

        let c_per_sec = (c_steps as f64) / (c_ns as f64 / 1e9);
        let n_per_sec = (n_steps as f64) / (n_ns as f64 / 1e9);
        let ratio = n_per_sec / c_per_sec;

        println!(
            "{:>10} | {:>18.0} | {:>18.0} | {:>10.0}x",
            n, c_per_sec, n_per_sec, ratio
        );
    }
}
