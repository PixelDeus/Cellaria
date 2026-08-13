use super::super::*;
use super::common::*;
use crate::types::{Cell, CellType, ChangeValue, Direction, Rule, ShiftSpec};

/// `Rule::max_activations` — правило с бюджетом 3 побеждает ровно 3 раза,
/// затем гейт закрывается НАВСЕГДА (не сбрасывается, не открывается заново).
/// Проверка не просто "значение перестало обновляться" (неотличимо от "и не
/// пыталось") — между тиком 3 и тиком 4 клетка (1,0) сбрасывается НАПРЯМУЮ
/// (в обход правил) в 0, и тик 4 обязан оставить её 0 -- если бы гейт был
/// всё ещё открыт, правило переписало бы её обратно в 200.
#[test]
fn test_max_activations_gate_closes_permanently_after_budget() {
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
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    for tick in 1..=3 {
        engine.run_tick();
        assert_eq!(
            engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
            Some(200),
            "тик {tick}: правило ещё в пределах бюджета, обязано сработать"
        );
    }
    assert_eq!(
        engine.state.snapshot().activation_counters().get(&(CellType(1), 0)),
        Some(&BUDGET),
        "счётчик активаций должен достичь бюджета ровно после 3-го срабатывания"
    );

    // Клетка (1,0) сбрасывается НАПРЯМУЮ, в обход правил -- честная проверка
    // "гейт закрыт", а не просто "значение совпало со старым".
    engine.grid_mut().set_cell(1, 0, Cell::new(0));

    for tick in 4..=10 {
        engine.run_tick();
        assert_eq!(
            engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
            Some(0),
            "тик {tick}: бюджет исчерпан НАВСЕГДА -- правило не должно сработать снова, даже спустя много тиков"
        );
    }
}

/// Мотивирующий случай: `ShiftSpec::keep_source` без ограничения может
/// копировать головку неограниченно (источник никогда не убывает). Правило
/// требует ПУСТОГО соседа справа перед копированием (иначе бы каждая копия
/// провоцировала переоценку у ВСЕХ предыдущих позиций тоже — паттерн-гейт,
/// не свойство `max_activations`) — это делает рост линейным и предсказуемым
/// (одна новая копия за тик), значит после бюджета=3 итоговое число маркеров
/// НАВСЕГДА ограничено 1 (исходный) + 3 (копии) = 4, сколько бы тиков ни
/// прошло дальше -- глобальный (не по позиции) счётчик режет рост у ЛЮБОЙ
/// клетки, унаследовавшей тот же `rule_idx`, а не только у первой.
#[test]
fn test_max_activations_bounds_keep_source_growth() {
    const BUDGET: u32 = 3;
    const MARKER: u8 = 7;
    let mut grid = make_grid(20, 1);
    grid.set_cell(0, 0, Cell::new(MARKER));
    let rule = Rule {
        id: vec![CellType(MARKER)],
        pattern: vec![(0, 0, CellType(MARKER)), (1, 0, CellType(0))],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 1,
            broadcast: false,
            keep_source: true,
        }]],
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
        max_activations: Some(BUDGET),
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    for _ in 0..15 {
        engine.run_tick();
    }

    let marker_count = (0..20)
        .filter(|&x| engine.grid().get_cell(x, 0).map(|c| c.value.0 .0) == Some(MARKER))
        .count();
    assert_eq!(
        marker_count,
        1 + BUDGET as usize,
        "после исчерпания бюджета общее число маркеров обязано остаться ровно 1 (исходный) + BUDGET (копии), несмотря на 15 прогнанных тиков"
    );
}

/// Регрессия: `activation_counters` — та же проблема переиспользования
/// `rule_idx`, что и `starvation_counters` (см.
/// `test_rebuild_rule_cache_clears_stale_starvation_counter_on_rule_idx_reuse`),
/// только с ключом `(head, rule_idx)` вместо `(x, y, rule_idx)` — не нужен
/// grid-лукап вообще, достаточно совпадения `head`.
#[test]
fn test_rebuild_rule_cache_clears_stale_activation_counter_on_rule_idx_reuse() {
    let mut grid = make_grid(1, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let old_rule = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
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
        max_activations: Some(1),
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![old_rule]));
    engine.run_tick();
    assert_eq!(
        engine.state.snapshot().activation_counters().get(&(CellType(1), 0)),
        Some(&1),
        "старое правило должно было накопить 1 активацию"
    );

    let new_rule = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![],
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
        max_activations: Some(1),
        cross_layer_reads: Vec::new(),
    };
    let mut head_1_rules = engine.rule_index().get(&CellType(1)).unwrap().clone();
    head_1_rules[0] = new_rule;
    engine.set_rules_for_head(CellType(1), head_1_rules);

    assert_eq!(
        engine.state.snapshot().activation_counters().get(&(CellType(1), 0)),
        None,
        "rebuild_rule_cache должен был очистить унаследованный счётчик активаций на переиспользованном rule_idx"
    );
}
