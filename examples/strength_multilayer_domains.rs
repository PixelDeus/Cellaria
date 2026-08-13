//! Шестая сила: несколько СЛОЁВ — независимых 2D-решёток одного размера,
//! связанных только УСЛОВНЫМ ЧТЕНИЕМ (`Rule::cross_layer_reads`), не
//! записью. Каждый слой — обычный, немодифицированный `Engine`, тикающий
//! сам по себе; единственное новое — правило на ОДНОМ слое может
//! потребовать, чтобы клетка на ДРУГОМ слое (та же позиция + смещение
//! `dz`) была определённого типа, иначе правило не сработает в этот тик.
//!
//! Сценарий: конвейер (слой 0) везёт груз вправо, но каждая клетка
//! конвейера физически связана с той же позицией СЕТИ ПИТАНИЯ (слой 1) —
//! груз может ехать только пока питание включено. Питание мигает само по
//! себе (обычный, не межслойный, тумблер), конвейер про это ничего не
//! знает напрямую — он просто каждый тик спрашивает "а питание сейчас
//! есть?" у соседнего слоя.
//!
//! Важный, не очевидный момент, который этот пример показывает НА ДЕЛЕ, а
//! не только в доке: условие смотрит на состояние соседнего слоя КАКИМ ОНО
//! БЫЛО НА НАЧАЛО этого тика — даже если питание САМО переключается в
//! ЭТОМ ЖЕ тике. Груз не "видит будущее" питания.

use std::collections::HashMap;

use cellaria::types::{CellType, ChangeValue, Direction, Rule, ShiftSpec};
use cellaria::{Grid, LayeredEngine, VecStorage};

const ITEM: u8 = 1;
const POWER_ON: u8 = 2;
const POWER_OFF: u8 = 3;

fn main() {
    const WIDTH: usize = 8;

    // Слой 0 — конвейер: один груз стартует у левого края.
    let mut conveyor = Grid::from_storage(VecStorage::new(WIDTH, 1));
    conveyor.set_cell(
        0,
        0,
        cellaria::types::Cell {
            value: cellaria::types::CellValue(CellType(ITEM)),
            born_at: 0,
        },
    );

    // Слой 1 — сеть питания: вся решётка одинаково "включена" в начале, все
    // клетки мигают синхронно (одно и то же правило срабатывает на КАЖДОЙ
    // клетке независимо -- значит все они переключаются в унисон).
    let mut power = Grid::from_storage(VecStorage::new(WIDTH, 1));
    for x in 0..WIDTH {
        power.set_cell(
            x,
            0,
            cellaria::types::Cell {
                value: cellaria::types::CellValue(CellType(POWER_ON)),
                born_at: 0,
            },
        );
    }

    let conveyor_rule = Rule {
        id: vec![CellType(ITEM)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
        changes: vec![],
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
        // dz=1 -- слой ПИТАНИЯ (индекс 1 в стеке ниже). (0,0) -- та же
        // позиция, что и у самого груза. Без этого условия груз просто
        // ехал бы всегда, независимо от слоя 1.
        cross_layer_reads: vec![(0, 0, 1, CellType(POWER_ON))],
    };

    let power_on_toggle = Rule {
        id: vec![CellType(POWER_ON)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(POWER_OFF))],
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
    let power_off_toggle = Rule {
        id: vec![CellType(POWER_OFF)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(POWER_ON))],
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

    // ОДИН общий rule_index -- LayeredEngine's слои используют один и тот
    // же набор правил (правило не привязано к слою, `dz` само выбирает
    // цель). CellType не пересекаются между "доменами" (ITEM=1 против
    // POWER_ON/OFF=2/3) -- ровно та дисциплина, которую проверяет
    // `config::load_layered_config` при загрузке из YAML.
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(ITEM), vec![conveyor_rule]);
    rule_index.insert(CellType(POWER_ON), vec![power_on_toggle]);
    rule_index.insert(CellType(POWER_OFF), vec![power_off_toggle]);

    let mut layered = LayeredEngine::new(vec![conveyor, power], rule_index);

    println!("Слой 0 = конвейер (груз едет вправо), слой 1 = сеть питания (мигает сама по себе).");
    println!("Груз двигается ТОЛЬКО когда питание в его позиции включено на начало тика.\n");

    for tick in 0..10u32 {
        layered.run_tick();
        let item_x = (0..WIDTH).find(|&x| layered.layer(0).grid().get_cell(x, 0).map(|c| c.value.0 .0) == Some(ITEM));
        let power_state = layered.layer(1).grid().get_cell(0, 0).map(|c| c.value.0 .0);
        let power_label = if power_state == Some(POWER_ON) { "ON " } else { "OFF" };
        match item_x {
            Some(x) => println!("тик {tick:>2}: питание {power_label} -- груз на x={x}"),
            None => println!("тик {tick:>2}: питание {power_label} -- груз уехал за пределы решётки"),
        }
    }

    println!(
        "\nГруз продвигается только на тиках, где питание было ON на НАЧАЛО тика -- \
         межслойное условие видит состояние соседа как оно было ДО тика, даже когда \
         тот сам меняется в этом же тике."
    );
}
