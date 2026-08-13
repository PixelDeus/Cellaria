//! Девятая сила: возраст клетки — встроенное понятие движка (`born_at` +
//! `get_age`), а не то, что нужно вручную городить своим счётчиком в
//! правилах. Правило может сработать только после того, как клетка
//! "созрела" — просидела N тиков без изменений — задав всего одно число
//! `min_age`. В большинстве простых CA-движков для этого пришлось бы
//! заводить отдельный тип-счётчик и цепочку переходов N->N-1->...->0.

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Rule};
use cellaria::{Grid, VecStorage};

fn main() {
    let storage = VecStorage::new(1, 1);
    let mut grid = Grid::new(storage, Default::default());
    grid.set_cell(
        0,
        0,
        Cell {
            value: CellValue(CellType(1)),
            born_at: 0,
        },
    );

    // Правило сработает, только когда клетка просидит без изменений 3 тика.
    let rule = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(2))],
        active_only: false,
        priority: 10,
        min_age: 3,
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
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(1), vec![rule]);
    let mut engine = Engine::new(grid, rule_index);

    for tick in 1..=5 {
        engine.run_tick();
        let cell = engine.grid().get_cell(0, 0);
        println!(
            "тик {}: значение={:?}, возраст клетки={}",
            tick,
            cell.map(|c| c.value.0 .0),
            engine.grid().get_age(0, 0)
        );
    }

    println!("\nНи одного правила-счётчика — 'подожди 3 тика' задано одним числом min_age.");
}
