use super::super::*;
use super::common::*;
use crate::types::{Cell, CellType, CellValue, ChangeValue, Direction, OverflowAction, Rule, ShiftSpec};
use crate::BoundaryBuffer;
use crate::VecStorage;
use std::collections::HashSet;

#[test]
fn test_run_tick() {
    let mut grid = make_grid(2, 2);
    grid.set_cell(0, 0, Cell::new(5));

    let rule = Rule {
        id: vec![CellType(5)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(9))],
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
    let mut engine = Engine::new(grid, rule_index);

    let matches = engine.detect_matches();
    assert_eq!(matches.len(), 1);

    let accepted = engine.arbitrate(matches);
    assert_eq!(accepted.len(), 1);

    engine.apply_matches(accepted);

    assert_eq!(engine.grid.get_cell(0, 0).unwrap().value, CellValue(CellType(9)));
}

#[test]
fn test_shift_right() {
    let mut grid = make_grid(3, 1);
    grid.set_cell(0, 0, Cell::new(5));

    let rule = Rule {
        id: vec![CellType(5)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 1,
            broadcast: false,
            keep_source: false,
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
    };

    let rule_index = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, rule_index);

    let matches = engine.detect_matches();
    let accepted = engine.arbitrate(matches);
    engine.apply_matches(accepted);

    assert_eq!(engine.grid.get_cell(0, 0).unwrap().value, CellValue(CellType(0)));
    assert_eq!(engine.grid.get_cell(1, 0).unwrap().value, CellValue(CellType(5)));
}

#[test]
fn test_shift_with_change() {
    let mut grid = make_grid(3, 1);
    grid.set_cell(0, 0, Cell::new(5));
    grid.set_cell(1, 0, Cell::new(7));

    let rule = Rule {
        id: vec![CellType(5), CellType(7)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 1,
            broadcast: false,
            keep_source: false,
        }]],
        changes: vec![(0, 0, ChangeValue::Literal(1)), (1, 0, ChangeValue::Literal(2))],
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
    let mut engine = Engine::new(grid, rule_index);
    let matches = engine.detect_matches();
    assert_eq!(matches.len(), 1);

    let accepted = engine.arbitrate(matches);
    assert_eq!(accepted.len(), 1);

    // Применяем правило с возрастными эффектами
    let (regions, _) = engine.apply_matches(accepted);
    engine.advance_age();
    engine.reset_age(&regions);

    assert_eq!(
        engine.grid.get_cell(0, 0).unwrap().value,
        CellValue(CellType(0)),
        "original cell is cleared by shift"
    );
    assert_eq!(
        engine.grid.get_cell(1, 0).unwrap().value,
        CellValue(CellType(1)),
        "change (0,0) + total_dx=1 => (1,0) = 1"
    );
    assert_eq!(
        engine.grid.get_cell(2, 0).unwrap().value,
        CellValue(CellType(2)),
        "change (1,0) + total_dx=1 => (2,0) = 2"
    );
}

#[test]
fn test_overflow_discard() {
    let mut grid = make_grid(1, 1);
    grid.set_cell(0, 0, Cell::new(5));

    let rule = Rule {
        id: vec![CellType(5)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 1,
            broadcast: false,
            keep_source: false,
        }]],
        changes: vec![],
        active_only: false,
        priority: 10,
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

    let rule_index = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, rule_index);
    let matches = engine.detect_matches();
    let accepted = engine.arbitrate(matches);
    engine.apply_matches(accepted);

    assert_eq!(engine.grid.get_cell(0, 0).unwrap().value, CellValue(CellType(0)));
}

#[test]
fn test_overflow_write() {
    let mut grid = make_grid(1, 1);
    grid.set_cell(0, 0, Cell::new(42));

    let rule = Rule {
        id: vec![CellType(42)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 1,
            broadcast: false,
            keep_source: false,
        }]],
        changes: vec![],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: OverflowAction::Write(99),
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
    let mut engine = Engine::new(grid, rule_index);
    let matches = engine.detect_matches();
    let accepted = engine.arbitrate(matches);
    engine.apply_matches(accepted);

    assert_eq!(engine.grid.get_cell(0, 0).unwrap().value, CellValue(CellType(99)));
}

#[test]
fn test_overflow_write_literal_zero_fallback() {
    // `Write(0)` means "carry the head's own value", so a literal `0` was
    // previously inexpressible through `OverflowAction` at all — this is
    // exactly what `WriteLiteral` exists to fix (see types.rs doc comment).
    // No boundary at the overflow target, so this exercises the fallback
    // grid-write path, mirroring `test_overflow_write` above.
    let mut grid = make_grid(1, 1);
    grid.set_cell(0, 0, Cell::new(42));

    let rule = Rule {
        id: vec![CellType(42)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 1,
            broadcast: false,
            keep_source: false,
        }]],
        changes: vec![],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: OverflowAction::WriteLiteral(0),
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
    let mut engine = Engine::new(grid, rule_index);
    let matches = engine.detect_matches();
    let accepted = engine.arbitrate(matches);
    engine.apply_matches(accepted);

    assert_eq!(engine.grid.get_cell(0, 0).unwrap().value, CellValue(CellType(0)));
}

#[test]
fn test_overflow_write_literal_zero_boundary() {
    // Same as above, but with a boundary present at the overflow target —
    // exercises the enqueue path (`buf.enqueue`), the one actually used by
    // `examples/proof_universal_self_reflection.rs` to transmit literal
    // zero bytes through an output port.
    let mut grid = make_grid(1, 1);
    grid.set_cell(0, 0, Cell::new(42));
    grid.set_boundary(0, 0, BoundaryBuffer::new());

    let rule = Rule {
        id: vec![CellType(42)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 1,
            broadcast: false,
            keep_source: false,
        }]],
        changes: vec![],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: OverflowAction::WriteLiteral(0),
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
    let mut engine = Engine::new(grid, rule_index);
    let matches = engine.detect_matches();
    let accepted = engine.arbitrate(matches);
    engine.apply_matches(accepted);

    let queued: Vec<u8> = engine
        .grid
        .get_boundary(0, 0)
        .and_then(|b| b.queues.get(&0))
        .map(|q| q.iter().map(|c| c.value.0 .0).collect())
        .unwrap_or_default();
    assert_eq!(queued, vec![0]);
}

#[test]
fn test_guarded_self_modification_accepts_safe_and_rejects_unsafe() {
    // Модуль A (id=1) существует с самого начала — "чужая территория" для
    // любой последующей самомодификации. Отправляем два пакета AddRule
    // напрямую в очередь выходного буфера (транспорт клетками-носителями
    // уже отдельно доказан в examples/strength_self_modification*.rs —
    // здесь проверяется только решение охраны).
    let mut grid = make_grid(1, 1);
    let mut out = BoundaryBuffer::new();
    out.direction = "output".to_string();
    grid.set_boundary(0, 0, out);

    let rule_index = make_rule_index(vec![Rule {
        id: vec![CellType(1)],
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
    }]);
    let mut engine = Engine::new(grid, rule_index);
    engine.enable_guarded_self_modification();

    let inject = |engine: &mut Engine<VecStorage>, packet: &[u8]| {
        let buf = engine.grid_mut().get_boundary_mut(0, 0).unwrap();
        for &b in packet {
            buf.enqueue(
                0,
                Cell {
                    value: CellValue(CellType(b)),
                    born_at: 0,
                },
            );
        }
        engine.run_tick();
    };

    // Безопасный пакет: новый id=50, меняет свою же клетку — не пересекается
    // ни с чем. [priority, id_len, id_byte, dx, dy, value, terminator]
    inject(&mut engine, &[10, 1, 50, 0, 0, 77, 0xFF]);
    assert!(engine.rule_index().contains_key(&CellType(50)));
    assert_eq!(engine.rejected_self_modifications, 0);

    // Опасный пакет: id=1, та же голова и та же (0,0) цель записи, что у
    // модуля A — доказуемый конфликт, должен быть отклонён.
    inject(&mut engine, &[10, 1, 1, 0, 0, 99, 0xFF]);
    assert_eq!(engine.rejected_self_modifications, 1);
    let a_rule = &engine.rule_index()[&CellType(1)];
    assert_eq!(a_rule.len(), 1);
    assert_eq!(a_rule[0].changes, vec![(0, 0, ChangeValue::Literal(3))]);
}

#[test]
fn test_self_modification_extending_existing_head_preserves_original() {
    // `RuleStore::get_index()` rebuilds entirely from the rules it has
    // itself seen via `AddRule` — it knows nothing about rules that were
    // part of `rule_index` from `Engine::new` (added outside the protocol).
    // Merging by blind `rule_index.insert(head, get_index()[head])` would
    // silently replace the original with just the self-added rule instead
    // of adding to it.
    let mut grid = make_grid(1, 1);
    let mut out = BoundaryBuffer::new();
    out.direction = "output".to_string();
    grid.set_boundary(0, 0, out);

    let original = Rule {
        id: vec![CellType(1)],
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
    let mut engine = Engine::new(grid, make_rule_index(vec![original.clone()]));
    engine.enable_self_modification();

    let buf = engine.grid_mut().get_boundary_mut(0, 0).unwrap();
    for &b in &[20u8, 1, 1, 0, 0, 77, 0xFF] {
        buf.enqueue(
            0,
            Cell {
                value: CellValue(CellType(b)),
                born_at: 0,
            },
        );
    }
    engine.run_tick();

    let rules = &engine.rule_index()[&CellType(1)];
    assert!(rules.contains(&original), "original rule must survive the merge");
    assert!(
        rules
            .iter()
            .any(|r| r.changes == vec![(0, 0, ChangeValue::Literal(77))]),
        "self-added rule must also be present"
    );
}

#[test]
fn test_self_modification_remove_rule_actually_removes() {
    let mut grid = make_grid(1, 1);
    let mut out = BoundaryBuffer::new();
    out.direction = "output".to_string();
    grid.set_boundary(0, 0, out);

    let mut engine = Engine::new(grid, HashMap::new());
    engine.enable_self_modification();

    let inject = |engine: &mut Engine<VecStorage>, packet: &[u8]| {
        let buf = engine.grid_mut().get_boundary_mut(0, 0).unwrap();
        for &b in packet {
            buf.enqueue(
                0,
                Cell {
                    value: CellValue(CellType(b)),
                    born_at: 0,
                },
            );
        }
        engine.run_tick();
    };

    inject(&mut engine, &[10, 1, 50, 0xFF]);
    assert!(engine.rule_index().contains_key(&CellType(50)));

    inject(&mut engine, &[0xF0, 1, 50, 0xFF]); // RemoveRule(50): [OP_REMOVE, id_len, id, terminator]
    assert!(
        !engine.rule_index().contains_key(&CellType(50)),
        "RemoveRule must actually take effect in rule_index, not just in RuleStore's internal state"
    );
}

#[test]
fn test_self_modification_preserves_rule_added_after_construction() {
    // A rule inserted directly into `rule_index` AFTER `Engine::new` (the
    // documented `strength_live_rules.rs` pattern — set it, then call
    // `rebuild_rule_cache`) is just as "foreign" to `RuleStore` as one
    // present at construction time. `original_rule_index` must capture it
    // when self-modification is enabled, not only what existed at
    // `Engine::new`.
    let mut grid = make_grid(1, 1);
    let mut out = BoundaryBuffer::new();
    out.direction = "output".to_string();
    grid.set_boundary(0, 0, out);

    let mut engine = Engine::new(grid, HashMap::new());
    let original = Rule {
        id: vec![CellType(1)],
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
    engine.set_rules_for_head(CellType(1), vec![original.clone()]);
    engine.enable_self_modification();

    let buf = engine.grid_mut().get_boundary_mut(0, 0).unwrap();
    for &b in &[20u8, 1, 1, 0, 0, 77, 0xFF] {
        buf.enqueue(
            0,
            Cell {
                value: CellValue(CellType(b)),
                born_at: 0,
            },
        );
    }
    engine.run_tick();

    let rules = &engine.rule_index()[&CellType(1)];
    assert!(
        rules.contains(&original),
        "rule added after Engine::new must survive a self-mod extension"
    );
    assert!(rules
        .iter()
        .any(|r| r.changes == vec![(0, 0, ChangeValue::Literal(77))]));
}

#[test]
fn test_guarded_self_modification_on_chunk_storage() {
    // None of this session's self-modification/guard work was ever tested
    // against `ChunkStorage` (the unbounded grid) — only `VecStorage`.
    // Neither `composition_allows` nor the merge logic in
    // `absorb_self_modifications` reference the storage backend at all
    // (they operate purely on `rule_index`/`RuleStore`), so this is
    // expected to just work — confirmed here rather than assumed. The
    // boundary itself sits at a large, "arbitrary" coordinate (matching
    // ChunkStorage's actual use case) rather than the origin.
    use crate::storage::ChunkStorage;

    const BOUNDARY_X: usize = 1_000_000;
    let mut grid = Grid::new(ChunkStorage::new(), HashSet::new());
    let mut out = BoundaryBuffer::new();
    out.direction = "output".to_string();
    grid.set_boundary(BOUNDARY_X, 0, out);

    let rule_index = make_rule_index(vec![Rule {
        id: vec![CellType(1)],
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
    }]);
    let mut engine = Engine::new(grid, rule_index);
    engine.enable_guarded_self_modification();

    let inject = |engine: &mut Engine<ChunkStorage>, packet: &[u8]| {
        let buf = engine.grid_mut().get_boundary_mut(BOUNDARY_X, 0).unwrap();
        for &b in packet {
            buf.enqueue(
                0,
                Cell {
                    value: CellValue(CellType(b)),
                    born_at: 0,
                },
            );
        }
        engine.run_tick();
    };

    inject(&mut engine, &[10, 1, 50, 0, 0, 77, 0xFF]);
    assert!(engine.rule_index().contains_key(&CellType(50)));
    assert_eq!(engine.rejected_self_modifications, 0);

    inject(&mut engine, &[10, 1, 1, 0, 0, 99, 0xFF]);
    assert_eq!(engine.rejected_self_modifications, 1);
    let a_rule = &engine.rule_index()[&CellType(1)];
    assert_eq!(a_rule.len(), 1);
    assert_eq!(a_rule[0].changes, vec![(0, 0, ChangeValue::Literal(3))]);
}

#[test]
fn test_guarded_self_modification_catches_conflict_within_same_batch() {
    // Two self-installed rules that conflict with EACH OTHER (not with
    // anything pre-existing) can complete in the very same tick — e.g. two
    // packets that happen to finish decoding together. `rule_index` is only
    // updated once, at the end of the whole batch, so checking each op
    // against `rule_index` would let both through, each seeing a world
    // without the other. The guard must check against what `RuleStore` has
    // already accepted earlier in the same batch, not just the pre-batch
    // state.
    let mut grid = make_grid(1, 1);
    let mut out = BoundaryBuffer::new();
    out.direction = "output".to_string();
    grid.set_boundary(0, 0, out);

    let mut engine = Engine::new(grid, HashMap::new());
    engine.enable_guarded_self_modification();

    // Packet 1: id_len=2, id=[10, 11] -> pattern [(0,0,10),(1,0,11)],
    // writes to its neighbor (offset 1,0) — exactly where id=11's own
    // pattern requires a match at its own center.
    // Packet 2: id=[11], writes to itself (offset 0,0) — the same cell
    // packet 1's rule targets, if their centers are adjacent.
    let packet1 = [10u8, 2, 10, 11, 1, 0, 77, 0xFF];
    let packet2 = [10u8, 1, 11, 0, 0, 99, 0xFF];

    let buf = engine.grid_mut().get_boundary_mut(0, 0).unwrap();
    for &b in packet1.iter().chain(packet2.iter()) {
        buf.enqueue(
            0,
            Cell {
                value: CellValue(CellType(b)),
                born_at: 0,
            },
        );
    }
    engine.run_tick();

    assert!(
        engine.rule_index().contains_key(&CellType(10)),
        "the first-processed rule should install"
    );
    assert!(
        !engine.rule_index().contains_key(&CellType(11)),
        "the second rule conflicts with the first (already accepted this same batch) and must be rejected"
    );
    assert_eq!(engine.rejected_self_modifications, 1);
}

#[test]
fn test_age_advancement() {
    let mut grid = make_grid(2, 2);
    grid.set_cell(0, 0, Cell::new(1));

    let rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    let mut engine = Engine::new(grid, rule_index);
    engine.advance_age();

    assert_eq!(engine.grid().get_age(0, 0), 1);
}

#[test]
fn test_reset_age() {
    let mut grid = make_grid(2, 2);
    grid.set_cell(0, 0, Cell::new(1));

    let rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    let mut engine = Engine::new(grid, rule_index);

    // Advance age so born_at < generation
    engine.advance_age();
    engine.advance_age();
    engine.advance_age();
    assert_eq!(engine.grid().get_age(0, 0), 3);

    let region = AffectedRegion {
        x_start: 0,
        x_end: 1,
        y_start: 0,
        y_end: 1,
        has_changes: true,
        written_cells: vec![(0, 0)],
    };

    engine.reset_age(&[region]);

    assert_eq!(engine.grid().get_age(0, 0), 0);
}

#[test]
fn test_detect_termination_stable() {
    let grid = make_grid(2, 2);
    let rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    let engine = Engine::new(grid, rule_index);

    assert_eq!(engine.detect_termination(0), TerminationVerdict::Stable);
}

#[test]
fn test_detect_termination_active() {
    let mut grid = make_grid(2, 2);
    grid.set_cell(0, 0, Cell::new(1));

    let rule = Rule {
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

    let rule_index = make_rule_index(vec![rule]);
    let engine = Engine::new(grid, rule_index);

    assert_eq!(engine.detect_termination(0), TerminationVerdict::Active);
}

#[test]
fn test_apply_match() {
    let mut grid = make_grid(3, 3);
    grid.set_cell(1, 1, Cell::new(5));

    let rule = Rule {
        id: vec![CellType(5)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(9))],
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
    let mut engine = Engine::new(grid, rule_index);

    let matches = engine.detect_matches();
    assert_eq!(matches.len(), 1);
    let accepted = engine.arbitrate(matches);
    assert_eq!(accepted.len(), 1);

    engine.apply_matches(accepted);

    assert_eq!(engine.grid.get_cell(1, 1).unwrap().value, CellValue(CellType(9)));
}

#[test]
fn test_apply_matches_empty() {
    let grid = make_grid(3, 3);
    let rule_index = make_rule_index(vec![]);
    let mut engine = Engine::new(grid, rule_index);
    let (regions, _) = engine.apply_matches(vec![]);
    assert!(regions.is_empty());
}

#[test]
fn test_run_tick_simple() {
    let mut grid = make_grid(3, 3);
    grid.set_cell(1, 1, Cell::new(7));

    let rule = Rule {
        id: vec![CellType(7)],
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

    let rule_index = make_rule_index(vec![rule]);
    let (accepted, _) = run_tick(&mut grid, &rule_index);

    assert_eq!(accepted.len(), 1);
    assert_eq!(grid.get_cell(1, 1).unwrap().value, CellValue(CellType(3)));
}

#[test]
fn test_run_tick_empty_grid() {
    let mut grid = make_grid(3, 3);
    let rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    let (accepted, _) = run_tick(&mut grid, &rule_index);
    assert!(accepted.is_empty());
}

#[test]
fn test_io_boundary() {
    let mut grid = make_grid(8, 1);
    let mut input_buf = BoundaryBuffer::new();
    input_buf.direction = "input".to_string();
    grid.set_boundary(0, 0, input_buf);

    let mut output_buf = BoundaryBuffer::new();
    output_buf.direction = "output".to_string();
    grid.set_boundary(7, 0, output_buf);

    let rule = Rule {
        id: vec![CellType(5)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 1,
            broadcast: false,
            keep_source: false,
        }]],
        changes: vec![(0, 0, ChangeValue::Literal(0))],
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
    let mut engine = Engine::new(grid, rule_index);

    let matches = engine.detect_matches();
    assert!(matches.is_empty(), "No 5 on grid");

    let outputs = engine.pop_output();
    assert!(outputs.is_empty());
}

/// Регрессия: `apply_input` раньше только "подсматривал" (`front()`) в
/// очередь входного буфера, ни разу не вызывая pop — значение, попавшее
/// в очередь первым, применялось к решётке на КАЖДОМ тике бесконечно, а
/// все следующие запушенные значения никогда не доходили до решётки.
/// Это полностью ломало саму идею потокового входа (например, подачи
/// ленты в симуляцию машины Тьюринга по одному символу за тик).
#[test]
fn test_apply_input_consumes_queue() {
    let mut grid = make_grid(5, 1);
    let mut buf = BoundaryBuffer::new();
    buf.direction = "input".to_string();
    grid.set_boundary(0, 0, buf);

    let mut engine = Engine::new(grid, HashMap::new());
    engine.push_input(0, 11);
    engine.push_input(0, 22);
    engine.push_input(0, 33);

    engine.apply_input();
    assert_eq!(
        engine.grid().get_cell(0, 0).unwrap().value.0 .0,
        11,
        "первый тик должен увидеть первое запушенное значение"
    );

    engine.apply_input();
    assert_eq!(
        engine.grid().get_cell(0, 0).unwrap().value.0 .0,
        22,
        "второй тик должен увидеть ВТОРОЕ значение, а не залипнуть на первом"
    );

    engine.apply_input();
    assert_eq!(
        engine.grid().get_cell(0, 0).unwrap().value.0 .0,
        33,
        "третий тик должен увидеть третье значение"
    );

    // Очередь пуста — значение держится (нет новых данных, писать нечего).
    engine.apply_input();
    assert_eq!(
        engine.grid().get_cell(0, 0).unwrap().value.0 .0,
        33,
        "после исчерпания очереди клетка сохраняет последнее значение"
    );
}

#[test]
fn test_2d_pattern_match() {
    // Правило: pattern 3×3 L-образный
    // (0,0,1), (1,0,2), (0,1,3) → меняем на 4,5,6
    let rule = Rule {
        id: vec![CellType(1), CellType(2), CellType(3)],
        pattern: vec![
            (0i8, 0i8, CellType(1)),
            (1i8, 0i8, CellType(2)),
            (0i8, 1i8, CellType(3)),
        ],
        shifts: vec![],
        changes: vec![
            (0, 0, ChangeValue::Literal(4)),
            (1, 0, ChangeValue::Literal(5)),
            (0, 1, ChangeValue::Literal(6)),
        ],
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

    let mut grid = make_grid(3, 3);
    grid.set_cell(0, 0, Cell::new(1));
    grid.set_cell(1, 0, Cell::new(2));
    grid.set_cell(0, 1, Cell::new(3));

    let rule_index = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, rule_index);

    let matches = engine.detect_matches();
    assert_eq!(matches.len(), 1, "должно быть ровно одно совпадение");

    let accepted = engine.arbitrate(matches);
    assert_eq!(accepted.len(), 1, "арбитраж должен пропустить ровно одно");

    engine.apply_matches(accepted);

    // Проверяем изменения
    assert_eq!(
        engine.grid.get_cell(0, 0).unwrap().value,
        CellValue(CellType(4)),
        "ячейка (0,0) должна стать 4"
    );
    assert_eq!(
        engine.grid.get_cell(1, 0).unwrap().value,
        CellValue(CellType(5)),
        "ячейка (1,0) должна стать 5"
    );
    assert_eq!(
        engine.grid.get_cell(0, 1).unwrap().value,
        CellValue(CellType(6)),
        "ячейка (0,1) должна стать 6"
    );
}

/// matcher.rs упаковывает паттерн ≤ 16 клеток в u128 для однокомандного
/// сравнения (было ≤ 8 клеток в u64 — паттерн Game of Life, центр + 8
/// соседей = 9 клеток, уже не влезал и всегда шёл по медленному fallback-
/// циклу). Property-тесты этот диапазон не покрывают (генератор паттернов
/// там ограничен 1-3 клетками) — эти три случая явно проверяют границы:
/// 9 клеток (реальный размер паттерна GoL), 16 (граница u128) и 17 (уже
/// должен уйти в fallback-цикл, а не тихо обрезаться).
#[test]
fn test_pattern_packing_9_16_17_cells() {
    // 9 клеток: полностью живой блок 3×3 должен матчиться по центру, и
    // переставать матчиться, если хоть один сосед отличается.
    let pattern9: Vec<(i8, i8, CellType)> = vec![
        (0, 0, CellType(1)),
        (-1, -1, CellType(1)),
        (0, -1, CellType(1)),
        (1, -1, CellType(1)),
        (-1, 0, CellType(1)),
        (1, 0, CellType(1)),
        (-1, 1, CellType(1)),
        (0, 1, CellType(1)),
        (1, 1, CellType(1)),
    ];
    assert_eq!(pattern9.len(), 9);
    let rule9 = Rule {
        id: vec![CellType(1)],
        pattern: pattern9,
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
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut grid9 = make_grid(5, 5);
    for y in 1..=3 {
        for x in 1..=3 {
            grid9.set_cell(x, y, Cell::new(1));
        }
    }
    let mut engine9 = Engine::new(grid9, make_rule_index(vec![rule9]));
    let matches = engine9.detect_matches();
    assert_eq!(
        matches.len(),
        1,
        "полный 3×3 блок должен дать ровно одно совпадение (9-клеточный паттерн)"
    );
    assert_eq!((matches[0].x, matches[0].y), (2, 2));

    // Ломаем одного соседа — совпадений быть не должно.
    engine9.grid.set_cell(2, 1, Cell::new(2));
    let matches_broken = engine9.detect_matches();
    assert_eq!(
        matches_broken.len(),
        0,
        "с одним отличающимся соседом 9-клеточный паттерн не должен матчиться"
    );

    // 16 клеток — ровно на границе u128-упаковки.
    let mut pattern16: Vec<(i8, i8, CellType)> = vec![(0, 0, CellType(1))];
    for dy in 0..4i8 {
        for dx in 0..4i8 {
            if dx == 0 && dy == 0 {
                continue;
            }
            pattern16.push((dx, dy, CellType(1)));
        }
    }
    assert_eq!(pattern16.len(), 16);
    let rule16 = Rule {
        id: vec![CellType(1)],
        pattern: pattern16,
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
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut grid16 = make_grid(6, 6);
    for y in 0..4 {
        for x in 0..4 {
            grid16.set_cell(x, y, Cell::new(1));
        }
    }
    let engine16 = Engine::new(grid16, make_rule_index(vec![rule16]));
    let matches16 = engine16.detect_matches();
    assert_eq!(
        matches16.len(),
        1,
        "полный 4×4 блок должен дать ровно одно совпадение (16-клеточный паттерн, граница u128)"
    );
    assert_eq!((matches16[0].x, matches16[0].y), (0, 0));

    // 17 клеток — уже за пределом u128-упаковки, должен пойти по
    // fallback-циклу (не быть молча отброшенным/некорректно обрезанным).
    let mut pattern17 = pattern16_from_scratch();
    pattern17.push((4, 0, CellType(0)));
    assert_eq!(pattern17.len(), 17);
    let rule17 = Rule {
        id: vec![CellType(1)],
        pattern: pattern17,
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
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut grid17 = make_grid(7, 6);
    for y in 0..4 {
        for x in 0..4 {
            grid17.set_cell(x, y, Cell::new(1));
        }
    }
    let engine17 = Engine::new(grid17, make_rule_index(vec![rule17]));
    let matches17 = engine17.detect_matches();
    assert_eq!(
        matches17.len(),
        1,
        "17-клеточный паттерн (fallback-путь) должен корректно матчиться"
    );
    assert_eq!((matches17[0].x, matches17[0].y), (0, 0));
}

fn pattern16_from_scratch() -> Vec<(i8, i8, CellType)> {
    let mut pattern: Vec<(i8, i8, CellType)> = vec![(0, 0, CellType(1))];
    for dy in 0..4i8 {
        for dx in 0..4i8 {
            if dx == 0 && dy == 0 {
                continue;
            }
            pattern.push((dx, dy, CellType(1)));
        }
    }
    pattern
}

#[test]
fn test_nondeterministic_same_priority() {
    let mut grid = make_grid(8, 1);
    grid.set_cell(1, 0, Cell::new(1));
    grid.set_cell(2, 0, Cell::new(2));

    let rule_a = Rule {
        id: vec![CellType(1), CellType(2)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 1,
            broadcast: false,
            keep_source: false,
        }]],
        changes: vec![(0, 0, ChangeValue::Literal(5)), (1, 0, ChangeValue::Literal(5))],
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

    let rule_b = Rule {
        id: vec![CellType(1), CellType(2)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Left,
            steps: 1,
            broadcast: false,
            keep_source: false,
        }]],
        changes: vec![(0, 0, ChangeValue::Literal(5)), (1, 0, ChangeValue::Literal(5))],
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

    let rule_index = make_rule_index(vec![rule_a, rule_b]);
    let engine = Engine::new(grid, rule_index);

    let matches = engine.detect_matches();
    assert_eq!(matches.len(), 2, "two rules match the same cells");

    let accepted = engine.arbitrate(matches);
    assert_eq!(accepted.len(), 1, "only one should be accepted");
}

/// Регрессия: при совпадающем `id` у двух правил с разным `min_age`
/// должно применяться то правило, что реально сработало (прошло проверку
/// min_age), а не первое по приоритету правило с тем же id — даже если
/// оно вообще не совпало для данной ячейки.
///
/// До фикса `apply_matches`/`RuleDataCache` резолвили правило поиском
/// по одному лишь `id` (`rules.iter().find(|r| r.id == m.rule_id)`),
/// что для правил с общим id всегда возвращало первый по приоритету
/// вариант — независимо от того, какое именно правило породило match.
#[test]
fn test_same_id_resolves_actually_matched_rule() {
    let mut grid = make_grid(3, 1);
    grid.set_cell(1, 0, Cell::new(5));

    // Выше приоритет → после сортировки в rule_index идёт первым,
    // но min_age = 100 не даёт ему сработать для свежей ячейки (age 0).
    let rule_hi = Rule {
        id: vec![CellType(5)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 1,
            broadcast: false,
            keep_source: false,
        }]],
        changes: vec![],
        active_only: false,
        priority: 20,
        min_age: 100,
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

    // Ниже приоритет, второй в отсортированном Vec, но именно оно
    // реально совпадает: min_age = 0.
    let rule_lo = Rule {
        id: vec![CellType(5)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Left,
            steps: 1,
            broadcast: false,
            keep_source: false,
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
    };

    let rule_index = make_rule_index(vec![rule_hi, rule_lo]);
    let mut engine = Engine::new(grid, rule_index);

    let matches = engine.detect_matches();
    assert_eq!(matches.len(), 1, "только rule_lo проходит проверку min_age");

    let accepted = engine.arbitrate(matches);
    assert_eq!(accepted.len(), 1);

    engine.apply_matches(accepted);

    assert_eq!(
        engine.grid.get_cell(0, 0).unwrap().value,
        CellValue(CellType(5)),
        "должен применяться Left-сдвиг rule_lo (реально сработавшего), а не Right-сдвиг rule_hi"
    );
    assert_eq!(
        engine.grid.get_cell(1, 0).unwrap().value,
        CellValue::default(),
        "исходная позиция головки должна очиститься"
    );
    assert_eq!(
        engine.grid.get_cell(2, 0).unwrap().value,
        CellValue::default(),
        "rule_hi не должно было применяться вовсе"
    );
}
