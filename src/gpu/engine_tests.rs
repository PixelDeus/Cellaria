use std::collections::HashMap;

use super::*;
use crate::types::{CellValue, ChangeValue, Direction, OverflowAction, ShiftSpec};

fn rule(id: u8, pattern: Vec<(i8, i8, u8)>, new_value: u8, priority: u32) -> Rule {
    Rule {
        id: vec![CellType(id)],
        pattern: pattern.into_iter().map(|(dx, dy, t)| (dx, dy, CellType(t))).collect(),
        shifts: Vec::new(),
        changes: vec![(0, 0, ChangeValue::Literal(new_value))],
        active_only: false,
        priority,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
    }
}

/// Правило "клетка 1 с пустым соседом справа" -> 2 (низкий приоритет) и
/// "клетка 1 с живым соседом справа" -> 3 (высокий приоритет) — та же
/// сцена, что уже проверена на реальном железе вручную (scratch-прототип
/// `v1_check.rs`), теперь как честный тест поверх настоящего `GpuEngine`.
#[test]
fn test_gpu_engine_pattern_match_and_priority_tie_break() {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    idx.insert(
        CellType(1),
        vec![
            rule(1, vec![(0, 0, 1), (1, 0, 0)], 2, 1),
            rule(1, vec![(0, 0, 1), (1, 0, 1)], 3, 5),
        ],
    );

    let initial = vec![
        (0usize, 0usize, Cell { value: CellValue::new(1), born_at: 0 }),
        (2, 0, Cell { value: CellValue::new(1), born_at: 0 }),
        (3, 0, Cell { value: CellValue::new(1), born_at: 0 }),
    ];
    let mut engine = GpuEngine::new(4, 1, &initial, &idx).expect("GoL-shaped v1 config must build");
    engine.run_tick();
    let grid = engine.read_grid();

    assert_eq!(grid[0].value.0 .0, 2); // self=1, right=0(in bounds) -> rule prio 1
    assert_eq!(grid[1].value.0 .0, 0); // untouched empty cell
    assert_eq!(grid[2].value.0 .0, 3); // self=1, right=1 -> rule prio 5
    // Клетка 3 читает офсет (1,0) -> x=4, вне решётки (width=4) — union-gate
    // (см. rule_table.rs::GpuHeadSlot::offsets_start) отключает ВСЮ голову
    // целиком для этой клетки, как и CPU-матчер — клетка остаётся нетронутой.
    assert_eq!(grid[3].value.0 .0, 1);
    assert_eq!(grid[0].born_at, 1);
    assert_eq!(grid[2].born_at, 1);
    assert_eq!(grid[3].born_at, 0);
}

#[test]
fn test_gpu_engine_run_ticks_matches_repeated_run_tick() {
    // Простое "приращение": 1 -> 2 -> 3 -> ... каждый тик, без соседей.
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for v in 1u8..=5 {
        idx.entry(CellType(v)).or_default().push(rule(v, vec![(0, 0, v)], v + 1, 0));
    }
    let initial = vec![(0usize, 0usize, Cell { value: CellValue::new(1), born_at: 0 })];

    let mut stepwise = GpuEngine::new(1, 1, &initial, &idx).unwrap();
    for _ in 0..4 {
        stepwise.run_tick();
    }
    let stepwise_result = stepwise.read_grid();

    let mut batched = GpuEngine::new(1, 1, &initial, &idx).unwrap();
    batched.run_ticks(4);
    let batched_result = batched.read_grid();

    assert_eq!(stepwise_result[0].value.0 .0, 5);
    assert_eq!(stepwise_result[0].value.0 .0, batched_result[0].value.0 .0);
    assert_eq!(stepwise_result[0].born_at, batched_result[0].born_at);
}

#[test]
fn test_gpu_engine_rejects_unsupported_rule() {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    idx.insert(CellType(1), vec![rule(1, vec![(0, 0, 1)], 2, 0)]);
    idx.get_mut(&CellType(1)).unwrap()[0].changes = vec![];

    match GpuEngine::new(1, 1, &[], &idx) {
        Err(err) => assert_eq!(err, GpuUnsupportedReason::NoEffect { head: 1, rule_idx: 0 }),
        Ok(_) => panic!("expected GpuUnsupportedReason::NoEffect"),
    }
}

fn mover_rule(id: u8, direction: Direction, priority: u32) -> Rule {
    Rule {
        id: vec![CellType(id)],
        pattern: vec![(0, 0, CellType(id))],
        shifts: vec![vec![ShiftSpec::new(direction, 1)]],
        changes: vec![],
        active_only: false,
        priority,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
    }
}

/// Одна частица, без конфликтов — использует arbitrated-пайплайн
/// (needs_arbitration=true, т.к. правило со сдвигом), но арбитраж тут
/// тривиален (единственный претендент на обе свои ячейки).
#[test]
fn test_gpu_engine_arbitrated_single_move_no_conflict() {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    idx.insert(CellType(1), vec![mover_rule(1, Direction::Right, 0)]);

    let initial = vec![(0usize, 0usize, Cell { value: CellValue::new(1), born_at: 0 })];
    let mut engine = GpuEngine::new(4, 1, &initial, &idx).expect("single Discard shift is supported");
    engine.run_tick();
    let grid = engine.read_grid();

    assert_eq!(grid[0].value.0 .0, 0, "source must be cleared");
    assert_eq!(grid[1].value.0 .0, 1, "target must carry the moved value");
    assert_eq!(grid[2].value.0 .0, 0);
    assert_eq!(grid[3].value.0 .0, 0);
    // Оба реально записанных write cell (source-clear и target) получают
    // born_at = generation+1, как и в CPU reset_age_for_regions.
    assert_eq!(grid[0].born_at, 1);
    assert_eq!(grid[1].born_at, 1);
}

/// Реальный конфликт записи: частица A (голова 1, приоритет 5) движется
/// вправо в (1,0), частица B (голова 2, приоритет 1) движется влево тоже в
/// (1,0). A должна выиграть арбитраж ЦЕЛИКОМ (её собственный source (0,0) +
/// target (1,0)) — а B, проиграв всего ОДНУ из своих ячеек ((1,0)), должна
/// быть отклонена ПОЛНОСТЬЮ (её source (2,0) тоже НЕ должен очиститься —
/// см. arbitrator::arbitrate: принятие/отклонение матча атомарно по ВСЕМ
/// его write cells, а не по каждой ячейке независимо).
#[test]
fn test_gpu_engine_arbitrated_write_conflict_all_or_nothing() {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    idx.insert(CellType(1), vec![mover_rule(1, Direction::Right, 5)]);
    idx.insert(CellType(2), vec![mover_rule(2, Direction::Left, 1)]);

    let initial = vec![
        (0usize, 0usize, Cell { value: CellValue::new(1), born_at: 0 }),
        (2, 0, Cell { value: CellValue::new(2), born_at: 0 }),
    ];
    let mut engine = GpuEngine::new(3, 1, &initial, &idx).expect("two Discard-shift movers are supported");
    engine.run_tick();
    let grid = engine.read_grid();

    assert_eq!(grid[0].value.0 .0, 0, "A's source must be cleared (A accepted)");
    assert_eq!(grid[1].value.0 .0, 1, "A must win the contested target (higher priority)");
    assert_eq!(grid[2].value.0 .0, 2, "B's source must stay UNCHANGED (B rejected entirely, all-or-nothing)");
    assert_eq!(grid[0].born_at, 1);
    assert_eq!(grid[1].born_at, 1);
    assert_eq!(grid[2].born_at, 0, "B untouched -> born_at stays at its original value");
}
