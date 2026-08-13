use super::super::*;
use super::common::*;
use crate::types::{Cell, CellType, Direction, OverflowAction, Rule, ShiftSpec};

const EMITTER: u8 = 50;

/// Источник очищается, ВСЕ клетки пути (не только финальная) получают
/// копию значения.
#[test]
fn test_broadcast_shift_fills_entire_path() {
    let mut grid = make_grid(10, 1);
    grid.set_cell(0, 0, Cell::new(EMITTER));
    let rule = Rule {
        id: vec![CellType(EMITTER)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec::broadcast(Direction::Right, 4)]],
        changes: vec![],
        active_only: false,
        priority: 0,
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
    let ri = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, ri);
    engine.run_tick();

    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(0),
        "source must be cleared"
    );
    for x in 1..=4 {
        assert_eq!(
            engine.grid().get_cell(x, 0).map(|c| c.value.0 .0),
            Some(EMITTER),
            "cell x={x} on the path must get a copy"
        );
    }
    assert_eq!(
        engine.grid().get_cell(5, 0).map(|c| c.value.0 .0),
        Some(0),
        "cell past the path must stay untouched"
    );
}

/// Обычный (не broadcast) сдвиг с тем же `steps` — контроль: промежуточные
/// клетки НЕ трогаются, только финальная (существующее поведение, не
/// должно было измениться).
#[test]
fn test_ordinary_shift_skips_intermediate_cells_unlike_broadcast() {
    let mut grid = make_grid(10, 1);
    grid.set_cell(0, 0, Cell::new(EMITTER));
    let rule = Rule {
        id: vec![CellType(EMITTER)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec::new(Direction::Right, 4)]],
        changes: vec![],
        active_only: false,
        priority: 0,
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
    let ri = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, ri);
    engine.run_tick();

    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(0),
        "source cleared"
    );
    for x in 1..=3 {
        assert_eq!(
            engine.grid().get_cell(x, 0).map(|c| c.value.0 .0),
            Some(0),
            "intermediate cell x={x} must stay untouched (ordinary shift = teleport)"
        );
    }
    assert_eq!(
        engine.grid().get_cell(4, 0).map(|c| c.value.0 .0),
        Some(EMITTER),
        "only the final target gets the value"
    );
}

/// Broadcast за пределами решётки: путь заполняется до края, дальше
/// `OverflowAction::Discard` — головка "теряется" в точке выхода, клетки
/// пути ДО края уже записаны и не откатываются.
#[test]
fn test_broadcast_shift_stops_at_grid_boundary() {
    let mut grid = make_grid(4, 1); // width=4: x=0..3
    grid.set_cell(0, 0, Cell::new(EMITTER));
    let rule = Rule {
        id: vec![CellType(EMITTER)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec::broadcast(Direction::Right, 10)]], // намного больше решётки
        changes: vec![],
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let ri = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, ri);
    engine.run_tick();

    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(0),
        "source cleared"
    );
    for x in 1..4 {
        assert_eq!(
            engine.grid().get_cell(x, 0).map(|c| c.value.0 .0),
            Some(EMITTER),
            "cell x={x} within grid bounds must get a copy"
        );
    }
}

// ──────────────────────────────────────────────────────────────
// `ShiftSpec::keep_source` ("излучение") — как broadcast, но источник НЕ
// очищается: значение КОПИРУЕТСЯ, а не ПЕРЕМЕЩАЕТСЯ.
// ──────────────────────────────────────────────────────────────

/// "Излучение" (`broadcast=true, keep_source=true`, `ShiftSpec::emit`):
/// источник сохраняет значение, ВСЕ клетки пути (не только финальная)
/// получают копию — контраст с `test_broadcast_shift_fills_entire_path`,
/// где источник очищается.
#[test]
fn test_emit_keeps_source_and_fills_entire_path() {
    let mut grid = make_grid(10, 1);
    grid.set_cell(0, 0, Cell::new(EMITTER));
    let rule = Rule {
        id: vec![CellType(EMITTER)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec::emit(Direction::Right, 4)]],
        changes: vec![],
        active_only: false,
        priority: 0,
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
    let ri = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, ri);
    engine.run_tick();

    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(EMITTER),
        "источник ДОЛЖЕН сохранить значение — keep_source не даёт его очистить"
    );
    for x in 1..=4 {
        assert_eq!(
            engine.grid().get_cell(x, 0).map(|c| c.value.0 .0),
            Some(EMITTER),
            "клетка x={x} на пути должна получить копию"
        );
    }
    assert_eq!(
        engine.grid().get_cell(5, 0).map(|c| c.value.0 .0),
        Some(0),
        "клетка за пределами пути должна остаться нетронутой"
    );
}

/// "Точечное излучение" (`broadcast=false, keep_source=true`): копия ТОЛЬКО
/// в конечную точку, промежуточные клетки не трогаются (как у обычного
/// сдвига), но, в отличие от обычного сдвига, источник тоже сохраняется —
/// значение КОПИРУЕТСЯ в конечную точку, а не ПЕРЕМЕЩАЕТСЯ.
#[test]
fn test_point_emit_copies_to_target_without_clearing_source() {
    let mut grid = make_grid(10, 1);
    grid.set_cell(0, 0, Cell::new(EMITTER));
    let rule = Rule {
        id: vec![CellType(EMITTER)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 4,
            broadcast: false,
            keep_source: true,
        }]],
        changes: vec![],
        active_only: false,
        priority: 0,
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
    let ri = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, ri);
    engine.run_tick();

    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(EMITTER),
        "источник должен сохранить значение"
    );
    for x in 1..4 {
        assert_eq!(
            engine.grid().get_cell(x, 0).map(|c| c.value.0 .0),
            Some(0),
            "промежуточная клетка x={x} не должна трогаться (не broadcast)"
        );
    }
    assert_eq!(
        engine.grid().get_cell(4, 0).map(|c| c.value.0 .0),
        Some(EMITTER),
        "конечная точка должна получить копию"
    );
}

fn emit_chain_rule(id_type: u8, direction: Direction, front_gated: bool) -> Rule {
    let pattern = if front_gated {
        vec![(0, 0, CellType(id_type)), (1, 0, CellType(0))]
    } else {
        vec![]
    };
    Rule {
        id: vec![CellType(id_type)],
        pattern,
        shifts: vec![vec![ShiftSpec {
            direction,
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
        max_activations: None,
        cross_layer_reads: Vec::new(),
    }
}

/// Регрессионный тест на реальный, найденный при построении
/// `examples/proof_reversibility_keep_source_cascade.rs` тупик: правило
/// `id: [SRC]` (без доп. условия на pattern) с `keep_source` шагом 1
/// НАВСЕГДА застревает на [source, copy] вместо роста в цепочку. Причина —
/// не конфликт `write_cells` сам по себе, а порядок тай-брейка арбитража
/// (`priority, age, ...` — `age` раньше координат): копия каждый тик
/// получает свежий `born_at` (apply всегда пишет `born_at: gen`, даже
/// повторно записывая то же значение), так что её age вечно 0 и она
/// никогда не выигрывает у куда более старого источника. Если тай-брейк
/// когда-нибудь изменится (порядок полей, добавление/удаление `age`), этот
/// тест должен пере-подтвердить или опровергнуть застревание явно, а не
/// молча разойтись с doc-комментарием примера.
#[test]
fn test_keep_source_naive_chain_rule_stalls_at_two_cells_due_to_age_tie_break() {
    let mut grid = make_grid(10, 1);
    grid.set_cell(0, 0, Cell::new(EMITTER));
    let ri = make_rule_index(vec![emit_chain_rule(EMITTER, Direction::Right, false)]);
    let mut engine = Engine::new(grid.clone(), ri);

    for _ in 0..5 {
        engine.run_tick();
    }

    let occupied: Vec<usize> = (0..10)
        .filter(|&x| engine.grid().get_cell(x, 0).map(|c| c.value.0 .0) == Some(EMITTER))
        .collect();
    assert_eq!(occupied, vec![0, 1], "naive id-only keep_source chain must stall at exactly [source, copy] after 5 ticks — age tie-break keeps resetting the copy's age every tick");
}

/// Тот же сценарий, но с фикс-условием ("моя цель сдвига сейчас пуста" —
/// front-gate) в pattern: внутренние звенья цепочки перестают матчиться
/// вообще (их цель уже занята следующей копией), так что на тик
/// существует РОВНО один матч и тай-брейк по age становится не при делах.
/// Цепочка растёт ровно на 1 клетку за тик — доказывает, что находка выше
/// была исправимым тупиком (per user's standing instruction to look for a
/// workaround before reporting a limitation), не фундаментальным свойством
/// `keep_source`.
#[test]
fn test_keep_source_front_gated_chain_rule_grows_one_cell_per_tick() {
    let mut grid = make_grid(10, 1);
    grid.set_cell(0, 0, Cell::new(EMITTER));
    let ri = make_rule_index(vec![emit_chain_rule(EMITTER, Direction::Right, true)]);
    let mut engine = Engine::new(grid, ri);

    for tick in 1..=5 {
        engine.run_tick();
        let occupied: Vec<usize> = (0..10)
            .filter(|&x| engine.grid().get_cell(x, 0).map(|c| c.value.0 .0) == Some(EMITTER))
            .collect();
        let expected: Vec<usize> = (0..=tick as usize).collect();
        assert_eq!(
            occupied, expected,
            "front-gated chain must grow by exactly one cell per tick, no stall, no skip"
        );
    }
}

/// Адверсариальный тест на класс бага, который этот проект уже находил
/// раньше (см. `AffectedRegion::written_cells`'s история): клетка, реально
/// НЕ записанная этим тиком, не должна получать сброшенный возраст.
/// Источник с `keep_source: true` — ровно такая клетка: значение не
/// поменялось, но её легко было бы по ошибке включить в bbox/written_cells
/// (как это и происходит без `keep_source`).
#[test]
fn test_emit_source_age_is_not_reset() {
    let mut grid = make_grid(5, 1);
    grid.set_cell(0, 0, Cell::new(EMITTER)); // born_at = 0 (Cell::new)
    let rule = Rule {
        id: vec![CellType(EMITTER)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec::emit(Direction::Right, 2)]],
        changes: vec![],
        active_only: false,
        priority: 0,
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
    let ri = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, ri);
    engine.run_tick();

    // generation после одного тика = 1. Если бы источник был (неверно)
    // включён в written_cells, reset_age_for_regions выставил бы ему
    // born_at = 1 (текущее поколение), и возраст стал бы 0 сразу после
    // тика, где он якобы только что "создан" — хотя физически не менялся.
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.born_at),
        Some(0),
        "born_at источника не должен меняться — клетка физически не записывалась этим тиком"
    );
    assert_eq!(
        engine.grid().get_age(0, 0),
        1,
        "возраст источника должен идти естественно (1 тик прошёл), а не обнуляться"
    );
}

// ──────────────────────────────────────────────────────────────
// `keep_source` × `OverflowAction` at the grid boundary — adversarial
// composition tests. `apply_overflow_write` (clamped boundary write) and
// the source-clear skip (`keep_source`) are two independent code paths
// inside `apply_shift_buffered`; these tests check they don't corrupt
// each other's `write_buffer`/`AffectedRegion::written_cells` bookkeeping,
// including the geometric edge case where the clamped boundary position
// coincides with an already-written path cell or with the source itself.
// ──────────────────────────────────────────────────────────────

/// Broadcast + `keep_source` + `OverflowAction::WriteLiteral` where the path
/// overshoots the grid by exactly one step. The clamped boundary position
/// (`w-1`) is unavoidably identical to the LAST cell the path fill already
/// wrote in-bounds (monotonic path from an interior source always reaches
/// the edge before overflowing) — so the overflow write's literal value
/// overwrites the broadcast value that was just placed there one loop
/// iteration earlier. This is independent of `keep_source` (same clash
/// exists with `keep_source: false`); the point of this test is to confirm
/// (a) the source at x=0 truly stays untouched, (b) the interior path cells
/// keep the emitted value, (c) the boundary cell ends up with the OVERFLOW
/// literal (not the emitted value, not lost, not corrupted), and (d) no
/// panic / no wrong born_at results from the double write to that cell.
#[test]
fn test_emit_broadcast_writeliteral_overflow_overwrites_last_path_cell() {
    let mut grid = make_grid(6, 1); // x = 0..=5
    grid.set_cell(0, 0, Cell::new(EMITTER));
    let rule = Rule {
        id: vec![CellType(EMITTER)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 8,
            broadcast: true,
            keep_source: true,
        }]],
        changes: vec![],
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: OverflowAction::WriteLiteral(77),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let ri = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, ri);
    engine.run_tick();

    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(EMITTER),
        "source (x=0) must stay untouched — keep_source"
    );
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.born_at),
        Some(0),
        "source born_at must not be reset — it was never written"
    );
    for x in 1..5 {
        assert_eq!(
            engine.grid().get_cell(x, 0).map(|c| c.value.0 .0),
            Some(EMITTER),
            "interior path cell x={x} keeps the emitted value"
        );
    }
    assert_eq!(
        engine.grid().get_cell(5, 0).map(|c| c.value.0 .0),
        Some(77),
        "boundary cell (x=5, where the path exits) ends up with the overflow literal, overwriting the emitted value written moments earlier"
    );
    assert_eq!(
        engine.grid().get_cell(5, 0).map(|c| c.born_at),
        Some(1),
        "boundary cell born_at must be the current generation — it WAS genuinely written (twice)"
    );
}

/// Degenerate coincidence: source sits AT the grid edge, `steps: 1`, so the
/// shift's only target is immediately out of bounds and the overflow clamp
/// lands EXACTLY on the source's own coordinates — the sole write this rule
/// produces is the overflow write, and it targets the "kept" source cell.
/// Checks parity between `keep_source: true` and `keep_source: false`: since
/// the overflow write is unconditional (independent of the source-clear
/// skip), both must produce the IDENTICAL final value/born_at at that cell
/// — `keep_source` doesn't (and per its doc-comment, only promises to skip
/// its OWN clear/move step) prevent an unrelated overflow write from a
/// DIFFERENT computation landing on the same coordinates.
#[test]
fn test_emit_broadcast_overflow_source_coincidence_parity_with_non_keep_source() {
    let run = |keep_source: bool| {
        let mut grid = make_grid(3, 1); // x = 0..=2
        grid.set_cell(2, 0, Cell::new(EMITTER)); // source AT the right edge
        let rule = Rule {
            id: vec![CellType(EMITTER)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec {
                direction: Direction::Right,
                steps: 1,
                broadcast: true,
                keep_source,
            }]],
            changes: vec![],
            active_only: false,
            priority: 0,
            min_age: 0,
            overflow: OverflowAction::WriteLiteral(99),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        };
        let ri = make_rule_index(vec![rule]);
        let mut engine = Engine::new(grid, ri);
        engine.run_tick();
        (
            engine.grid().get_cell(2, 0).map(|c| c.value.0 .0),
            engine.grid().get_cell(2, 0).map(|c| c.born_at),
        )
    };

    let with_keep_source = run(true);
    let without_keep_source = run(false);

    assert_eq!(
        with_keep_source, without_keep_source,
        "overflow clamping onto the source coordinate must behave identically regardless of keep_source"
    );
    assert_eq!(with_keep_source.0, Some(99), "the overflow literal wins at the coincidence cell — keep_source cannot protect a cell that a DIFFERENT write (overflow) independently targets");
    assert_eq!(
        with_keep_source.1,
        Some(1),
        "born_at reflects a genuine write this tick"
    );
}

/// Non-broadcast point-emit (`keep_source: true, broadcast: false`) with the
/// single target off-grid and `OverflowAction::Write(0)` — the zero-literal
/// special case meaning "carry the head's own value as-is" (see
/// `apply_overflow_write`'s doc-comment). Target position (clamped) is
/// distinct from the source, so this isolates task-item #2: does keep_source
/// change the code path leading to the boundary write at all? It shouldn't —
/// the overflow-write call is unconditional, below and independent of the
/// keep_source-gated clear block. Verifies value/born_at parity between
/// keep_source true/false at the target cell, plus the source-preservation
/// difference.
#[test]
fn test_point_emit_overflow_write_zero_carries_own_value_at_boundary() {
    let run = |keep_source: bool| {
        let mut grid = make_grid(5, 1); // x = 0..=4
        grid.set_cell(2, 0, Cell::new(EMITTER));
        let rule = Rule {
            id: vec![CellType(EMITTER)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec {
                direction: Direction::Right,
                steps: 5,
                broadcast: false,
                keep_source,
            }]], // target x=7, clamps to x=4
            changes: vec![],
            active_only: false,
            priority: 0,
            min_age: 0,
            overflow: OverflowAction::Write(0), // 0 == "carry own value", not literal 0
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        };
        let ri = make_rule_index(vec![rule]);
        let mut engine = Engine::new(grid, ri);
        engine.run_tick();
        (
            engine.grid().get_cell(2, 0).map(|c| c.value.0 .0), // source
            engine.grid().get_cell(4, 0).map(|c| c.value.0 .0), // clamped boundary target
            engine.grid().get_cell(4, 0).map(|c| c.born_at),
        )
    };

    let (src_keep, target_keep, born_keep) = run(true);
    let (src_no_keep, target_no_keep, born_no_keep) = run(false);

    assert_eq!(src_keep, Some(EMITTER), "keep_source: source retains its value");
    assert_eq!(
        src_no_keep,
        Some(0),
        "without keep_source: source is cleared by the shift"
    );
    assert_eq!(
        target_keep,
        Some(EMITTER),
        "boundary cell carries the head's own value (Write(0) semantics)"
    );
    assert_eq!(target_keep, target_no_keep, "boundary write value must be identical regardless of keep_source — the overflow-write call site is unconditional");
    assert_eq!(
        born_keep, born_no_keep,
        "boundary born_at must be identical regardless of keep_source"
    );
    assert_eq!(
        born_keep,
        Some(1),
        "boundary cell born_at reflects the write this tick, not the stale pre-tick born_at carried inside head_cell"
    );
}

/// Directly inspects `AffectedRegion::written_cells` (not just final grid
/// state) for the source-coincidence scenario from
/// `test_emit_broadcast_overflow_source_coincidence_parity_with_non_keep_source`,
/// using `apply_matches` instead of `run_tick` so the region is observable
/// before/independent of age-reset. Checks the specific bookkeeping concern
/// from task item #3: with `keep_source: true`, the source-clear step never
/// runs, so `written_cells` should contain the coincidence cell (2,0) exactly
/// ONCE (from the overflow write alone) — not zero times (which would wrongly
/// skip the age reset for a cell that WAS genuinely written) and not
/// corrupted by an interaction with the skipped clear step.
#[test]
fn test_emit_broadcast_overflow_source_coincidence_written_cells_bookkeeping() {
    let mut grid = make_grid(3, 1);
    grid.set_cell(2, 0, Cell::new(EMITTER));
    let rule = Rule {
        id: vec![CellType(EMITTER)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 1,
            broadcast: true,
            keep_source: true,
        }]],
        changes: vec![],
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: OverflowAction::WriteLiteral(99),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let ri = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, ri);
    let matches = engine.detect_matches();
    assert_eq!(matches.len(), 1);
    let accepted = engine.arbitrate(matches);
    assert_eq!(accepted.len(), 1);

    let (regions, _) = engine.apply_matches(accepted);
    assert_eq!(regions.len(), 1);
    let written: Vec<_> = regions[0]
        .written_cells
        .iter()
        .filter(|&&(x, y)| (x, y) == (2, 0))
        .collect();
    assert_eq!(written.len(), 1, "coincidence cell (2,0) must appear in written_cells exactly once (overflow write only — keep_source skipped its own clear/push entirely)");

    engine.advance_age();
    engine.reset_age(&regions);
    assert_eq!(
        engine.grid().get_cell(2, 0).map(|c| c.value.0 .0),
        Some(99),
        "value is the overflow literal"
    );
    assert_eq!(engine.grid().get_age(2, 0), 0, "age correctly reset — the cell WAS genuinely written this tick, just not via the keep_source-skipped clear path");
}

// ──────────────────────────────────────────────────────────────
// "Активный таймер" — доказательство, что это УЖЕ выражается
// существующими примитивами (min_age / счётная цепочка self-change),
// не новая возможность модели.
// ──────────────────────────────────────────────────────────────
