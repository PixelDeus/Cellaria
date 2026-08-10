//! Обратимость: если КАЖДОЕ правило по отдельности — биекция (разные входы
//! никогда не дают одинаковый выход) и набор бесконфликтен (CF, уже
//! доказанное свойство из paper2.md), то весь тик — тоже биекция: у каждой
//! решётки после тика есть РОВНО ОДНА решётка "до". Значит существует
//! обратный набор правил, откатывающий вычисление назад БУКВАЛЬНО, а не
//! приблизительно — не "восстановить похожее состояние", а восстановить
//! ТО ЖЕ САМОЕ, клетка в клетку.
//!
//! Две проверки в одном прогоне (два разных источника необратимости в
//! обычных движках):
//!   1. Циклический счётчик (0→1→2→3→4→5→0…) — проверяет `changes`:
//!      перестановка 6 состояний, обратная перестановка идёт в другую
//!      сторону (0→5→4→3→2→1→0).
//!   2. Токен, двигающийся сдвигом — проверяет `shifts`: сдвиг вправо
//!      обращается сдвигом влево.
//!
//! Прогоняем 20 тиков вперёд, запоминаем финальную решётку, строим
//! ОБРАТНЫЙ набор правил (стрелки развёрнуты), прогоняем 20 тиков от
//! финальной решётки — и сверяем с исходной клетка в клетку по всей
//! решётке, а не только в местах, где что-то менялось.

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Direction, Rule, ShiftSpec};
use cellaria::{Grid, VecStorage};

const WIDTH: usize = 40;
const CYCLE_LEN: u8 = 6;
const CYCLE_BASE: u8 = 10; // состояния счётчика — типы 10..15
const TOKEN: u8 = 100;
const TICKS: u32 = 20;

fn cycle_rules(forward: bool) -> HashMap<CellType, Vec<Rule>> {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for s in 0..CYCLE_LEN {
        let next = if forward { (s + 1) % CYCLE_LEN } else { (s + CYCLE_LEN - 1) % CYCLE_LEN };
        idx.insert(CellType(CYCLE_BASE + s), vec![Rule {
            id: vec![CellType(CYCLE_BASE + s)], pattern: vec![], shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(CYCLE_BASE + next))],
            active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
        }]);
    }
    let dir = if forward { Direction::Right } else { Direction::Left };
    idx.insert(CellType(TOKEN), vec![Rule {
        id: vec![CellType(TOKEN)], pattern: vec![], shifts: vec![vec![ShiftSpec::new(dir, 1)]],
        changes: vec![], active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
    }]);
    idx
}

fn build_grid() -> Grid<VecStorage> {
    let storage = VecStorage::new(WIDTH, 1);
    let mut grid = Grid::new(storage, Default::default());
    grid.set_cell(0, 0, Cell { value: CellValue(CellType(CYCLE_BASE + 0)), born_at: 0 });
    grid.set_cell(4, 0, Cell { value: CellValue(CellType(CYCLE_BASE + 2)), born_at: 0 });
    grid.set_cell(8, 0, Cell { value: CellValue(CellType(CYCLE_BASE + 4)), born_at: 0 });
    grid.set_cell(15, 0, Cell { value: CellValue(CellType(TOKEN)), born_at: 0 });
    grid
}

fn snapshot(engine: &Engine<VecStorage>) -> Vec<u8> {
    (0..WIDTH).map(|x| engine.grid().get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(0)).collect()
}

fn main() {
    let initial_grid = build_grid();
    let initial_snapshot: Vec<u8> = (0..WIDTH)
        .map(|x| initial_grid.get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(0))
        .collect();

    let mut forward_engine = Engine::new(initial_grid, cycle_rules(true));
    for _ in 1..=TICKS {
        forward_engine.run_tick();
    }
    let final_snapshot = snapshot(&forward_engine);

    println!("Исходная решётка:  {:?}", initial_snapshot);
    println!("После {} тиков вперёд: {:?}", TICKS, final_snapshot);

    // Обратный набор правил строится ИЗ прямого — не подгоняется под ответ,
    // это буквально симметричная конструкция (стрелки развёрнуты).
    let mut reverse_grid = Grid::new(VecStorage::new(WIDTH, 1), Default::default());
    for (x, &v) in final_snapshot.iter().enumerate() {
        if v != 0 {
            reverse_grid.set_cell(x, 0, Cell { value: CellValue(CellType(v)), born_at: 0 });
        }
    }
    let mut reverse_engine = Engine::new(reverse_grid, cycle_rules(false));
    for _ in 1..=TICKS {
        reverse_engine.run_tick();
    }
    let recovered_snapshot = snapshot(&reverse_engine);

    println!("После {} тиков назад:  {:?}", TICKS, recovered_snapshot);
    println!(
        "\nВосстановленная решётка совпадает с исходной клетка в клетку: {}",
        if recovered_snapshot == initial_snapshot { "ДА" } else { "НЕТ" }
    );
}
