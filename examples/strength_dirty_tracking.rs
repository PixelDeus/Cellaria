//! Не "флагманское" сравнение (Cellaria vs голый Rust-цикл под одну задачу),
//! а сравнение с "альтернативным движком" той же степени общности:
//! обычный CA-движок, который каждый тик пересканирует ВЕСЬ активный набор
//! клеток в поисках совпадений (так устроено большинство простых/наивных
//! реализаций клеточных автоматов), против Cellaria с её реальным
//! инкрементальным (dirty-tracking) обнаружением совпадений.
//!
//! Сценарий: огромный "провод" (WIRE, инертный тип без единого правила —
//! просто активная, но не участвующая ни в каких совпадениях клетка) и один
//! "челнок" (голова, которая едет туда-сюда в фиксированном маленьком окне
//! в начале провода, независимо от длины провода). С ростом длины провода N:
//!   - активный набор (`active_coords`) растёт линейно с N — обе стороны
//!     видят одну и ту же "живую" решётку;
//!   - но РЕАЛЬНО меняется на каждом тике только пара клеток у челнока —
//!     O(1) независимо от N.
//! Наивный движок (пересканирует весь активный набор) должен показывать
//! время/тик, растущее с N. Cellaria (dirty-tracking, обрабатывает только
//! реально изменившееся) должна оставаться плоской по N.
//!
//! Обе стороны используют ОДИН И ТОТ ЖЕ движок сопоставления/арбитража/
//! применения Cellaria (`detect_matches`/`arbitrate`/`apply_matches`) —
//! единственная разница — какой список кандидатов подаётся в `detect_matches`
//! каждый тик: у "наивного" это всегда полный `grid.active_coords()`, у
//! Cellaria — её собственный dirty-based кандидатный набор (через
//! `Engine::run_tick`). Это специально изолирует ИМЕННО вклад
//! dirty-tracking, а не разницу в реализации матчера.

use std::collections::HashMap;
use std::time::Instant;

use cellaria::conflict_analyzer::build_rule_data_cache;
use cellaria::engine::{apply_matches, arbitrate, detect_matches, Engine};
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Direction, Rule, ShiftSpec};
use cellaria::{Grid, VecStorage};

const WIRE: u8 = 5;
const WALL: u8 = 9;
const HEAD_R: u8 = 1;
const HEAD_L: u8 = 2;
const SHUTTLE_WIDTH: usize = 20;

fn build_grid(n: usize) -> Grid<VecStorage> {
    let storage = VecStorage::new(n, 1);
    let mut grid = Grid::new(storage, Default::default());
    // Весь провод — WIRE: активная (не-дефолтная) клетка без единого
    // привязанного правила. Раздувает active_coords, не раздувая dirty.
    for i in 0..n {
        grid.set_cell(i, 0, Cell { value: CellValue(CellType(WIRE)), born_at: 0 });
    }
    // Стены и челнок — в фиксированном окне в начале, не зависят от N.
    grid.set_cell(0, 0, Cell { value: CellValue(CellType(WALL)), born_at: 0 });
    grid.set_cell(SHUTTLE_WIDTH + 1, 0, Cell { value: CellValue(CellType(WALL)), born_at: 0 });
    grid.set_cell(1, 0, Cell { value: CellValue(CellType(HEAD_R)), born_at: 0 });
    grid
}

fn shuttle_rules() -> Vec<Rule> {
    vec![
        // Едет вправо; следующая клетка — стена -> разворот на месте.
        Rule {
            id: vec![CellType(HEAD_R)],
            pattern: vec![(0, 0, CellType(HEAD_R)), (1, 0, CellType(WALL))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(HEAD_L))],
            active_only: false, priority: 20, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None, cross_layer_reads: Vec::new(),
        },
        // Едет вправо; обычное движение.
        Rule {
            id: vec![CellType(HEAD_R)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
            changes: vec![],
            active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None, cross_layer_reads: Vec::new(),
        },
        // Едет влево; следующая клетка — стена -> разворот на месте.
        Rule {
            id: vec![CellType(HEAD_L)],
            pattern: vec![(0, 0, CellType(HEAD_L)), (-1, 0, CellType(WALL))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(HEAD_R))],
            active_only: false, priority: 20, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None, cross_layer_reads: Vec::new(),
        },
        // Едет влево; обычное движение.
        Rule {
            id: vec![CellType(HEAD_L)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec::new(Direction::Left, 1)]],
            changes: vec![],
            active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None, cross_layer_reads: Vec::new(),
        },
    ]
}

fn make_rule_index(rules: Vec<Rule>) -> HashMap<CellType, Vec<Rule>> {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        idx.entry(rule.id[0]).or_default().push(rule);
    }
    idx
}

/// Cellaria по-настоящему инкрементальная — Engine::run_tick с
/// dirty-tracking.
fn run_smart(n: usize, ticks: u32) -> u128 {
    let grid = build_grid(n);
    let rule_index = make_rule_index(shuttle_rules());
    let mut engine = Engine::new(grid, rule_index);
    let t0 = Instant::now();
    for _ in 0..ticks {
        engine.run_tick();
    }
    t0.elapsed().as_nanos() as u128
}

/// "Наивный" движок: та же связка detect_matches/arbitrate/apply_matches,
/// но кандидатный список для detect_matches — ВЕСЬ активный набор решётки
/// каждый тик, без dirty-tracking. Представляет типичный CA-движок без
/// инкрементального обнаружения изменений.
fn run_naive(n: usize, ticks: u32) -> u128 {
    let mut grid = build_grid(n);
    let rule_index = make_rule_index(shuttle_rules());
    let rule_cache = build_rule_data_cache(&rule_index);
    let bounds = (grid.width(), grid.height());

    let t0 = Instant::now();
    for _ in 0..ticks {
        let active = grid.active_coords().clone();
        let matches = detect_matches(&grid, &rule_index, &active);
        if matches.is_empty() {
            continue;
        }
        let accepted = arbitrate(matches, &rule_index, &rule_cache, bounds, |x, y| {
            grid.get_age(x, y) as u32
        });
        if accepted.is_empty() {
            continue;
        }
        let (_regions, _outputs) = apply_matches(&mut grid, accepted, &rule_index, &rule_cache);
        grid.advance_age();
    }
    t0.elapsed().as_nanos() as u128
}

fn main() {
    println!("Разреженный челнок в огромном 'проводе' — Cellaria (dirty-tracking)\n\
              vs наивный движок (полный пересчёт активного набора каждый тик)\n");
    println!(
        "{:>12} | {:>16} | {:>16} | {:>12}",
        "N (провод)", "Cellaria (нс/тик)", "Наивный (нс/тик)", "во сколько раз"
    );
    println!("{}", "-".repeat(66));

    let ticks = 4000u32;
    for &n in &[1_000usize, 10_000, 100_000, 1_000_000] {
        let smart_ns = run_smart(n, ticks) as f64 / ticks as f64;
        let naive_ns = run_naive(n, ticks) as f64 / ticks as f64;
        let ratio = naive_ns / smart_ns;
        println!(
            "{:>12} | {:>16.1} | {:>16.1} | {:>10.1}x",
            n, smart_ns, naive_ns, ratio
        );
    }

    println!(
        "\nОбе стороны видят один и тот же (растущий с N) активный набор клеток —\n\
         разница ТОЛЬКО в том, пересканирует ли движок его целиком каждый тик\n\
         (наивный) или только реально изменившееся (Cellaria). Ожидание: у\n\
         Cellaria время/тик не растёт с N, у наивного — растёт линейно."
    );
}
