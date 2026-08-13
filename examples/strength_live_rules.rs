//! Третья ось: правила можно менять ПРЯМО ВО ВРЕМЯ РАБОТЫ симуляции —
//! без остановки, без сброса решётки, без пересоздания движка. `rule_index`
//! у `Engine` — публичное поле, а `rebuild_rule_cache()` существует именно
//! для того, чтобы после его правки на лету всё продолжило работать
//! корректно. У большинства движков набор правил либо жёстко зашит на
//! компиляции, либо требует остановки/перезапуска для смены.

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

    // Стартуем только с правилом "1 -> 2".
    let rule_1_to_2 = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
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
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(1), vec![rule_1_to_2]);
    let mut engine = Engine::new(grid, rule_index);

    for tick in 1..=2 {
        engine.run_tick();
        println!(
            "тик {}: клетка = {:?} (правила: только 1->2)",
            tick,
            engine.grid().get_cell(0, 0).map(|c| c.value.0 .0)
        );
    }

    // Симуляция идёт полным ходом — теперь добавляем НОВОЕ правило "2 -> 3"
    // прямо в работающий Engine, без остановки и без сброса решётки.
    let rule_2_to_3 = Rule {
        id: vec![CellType(2)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(3))],
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
    engine.set_rules_for_head(CellType(2), vec![rule_2_to_3]); // сама будит все активные клетки и вызывает rebuild_rule_cache — см. её doc-комментарий
    println!("--- добавили правило 2->3 на лету, решётка не трогалась ---");

    for tick in 3..=4 {
        engine.run_tick();
        println!(
            "тик {}: клетка = {:?} (правила: 1->2 и 2->3)",
            tick,
            engine.grid().get_cell(0, 0).map(|c| c.value.0 .0)
        );
    }

    println!("\nПоведение изменилось на следующем же тике после добавления правила —\nбез перезапуска движка и без потери состояния решётки.");
}
