use super::*;
use crate::types::{Cell, CellType, ChangeValue, ShiftSpec};
use crate::VecStorage;
use std::collections::HashSet;

fn make_grid(w: usize, h: usize) -> Grid<VecStorage> {
    Grid::new(VecStorage::new(w, h), HashSet::new())
}

fn make_rule_index(rules: Vec<Rule>) -> HashMap<CellType, Vec<Rule>> {
    let mut index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        if let Some(first) = rule.id.first() {
            index.entry(*first).or_default().push(rule);
        }
    }
    index
}

/// Правило без `cross_layer_reads` — базовая проверка: слой ведёт себя
/// ТОЧНО как обычный `Engine`, `LayeredEngine` ничего не меняет, когда
/// правило не просит межслойного чтения (нулевые накладные расходы,
/// заявленные в doc-комментарии `Rule::cross_layer_reads`).
#[test]
fn test_layer_without_cross_layer_reads_behaves_like_plain_engine() {
    let mut grid0 = make_grid(3, 1);
    grid0.set_cell(0, 0, Cell::new(1));
    let grid1 = make_grid(3, 1);

    let rule = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec::new(crate::types::Direction::Right, 1)]],
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
        cross_layer_reads: Vec::new(),
    };
    let rule_index = make_rule_index(vec![rule]);

    let mut layered = LayeredEngine::new(vec![grid0, grid1], rule_index);
    layered.run_tick();

    assert_eq!(layered.layer(0).grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(0), "источник должен очиститься -- обычный сдвиг сработал");
    assert_eq!(layered.layer(0).grid().get_cell(1, 0).map(|c| c.value.0 .0), Some(1), "маркер должен переехать -- LayeredEngine не должен мешать обычному правилу без cross_layer_reads");
    assert_eq!(layered.layer(1).grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(0), "слой 1 пуст и должен остаться пустым -- на него ничего не действует");
}

/// Основная проверка: правило на слое 0 условно на клетке слоя 1 через
/// `cross_layer_reads`. Один и тот же движок, две решётки — сценарий А
/// (условие ВЫПОЛНЕНО) и сценарий Б (условие НЕ выполнено) должны дать
/// РАЗНЫЙ результат — если бы фильтр был no-op, оба сценария вели бы себя
/// одинаково (сработало бы всегда).
#[test]
fn test_cross_layer_read_gates_rule_when_condition_holds() {
    let rule = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(99))],
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
        // Условие: клетка (0,0) НА СЛЕДУЮЩЕМ слое (dz=1) должна быть типа 5.
        cross_layer_reads: vec![(0, 0, 1, CellType(5))],
    };
    let rule_index = make_rule_index(vec![rule]);

    // Сценарий А: слой 1 клетка (0,0) = 5 -- условие выполнено, правило обязано сработать.
    let mut grid0_a = make_grid(1, 1);
    grid0_a.set_cell(0, 0, Cell::new(1));
    let mut grid1_a = make_grid(1, 1);
    grid1_a.set_cell(0, 0, Cell::new(5));
    let mut layered_a = LayeredEngine::new(vec![grid0_a, grid1_a], rule_index.clone());
    layered_a.run_tick();
    assert_eq!(layered_a.layer(0).grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(99), "условие на слое 1 выполнено -- правило обязано сработать");

    // Сценарий Б: слой 1 клетка (0,0) = 7 (не 5) -- условие НЕ выполнено, правило НЕ должно сработать.
    let mut grid0_b = make_grid(1, 1);
    grid0_b.set_cell(0, 0, Cell::new(1));
    let mut grid1_b = make_grid(1, 1);
    grid1_b.set_cell(0, 0, Cell::new(7));
    let mut layered_b = LayeredEngine::new(vec![grid0_b, grid1_b], rule_index);
    layered_b.run_tick();
    assert_eq!(layered_b.layer(0).grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(1), "условие на слое 1 НЕ выполнено -- правило не должно было сработать, клетка остаётся исходной (1)");
}

/// Дисциплина снимка тика (2.2.1) для ЧУЖОГО слоя: слой 1 меняет СВОЮ
/// клетку (0,0) СВОИМ ЖЕ правилом в ЭТОМ тике -- слой 0's cross-layer
/// проверка ЭТОГО ЖЕ тика обязана увидеть ПРЕДТИКОВОЕ значение слоя 1, не
/// то, что слой 1 запишет только что.
#[test]
fn test_cross_layer_read_sees_pre_tick_state_of_other_layer_even_when_it_changes_this_tick() {
    let rule_layer0 = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(99))],
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
        // Требует, чтобы слой 1 (dz=1) в (0,0) БЫЛ типа 5 -- это ПРЕДТИКОВОЕ значение.
        cross_layer_reads: vec![(0, 0, 1, CellType(5))],
    };
    let rule_layer1 = Rule {
        id: vec![CellType(5)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(6))], // слой 1 сам меняет себя в этом же тике
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
    let rule_index = make_rule_index(vec![rule_layer0, rule_layer1]);

    let mut grid0 = make_grid(1, 1);
    grid0.set_cell(0, 0, Cell::new(1));
    let mut grid1 = make_grid(1, 1);
    grid1.set_cell(0, 0, Cell::new(5)); // предтиковое значение -- условие layer0 должно его увидеть

    let mut layered = LayeredEngine::new(vec![grid0, grid1], rule_index);
    layered.run_tick();

    assert_eq!(
        layered.layer(0).grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(99),
        "layer0 обязан увидеть ПРЕДТИКОВОЕ значение layer1 (5), а не то, что layer1 сам записал в ЭТОМ ЖЕ тике (6)"
    );
    assert_eq!(layered.layer(1).grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(6), "layer1 своё правило тоже применилось -- 5 -> 6");
}

/// `dz`, уводящий за пределы стека слоёв (отрицательный индекс или индекс
/// >= числа слоёв) -- условие должно просто НЕ выполниться (правило не
/// срабатывает), а не паниковать.
#[test]
fn test_cross_layer_read_with_out_of_range_dz_fails_condition_without_panic() {
    let rule = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(99))],
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
        // dz=5 -- за пределами стека (всего 2 слоя, индексы 0 и 1).
        cross_layer_reads: vec![(0, 0, 5, CellType(5))],
    };
    let rule_index = make_rule_index(vec![rule]);

    let mut grid0 = make_grid(1, 1);
    grid0.set_cell(0, 0, Cell::new(1));
    let grid1 = make_grid(1, 1);

    let mut layered = LayeredEngine::new(vec![grid0, grid1], rule_index);
    layered.run_tick(); // не должно паниковать

    assert_eq!(layered.layer(0).grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(1), "dz за пределами стека слоёв -- условие не выполнено, правило не сработало");
}

/// `LayeredEngine::new` обязан отвергать разноразмерные слои -- иначе
/// координата, валидная на одном слое, могла бы молча указывать на другую
/// клетку (или вообще ни на какую) на другом слое внутри
/// `cross_layer_condition_holds`, без единой ошибки.
#[test]
#[should_panic(expected = "must share the same grid dimensions")]
fn test_new_panics_on_mismatched_layer_dimensions() {
    let grid0 = make_grid(2, 2);
    let grid1 = make_grid(3, 3);
    let _ = LayeredEngine::new(vec![grid0, grid1], HashMap::new());
}

/// `LayeredEngine::snapshot`/`from_snapshot` (сериализованные через
/// `serde_yaml`, та же дисциплина, что `Engine::snapshot` -- см. её
/// doc-комментарий про non-string ключи `HashMap`, из-за которых
/// `serde_json` не подходит): движок, остановленный посреди прогона,
/// сериализованный, десериализованный и продолженный, обязан дать ТОТ ЖЕ
/// результат, что и движок, прогнанный без остановки -- через ВСЕ слои
/// сразу, включая cross-layer чтение между ними.
#[test]
fn test_snapshot_restore_continues_identically_across_all_layers() {
    let rule_layer0 = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec::new(crate::types::Direction::Right, 1)]],
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
        // Требует, чтобы слой 1 (dz=1) в (0,0) БЫЛ типа 5 -- гейтит движение по нескольким тикам подряд.
        cross_layer_reads: vec![(0, 0, 1, CellType(5))],
    };
    let rule_index = make_rule_index(vec![rule_layer0]);

    let build = || {
        let mut grid0 = make_grid(4, 1);
        grid0.set_cell(0, 0, Cell::new(1));
        let mut grid1 = make_grid(4, 1);
        grid1.set_cell(0, 0, Cell::new(5));
        LayeredEngine::new(vec![grid0, grid1], rule_index.clone())
    };

    let mut straight = build();
    for _ in 0..4 {
        straight.run_tick();
    }

    let mut interrupted = build();
    interrupted.run_tick();
    interrupted.run_tick();
    let yaml = serde_yaml::to_string(&interrupted.snapshot()).expect("snapshot must serialize to YAML");
    let restored_snapshot: LayeredSnapshot<VecStorage> = serde_yaml::from_str(&yaml).expect("snapshot must deserialize from YAML");
    let mut restored = LayeredEngine::from_snapshot(restored_snapshot);
    restored.run_tick();
    restored.run_tick();

    for layer in 0..2 {
        for x in 0..4 {
            assert_eq!(
                straight.layer(layer).grid().get_cell(x, 0),
                restored.layer(layer).grid().get_cell(x, 0),
                "layer {layer}, x={x}: snapshot/restore must continue identically to an uninterrupted run"
            );
        }
    }
}
