use super::*;
use crate::types::{Cell, CellType, ChangeValue, ShiftSpec};
use crate::VecStorage;

fn make_grid(w: usize, h: usize) -> Grid<VecStorage> {
    Grid::from_storage(VecStorage::new(w, h))
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

    assert_eq!(
        layered.layer(0).grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(0),
        "источник должен очиститься -- обычный сдвиг сработал"
    );
    assert_eq!(
        layered.layer(0).grid().get_cell(1, 0).map(|c| c.value.0 .0),
        Some(1),
        "маркер должен переехать -- LayeredEngine не должен мешать обычному правилу без cross_layer_reads"
    );
    assert_eq!(
        layered.layer(1).grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(0),
        "слой 1 пуст и должен остаться пустым -- на него ничего не действует"
    );
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
    assert_eq!(
        layered_a.layer(0).grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(99),
        "условие на слое 1 выполнено -- правило обязано сработать"
    );

    // Сценарий Б: слой 1 клетка (0,0) = 7 (не 5) -- условие НЕ выполнено, правило НЕ должно сработать.
    let mut grid0_b = make_grid(1, 1);
    grid0_b.set_cell(0, 0, Cell::new(1));
    let mut grid1_b = make_grid(1, 1);
    grid1_b.set_cell(0, 0, Cell::new(7));
    let mut layered_b = LayeredEngine::new(vec![grid0_b, grid1_b], rule_index);
    layered_b.run_tick();
    assert_eq!(
        layered_b.layer(0).grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(1),
        "условие на слое 1 НЕ выполнено -- правило не должно было сработать, клетка остаётся исходной (1)"
    );
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
    assert_eq!(
        layered.layer(1).grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(6),
        "layer1 своё правило тоже применилось -- 5 -> 6"
    );
}

/// `dz`, уводящий за пределы стека слоёв (отрицательный индекс или индекс,
/// не меньший числа слоёв) -- условие должно просто НЕ выполниться
/// (правило не срабатывает), а не паниковать.
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

    assert_eq!(
        layered.layer(0).grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(1),
        "dz за пределами стека слоёв -- условие не выполнено, правило не сработало"
    );
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
    let restored_snapshot: LayeredSnapshot<VecStorage> =
        serde_yaml::from_str(&yaml).expect("snapshot must deserialize from YAML");
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

/// Регрессия для бага, найденного через вопрос "все ли расширения
/// правильно взаимодействуют друг с другом через `LayeredEngine`?":
/// `run_tick` раньше собирал матчи/арбитраж/применение вручную через
/// "raw"-методы `Engine::detect_matches`/`arbitrate`/`apply_matches`,
/// каждый из которых, по СОБСТВЕННЫМ doc-комментариям, не хранит
/// состояние между вызовами -- `starvation_after` был безмолвным no-op
/// на ЛЮБОМ `LayeredEngine` (см. `CHANGELOG.md` и doc-комментарий модуля
/// для полного списка сломанных расширений: `cam`, `feedback`, `memory`,
/// `max_activations` тоже страдали тем же путём). До фикса HIGH выигрывал
/// все 30/30 тиков подряд; после -- LOW периодически выигрывает.
#[test]
fn test_layered_starvation_after_periodically_protects_low_priority_rule() {
    let rule_high = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(1, 0, ChangeValue::Literal(100))],
        active_only: false,
        priority: 20,
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
    let rule_low = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(1, 0, ChangeValue::Literal(200))],
        active_only: false,
        priority: 5,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: Some(3),
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(1), vec![rule_high, rule_low]);

    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let mut layered = LayeredEngine::new(vec![grid], rule_index);

    let mut low_wins = 0;
    for _ in 0..30 {
        layered.run_tick();
        let val = layered.layer(0).grid().get_cell(1, 0).map(|c| c.value.0 .0);
        if val == Some(200) {
            low_wins += 1;
        }
    }
    assert_eq!(low_wins, 7, "starvation_after=Some(3) must force LOW to periodically win through LayeredEngine (deterministic 1-in-4 pattern) -- low_wins=0 would mean starvation bookkeeping is silently lost between ticks, as it was before the fix");
}

/// Тот же класс бага, для `max_activations`: раньше учёт активаций вообще
/// не существовал вне `run_tick_with_cache`, значит и через `LayeredEngine`
/// бюджет не расходовался никогда -- правило срабатывало бы бесконечно.
#[test]
fn test_layered_max_activations_gate_closes_permanently_after_budget() {
    const BUDGET: u32 = 3;
    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let rule = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(1, 0, ChangeValue::Literal(200))],
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
        max_activations: Some(BUDGET),
        cross_layer_reads: Vec::new(),
    };
    let mut layered = LayeredEngine::new(vec![grid], make_rule_index(vec![rule]));

    for tick in 1..=3 {
        layered.run_tick();
        assert_eq!(
            layered.layer(0).grid().get_cell(1, 0).map(|c| c.value.0 .0),
            Some(200),
            "тик {tick}: правило ещё в пределах бюджета, обязано сработать"
        );
    }

    // Клетка сбрасывается НАПРЯМУЮ, в обход правил -- честная проверка "гейт
    // закрыт", а не просто "значение совпало со старым" (см. аналогичный
    // приём в engine::tests::max_activations).
    layered.layer_mut(0).grid_mut().set_cell(1, 0, Cell::new(0));

    for tick in 4..=10 {
        layered.run_tick();
        assert_eq!(
            layered.layer(0).grid().get_cell(1, 0).map(|c| c.value.0 .0),
            Some(0),
            "тик {tick}: бюджет исчерпан НАВСЕГДА через LayeredEngine, как и через Engine"
        );
    }
}

/// Тот же класс бага, для `feedback`: свежий `FeedbackCounters::default()`
/// на каждый (бы) вызов `apply_matches` означал, что счётчик тайм-аута
/// никогда не рос -- переключения направления не происходило вообще.
#[test]
fn test_layered_feedback_latches_new_direction_after_timeout() {
    const TIMEOUT: u64 = 3;
    let mut grid = make_grid(10, 10);
    grid.set_cell(2, 2, Cell::new(9));

    let rule = Rule {
        id: vec![CellType(9)],
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
        feedback: Some(crate::types::FeedbackSpec {
            timeout: TIMEOUT,
            new_direction: crate::types::Direction::Up,
        }),
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut layered = LayeredEngine::new(vec![grid], make_rule_index(vec![rule]));

    fn find_marker(layered: &LayeredEngine<VecStorage>) -> (usize, usize) {
        for y in 0..10 {
            for x in 0..10 {
                if layered.layer(0).grid().get_cell(x, y).map(|c| c.value.0 .0) == Some(9) {
                    return (x, y);
                }
            }
        }
        panic!("маркер не найден на решётке");
    }

    // Та же дисциплина снимка тика, что и у обычного Engine (см.
    // engine::tests::extensions_interactions): при TIMEOUT=3 переключение
    // на Up должно произойти ровно на тике 4, не раньше.
    for _ in 0..3 {
        layered.run_tick();
        assert_eq!(
            find_marker(&layered).1,
            2,
            "первые 3 тика -- ещё Right, счётчик ещё не достиг таймаута"
        );
    }
    layered.run_tick();
    let (_, y) = find_marker(&layered);
    assert_eq!(y, 1, "тик 4: счётчик достиг TIMEOUT=3 -- направление обязано переключиться на Up (y уменьшается) через LayeredEngine, как и через Engine");
}

/// Тот же класс бага, для `memory`: `MemoryBuffers::default()` на каждый
/// (бы) вызов `apply_matches` означал, что буфер никогда не накапливал
/// историю -- гейт `RecordTrigger::NeighborType` не открывался никогда.
#[test]
fn test_layered_memory_neighbor_type_gate_opens_after_matching_sequence() {
    const WATCHER: u8 = 30;
    const NEIGH_A: u8 = 31;
    const NEIGH_B: u8 = 32;
    const FIRED: u8 = 33;

    let mut grid = make_grid(5, 1);
    grid.set_cell(2, 0, Cell::new(WATCHER));
    grid.set_cell(3, 0, Cell::new(NEIGH_A)); // (2,0) + Right = (3,0)

    let rule = Rule {
        id: vec![CellType(WATCHER)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(FIRED))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: Some(crate::types::MemorySpec {
            window: 3,
            record_trigger: crate::types::RecordTrigger::NeighborType(crate::types::Direction::Right),
            match_pattern: vec![
                crate::types::RecordedValue::Type(CellType(NEIGH_A)),
                crate::types::RecordedValue::Type(CellType(NEIGH_B)),
                crate::types::RecordedValue::Type(CellType(NEIGH_A)),
            ],
        }),
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut layered = LayeredEngine::new(vec![grid], make_rule_index(vec![rule]));

    // Тик 1: буфер получает Type(A), len=1 != window=3 -> гейт закрыт.
    layered.run_tick();
    assert_eq!(
        layered.layer(0).grid().get_cell(2, 0).map(|c| c.value.0 .0),
        Some(WATCHER),
        "тик 1: гейт ещё закрыт -- буфер не полон"
    );

    // Тик 2: сосед -> B. Буфер [A, B], len=2 != 3 -> гейт всё ещё закрыт.
    layered.layer_mut(0).grid_mut().set_cell(3, 0, Cell::new(NEIGH_B));
    layered.run_tick();
    assert_eq!(
        layered.layer(0).grid().get_cell(2, 0).map(|c| c.value.0 .0),
        Some(WATCHER),
        "тик 2: гейт всё ещё закрыт -- буфер не полон"
    );

    // Тик 3: сосед -> A. Буфер [A, B, A] == match_pattern -> гейт открыт,
    // но результат виден только СЛЕДУЮЩИМ тиком (буфер читается КАК ОН БЫЛ
    // до этого тика).
    layered.layer_mut(0).grid_mut().set_cell(3, 0, Cell::new(NEIGH_A));
    layered.run_tick();
    assert_eq!(
        layered.layer(0).grid().get_cell(2, 0).map(|c| c.value.0 .0),
        Some(WATCHER),
        "тик 3: буфер только что заполнился ДО конца этого тика -- гейт для этого тика ещё читал неполный буфер"
    );

    layered.run_tick();
    assert_eq!(layered.layer(0).grid().get_cell(2, 0).map(|c| c.value.0 .0), Some(FIRED), "тик 4: буфер [A,B,A] полон и совпадает с match_pattern -- гейт обязан открыться через LayeredEngine, как и через Engine");
}

/// Тот же класс бага, для `cam`: `Engine::detect_matches` явно пропускает
/// `cam`-совпадения (см. её doc-комментарий) -- через старую ручную связку
/// `detect_matches`/`arbitrate`/`apply_matches` магнит в `LayeredEngine`
/// никогда бы не нашёл цель, сколько бы тиков ни прошло.
#[test]
fn test_layered_cam_magnet_pulls_nearest_target() {
    const MAGNET: u8 = 40;
    const TARGET: u8 = 41;
    let mut grid = make_grid(10, 1);
    grid.set_cell(0, 0, Cell::new(MAGNET));
    grid.set_cell(4, 0, Cell::new(TARGET));

    let rule = Rule {
        id: vec![CellType(MAGNET)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![],
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: Default::default(),
        cam: Some(crate::types::CamSearch {
            radius: 5,
            target_type: CellType(TARGET),
        }),
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut layered = LayeredEngine::new(vec![grid], make_rule_index(vec![rule]));

    layered.run_tick();

    assert_eq!(
        layered.layer(0).grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(TARGET),
        "magnet must become the target type through LayeredEngine, as it does through plain Engine"
    );
    assert_eq!(
        layered.layer(0).grid().get_cell(4, 0).map(|c| c.value.0 .0),
        Some(0),
        "found cell must be cleared to default"
    );
}

/// Адверсариальная проверка непротестированной комбинации: самомодификация
/// (`RuleStore`, канал ввода через boundary-буфер) через `LayeredEngine`.
/// `Engine::self_mod` -- поле САМОГО `Engine`, не `LayeredEngine`, и
/// `absorb_self_modifications()` читает ТОЛЬКО СВОЙ `self.grid` -- значит
/// самомодификация должна работать НЕЗАВИСИМО по слоям (слой 0 получает
/// пакет через свой boundary-буфер и меняет ТОЛЬКО свой rule_index, слой 1
/// остаётся нетронутым), в отличие от общего конструктора `new` (клонирует
/// ОДИН И ТОТ ЖЕ набор правил на старте, но ничего не обещает про
/// синхронизацию ПОСЛЕ старта). Это прямое следствие фикса
/// `LayeredEngine::run_tick` этой же сессии (каждый слой тикает через
/// полный `Engine`-пайплайн, включающий `absorb_self_modifications`), но
/// сама эта комбинация не была протестирована ни разу — стоило проверить
/// конструктивно, а не полагаться на "should work по построению".
#[test]
fn test_self_modification_through_layered_engine_affects_only_the_layer_that_received_the_packet() {
    let mut grid0 = make_grid(3, 1);
    let mut grid1 = make_grid(3, 1);
    grid0.set_cell(0, 0, Cell::new(9));
    grid1.set_cell(0, 0, Cell::new(9));

    let mut layered = LayeredEngine::new(vec![grid0, grid1], HashMap::new());
    layered.layer_mut(0).enable_self_modification();

    let new_rule = Rule {
        id: vec![CellType(9)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(77))],
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
    let packet =
        crate::rule_store::serialize_add_rule(&new_rule).expect("serialize_add_rule must succeed for a plain rule");

    layered.layer_mut(0).grid_mut().set_boundary(
        1,
        0,
        crate::types::BoundaryBuffer {
            direction: "output".to_string(),
            ..Default::default()
        },
    );
    if let Some(buf) = layered.layer_mut(0).grid_mut().get_boundary_mut(1, 0) {
        for &b in &packet {
            buf.enqueue(
                0,
                Cell {
                    value: crate::types::CellValue(CellType(b)),
                    born_at: 0,
                },
            );
        }
    }

    // Тик 1: пакет попадает в rule_store, но матчинг этого же тика уже
    // прошёл по СТАРОМУ индексу (см. `absorb_self_modifications`'s
    // порядок вызова в конце тика) -- клетка(9) ещё не должна была
    // превратиться в 77 на этом самом тике.
    layered.run_tick();

    // Тик 2: новое правило уже проиндексировано слоем 0 -- клетка(0,0)
    // слоя 0 обязана стать 77. Слой 1 НИКОГДА не получал пакет и не
    // включал self-mod -- его клетка(0,0) обязана остаться 9 неизменной.
    layered.run_tick();

    assert_eq!(
        layered.layer(0).grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(77),
        "layer 0 must have absorbed and applied the self-modification packet it received"
    );
    assert_eq!(
        layered.layer(1).grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(9),
        "layer 1 received no packet and never enabled self-modification -- must remain completely untouched"
    );
}
