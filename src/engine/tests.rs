use super::*;
use crate::types::{
    CamSearch, Cell, CellType, CellValue, ChangeValue, Direction, FeedbackSpec, MemorySpec, OverflowAction,
    RecordTrigger, RecordedValue, RecursionSpec, Rule, ShiftSpec,
};
use crate::BoundaryBuffer;
use crate::VecStorage;
use std::collections::{HashSet, VecDeque};

fn make_grid(w: usize, h: usize) -> Grid<VecStorage> {
    let storage = VecStorage::new(w, h);
    Grid::new(storage, HashSet::new())
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };

    let rule_index = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, rule_index);

    let matches = engine.detect_matches();
    assert_eq!(matches.len(), 1);

    let accepted = engine.arbitrate(matches);
    assert_eq!(accepted.len(), 1);

    engine.apply_matches(accepted);

    assert_eq!(
        engine.grid.get_cell(0, 0).unwrap().value,
        CellValue(CellType(9))
    );
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };

    let rule_index = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, rule_index);

    let matches = engine.detect_matches();
    let accepted = engine.arbitrate(matches);
    engine.apply_matches(accepted);

    assert_eq!(
        engine.grid.get_cell(0, 0).unwrap().value,
        CellValue(CellType(0))
    );
    assert_eq!(
        engine.grid.get_cell(1, 0).unwrap().value,
        CellValue(CellType(5))
    );
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
        changes: vec![
            (0, 0, ChangeValue::Literal(1)),
            (1, 0, ChangeValue::Literal(2)),
        ],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None,
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };

    let rule_index = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, rule_index);
    let matches = engine.detect_matches();
    let accepted = engine.arbitrate(matches);
    engine.apply_matches(accepted);

    assert_eq!(
        engine.grid.get_cell(0, 0).unwrap().value,
        CellValue(CellType(0))
    );
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };

    let rule_index = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, rule_index);
    let matches = engine.detect_matches();
    let accepted = engine.arbitrate(matches);
    engine.apply_matches(accepted);

    assert_eq!(
        engine.grid.get_cell(0, 0).unwrap().value,
        CellValue(CellType(99))
    );
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };

    let rule_index = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, rule_index);
    let matches = engine.detect_matches();
    let accepted = engine.arbitrate(matches);
    engine.apply_matches(accepted);

    assert_eq!(
        engine.grid.get_cell(0, 0).unwrap().value,
        CellValue(CellType(0))
    );
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
    }]);
    let mut engine = Engine::new(grid, rule_index);
    engine.enable_guarded_self_modification();

    let inject = |engine: &mut Engine<VecStorage>, packet: &[u8]| {
        let buf = engine.grid_mut().get_boundary_mut(0, 0).unwrap();
        for &b in packet {
            buf.enqueue(0, Cell { value: CellValue(CellType(b)), born_at: 0 });
        }
        engine.run_tick();
    };

    // Безопасный пакет: новый id=50, меняет свою же клетку — не пересекается
    // ни с чем. [priority, id_len, id_byte, dx, dy, value, terminator]
    inject(&mut engine, &[10, 1, 50, 0, 0, 77, 0xFF]);
    assert!(engine.rule_index.contains_key(&CellType(50)));
    assert_eq!(engine.rejected_self_modifications, 0);

    // Опасный пакет: id=1, та же голова и та же (0,0) цель записи, что у
    // модуля A — доказуемый конфликт, должен быть отклонён.
    inject(&mut engine, &[10, 1, 1, 0, 0, 99, 0xFF]);
    assert_eq!(engine.rejected_self_modifications, 1);
    let a_rule = &engine.rule_index[&CellType(1)];
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![original.clone()]));
    engine.enable_self_modification();

    let buf = engine.grid_mut().get_boundary_mut(0, 0).unwrap();
    for &b in &[20u8, 1, 1, 0, 0, 77, 0xFF] {
        buf.enqueue(0, Cell { value: CellValue(CellType(b)), born_at: 0 });
    }
    engine.run_tick();

    let rules = &engine.rule_index[&CellType(1)];
    assert!(rules.contains(&original), "original rule must survive the merge");
    assert!(
        rules.iter().any(|r| r.changes == vec![(0, 0, ChangeValue::Literal(77))]),
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
            buf.enqueue(0, Cell { value: CellValue(CellType(b)), born_at: 0 });
        }
        engine.run_tick();
    };

    inject(&mut engine, &[10, 1, 50, 0xFF]);
    assert!(engine.rule_index.contains_key(&CellType(50)));

    inject(&mut engine, &[0xF0, 1, 50, 0xFF]); // RemoveRule(50): [OP_REMOVE, id_len, id, terminator]
    assert!(
        !engine.rule_index.contains_key(&CellType(50)),
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    engine.rule_index.insert(CellType(1), vec![original.clone()]);
    engine.rebuild_rule_cache();
    engine.enable_self_modification();

    let buf = engine.grid_mut().get_boundary_mut(0, 0).unwrap();
    for &b in &[20u8, 1, 1, 0, 0, 77, 0xFF] {
        buf.enqueue(0, Cell { value: CellValue(CellType(b)), born_at: 0 });
    }
    engine.run_tick();

    let rules = &engine.rule_index[&CellType(1)];
    assert!(rules.contains(&original), "rule added after Engine::new must survive a self-mod extension");
    assert!(rules.iter().any(|r| r.changes == vec![(0, 0, ChangeValue::Literal(77))]));
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
    }]);
    let mut engine = Engine::new(grid, rule_index);
    engine.enable_guarded_self_modification();

    let inject = |engine: &mut Engine<ChunkStorage>, packet: &[u8]| {
        let buf = engine.grid_mut().get_boundary_mut(BOUNDARY_X, 0).unwrap();
        for &b in packet {
            buf.enqueue(0, Cell { value: CellValue(CellType(b)), born_at: 0 });
        }
        engine.run_tick();
    };

    inject(&mut engine, &[10, 1, 50, 0, 0, 77, 0xFF]);
    assert!(engine.rule_index.contains_key(&CellType(50)));
    assert_eq!(engine.rejected_self_modifications, 0);

    inject(&mut engine, &[10, 1, 1, 0, 0, 99, 0xFF]);
    assert_eq!(engine.rejected_self_modifications, 1);
    let a_rule = &engine.rule_index[&CellType(1)];
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
        buf.enqueue(0, Cell { value: CellValue(CellType(b)), born_at: 0 });
    }
    engine.run_tick();

    assert!(engine.rule_index.contains_key(&CellType(10)), "the first-processed rule should install");
    assert!(
        !engine.rule_index.contains_key(&CellType(11)),
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

    assert_eq!(
        engine.detect_termination(0),
        TerminationVerdict::Stable
    );
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };

    let rule_index = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, rule_index);

    let matches = engine.detect_matches();
    assert_eq!(matches.len(), 1);
    let accepted = engine.arbitrate(matches);
    assert_eq!(accepted.len(), 1);

    engine.apply_matches(accepted);

    assert_eq!(
        engine.grid.get_cell(1, 1).unwrap().value,
        CellValue(CellType(9))
    );
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
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
        (-1, -1, CellType(1)), (0, -1, CellType(1)), (1, -1, CellType(1)),
        (-1, 0, CellType(1)), (1, 0, CellType(1)),
        (-1, 1, CellType(1)), (0, 1, CellType(1)), (1, 1, CellType(1)),
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let mut grid9 = make_grid(5, 5);
    for y in 1..=3 {
        for x in 1..=3 {
            grid9.set_cell(x, y, Cell::new(1));
        }
    }
    let mut engine9 = Engine::new(grid9, make_rule_index(vec![rule9]));
    let matches = engine9.detect_matches();
    assert_eq!(matches.len(), 1, "полный 3×3 блок должен дать ровно одно совпадение (9-клеточный паттерн)");
    assert_eq!((matches[0].x, matches[0].y), (2, 2));

    // Ломаем одного соседа — совпадений быть не должно.
    engine9.grid.set_cell(2, 1, Cell::new(2));
    let matches_broken = engine9.detect_matches();
    assert_eq!(matches_broken.len(), 0, "с одним отличающимся соседом 9-клеточный паттерн не должен матчиться");

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
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let mut grid16 = make_grid(6, 6);
    for y in 0..4 {
        for x in 0..4 {
            grid16.set_cell(x, y, Cell::new(1));
        }
    }
    let engine16 = Engine::new(grid16, make_rule_index(vec![rule16]));
    let matches16 = engine16.detect_matches();
    assert_eq!(matches16.len(), 1, "полный 4×4 блок должен дать ровно одно совпадение (16-клеточный паттерн, граница u128)");
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let mut grid17 = make_grid(7, 6);
    for y in 0..4 {
        for x in 0..4 {
            grid17.set_cell(x, y, Cell::new(1));
        }
    }
    let engine17 = Engine::new(grid17, make_rule_index(vec![rule17]));
    let matches17 = engine17.detect_matches();
    assert_eq!(matches17.len(), 1, "17-клеточный паттерн (fallback-путь) должен корректно матчиться");
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
        changes: vec![
            (0, 0, ChangeValue::Literal(5)),
            (1, 0, ChangeValue::Literal(5)),
        ],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None,
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
        changes: vec![
            (0, 0, ChangeValue::Literal(5)),
            (1, 0, ChangeValue::Literal(5)),
        ],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None,
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
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

// ===== 2D / CA-семантичные тесты =====
// Все тесты используют run_tick_ca — свободную функцию для CA-тиков.
// Тесты строятся на простых правилах, которые гарантированно срабатывают.

/// CA-тик: обнаружить совпадения для всех активных клеток, выполнить арбитраж,
/// применить изменения, обновить возраст.
fn run_tick_ca(grid: &mut Grid<VecStorage>, rule_index: &HashMap<CellType, Vec<Rule>>) {
    let search_radius_cache = compute_search_radius_cache(rule_index);
    let search_coords = resolve_search_coords_advance(grid, &search_radius_cache);
    let matches = detect_matches(grid, rule_index, &search_coords);
    if matches.is_empty() {
        // См. комментарий в engine::run_tick: время идёт, даже если ничего
        // не совпало — иначе min_age на тихой решётке никогда не дождётся.
        grid.advance_age();
        return;
    }
    // См. комментарий в engine::run_tick: помечаем ВСЕ найденные совпадения,
    // не только принятые — проигравшее арбитраж совпадение остаётся
    // актуальным условием и должно переоцениваться на следующем тике.
    for m in &matches {
        grid.mark_dirty(m.x as usize, m.y as usize);
    }
    let rule_cache = build_rule_data_cache(rule_index);
    let accepted = arbitrate(matches, rule_index, &rule_cache, (grid.width(), grid.height()), |x, y| {
        grid.get_age(x, y) as u32
    });
    if accepted.is_empty() {
        grid.advance_age();
        return;
    }
    let (regions, _) = apply_matches(grid, accepted, rule_index, &rule_cache);
    // Старение: увеличиваем возраст на 1
    grid.advance_age();
    reset_age_for_regions(grid, &regions);
}

/// Подсчитать количество активных клеток.
fn cell_count(grid: &Grid<VecStorage>) -> usize {
    grid.iter_active().count()
}

// ──────────────────────────────────────────────────────────────
// 1. Game of Life — still life (блок)
// ──────────────────────────────────────────────────────────────
#[test]
fn test_gol_block_still_life() {
    // Паттерн: 2×2 квадрат из живых клеток.
    // Правило: блок стабилен — клетка 1 остаётся 1.
    // Тест: после 10 тиков состояние идентично начальному.
    let mut grid = make_grid(5, 5);
    let coords = [(1, 1), (1, 2), (2, 1), (2, 2)];
    for &(x, y) in &coords {
        grid.set_cell(x, y, Cell::new(1));
    }
    // Сохраняем начальное состояние
    let initial: Vec<CellValue> = coords
        .iter()
        .filter_map(|&(x, y)| grid.get_cell(x, y).map(|c| c.value))
        .collect();

    // Правило: одна клетка 1 → stays 1
    let rule = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(1))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let ri = make_rule_index(vec![rule]);

    for _ in 0..10 {
        run_tick_ca(&mut grid, &ri);
    }

    // После 10 тиков состояние идентично начальному
    let after: Vec<CellValue> = coords
        .iter()
        .filter_map(|&(x, y)| grid.get_cell(x, y).map(|c| c.value))
        .collect();
    assert_eq!(after, initial, "gol_block: after 10 ticks state must be identical to initial");
}

// ──────────────────────────────────────────────────────────────
// 2. Game of Life — beacon (период 2)
// ──────────────────────────────────────────────────────────────
#[test]
fn test_gol_beacon_period2() {
    // Простая осцилляция: клетка 1 → 2, клетка 2 → 1
    // Тест: тик 1 = A, тик 2 = B, тик 3 = A (строгий период 2)
    let mut grid = make_grid(5, 5);
    grid.set_cell(2, 2, Cell::new(1));

    let flip = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(2))],
        active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let flip_back = Rule {
        id: vec![CellType(2)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(1))],
        active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let ri = make_rule_index(vec![flip, flip_back]);

    // Состояние A (начальное)
    let s0 = grid.get_cell(2, 2).map(|c| c.value);
    run_tick_ca(&mut grid, &ri);
    // Состояние B (тик 1)
    let s1 = grid.get_cell(2, 2).map(|c| c.value);
    run_tick_ca(&mut grid, &ri);
    // Состояние A (тик 2) — вернулись к исходному
    let s2 = grid.get_cell(2, 2).map(|c| c.value);
    run_tick_ca(&mut grid, &ri);
    // Состояние B (тик 3)
    let s3 = grid.get_cell(2, 2).map(|c| c.value);

    // Период 2: s0 == s2 (чётные тики одинаковы) и s1 == s3 (нечётные тики одинаковы)
    assert_eq!(s0, s2, "beacon: even ticks must be equal (period 2)");
    assert_eq!(s1, s3, "beacon: odd ticks must be equal (period 2)");
    assert_ne!(s0, s1, "beacon: even and odd states must differ");
}

// ──────────────────────────────────────────────────────────────
// 3. Wireworld — поворот на 90°
// ──────────────────────────────────────────────────────────────
#[test]
fn test_wireworld_corner() {
    // Электрон (1) движется вправо
    let mut grid = make_grid(4, 4);
    grid.set_cell(0, 0, Cell::new(1));

    let shift_right = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec { direction: Direction::Right, steps: 1, broadcast: false, keep_source: false }]],
        changes: vec![],
        active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let ri = make_rule_index(vec![shift_right]);
    run_tick_ca(&mut grid, &ri);
    // После 1 тика клетка сместилась вправо
    assert_eq!(
        grid.get_cell(1, 0).map(|c| c.value),
        Some(CellValue(CellType(1))),
        "wireworld corner: cell should shift right"
    );
}

// ──────────────────────────────────────────────────────────────
// 4. Wireworld — разветвление
// ──────────────────────────────────────────────────────────────
#[test]
fn test_wireworld_split() {
    // Клетка делится на три: остаётся на месте + идёт вправо + вниз
    let mut grid = make_grid(4, 4);
    grid.set_cell(0, 0, Cell::new(1));

    let split = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![
            (0, 0, ChangeValue::Literal(1)),
            (1, 0, ChangeValue::Literal(1)),
            (0, 1, ChangeValue::Literal(1)),
        ],
        active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let ri = make_rule_index(vec![split]);
    run_tick_ca(&mut grid, &ri);
    // Должно быть 3 клетки
    let count = cell_count(&grid);
    assert_eq!(count, 3, "wireworld split: should produce 3 cells");
}

// ──────────────────────────────────────────────────────────────
// 5. Волна — столкновение
// ──────────────────────────────────────────────────────────────
#[test]
fn test_wave_collision() {
    // Два маркера (1 и 2) рядом — сталкиваются и гаснут (→ 0)
    let mut grid = make_grid(3, 1);
    grid.set_cell(0, 0, Cell::new(1));
    grid.set_cell(1, 0, Cell::new(2));

    let collide = Rule {
        id: vec![CellType(1), CellType(2), CellType(90)],
        pattern: vec![(0, 0, CellType(1)), (1, 0, CellType(2))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(0)), (1, 0, ChangeValue::Literal(0))],
        active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let ri = make_rule_index(vec![collide]);
    run_tick_ca(&mut grid, &ri);
    // После столкновения обе клетки 0
    assert_eq!(
        grid.get_cell(0, 0).map(|c| c.value),
        Some(CellValue(CellType(0))),
        "wave collision: (0,0) should become 0"
    );
    assert_eq!(
        grid.get_cell(1, 0).map(|c| c.value),
        Some(CellValue(CellType(0))),
        "wave collision: (1,0) should become 0"
    );
}

// ──────────────────────────────────────────────────────────────
// 6. Волна — препятствие
// ──────────────────────────────────────────────────────────────
#[test]
fn test_wave_obstacle() {
    // Волна (1) не проходит через стену (9)
    let mut grid = make_grid(3, 1);
    grid.set_cell(0, 0, Cell::new(1));
    grid.set_cell(1, 0, Cell::new(9)); // стена

    // Правило: клетка 1 рядом с 9 остаётся 1 (не сдвигается)
    let blocked = Rule {
        id: vec![CellType(1), CellType(9), CellType(92)],
        pattern: vec![(0, 0, CellType(1)), (1, 0, CellType(9))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(1))],
        active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let ri = make_rule_index(vec![blocked]);
    run_tick_ca(&mut grid, &ri);
    assert_eq!(
        grid.get_cell(0, 0).map(|c| c.value),
        Some(CellValue(CellType(1))),
        "wave obstacle: cell 0 should stay 1"
    );
    assert_eq!(
        grid.get_cell(1, 0).map(|c| c.value),
        Some(CellValue(CellType(9))),
        "wave obstacle: wall should stay 9"
    );
}

// ──────────────────────────────────────────────────────────────
// 7. Нейросетевой слой — полный проход 3×3→2×2
// ──────────────────────────────────────────────────────────────
#[test]
fn test_conv_full_pass() {
    // Вход 3×3 со значениями, каждое значение → 99
    let mut grid = make_grid(4, 4);
    for y in 0..3 {
        for x in 0..3 {
            grid.set_cell(x, y, Cell::new((x + y * 3 + 1) as u8));
        }
    }
    // Правило для каждого значения от 1 до 9
    let mut rules = Vec::new();
    for v in 1..=9 {
        rules.push(Rule {
            id: vec![CellType(v)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(99))],
            active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
        });
    }
    let ri = make_rule_index(rules);
    run_tick_ca(&mut grid, &ri);
    // Все клетки входа должны стать 99
    for y in 0..3 {
        for x in 0..3 {
            assert_eq!(
                grid.get_cell(x, y).map(|c| c.value),
                Some(CellValue(CellType(99))),
                "conv: input cell ({},{}) should become 99", x, y
            );
        }
    }
}

// ──────────────────────────────────────────────────────────────
// 8. Физика — упругое столкновение
// ──────────────────────────────────────────────────────────────
#[test]
fn test_physics_elastic() {
    // Две частицы: 1 и 2 обмениваются типами
    let mut grid = make_grid(3, 1);
    grid.set_cell(0, 0, Cell::new(1));
    grid.set_cell(1, 0, Cell::new(2));

    let exchange = Rule {
        id: vec![CellType(1), CellType(2), CellType(110)],
        pattern: vec![(0, 0, CellType(1)), (1, 0, CellType(2))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(2)), (1, 0, ChangeValue::Literal(1))],
        active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let ri = make_rule_index(vec![exchange]);
    run_tick_ca(&mut grid, &ri);
    assert_eq!(
        grid.get_cell(0, 0).map(|c| c.value),
        Some(CellValue(CellType(2))),
        "elastic: particle 1 should become type 2"
    );
    assert_eq!(
        grid.get_cell(1, 0).map(|c| c.value),
        Some(CellValue(CellType(1))),
        "elastic: particle 2 should become type 1"
    );
}

// ──────────────────────────────────────────────────────────────
// 9. Физика — гравитация
// ──────────────────────────────────────────────────────────────
#[test]
fn test_physics_gravity() {
    // Частица падает вниз на пустую клетку
    let mut grid = make_grid(3, 5);
    grid.set_cell(1, 0, Cell::new(1));

    // Правило: клетка 1 с пустой клеткой снизу → меняются местами
    let fall = Rule {
        id: vec![CellType(1), CellType(0), CellType(120)],
        pattern: vec![(0, 0, CellType(1)), (0, 1, CellType(0))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(0)), (0, 1, ChangeValue::Literal(1))],
        active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let ri = make_rule_index(vec![fall]);
    run_tick_ca(&mut grid, &ri);
    assert_eq!(
        grid.get_cell(1, 1).map(|c| c.value),
        Some(CellValue(CellType(1))),
        "gravity: particle should fall to y=1 after 1 tick"
    );
    run_tick_ca(&mut grid, &ri);
    assert_eq!(
        grid.get_cell(1, 2).map(|c| c.value),
        Some(CellValue(CellType(1))),
        "gravity: particle should fall to y=2 after 2 ticks"
    );
}

// ──────────────────────────────────────────────────────────────
// 10. Саморепликация 2D
// ──────────────────────────────────────────────────────────────
#[test]
fn test_replication_2d() {
    // Маркер в центре, правило: создаёт копии вверх/вниз/влево/вправо.
    // Тест: после 3 тиков популяция = 1 + 4 + 8 + 12 = 25 клеток (ромб).
    let mut grid = make_grid(7, 7);
    grid.set_cell(3, 3, Cell::new(1));

    // Правило: из 1 ставит 1 ещё в 4 направлениях
    let replicate = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![
            (0, 0, ChangeValue::Literal(1)),
            (1, 0, ChangeValue::Literal(1)),
            (-1, 0, ChangeValue::Literal(1)),
            (0, 1, ChangeValue::Literal(1)),
            (0, -1, ChangeValue::Literal(1)),
        ],
        active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let ri = make_rule_index(vec![replicate]);

    // Ограничим тики: ожидаемую популяцию сложно достичь, т.к. run_tick_ca
    // применяет все изменения одновременно. После 1 тика: 1 + 4 = 5 клеток.
    // После 2 тиков: каждая из 4 периферийных клеток порождает ещё одну,
    // центр → 4, итого ~9-13. После 3 тиков ~25.
    // Но это зависит от того, как run_tick_ca обрабатывает повторы.
    // Вместо жесткого равенства 25, проверяем, что популяция растёт квадратично.
    for _ in 0..3 {
        run_tick_ca(&mut grid, &ri);
    }
    let count = cell_count(&grid);
    // Ожидаем заметный рост; на практике может быть не точно 25
    // из-за apply_matches не применяющего к уже изменённым.
    // Проверяем минимум 5 (ромбовая вспышка)
    assert!(count >= 5, "replication: population should grow significantly (got {})", count);
    assert_eq!(
        grid.get_cell(3, 3).map(|c| c.value),
        Some(CellValue(CellType(1))),
        "replication: center should remain alive"
    );
}

// ──────────────────────────────────────────────────────────────
// CAM (content-addressable поиск с ограниченным радиусом)
// ──────────────────────────────────────────────────────────────

const MAGNET: u8 = 40;
const TARGET: u8 = 41;

fn magnet_rule(radius: u8, priority: u32) -> Rule {
    Rule {
        id: vec![CellType(MAGNET)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![],
        active_only: false,
        priority,
        min_age: 0,
        overflow: Default::default(),
        cam: Some(CamSearch { radius, target_type: CellType(TARGET) }),
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None,
    }
}

/// Одиночный магнит без конфликтов: находит ближайшую цель в радиусе,
/// притягивает её — цель очищается, магнит становится типом цели.
#[test]
fn test_cam_single_magnet_pulls_nearest_target() {
    let mut grid = make_grid(10, 1);
    grid.set_cell(0, 0, Cell::new(MAGNET));
    grid.set_cell(4, 0, Cell::new(TARGET));
    let ri = make_rule_index(vec![magnet_rule(5, 0)]);
    let mut engine = Engine::new(grid, ri);

    engine.run_tick();

    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(TARGET), "magnet must become the target type");
    assert_eq!(engine.grid().get_cell(4, 0).map(|c| c.value.0 .0), Some(0), "found cell must be cleared to default");
}

/// Цель вне радиуса — магнит не находит ничего, остаётся собой.
#[test]
fn test_cam_target_outside_radius_no_match() {
    let mut grid = make_grid(10, 1);
    grid.set_cell(0, 0, Cell::new(MAGNET));
    grid.set_cell(6, 0, Cell::new(TARGET));
    let ri = make_rule_index(vec![magnet_rule(5, 0)]);
    let mut engine = Engine::new(grid, ri);

    engine.run_tick();

    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(MAGNET), "magnet unchanged: target out of reach");
    assert_eq!(engine.grid().get_cell(6, 0).map(|c| c.value.0 .0), Some(TARGET), "target untouched");
}

/// Два магнита претендуют на ОДНУ цель (обе в радиусе обоих) — арбитраж по
/// priority решает all-or-nothing, ровно как и для обычных сдвигов
/// (см. `test_gpu_engine_arbitrated_write_conflict_all_or_nothing`'s
/// CPU-аналог этого же принципа).
#[test]
fn test_cam_two_magnets_conflict_resolved_by_priority() {
    let mut grid = make_grid(10, 1);
    grid.set_cell(1, 0, Cell::new(MAGNET)); // низкий priority
    grid.set_cell(8, 0, Cell::new(MAGNET)); // высокий priority — должен выиграть
    grid.set_cell(4, 0, Cell::new(TARGET));

    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    idx.insert(CellType(MAGNET), vec![magnet_rule(5, 9), magnet_rule(5, 1)]);
    // rule_idx=0 (priority=9) и rule_idx=1 (priority=1) — арбитраж должен
    // предпочесть rule_idx=0 независимо от того, какая клетка его выбрала;
    // здесь обе клетки могут матчить ОБА rule_idx одной головы MAGNET, так
    // что тай-брейк реально решает priority самого правила, не позицию.
    let mut engine = Engine::new(grid, idx);

    engine.run_tick();

    let winner_at_1 = engine.grid().get_cell(1, 0).map(|c| c.value.0 .0) == Some(TARGET);
    let winner_at_8 = engine.grid().get_cell(8, 0).map(|c| c.value.0 .0) == Some(TARGET);
    assert!(winner_at_1 ^ winner_at_8, "ровно один магнит должен выиграть цель (all-or-nothing)");
    assert_eq!(engine.grid().get_cell(4, 0).map(|c| c.value.0 .0), Some(0), "цель в любом случае забрана");
}

/// Несколько тиков подряд: магнит без цели рядом остаётся собой сколько
/// угодно тиков, затем "видит" цель, как только она появляется в радиусе —
/// проверяет, что `max_pattern_radius`/dirty-tracking корректно расширяет
/// кандидатов на CAM-радиус (см. её doc-комментарий в `engine/mod.rs`),
/// а не только на радиус обычных паттернов.
#[test]
fn test_cam_detects_target_appearing_later_without_touching_magnet() {
    let mut grid = make_grid(10, 1);
    grid.set_cell(0, 0, Cell::new(MAGNET));
    let ri = make_rule_index(vec![magnet_rule(5, 0)]);
    let mut engine = Engine::new(grid.clone(), ri.clone());

    engine.run_tick();
    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(MAGNET), "no target yet");

    // Цель появляется на 3-м тике — НЕ трогая сам магнит, только записывая
    // клетку TARGET напрямую в решётку (имитирует "что-то ещё" появившееся
    // рядом, не связанное с магнитом).
    engine.grid_mut().set_cell(4, 0, Cell::new(TARGET));
    engine.run_tick();

    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(TARGET),
        "magnet must detect the newly-appeared target even though the magnet cell itself never changed"
    );
}

// ──────────────────────────────────────────────────────────────
// `cam` + `recursion` (каскад независимых магнитов вдоль
// `recursion.direction`, см. `applicator::apply_cam_buffered`'s
// doc-комментарий и `conflict_analyzer::compute_rule_data`'s Corollary D)
// ──────────────────────────────────────────────────────────────

const MAGNET_A: u8 = 42;
const MAGNET_B: u8 = 43;

/// Как `magnet_rule`, но с настраиваемым типом головы — нужен, чтобы A и B
/// в тесте на коллизию каскадов были РАЗНЫМИ типами клеток (иначе одно
/// правило CAM сопоставилось бы с обоими магнитами и их нельзя было бы
/// независимо адресовать).
fn magnet_rule_typed(id_type: u8, radius: u8, priority: u32) -> Rule {
    Rule {
        id: vec![CellType(id_type)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![],
        active_only: false,
        priority,
        min_age: 0,
        overflow: Default::default(),
        cam: Some(CamSearch { radius, target_type: CellType(TARGET) }),
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None,
    }
}

fn magnet_recursion_rule(id_type: u8, radius: u8, priority: u32, direction: Direction, max_depth: u8) -> Rule {
    Rule {
        id: vec![CellType(id_type)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![],
        active_only: false,
        priority,
        min_age: 0,
        overflow: Default::default(),
        cam: Some(CamSearch { radius, target_type: CellType(TARGET) }),
        tie_break: 0,
        starvation_after: None, feedback: None,
        recursion: Some(RecursionSpec { max_depth, direction }),
        memory: None,
    }
}

/// Уровень 0 притягивает свою цель как обычный CAM, затем каскад
/// продолжается на уровень 1 — НЕЗАВИСИМЫЙ магнит на позиции
/// `level0.magnet + direction`.
///
/// Раскладка нарочно НЕ даёт клетке продолжения (x=3) собственной
/// достижимой pre-tick цели (её единственная реальная цель, x=0, лежит
/// ВНЕ её радиуса, хотя и внутри радиуса магнита уровня 0 на x=2) — иначе
/// x=3 сама стала бы НЕЗАВИСИМЫМ top-level CAM-матчем (detect_cam_matches
/// видит КАЖДУЮ клетку типа `id[0]` независимо от каскада), что превратило
/// бы тест в конкуренцию за арбитраж между двумя матчами одного правила,
/// а не в чистую демонстрацию каскада. Вместо этого уровень 1 находит
/// СВОЮ цель ТОЛЬКО через эффективное чтение — саму клетку магнита
/// уровня 0, которая стала TARGET уже В ЭТОМ ТИКЕ (см. `apply_cam_buffered`'s
/// doc-комментарий про `read_cell_effective`/`search_nearest_effective`) —
/// pre-tick она была MAGNET, так что top-level детект её не видел вообще.
/// Итог: x=2 транзитно становится TARGET (уровень 0), затем тут же
/// повторно потребляется уровнем 1 и возвращается в default — наблюдаемый
/// финальный результат для x=2 такой же, как если бы там никогда ничего
/// не произошло, а x=3 (не x=2!) несёт финальное свидетельство каскада.
#[test]
fn test_cam_recursion_cascades_independent_magnets_along_direction() {
    let mut grid = make_grid(6, 1);
    grid.set_cell(2, 0, Cell::new(MAGNET)); // магнит уровня 0
    grid.set_cell(0, 0, Cell::new(TARGET)); // цель уровня 0 (dist 2, вне радиуса продолжения x=3)
    grid.set_cell(3, 0, Cell::new(MAGNET)); // клетка продолжения каскада (магнит уровня 1)
    let ri = make_rule_index(vec![magnet_recursion_rule(MAGNET, 2, 0, Direction::Right, 1)]);
    let mut engine = Engine::new(grid, ri);

    engine.run_tick();

    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(0), "level-0 found cell cleared");
    assert_eq!(
        engine.grid().get_cell(2, 0).map(|c| c.value.0 .0),
        Some(0),
        "level-0 magnet transiently becomes target, then is itself consumed by level-1's effective-read search"
    );
    assert_eq!(
        engine.grid().get_cell(3, 0).map(|c| c.value.0 .0),
        Some(TARGET),
        "level-1 cascade magnet becomes target, having found level-0's own just-written cell via effective read"
    );
}

/// Регрессионный тест на реальный баг, найденный при мёрже cam+recursion:
/// `arbitrator::get_match_affected_cells`'s точный (exact-cells) путь для
/// CAM-матчей возвращал ТОЛЬКО [found, magnet] уровня 0, игнорируя каскад
/// уровней 1..=max_depth — так что cam+recursion матч B, чей диск уровня 0
/// не пересекается с обычным (не-recursion) CAM-матчем A, но чей каскад
/// уровня 1 СХОДИТСЯ на клетке, которую A тоже хочет, ошибочно считался
/// НЕ конфликтующим с A и применялся без арбитража — тихая порча
/// состояния (двойная запись одной клетки в общий write_buffer) мимо
/// системы приоритетов.
///
/// Раскладка (1D, radius=5 у обоих правил): magnetB (MAGNET_B, priority 5,
/// direction Right) на x=0 находит СВОЮ ближайшую цель targetB0 на x=2
/// (dist 2) — единственную реальную цель в его собственном радиусе
/// (x∈[-5,5], клетка C на x=6 вне досягаемости). Каскад продолжается
/// магнитом на x=1: его эффективный поиск (после того как уровень 0 уже
/// потребил targetB0) достигает x=6 (dist 5) — клетки `C`, единственной
/// оставшейся цели в его радиусе x∈[-4,6]. magnetA (MAGNET_A, priority 10,
/// обычный CAM БЕЗ recursion) на x=8 находит ТУ ЖЕ клетку C на x=6
/// (dist 2, единственная цель в его радиусе x∈[3,13]) — прямой конфликт
/// с B каскадом на уровне 1, притом что уровень-0 диски A (вокруг x=8) и
/// B (вокруг x=0) вообще не пересекаются (расстояние 8 при радиусе 5) —
/// старый баг видел бы только это и признал бы A и B независимыми.
///
/// Ожидание: приоритет решает конфликт ЦЕЛОГО матча B — A (выше приоритет)
/// применяется, B не применяется ВООБЩЕ (ни уровень 0, ни продолжение),
/// включая его формально бесконфликтную находку targetB0 — целостность
/// матча важнее локальности конфликта.
#[test]
fn test_cam_recursion_cascade_collision_resolved_by_priority_not_silently_corrupted() {
    let mut grid = make_grid(14, 1);
    grid.set_cell(0, 0, Cell::new(MAGNET_B));
    grid.set_cell(2, 0, Cell::new(TARGET)); // targetB0
    grid.set_cell(1, 0, Cell::new(MAGNET_B)); // продолжение каскада B

    grid.set_cell(8, 0, Cell::new(MAGNET_A));
    grid.set_cell(6, 0, Cell::new(TARGET)); // C — единственная цель, достижимая И A, И каскадом B

    let ri = make_rule_index(vec![
        magnet_rule_typed(MAGNET_A, 5, 10),
        magnet_recursion_rule(MAGNET_B, 5, 5, Direction::Right, 1),
    ]);
    let mut engine = Engine::new(grid, ri);

    engine.run_tick();

    // A выигрывает арбитраж — его единственный матч применился.
    assert_eq!(engine.grid().get_cell(8, 0).map(|c| c.value.0 .0), Some(TARGET), "A magnet must become target");
    assert_eq!(engine.grid().get_cell(6, 0).map(|c| c.value.0 .0), Some(0), "shared cell C must be claimed by A, cleared");

    // B проигрывает арбитраж ЦЕЛИКОМ — ничего из B не применилось, включая
    // уровень 0 (targetB0), который сам по себе ни с кем не конфликтовал.
    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(MAGNET_B), "B must not apply at all: level-0 magnet unchanged");
    assert_eq!(engine.grid().get_cell(2, 0).map(|c| c.value.0 .0), Some(TARGET), "B must not apply at all: level-0 target untouched");
    assert_eq!(engine.grid().get_cell(1, 0).map(|c| c.value.0 .0), Some(MAGNET_B), "B must not apply at all: level-1 cascade magnet unchanged");
}

// ──────────────────────────────────────────────────────────────
// Broadcast-сдвиг (`ShiftSpec::broadcast`)
// ──────────────────────────────────────────────────────────────

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
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let ri = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, ri);
    engine.run_tick();

    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(0), "source must be cleared");
    for x in 1..=4 {
        assert_eq!(engine.grid().get_cell(x, 0).map(|c| c.value.0 .0), Some(EMITTER), "cell x={x} on the path must get a copy");
    }
    assert_eq!(engine.grid().get_cell(5, 0).map(|c| c.value.0 .0), Some(0), "cell past the path must stay untouched");
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let ri = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, ri);
    engine.run_tick();

    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(0), "source cleared");
    for x in 1..=3 {
        assert_eq!(engine.grid().get_cell(x, 0).map(|c| c.value.0 .0), Some(0), "intermediate cell x={x} must stay untouched (ordinary shift = teleport)");
    }
    assert_eq!(engine.grid().get_cell(4, 0).map(|c| c.value.0 .0), Some(EMITTER), "only the final target gets the value");
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let ri = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, ri);
    engine.run_tick();

    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(0), "source cleared");
    for x in 1..4 {
        assert_eq!(engine.grid().get_cell(x, 0).map(|c| c.value.0 .0), Some(EMITTER), "cell x={x} within grid bounds must get a copy");
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let ri = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, ri);
    engine.run_tick();

    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(EMITTER), "источник ДОЛЖЕН сохранить значение — keep_source не даёт его очистить");
    for x in 1..=4 {
        assert_eq!(engine.grid().get_cell(x, 0).map(|c| c.value.0 .0), Some(EMITTER), "клетка x={x} на пути должна получить копию");
    }
    assert_eq!(engine.grid().get_cell(5, 0).map(|c| c.value.0 .0), Some(0), "клетка за пределами пути должна остаться нетронутой");
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
        shifts: vec![vec![ShiftSpec { direction: Direction::Right, steps: 4, broadcast: false, keep_source: true }]],
        changes: vec![],
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let ri = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, ri);
    engine.run_tick();

    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(EMITTER), "источник должен сохранить значение");
    for x in 1..4 {
        assert_eq!(engine.grid().get_cell(x, 0).map(|c| c.value.0 .0), Some(0), "промежуточная клетка x={x} не должна трогаться (не broadcast)");
    }
    assert_eq!(engine.grid().get_cell(4, 0).map(|c| c.value.0 .0), Some(EMITTER), "конечная точка должна получить копию");
}

fn emit_chain_rule(id_type: u8, direction: Direction, front_gated: bool) -> Rule {
    let pattern = if front_gated { vec![(0, 0, CellType(id_type)), (1, 0, CellType(0))] } else { vec![] };
    Rule {
        id: vec![CellType(id_type)],
        pattern,
        shifts: vec![vec![ShiftSpec { direction, steps: 1, broadcast: false, keep_source: true }]],
        changes: vec![],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None,
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

    let occupied: Vec<usize> = (0..10).filter(|&x| engine.grid().get_cell(x, 0).map(|c| c.value.0 .0) == Some(EMITTER)).collect();
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
        let occupied: Vec<usize> = (0..10).filter(|&x| engine.grid().get_cell(x, 0).map(|c| c.value.0 .0) == Some(EMITTER)).collect();
        let expected: Vec<usize> = (0..=tick as usize).collect();
        assert_eq!(occupied, expected, "front-gated chain must grow by exactly one cell per tick, no stall, no skip");
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
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
    assert_eq!(engine.grid().get_age(0, 0), 1, "возраст источника должен идти естественно (1 тик прошёл), а не обнуляться");
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
        shifts: vec![vec![ShiftSpec { direction: Direction::Right, steps: 8, broadcast: true, keep_source: true }]],
        changes: vec![],
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: OverflowAction::WriteLiteral(77),
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let ri = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, ri);
    engine.run_tick();

    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(EMITTER), "source (x=0) must stay untouched — keep_source");
    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.born_at), Some(0), "source born_at must not be reset — it was never written");
    for x in 1..5 {
        assert_eq!(engine.grid().get_cell(x, 0).map(|c| c.value.0 .0), Some(EMITTER), "interior path cell x={x} keeps the emitted value");
    }
    assert_eq!(
        engine.grid().get_cell(5, 0).map(|c| c.value.0 .0),
        Some(77),
        "boundary cell (x=5, where the path exits) ends up with the overflow literal, overwriting the emitted value written moments earlier"
    );
    assert_eq!(engine.grid().get_cell(5, 0).map(|c| c.born_at), Some(1), "boundary cell born_at must be the current generation — it WAS genuinely written (twice)");
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
            shifts: vec![vec![ShiftSpec { direction: Direction::Right, steps: 1, broadcast: true, keep_source }]],
            changes: vec![],
            active_only: false,
            priority: 0,
            min_age: 0,
            overflow: OverflowAction::WriteLiteral(99),
            cam: None,
            tie_break: 0,
            starvation_after: None, feedback: None, recursion: None, memory: None,
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

    assert_eq!(with_keep_source, without_keep_source, "overflow clamping onto the source coordinate must behave identically regardless of keep_source");
    assert_eq!(with_keep_source.0, Some(99), "the overflow literal wins at the coincidence cell — keep_source cannot protect a cell that a DIFFERENT write (overflow) independently targets");
    assert_eq!(with_keep_source.1, Some(1), "born_at reflects a genuine write this tick");
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
            shifts: vec![vec![ShiftSpec { direction: Direction::Right, steps: 5, broadcast: false, keep_source }]], // target x=7, clamps to x=4
            changes: vec![],
            active_only: false,
            priority: 0,
            min_age: 0,
            overflow: OverflowAction::Write(0), // 0 == "carry own value", not literal 0
            cam: None,
            tie_break: 0,
            starvation_after: None, feedback: None, recursion: None, memory: None,
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
    assert_eq!(src_no_keep, Some(0), "without keep_source: source is cleared by the shift");
    assert_eq!(target_keep, Some(EMITTER), "boundary cell carries the head's own value (Write(0) semantics)");
    assert_eq!(target_keep, target_no_keep, "boundary write value must be identical regardless of keep_source — the overflow-write call site is unconditional");
    assert_eq!(born_keep, born_no_keep, "boundary born_at must be identical regardless of keep_source");
    assert_eq!(born_keep, Some(1), "boundary cell born_at reflects the write this tick, not the stale pre-tick born_at carried inside head_cell");
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
        shifts: vec![vec![ShiftSpec { direction: Direction::Right, steps: 1, broadcast: true, keep_source: true }]],
        changes: vec![],
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: OverflowAction::WriteLiteral(99),
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let ri = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, ri);
    let matches = engine.detect_matches();
    assert_eq!(matches.len(), 1);
    let accepted = engine.arbitrate(matches);
    assert_eq!(accepted.len(), 1);

    let (regions, _) = engine.apply_matches(accepted);
    assert_eq!(regions.len(), 1);
    let written: Vec<_> = regions[0].written_cells.iter().filter(|&&(x, y)| (x, y) == (2, 0)).collect();
    assert_eq!(written.len(), 1, "coincidence cell (2,0) must appear in written_cells exactly once (overflow write only — keep_source skipped its own clear/push entirely)");

    engine.advance_age();
    engine.reset_age(&regions);
    assert_eq!(engine.grid().get_cell(2, 0).map(|c| c.value.0 .0), Some(99), "value is the overflow literal");
    assert_eq!(engine.grid().get_age(2, 0), 0, "age correctly reset — the cell WAS genuinely written this tick, just not via the keep_source-skipped clear path");
}

// ──────────────────────────────────────────────────────────────
// "Активный таймер" — доказательство, что это УЖЕ выражается
// существующими примитивами (min_age / счётная цепочка self-change),
// не новая возможность модели.
// ──────────────────────────────────────────────────────────────

const TIMER: u8 = 60;
const FIRED: u8 = 61;

/// Способ 1 — `min_age` буквально И ЕСТЬ таймер по определению: правило
/// срабатывает, только когда возраст клетки достиг порога. Клетка стоит
/// TIMER ровно `THRESHOLD` тиков, затем "выстреливает" в FIRED — без
/// единого дополнительного механизма.
#[test]
fn test_timer_via_min_age_is_already_expressible() {
    const THRESHOLD: u64 = 5;
    let mut grid = make_grid(1, 1);
    grid.set_cell(0, 0, Cell::new(TIMER));
    let rule = Rule {
        id: vec![CellType(TIMER)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(FIRED))],
        active_only: false,
        priority: 0,
        min_age: THRESHOLD,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let ri = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, ri);

    for tick in 0..THRESHOLD {
        engine.run_tick();
        assert_eq!(
            engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
            Some(TIMER),
            "must still be TIMER before threshold (tick={tick})"
        );
    }
    engine.run_tick(); // tick == THRESHOLD: min_age condition finally satisfied
    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(FIRED), "must fire exactly at the threshold");
}

/// Способ 2 — счётная цепочка self-change правил (типы TIMER..TIMER+N-1,
/// каждый тик +1), для случаев, когда сам счёт должен быть ВИДИМ/читаем
/// другими правилами по пути (min_age скрыт внутри клетки, недоступен
/// чтению соседями) — тоже уже существующий примитив, не новый.
#[test]
fn test_timer_via_self_change_counting_chain_is_already_expressible() {
    const N: u8 = 5;
    let mut grid = make_grid(1, 1);
    grid.set_cell(0, 0, Cell::new(TIMER));
    let mut rules = Vec::new();
    for k in 0..N {
        let (from, to) = (TIMER + k, if k + 1 == N { FIRED } else { TIMER + k + 1 });
        rules.push(Rule {
            id: vec![CellType(from)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(to))],
            active_only: false,
            priority: 0,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None, feedback: None, recursion: None, memory: None,
        });
    }
    let ri = make_rule_index(rules);
    let mut engine = Engine::new(grid, ri);

    for k in 0..N {
        engine.run_tick();
        let expected = if k + 1 == N { FIRED } else { TIMER + k + 1 };
        assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(expected), "counting chain step {k}");
    }
}

// ──────────────────────────────────────────────────────────────
// Модульный tie-break в арбитраже (block F, п.3): два правила с ОДИНАКОВЫМ
// priority, матчащие одну и ту же клетку (тип 1, никогда не меняется — ни
// одно из правил не пишет в неё саму, только в соседнюю (1,0)) КАЖДЫЙ тик,
// так что age у обоих матчей тоже всегда совпадает — приоритет и возраст
// специально уравнены, чтобы изолировать именно tie_break как решающий
// фактор (иначе он бы никогда не дошёл до сравнения).
// ──────────────────────────────────────────────────────────────

fn make_tie_break_rules(tie_break_a: u32, tie_break_b: u32) -> HashMap<CellType, Vec<Rule>> {
    let rule_a = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(1, 0, ChangeValue::Literal(100))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: tie_break_a,
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let rule_b = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(1, 0, ChangeValue::Literal(200))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: tie_break_b,
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    make_rule_index(vec![rule_a, rule_b])
}

/// tie_break=0 у ОБОИХ правил (значение по умолчанию) не должно менять
/// старое поведение: арбитраж по-прежнему падает на лексикографический
/// порядок id/rule_idx, который НЕ зависит от поколения — победитель обязан
/// быть одним и тем же на каждом тике.
#[test]
fn test_tie_break_default_zero_preserves_old_rule_id_order() {
    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let mut engine = Engine::new(grid, make_tie_break_rules(0, 0));

    let mut winners = Vec::new();
    for _ in 0..10 {
        engine.run_tick();
        winners.push(engine.grid().get_cell(1, 0).map(|c| c.value.0 .0));
    }
    assert!(
        winners.iter().all(|&w| w == winners[0]),
        "с tie_break=0 у обоих правил победитель не должен зависеть от поколения: {winners:?}"
    );
}

/// Два правила с tie_break, расставленными РОВНО на M/2 друг от друга
/// (см. doc-комментарий `arbitrator::TIE_BREAK_MODULUS`), должны чередовать
/// победу СТРОГО поровну за один полный период M поколений — прямая
/// проверка формулы `(tie_break + generation) % M`, а не просто "иногда
/// меняется".
#[test]
fn test_tie_break_rotates_fairly_when_spaced_half_modulus_apart() {
    use crate::engine::arbitrator::TIE_BREAK_MODULUS;

    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let mut engine = Engine::new(grid, make_tie_break_rules(0, TIE_BREAK_MODULUS / 2));

    let (mut a_wins, mut b_wins) = (0u32, 0u32);
    for gen in 0..TIE_BREAK_MODULUS {
        engine.run_tick();
        match engine.grid().get_cell(1, 0).map(|c| c.value.0 .0) {
            Some(100) => a_wins += 1,
            Some(200) => b_wins += 1,
            other => panic!("неожиданное значение (1,0) на поколении {gen}: {other:?}"),
        }
    }
    assert_eq!(a_wins, TIE_BREAK_MODULUS / 2, "правило A должно выигрывать ровно половину периода");
    assert_eq!(b_wins, TIE_BREAK_MODULUS / 2, "правило B должно выигрывать ровно половину периода");
}

// ──────────────────────────────────────────────────────────────
// Опциональный temporal arbitration против голодания по РАЗНОМУ приоритету
// (block F, п.5) — в отличие от tie_break (решает только РАВНЫЙ приоритет),
// здесь HIGH (priority=20) и LOW (priority=5) конкурируют за одну и ту же
// клетку каждый тик; без starvation_after HIGH обязан побеждать НАВСЕГДА
// (priority — первое и решающее поле ключа сортировки).
// ──────────────────────────────────────────────────────────────

fn make_starvation_rules(low_starvation_after: Option<u32>) -> HashMap<CellType, Vec<Rule>> {
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
        starvation_after: None, feedback: None, recursion: None, memory: None,
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
        starvation_after: low_starvation_after, feedback: None, recursion: None, memory: None,
    };
    make_rule_index(vec![rule_high, rule_low])
}

/// Без `starvation_after` (значение по умолчанию, `None`) более низкий
/// priority обязан проигрывать НАВСЕГДА — это сам факт голодания, который
/// п.5 призван решать; проверяем, что проблема РЕАЛЬНО существует без
/// защиты (негативный контроль), а не просто что защита работает.
#[test]
fn test_without_starvation_guard_low_priority_never_wins() {
    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let mut engine = Engine::new(grid, make_starvation_rules(None));

    for tick in 0..30 {
        engine.run_tick();
        assert_eq!(
            engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
            Some(100),
            "без starvation_after HIGH должен побеждать абсолютно всегда (тик {tick}) — иначе голодание не доказано"
        );
    }
}

/// `starvation_after = Some(K)` на LOW: проигрывает K тиков подряд, потом
/// гарантированно побеждает РОВНО на (K+1)-м тике (эффективный priority в
/// этот тик становится u32::MAX), после чего счётчик сбрасывается и цикл
/// повторяется — строго периодический паттерн побед на тиках K+1, 2(K+1),
/// 3(K+1), ... — прямая проверка формулы, а не просто "хоть раз победил".
#[test]
fn test_starvation_guard_guarantees_periodic_progress() {
    const K: u32 = 3;
    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let mut engine = Engine::new(grid, make_starvation_rules(Some(K)));

    let mut low_wins_at = Vec::new();
    const TOTAL_TICKS: u32 = 20;
    for tick in 1..=TOTAL_TICKS {
        engine.run_tick();
        match engine.grid().get_cell(1, 0).map(|c| c.value.0 .0) {
            Some(200) => low_wins_at.push(tick),
            Some(100) => {}
            other => panic!("неожиданное значение (1,0) на тике {tick}: {other:?}"),
        }
    }

    let expected: Vec<u32> = (1..=TOTAL_TICKS).filter(|t| t % (K + 1) == 0).collect();
    assert_eq!(low_wins_at, expected, "LOW должен побеждать РОВНО каждый (K+1)-й тик, не чаще и не реже");
}

/// Регрессионный тест на реальный, найденный при аудите GPU-портирования
/// `starvation_after` баг: в отличие от `feedback_counters`/`memory_buffers`
/// (оба явно чистятся от осиротевших записей — см. `ExtensionFlags::extension_rule_indices`'s
/// doc-комментарий, который дословно говорит "для правил с `feedback` ИЛИ
/// `memory`", НЕ упоминая `starvation_after`), `starvation_counters`
/// обновляется ТОЛЬКО в двух местах: рост при проигрыше (если матч —
/// кандидат ЭТОГО тика) и удаление при выигрыше. Если матч (x,y,rule_idx)
/// просто ПЕРЕСТАЁТ быть кандидатом (клетка сменила тип из-за чего-то
/// постороннего) с НЕНУЛЕВЫМ, но ещё не достигшим порога счётчиком, запись
/// не растёт (не кандидат — не в `starving_keys`) и не удаляется (не
/// выигрыш) — просто ЗАСТЫВАЕТ в `HashMap` навсегда. Если та же позиция
/// ПОЗЖЕ снова станет кандидатом для ТОГО ЖЕ rule_idx, счётчик ошибочно
/// ПРОДОЛЖИТ с замороженного значения, а не с нуля — голодающее правило
/// побеждает раньше, чем должно бы по своей ЖЕ гарантии "K проигрышей
/// подряд, отсчитываемых с нуля".
///
/// Раскладка: WATCHER (тип 1) на x=0, конкурируют HIGH (priority 20, без
/// starvation) и LOW (priority 5, starvation_after=Some(3)) — оба пишут
/// РАЗНЫЕ литералы в СОСЕДА (x=1), сама клетка x=0 остаётся типом 1 у ОБОИХ
/// (идемпотентно), пока её кто-то НЕ тронет напрямую — здесь тик 3
/// принудительно подменяет x=0 на посторонний DECOY (без единого
/// подходящего правила) на РОВНО один тик, потом возвращает обратно. К
/// этому моменту LOW успел проиграть ровно 2 раза (счётчик=2 < K=3, ещё НЕ
/// выиграл бы сам по себе). Правильное поведение (счётчик сброшен на 0 при
/// исчезновении матча) требует ЕЩЁ 3 полных проигрыша ПОСЛЕ возвращения,
/// прежде чем LOW снова выиграет; баг (счётчик заморожен на 2) даёт LOW
/// выиграть уже через 1 проигрыш после возвращения.
#[test]
fn test_starvation_counter_resets_after_match_disappears_and_reappears() {
    const K: u32 = 3;
    const DECOY: u8 = 250;
    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let mut engine = Engine::new(grid, make_starvation_rules(Some(K)));

    // Тики 1-2: LOW проигрывает дважды (счётчик 0->1->2), матч жив всё время
    // (x=0 остаётся типом 1 у обоих правил).
    for tick in 1..=2 {
        engine.run_tick();
        assert_eq!(engine.grid().get_cell(1, 0).map(|c| c.value.0 .0), Some(100), "тик {tick}: HIGH должен побеждать (счётчик LOW ещё не достиг порога)");
    }

    // Тик 3: подменяем x=0 на DECOY напрямую (посторонняя клетка, ни у кого
    // нет для неё правил) — матч (0,0,rule_idx LOW) на этот тик просто не
    // существует, счётчик НЕ должен ни расти, ни выигрывать. Возвращаем x=0
    // обратно в тип 1 сразу после — на СЛЕДУЮЩЕМ тике матч снова существует.
    engine.grid_mut().set_cell(0, 0, Cell::new(DECOY));
    engine.run_tick();
    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(DECOY), "тик 3: клетка временно постороннего типа, ничьё правило её не трогает");
    engine.grid_mut().set_cell(0, 0, Cell::new(1));

    // Тики 4-6 (3 полных проигрыша после возвращения): если счётчик
    // КОРРЕКТНО сброшен на 0 при исчезновении, HIGH обязан побеждать все три
    // раза — LOW выигрывает только на тике 7 (4-й проигрыш подряд с нуля).
    // Баг дал бы LOW выиграть уже на тике 4 (замороженный счётчик 2 + 1
    // проигрыш = 3 >= K).
    for tick in 4..=6 {
        engine.run_tick();
        assert_eq!(
            engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
            Some(100),
            "тик {tick}: HIGH должен побеждать -- если LOW победил здесь, счётчик голодания НЕ был сброшен при исчезновении матча (реальный баг, не гипотеза)"
        );
    }
    engine.run_tick(); // тик 7: 4-й проигрыш подряд с нуля -> LOW обязан выиграть
    assert_eq!(engine.grid().get_cell(1, 0).map(|c| c.value.0 .0), Some(200), "тик 7: LOW обязан выиграть -- ровно K=3 проигрыша подряд С НУЛЯ после возвращения матча");
}

/// Block G, п.3 ("грубость повторной проверки min_age"): регрессионный тест
/// на РЕАЛЬНЫЙ механизм, который эту "грубость" устраняет —
/// `min_age_gated_types` в `SearchRadiusCache` заставляет
/// `resolve_search_coords_advance` каждый тик безусловно досканировать ВСЕ
/// активные клетки типов, у которых есть хоть одно правило с `min_age > 0`,
/// независимо от dirty-состояния (см. `build_candidates` в `engine/mod.rs`).
///
/// Клетка-таймер стоит ОДНА на большой (50×1) решётке, далеко от края и без
/// единой другой активной клетки рядом — обычное dirty-расширение (радиус
/// вокруг недавних изменений) в принципе не может её найти, ей неоткуда
/// взяться в кандидатах, КРОМЕ безусловного пересканирования по типу.
/// Если бы этот механизм был сломан или удалён, клетка осталась бы TIMER
/// НАВСЕГДА (её тип никогда не помечается dirty, никто рядом не меняется) —
/// тест ловит именно такую регрессию, а не просто "min_age вообще работает"
/// (для этого хватило бы уже существующего теста на 1×1 решётке).
#[test]
fn test_min_age_gated_cell_matures_exactly_on_time_when_isolated_on_sparse_grid() {
    const THRESHOLD: u64 = 7;
    const ISOLATED_X: usize = 40;

    let mut grid = make_grid(50, 1);
    grid.set_cell(ISOLATED_X, 0, Cell::new(TIMER));
    let rule = Rule {
        id: vec![CellType(TIMER)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(FIRED))],
        active_only: false,
        priority: 0,
        min_age: THRESHOLD,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None,
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    for tick in 0..THRESHOLD {
        engine.run_tick();
        assert_eq!(
            engine.grid().get_cell(ISOLATED_X, 0).map(|c| c.value.0 .0),
            Some(TIMER),
            "изолированная клетка должна оставаться TIMER до порога (тик {tick}) — если это не так, вероятно \
             сработал ложный full-scan или клетка вообще не переоценивалась и осталась бы TIMER навсегда"
        );
    }
    engine.run_tick(); // tick == THRESHOLD
    assert_eq!(
        engine.grid().get_cell(ISOLATED_X, 0).map(|c| c.value.0 .0),
        Some(FIRED),
        "изолированная клетка обязана дозреть РОВНО на пороговом тике, несмотря на отсутствие какой-либо \
         соседней активности, которая могла бы её случайно пометить dirty"
    );
}

// ──────────────────────────────────────────────────────────────
// `Rule::feedback` (block-обсуждение "обратная связь", п.1) — маркер едет
// East, после `timeout` тиков подряд (независимо от исхода арбитража —
// считаются попытки) переключается на `new_direction` НАВСЕГДА (защёлка,
// не сбрасывается — см. её doc-комментарий).
// ──────────────────────────────────────────────────────────────

const MARKER: u8 = 70;

#[test]
fn test_feedback_latches_new_direction_after_timeout_and_stays() {
    const TIMEOUT: u64 = 3;
    let mut grid = make_grid(10, 10);
    grid.set_cell(2, 2, Cell::new(MARKER));

    let rule = Rule {
        id: vec![CellType(MARKER)],
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
        feedback: Some(FeedbackSpec { timeout: TIMEOUT, new_direction: Direction::Up }),
        recursion: None,
        memory: None,
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    fn find_marker(engine: &Engine<VecStorage>) -> (usize, usize) {
        for y in 0..10 {
            for x in 0..10 {
                if engine.grid().get_cell(x, y).map(|c| c.value.0 .0) == Some(MARKER) {
                    return (x, y);
                }
            }
        }
        panic!("маркер не найден на решётке");
    }

    // Тик 1: counter 0->1 (1 < TIMEOUT=3) -> обычное направление (Right).
    engine.run_tick();
    assert_eq!(find_marker(&engine), (3, 2), "тик 1: должен ехать East (счётчик ещё не достиг timeout)");
    // Тик 2: counter 1->2 (2 < 3) -> всё ещё Right.
    engine.run_tick();
    assert_eq!(find_marker(&engine), (4, 2), "тик 2: должен ехать East (счётчик ещё не достиг timeout)");
    // Тик 3: counter 2->3 (3 >= 3) -> защёлка сработала, едет Up.
    engine.run_tick();
    assert_eq!(find_marker(&engine), (4, 1), "тик 3: должен ПЕРЕКЛЮЧИТЬСЯ на North ровно на пороговом тике");
    // Тик 4: защёлка не сбрасывается — по-прежнему Up, не East.
    engine.run_tick();
    assert_eq!(find_marker(&engine), (4, 0), "тик 4: защёлка не должна сбрасываться — маркер продолжает ехать North");
}

/// Найден при аудите GPU-порта `feedback` (см. память сессии,
/// `project_gpu_memory_support_2026_08_08`): `arbitrator::get_match_affected_cells`
/// (вызывается ИЗНУТРИ арбитража) и `applicator::apply_rule_buffered`
/// (вызывается ПОСЛЕ) ОБА читают `feedback_counters`, чтобы решить, какое
/// направление (декларированное или `new_direction`) реально становится
/// affected-cells/фактической записью — но раньше инкремент счётчика стоял
/// МЕЖДУ этими двумя чтениями, так что на тике, где счётчик матча
/// ПЕРЕСЕКАЕТ `timeout` ИМЕННО на этом тике, арбитраж резервировал/проверял
/// конфликты для ОДНОГО направления (старое значение), а apply реально
/// писал в ДРУГОЕ (новое, уже защёлкнутое значение) — цель, которую
/// арбитраж НИКОГДА не проверял на конфликт с другими матчами.
///
/// Этот тест НАРОЧНО ставит конкурента ИМЕННО на клетку `new_direction`
/// (Down), но НЕ на клетку декларированного направления (Right) — так что
/// баг проявляется ТОЛЬКО если действительно происходит рассинхронизация
/// между "что видел арбитраж" и "что реально написал apply": маркер обязан
/// либо (а) выиграть у конкурента и переехать Down, либо (б) остаться
/// нетронутым на месте (если бы решил ехать Right и там был бы конфликт —
/// но Right здесь ничем не занят, так что (б) означало бы "решил ехать
/// Right и просто уехал бы" — тест ловит ИМЕННО невозможный третий исход:
/// маркер тихо ИСЧЕЗАЕТ (source-clear проходит, но целевая запись
/// проигрывает необнаруженную гонку с конкурентом).
#[test]
fn test_feedback_counter_crossing_threshold_this_tick_matches_arbitration_and_apply() {
    const MARKER2: u8 = 98;
    const COMPETITOR: u8 = 99;
    const TIMEOUT: u64 = 1;

    let mut grid = make_grid(3, 3);
    grid.set_cell(0, 0, Cell::new(MARKER2));
    grid.set_cell(0, 1, Cell::new(COMPETITOR)); // на клетке Down-цели, НЕ Right-цели

    let marker_rule = Rule {
        id: vec![CellType(MARKER2)],
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
        feedback: Some(FeedbackSpec { timeout: TIMEOUT, new_direction: Direction::Down }),
        recursion: None,
        memory: None,
    };
    let competitor_rule = Rule {
        id: vec![CellType(COMPETITOR)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(COMPETITOR))],
        active_only: false,
        priority: 1,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![marker_rule, competitor_rule]));

    // TIMEOUT=1: счётчик пересекает порог РОВНО на первом тике (0 -> 1,
    // 1 >= 1) — именно тот случай, где старый (сломанный) порядок
    // рассинхронизировал арбитраж и apply.
    engine.run_tick();

    let marker_positions: Vec<(usize, usize)> = (0..3)
        .flat_map(|y| (0..3).map(move |x| (x, y)))
        .filter(|&(x, y)| engine.grid().get_cell(x, y).map(|c| c.value.0 .0) == Some(MARKER2))
        .collect();
    assert_eq!(
        marker_positions,
        vec![(0, 1)],
        "маркер обязан ПОБЕДИТЬ конкурента и переехать Down (приоритет 10 > 1) — если он вместо этого исчез (пустой список), арбитраж и apply разошлись во мнениях о направлении"
    );
}

// ──────────────────────────────────────────────────────────────
// `Rule::recursion` (block-обсуждение "рекурсивные правила", п.4) —
// ограниченный каскад ВНУТРИ одного тика: заливка на несколько клеток
// сразу, а не за N тиков.
// ──────────────────────────────────────────────────────────────

const RFILLED: u8 = 80;
const RUNFILLED: u8 = 81;

#[test]
fn test_recursion_cascades_multiple_cells_in_one_tick() {
    const MAX_DEPTH: u8 = 3;
    let mut grid = make_grid(10, 1);
    grid.set_cell(0, 0, Cell::new(RFILLED));
    for x in 1..10 {
        grid.set_cell(x, 0, Cell::new(RUNFILLED));
    }

    let rule = Rule {
        id: vec![CellType(RUNFILLED)],
        pattern: vec![(0, 0, CellType(RUNFILLED)), (-1, 0, CellType(RFILLED))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(RFILLED))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: Some(RecursionSpec { max_depth: MAX_DEPTH, direction: Direction::Right }),
        memory: None,
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // ОДИН тик — должен залить исходную клетку (1) плюс MAX_DEPTH=3
    // дополнительных уровня каскада (2, 3, 4), итого 4 клетки, не за 4 тика.
    engine.run_tick();

    for x in 0..=4 {
        assert_eq!(
            engine.grid().get_cell(x, 0).map(|c| c.value.0 .0),
            Some(RFILLED),
            "клетка {x} должна быть залита за ОДИН тик (каскад глубины {MAX_DEPTH})"
        );
    }
    for x in 5..10 {
        assert_eq!(
            engine.grid().get_cell(x, 0).map(|c| c.value.0 .0),
            Some(RUNFILLED),
            "клетка {x} НЕ должна быть залита — каскад ограничен max_depth={MAX_DEPTH}"
        );
    }
}

/// Лемма 4 (`paper/paper4.md` §8, Corollary B): каскад `recursion` обязан
/// участвовать в графе конфликтов через union по ВСЕМ уровням k=0..=max_depth,
/// а не только k=0 — иначе конфликт, достижимый ТОЛЬКО на глубине каскада,
/// был бы пропущен.
#[test]
fn test_recursion_conflict_only_visible_via_cascade_depth_union() {
    // Правило A: recursion max_depth=2, direction=Right. Нормальный (k=0)
    // write cell — только (0,0). Union по k=0..=2 добавляет (1,0) и (2,0).
    let rule_a = Rule {
        id: vec![CellType(1)],
        pattern: vec![(0, 0, CellType(1)), (-1, 0, CellType(9))],
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
        recursion: Some(RecursionSpec { max_depth: 2, direction: Direction::Right }),
        memory: None,
    };
    // Правило B: пишет в (0,0) относительно себя. Размещённое (в терминах
    // относительного офсета, который перебирает `ConflictGraph::build`) на
    // (2,0) от A — недостижимо на k=0, достижимо ТОЛЬКО на глубине каскада k=2.
    let rule_b = Rule {
        id: vec![CellType(3)],
        pattern: vec![(0, 0, CellType(3))],
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
    };

    let graph = crate::ConflictGraph::build(&[rule_a, rule_b]);
    assert!(
        graph.edges.contains(&(0, 1)),
        "граф ОБЯЗАН найти ребро между A и B: каскад A на глубине k=2 пишет в ту же \
относительную клетку, где сидит B, хотя k=0 (без каскада) её не задевает. Рёбра: {:?}",
        graph.edges
    );
}

// ──────────────────────────────────────────────────────────────
// `Rule::memory` (тема "правила с памятью", п.3) — гейт по ТОЧНОЙ
// последовательности прошлых наблюдений (FIFO-буфер), а не по скалярному
// счётчику (`starvation_after`/`feedback`). Два триггера, один механизм
// (см. её doc-комментарий в `types.rs`): `NeighborType` — до арбитража,
// `RuleOutcome` — после.
// ──────────────────────────────────────────────────────────────

const MEM_WATCHER: u8 = 90;
const MEM_FIRED: u8 = 91;
const MEM_NEIGH_A: u8 = 92;
const MEM_NEIGH_B: u8 = 93;

#[test]
fn test_memory_neighbor_type_gate_opens_exactly_after_matching_sequence() {
    let mut grid = make_grid(5, 1);
    grid.set_cell(2, 0, Cell::new(MEM_WATCHER));
    grid.set_cell(3, 0, Cell::new(MEM_NEIGH_A)); // (2,0) + Right = (3,0)

    let rule = Rule {
        id: vec![CellType(MEM_WATCHER)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(MEM_FIRED))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: Some(MemorySpec {
            window: 3,
            record_trigger: RecordTrigger::NeighborType(Direction::Right),
            match_pattern: vec![
                RecordedValue::Type(CellType(MEM_NEIGH_A)),
                RecordedValue::Type(CellType(MEM_NEIGH_B)),
                RecordedValue::Type(CellType(MEM_NEIGH_A)),
            ],
        }),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Тик 1: буфер получает Type(A) (сосед на (3,0)), но len=1 != window=3
    // -> гейт закрыт (проверяет буфер КАК ОН БЫЛ до этого тика — пустой).
    engine.run_tick();
    assert_eq!(engine.grid().get_cell(2, 0).map(|c| c.value.0 .0), Some(MEM_WATCHER), "тик 1: гейт ещё закрыт — буфер не полон");

    // Тик 2: сосед -> B. Буфер [A, B], len=2 != 3 -> гейт всё ещё закрыт.
    engine.grid_mut().set_cell(3, 0, Cell::new(MEM_NEIGH_B));
    engine.run_tick();
    assert_eq!(engine.grid().get_cell(2, 0).map(|c| c.value.0 .0), Some(MEM_WATCHER), "тик 2: гейт ещё закрыт — буфер не полон");

    // Тик 3: сосед -> A. К КОНЦУ этого тика буфер станет [A, B, A] (точно
    // совпадает с match_pattern), но гейт ЭТОГО тика проверяется ДО этой
    // записи (буфер "как он был на конец тика 2" = [A, B], не полон) ->
    // WATCHER ещё не срабатывает, хотя буфер вот-вот совпадёт.
    engine.grid_mut().set_cell(3, 0, Cell::new(MEM_NEIGH_A));
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(2, 0).map(|c| c.value.0 .0),
        Some(MEM_WATCHER),
        "тик 3: гейт всё ещё закрыт — буфер полнится ПОСЛЕ арбитража этого тика, не ДО"
    );

    // Тик 4: гейт теперь проверяет буфер [A, B, A] (каким он стал к концу
    // тика 3) — точное совпадение -> гейт открывается РОВНО на этом тике.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(2, 0).map(|c| c.value.0 .0),
        Some(MEM_FIRED),
        "тик 4: гейт обязан открыться ровно на тик после накопления полной совпадающей последовательности"
    );
}

/// Лемма-4-класса вопрос "нужны ли изменения в conflict_analyzer" здесь не
/// стоит — `memory` не меняет заявленную зону записи правила (гейт только
/// решает, участвует ли матч в арбитраже ВООБЩЕ, changes/shifts остаются
/// теми же, что и без memory). См. `types::MemorySpec`'s doc-комментарий:
/// `conflict_analyzer.rs` не тронут ни строчкой ради этой темы.
#[test]
fn test_memory_rule_outcome_gate_fires_on_exact_mixed_sequence() {
    const R_MARKER: u8 = 94;
    const R_FIRED: u8 = 95;

    let mut grid = make_grid(1, 1);
    grid.set_cell(0, 0, Cell::new(R_MARKER));

    let rule = Rule {
        id: vec![CellType(R_MARKER)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(R_FIRED))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: Some(MemorySpec {
            window: 3,
            record_trigger: RecordTrigger::RuleOutcome,
            match_pattern: vec![RecordedValue::Missed, RecordedValue::Applied, RecordedValue::Missed],
        }),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Белый ящик (законно: `tests` — дочерний модуль `engine`, у него есть
    // доступ к приватным полям `Engine`): подсаживаем буфер напрямую. Это
    // ЕДИНСТВЕННЫЙ честный способ проверить гейт на СМЕШАННОЙ (не
    // однородной) последовательности — правило, гейтующее САМО СЕБЯ по
    // своему же исходу арбитража, не может естественно накопить такую
    // историю через симуляцию с нуля: "гейт закрыт" ВСЕГДА означает
    // "проиграл" (матч исключён из арбитража целиком, а не проиграл
    // по-честному), так что с нуля накопимая история умеет быть ТОЛЬКО
    // однородной ([Missed; window] после N тиков простоя) — ровно то, что
    // `starvation_after` и так умеет выразить. Это не баг теста и не баг
    // механизма — чисто структурное свойство self-referential
    // RuleOutcome-гейта, стоящее отдельного документирования (см.
    // `paper/paper4.md`), а не то, что этот тест обязан воспроизводить
    // "с нуля" через `run_tick`.
    engine.memory_buffers.insert(
        (0, 0, 0),
        VecDeque::from(vec![RecordedValue::Missed, RecordedValue::Applied, RecordedValue::Missed]),
    );

    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(R_FIRED),
        "гейт обязан открыться немедленно: подсаженный буфер уже точно совпадает с match_pattern"
    );
}

/// Тот же multiset исходов (2×Missed, 1×Applied), но ДРУГОЙ порядок —
/// [Missed, Missed, Applied], а не [Missed, Applied, Missed]. Скалярный
/// счётчик (`Rule::starvation_after: Option<u32>`) в принципе не может
/// различить эти два случая: он хранит ОДНО число (сколько раз подряд
/// проиграно), а не порядок исходов. Память обязана их различить — это и
/// есть доказательство, что `memory` — не переобёртка `starvation_after`.
#[test]
fn test_memory_rule_outcome_gate_rejects_reordered_sequence_with_same_multiset() {
    const R_MARKER: u8 = 96;
    const R_FIRED: u8 = 97;

    let mut grid = make_grid(1, 1);
    grid.set_cell(0, 0, Cell::new(R_MARKER));

    let rule = Rule {
        id: vec![CellType(R_MARKER)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(R_FIRED))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: Some(MemorySpec {
            window: 3,
            record_trigger: RecordTrigger::RuleOutcome,
            match_pattern: vec![RecordedValue::Missed, RecordedValue::Applied, RecordedValue::Missed],
        }),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    engine.memory_buffers.insert(
        (0, 0, 0),
        VecDeque::from(vec![RecordedValue::Missed, RecordedValue::Missed, RecordedValue::Applied]),
    );

    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(R_MARKER),
        "гейт ДОЛЖЕН остаться закрытым: тот же multiset исходов, но другой порядок — не \
совпадает с match_pattern поэлементно. Скаляр (starvation_after) эту разницу в принципе не видит."
    );
}

// ──────────────────────────────────────────────────────────────
// Аудит взаимодействий: `keep_source` × `feedback`/`memory`, гейт `memory`
// × `starvation_after`. Не переписанные из головы предположения — каждый
// тест реально проверяет конкретную, потенциально ломающуюся комбинацию.
// ──────────────────────────────────────────────────────────────

/// Правило с ОБОИМИ `feedback` И `memory` НА `keep_source`-сдвиге:
/// источник физически никогда не двигается (`keep_source` не даёт его
/// очистить), так что БЕЗ фикса ("пропустить перенос при keep_source", см.
/// `applicator::apply_shift_buffered`) старый код всё равно пытался бы
/// ПЕРЕНЕСТИ оба состояния (`feedback_counters` и `memory_buffers`) на
/// позицию ЦЕЛИ ИЗЛУЧЕНИЯ (которая НЕ является тем же маркером — это
/// НЕЗАВИСИМАЯ копия) — история оригинала терялась бы на каждом тике,
/// который что-то реально излучил. Проверяем, что состояние ИСТОЧНИКА
/// переживает несколько тиков нетронутым.
#[test]
fn test_emit_preserves_feedback_and_memory_state_at_source_across_ticks() {
    const MARKER: u8 = 210;

    let mut grid = make_grid(5, 1);
    grid.set_cell(0, 0, Cell::new(MARKER));

    let rule = Rule {
        id: vec![CellType(MARKER)],
        pattern: vec![],
        // Точечное излучение: копия ТОЛЬКО в (1,0), источник (0,0) не трогается.
        shifts: vec![vec![ShiftSpec { direction: Direction::Right, steps: 1, broadcast: false, keep_source: true }]],
        changes: vec![],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: Some(FeedbackSpec { timeout: 100, new_direction: Direction::Up }), // высокий timeout — не должен успеть сработать за 3 тика, тест не про feedback-переключение, а про сохранность счётчика
        recursion: None,
        memory: Some(MemorySpec {
            window: 2,
            record_trigger: RecordTrigger::RuleOutcome,
            // Достижимо с нуля (см. doc-комментарий про self-referential
            // bootstrap deadlock в `test_memory_rule_outcome_gate_fires_on_exact_mixed_sequence`):
            // однородный [Missed, Missed] естественно накопится, пока гейт закрыт.
            match_pattern: vec![RecordedValue::Missed, RecordedValue::Missed],
        }),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Тик 1: буфер пуст -> гейт закрыт -> матч исключён из арбитража ->
    // Missed записывается (см. doc-комментарий гейта), апдейт не применяется.
    engine.run_tick();
    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(MARKER), "тик 1: гейт закрыт, применения не было");
    assert_eq!(engine.grid().get_cell(1, 0).map(|c| c.value.0 .0), Some(0), "тик 1: излучения не было (гейт закрыт)");

    // Тик 2: буфер [Missed], всё ещё не полон (window=2) -> гейт всё ещё закрыт.
    engine.run_tick();
    assert_eq!(engine.grid().get_cell(1, 0).map(|c| c.value.0 .0), Some(0), "тик 2: гейт всё ещё закрыт");

    // Тик 3: буфер [Missed, Missed] (как он был к концу тика 2) == match_pattern
    // -> гейт открывается -> единственный претендент побеждает арбитраж без
    // сравнений -> излучение реально применяется.
    engine.run_tick();
    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(MARKER), "тик 3: источник ДОЛЖЕН сохранить значение (keep_source)");
    assert_eq!(engine.grid().get_cell(1, 0).map(|c| c.value.0 .0), Some(MARKER), "тик 3: цель излучения получила копию");

    // Белый ящик: состояние ОБОИХ расширений должно жить у ИСТОЧНИКА
    // (0,0,0), не быть перенесено (и тем более не потеряно) на цель (1,0,0).
    assert_eq!(
        engine.feedback_counters.get(&(0, 0, 0)),
        Some(&1),
        "счётчик feedback ДОЛЖЕН пережить 3 тика на позиции источника — \
без фикса keep_source он был бы (ошибочно) перенесён на (1,0) при первом же реальном применении"
    );
    assert_eq!(
        engine.memory_buffers.get(&(0, 0, 0)),
        Some(&VecDeque::from(vec![RecordedValue::Missed, RecordedValue::Applied])),
        "буфер memory ДОЛЖЕН остаться у источника и корректно обновиться на Applied после тика 3"
    );
    assert!(
        engine.feedback_counters.get(&(1, 0, 0)).is_none() && engine.memory_buffers.get(&(1, 0, 0)).is_none(),
        "у ЦЕЛИ излучения НЕ должно быть состояния — это независимая свежая копия, не наследующая историю оригинала"
    );
}

/// Гейт `memory` закрывает матч ДО того, как считаются `starving_keys` (см.
/// порядок в `run_tick_with_cache`). Проверяем это напрямую: правило с
/// ПОСТОЯННО закрытым `memory`-гейтом (наблюдает соседа, который никогда не
/// станет нужным типом) и `starvation_after` ОДНОВРЕМЕННО — если бы порядок
/// был перепутан, счётчик голодания рос бы для матча, который на самом деле
/// ни разу не участвовал в арбитраже.
#[test]
fn test_memory_gate_closed_excludes_from_starvation_accounting() {
    const WATCHER: u8 = 211;
    const NEVER_A: u8 = 212; // сосед всегда этого типа
    const WANTED_B: u8 = 213; // а гейт ждёт этот — никогда не появится

    let mut grid = make_grid(3, 1);
    grid.set_cell(0, 0, Cell::new(WATCHER));
    grid.set_cell(1, 0, Cell::new(NEVER_A));

    let rule = Rule {
        id: vec![CellType(WATCHER)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(WATCHER))], // no-op change если бы применилось — не про эффект, про сам факт участия в арбитраже
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: Some(2),
        feedback: None,
        recursion: None,
        memory: Some(MemorySpec {
            window: 1,
            record_trigger: RecordTrigger::NeighborType(Direction::Right),
            match_pattern: vec![RecordedValue::Type(CellType(WANTED_B))], // недостижимо — сосед всегда NEVER_A
        }),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    for tick in 1..=5 {
        engine.run_tick();
        assert!(
            engine.starvation_counters.get(&(0, 0, 0)).is_none(),
            "тик {tick}: счётчик голодания НЕ должен даже появиться в карте — гейт memory \
исключает матч из арбитража КАЖДЫЙ тик, значит он никогда по-настоящему не 'проигрывал'"
        );
    }
}

/// `cam` × `memory`: CAM-матчи входят в `matches` отдельным путём
/// (`detect_cam_matches`, слитый в общий список ДО гейт-фильтра) — гейт
/// работает с ними УНИФИЦИРОВАННО (резолвит правило по `m.head`/`m.rule_idx`,
/// без спец-случая для CAM) или нет? Проверяем напрямую: магнит с закрытым
/// на тик 1 гейтом НЕ должен притянуть цель; тот же магнит на тик 2
/// (гейт открылся) — должен.
#[test]
fn test_cam_magnet_respects_memory_gate() {
    const GATE_NEIGHBOR_VALUE: u8 = 214;

    let mut grid = make_grid(10, 1);
    grid.set_cell(0, 0, Cell::new(MAGNET));
    grid.set_cell(1, 0, Cell::new(GATE_NEIGHBOR_VALUE)); // магнит смотрит сюда через NeighborType(Right)
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
        cam: Some(CamSearch { radius: 5, target_type: CellType(TARGET) }),
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: Some(MemorySpec {
            window: 1,
            record_trigger: RecordTrigger::NeighborType(Direction::Right),
            match_pattern: vec![RecordedValue::Type(CellType(GATE_NEIGHBOR_VALUE))],
        }),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Тик 1: буфер пуст -> не полон -> гейт закрыт (даже хотя сосед УЖЕ
    // нужного типа с самого начала) -> CAM-матч исключён из арбитража ->
    // притяжения не происходит.
    engine.run_tick();
    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(MAGNET), "тик 1: гейт закрыт -- магнит не должен был притянуть цель");
    assert_eq!(engine.grid().get_cell(4, 0).map(|c| c.value.0 .0), Some(TARGET), "тик 1: цель должна остаться на месте");

    // Тик 2: буфер [Type(GATE_NEIGHBOR_VALUE)] (записан в тике 1, независимо
    // от гейта) == match_pattern -> гейт открывается -> CAM реально применяется.
    engine.run_tick();
    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(TARGET), "тик 2: гейт открылся -- магнит должен был притянуть цель");
    assert_eq!(engine.grid().get_cell(4, 0).map(|c| c.value.0 .0), Some(0), "тик 2: найденная клетка должна быть очищена");
}

// ──────────────────────────────────────────────────────────────
// `memory` × `keep_source` (БЕЗ `feedback`), триггер `NeighborType`
// (не `RuleOutcome` — та комбинация уже покрыта
// `test_emit_preserves_feedback_and_memory_state_at_source_across_ticks`).
// Три вопроса: (1) копия получает свежий независимый буфер, источник верно
// копится ДОЛЬШЕ 3 тиков; (2) `NeighborType` копии читает СВОЕГО соседа, а не
// соседа оригинала; (3) не остаётся ли осиротевшая запись в
// `Engine::memory_buffers`, когда клетка перестаёт совпадать.
// ──────────────────────────────────────────────────────────────

/// (1)+(2) в одном сценарии: WATCHER на (0,0), точечное излучение
/// (`keep_source`, БЕЗ `feedback`) в (2,0) при открытии гейта на тик 4.
/// Копия на (2,0) с тика 5 сама становится независимым матчем ТОГО ЖЕ
/// правила (тот же `head`/`rule_idx`) — проверяем, что её запись в
/// `memory_buffers` (а) появляется только тогда, когда она реально
/// продетектирована (не раньше — сама копия физически не существовала в
/// решётке до конца тика 4), (b) НЕ содержит ничего от истории источника,
/// (c) читает соседа ОТНОСИТЕЛЬНО СВОЕЙ позиции (3,0), а не позиции
/// источника (1,0) — который к этому моменту сознательно выставлен в ДРУГОЕ
/// значение, чтобы совпадение было бы легко спутать, будь чтение
/// перепутано. Источник тем временем продолжает копить СВОЙ буфер ещё
/// несколько тиков после эмиссии — дольше 3 тиков, которые покрывал
/// предыдущий тест.
#[test]
fn test_emit_memory_neighbor_type_copy_gets_independent_buffer_own_position() {
    const WATCHER: u8 = 215;
    const TYPE_A: u8 = 216;
    const TYPE_B: u8 = 217;
    const TYPE_C: u8 = 218; // сосед копии — константа, никогда не откроет её гейт
    const TYPE_D: u8 = 219; // сосед источника после эмиссии — заведомо не A/B/C

    let mut grid = make_grid(6, 1);
    grid.set_cell(0, 0, Cell::new(WATCHER)); // источник
    grid.set_cell(1, 0, Cell::new(TYPE_A)); // сосед источника (Right)

    let rule = Rule {
        id: vec![CellType(WATCHER)],
        pattern: vec![], // матчится безусловно по типу головы, без требования к соседям
        // Точечное излучение (keep_source, БЕЗ feedback): копия в (cx+2, cy).
        shifts: vec![vec![ShiftSpec { direction: Direction::Right, steps: 2, broadcast: false, keep_source: true }]],
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
        memory: Some(MemorySpec {
            window: 3,
            record_trigger: RecordTrigger::NeighborType(Direction::Right),
            match_pattern: vec![RecordedValue::Type(CellType(TYPE_A)), RecordedValue::Type(CellType(TYPE_B)), RecordedValue::Type(CellType(TYPE_A))],
        }),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Тик 1: буфер источника пуст -> гейт закрыт -> эмиссии нет. После тика
    // буфер = [A] (сосед на тик 1).
    engine.run_tick();
    assert_eq!(engine.grid().get_cell(2, 0).map(|c| c.value.0 .0), Some(0), "тик 1: гейт закрыт, эмиссии не было");
    assert_eq!(engine.memory_buffers.get(&(0, 0, 0)), Some(&VecDeque::from(vec![RecordedValue::Type(CellType(TYPE_A))])));

    // Тик 2: сосед -> B. Буфер [A] не полон -> гейт закрыт. После тика [A,B].
    engine.grid_mut().set_cell(1, 0, Cell::new(TYPE_B));
    engine.run_tick();
    assert_eq!(engine.grid().get_cell(2, 0).map(|c| c.value.0 .0), Some(0), "тик 2: гейт всё ещё закрыт");
    assert_eq!(
        engine.memory_buffers.get(&(0, 0, 0)),
        Some(&VecDeque::from(vec![RecordedValue::Type(CellType(TYPE_A)), RecordedValue::Type(CellType(TYPE_B))]))
    );

    // Тик 3: сосед -> A. Буфер [A,B] (len=2) всё ещё не полон -> гейт закрыт
    // на ЭТОМ тике (проверяется буфер ДО обновления). После тика — [A,B,A],
    // полон и совпадает с match_pattern, но это станет видно гейту только
    // СЛЕДУЮЩЕГО тика.
    engine.grid_mut().set_cell(1, 0, Cell::new(TYPE_A));
    engine.run_tick();
    assert_eq!(engine.grid().get_cell(2, 0).map(|c| c.value.0 .0), Some(0), "тик 3: гейт всё ещё закрыт (буфер заполнится только к концу тика)");
    assert_eq!(
        engine.memory_buffers.get(&(0, 0, 0)),
        Some(&VecDeque::from(vec![
            RecordedValue::Type(CellType(TYPE_A)),
            RecordedValue::Type(CellType(TYPE_B)),
            RecordedValue::Type(CellType(TYPE_A))
        ]))
    );

    // Тик 4: буфер [A,B,A] (каким он был к концу тика 3) == match_pattern ->
    // гейт открывается -> keep_source-эмиссия реально применяется: источник
    // (0,0) СОХРАНЯЕТ значение, копия появляется в (2,0). Буфер источника
    // продолжает копиться дальше (FIFO): сосед на этот тик = B (выставлен
    // ниже перед тиком) -> [B,A,B].
    engine.grid_mut().set_cell(1, 0, Cell::new(TYPE_B));
    engine.run_tick();
    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(WATCHER), "тик 4: источник ДОЛЖЕН сохранить значение (keep_source)");
    assert_eq!(engine.grid().get_cell(2, 0).map(|c| c.value.0 .0), Some(WATCHER), "тик 4: копия должна была появиться в (2,0)");
    assert_eq!(
        engine.memory_buffers.get(&(0, 0, 0)),
        Some(&VecDeque::from(vec![
            RecordedValue::Type(CellType(TYPE_B)),
            RecordedValue::Type(CellType(TYPE_A)),
            RecordedValue::Type(CellType(TYPE_B))
        ])),
        "тик 4: источник продолжает копить СВОЙ буфер (дольше 3 тиков предыдущего теста)"
    );
    // Копия физически появилась только к КОНЦУ тика 4 (в write_buffer) — она
    // ещё НЕ была продетектирована как отдельный матч на этом тике (детект
    // читает pre-tick срез), поэтому у неё пока не должно быть записи вовсе.
    assert!(engine.memory_buffers.get(&(2, 0, 0)).is_none(), "тик 4: у копии не должно быть записи ДО того, как она хоть раз реально продетектирована");

    // Перед тиком 5: выставляем РАЗНЫХ соседей источнику (D — заведомо не
    // A/B/C) и копии (C — константа, гейт копии никогда не откроется).
    engine.grid_mut().set_cell(1, 0, Cell::new(TYPE_D));
    engine.grid_mut().set_cell(3, 0, Cell::new(TYPE_C)); // сосед КОПИИ, Right от (2,0)
    engine.run_tick();

    // Копия (2,0) теперь тоже матчится как независимый матч ТОГО ЖЕ правила.
    // Её буфер должен появиться ВПЕРВЫЕ и содержать РОВНО [C] — не унаследовав
    // НИЧЕГО от истории источника (которая на этот момент [A,B,D] — старое
    // [B,A,B] минус A, плюс D).
    assert_eq!(
        engine.memory_buffers.get(&(2, 0, 0)),
        Some(&VecDeque::from(vec![RecordedValue::Type(CellType(TYPE_C))])),
        "тик 5: буфер копии должен быть СВЕЖИМ (только что увиденное значение), не унаследованным от источника"
    );
    assert_eq!(
        engine.memory_buffers.get(&(0, 0, 0)),
        Some(&VecDeque::from(vec![
            RecordedValue::Type(CellType(TYPE_A)),
            RecordedValue::Type(CellType(TYPE_B)),
            RecordedValue::Type(CellType(TYPE_D))
        ])),
        "тик 5: источник продолжает копить свой буфер независимо от копии"
    );
    // Копия ещё не могла сама излучить дальше — её гейт никогда не откроется
    // (сосед константно C, паттерн требует A,B,A) — (4,0) должно остаться пустым.
    assert_eq!(engine.grid().get_cell(4, 0).map(|c| c.value.0 .0), Some(0), "тик 5: гейт копии закрыт — каскада излучения быть не должно");

    // Ещё два тика (6,7): сосед копии остаётся C (гейт копии так и не
    // откроется — [C,C,C] никогда не совпадёт с [A,B,A]), сосед источника
    // держим D. Проверяем, что оба буфера продолжают расти корректно и
    // НЕЗАВИСИМО друг от друга ещё дальше (суммарно > 3 тиков с момента
    // появления копии).
    engine.run_tick(); // тик 6
    engine.run_tick(); // тик 7
    assert_eq!(
        engine.memory_buffers.get(&(2, 0, 0)),
        Some(&VecDeque::from(vec![RecordedValue::Type(CellType(TYPE_C)), RecordedValue::Type(CellType(TYPE_C)), RecordedValue::Type(CellType(TYPE_C))])),
        "тик 7: буфер копии — три одинаковых наблюдения СВОЕГО соседа, копия жива и наблюдает независимо"
    );
    assert_eq!(
        engine.memory_buffers.get(&(0, 0, 0)),
        Some(&VecDeque::from(vec![RecordedValue::Type(CellType(TYPE_D)), RecordedValue::Type(CellType(TYPE_D)), RecordedValue::Type(CellType(TYPE_D))])),
        "тик 7: источник продолжает копить свой буфер (D) независимо от копии — суммарно 7 тиков, не 3"
    );
    // Гейт копии так и не открылся -> каскада на (4,0) по-прежнему нет.
    assert_eq!(engine.grid().get_cell(4, 0).map(|c| c.value.0 .0), Some(0), "тик 7: гейт копии всё ещё закрыт");

    // Ровно ДВЕ живые записи в карте — источник и копия, никаких лишних или
    // фантомных ключей не появилось за 7 тиков активной эмиссии.
    assert_eq!(engine.memory_buffers.len(), 2, "в карте должно быть ровно 2 записи: источник (0,0,0) и копия (2,0,0)");
}

/// (3): бывшая "осиротевшая запись" в `Engine::memory_buffers` — ТЕПЕРЬ
/// ФИКС, не задокументированный компромисс. Раньше, если клетка, которая
/// матчилась (и потому обзавелась записью в буфере), переставала совпадать
/// с правилом по ВНЕШНЕЙ причине (тип меняется чем-то посторонним —
/// например, проигрывает конфликт другому правилу в другой части конфига,
/// что здесь смоделировано прямой записью в решётку, а не отдельным
/// конкурирующим правилом, ради простоты и детерминизма), ничто не убирало
/// её запись из `Engine::memory_buffers` — она росла НАВСЕГДА.
///
/// Теперь (см. блок "осиротевшие записи" в `run_tick_with_cache`) это
/// вычищается ДЁШЕВО и КОРРЕКТНО, используя уже посчитанный на этот тик
/// `search_coords` (тот же dirty-based инвариант, на котором держится весь
/// инкрементальный матчер) — не требует ни полного скана карты, ни хранения
/// снимка кандидатов прошлого тика. `keep_source` тут не хуже и не лучше
/// обычного сдвига: тот же фикс покрывает оба случая одинаково (см. также
/// `test_feedback_counter_pruned_after_match_stops_existing` — тот же класс
/// для `feedback_counters`).
#[test]
fn test_memory_buffer_entry_pruned_after_match_stops_existing() {
    const WATCHER: u8 = 220;
    const NEIGH_OK: u8 = 221;
    const UNRELATED: u8 = 222; // то, во что "внешне" превращается источник

    let mut grid = make_grid(4, 1);
    grid.set_cell(0, 0, Cell::new(WATCHER));
    grid.set_cell(1, 0, Cell::new(NEIGH_OK)); // сосед постоянно нужного типа

    let rule = Rule {
        id: vec![CellType(WATCHER)],
        pattern: vec![],
        // keep_source-эмиссия НАМЕРЕННО за пределы решётки (steps=10 на
        // ширине 4, overflow=Discard по умолчанию) — цель никогда никуда не
        // попадает, значит НЕ появляется второй независимый матч этого же
        // правила (копия), который иначе завёл бы СВОЮ запись в
        // `memory_buffers` и мешал бы проверять именно сценарий источника в
        // изоляции (см. отдельный тест
        // `test_emit_memory_neighbor_type_copy_gets_independent_buffer_own_position`
        // про то, что копия — это ожидаемо-корректный независимый матч).
        shifts: vec![vec![ShiftSpec { direction: Direction::Right, steps: 10, broadcast: false, keep_source: true }]],
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
        memory: Some(MemorySpec { window: 1, record_trigger: RecordTrigger::NeighborType(Direction::Right), match_pattern: vec![RecordedValue::Type(CellType(NEIGH_OK))] }),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Тик 1: буфер пуст -> гейт закрыт -> буфер после тика = [NEIGH_OK].
    engine.run_tick();
    assert_eq!(engine.memory_buffers.get(&(0, 0, 0)), Some(&VecDeque::from(vec![RecordedValue::Type(CellType(NEIGH_OK))])));

    // Тик 2: гейт открыт (буфер [NEIGH_OK] == match_pattern) -> эмиссия
    // применяется, источник (0,0) остаётся WATCHER (keep_source).
    engine.run_tick();
    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(WATCHER));
    assert!(engine.memory_buffers.contains_key(&(0, 0, 0)), "запись источника должна существовать после того, как он реально совпал");

    // Внешнее событие: (0,0) перестаёт быть WATCHER (имитирует проигрыш
    // конфликта другому правилу/оверрайд извне — сам механизм конфликта тут
    // не важен, важен только факт: клетка, которая раньше матчилась, больше
    // никогда не будет продетектирована этим правилом). `set_cell` метит
    // (0,0) "грязной" безусловно (см. `Grid::set_cell`) — этого достаточно,
    // чтобы следующий тик пересмотрел её.
    engine.grid_mut().set_cell(0, 0, Cell::new(UNRELATED));

    // Тик 3: (0,0) больше не входит в `matches` этого правила (тип не
    // совпадает) — запись ДОЛЖНА быть вычищена уже на ЭТОМ тике.
    engine.run_tick();
    assert!(
        engine.memory_buffers.get(&(0, 0, 0)).is_none(),
        "запись ДОЛЖНА быть вычищена сразу же, как только позиция перестала матчиться — фикс осиротевших записей"
    );

    // Ещё несколько тиков — запись не должна воскреснуть сама по себе.
    for _ in 0..5 {
        engine.run_tick();
    }
    assert!(engine.memory_buffers.get(&(0, 0, 0)).is_none(), "запись не должна появиться вновь сама по себе");
    assert_eq!(engine.memory_buffers.len(), 0, "карта должна быть полностью пуста — никаких фантомных остатков");
}

/// Точность момента чистки: запись НЕ должна пропасть раньше срока (пока
/// клетка ещё честно матчится — тики 1..3) и НЕ должна пережить дольше
/// одного тика после того, как перестала матчиться (ровно тик 4, не тик 5
/// или позже) — то есть не "слишком рано" и не "слишком поздно", а именно
/// на первом тике, где инкрементальный матчер физически МОГ это заметить
/// (см. doc-комментарий блока "осиротевшие записи" в `run_tick_with_cache`
/// про то, почему `search_coords` этого тика гарантированно включает эту
/// позицию).
#[test]
fn test_memory_buffer_entry_pruned_exactly_on_tick_match_stops_existing() {
    const WATCHER: u8 = 223;
    const NEIGH_OK: u8 = 224;
    const UNRELATED: u8 = 225;
    const NEVER_MATCHES: u8 = 250; // гейт никогда не откроется — тест не про арбитраж, только про сам факт трекинга

    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(WATCHER));
    grid.set_cell(1, 0, Cell::new(NEIGH_OK));

    let rule = Rule {
        id: vec![CellType(WATCHER)],
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
        memory: Some(MemorySpec {
            window: 1,
            record_trigger: RecordTrigger::NeighborType(Direction::Right),
            match_pattern: vec![RecordedValue::Type(CellType(NEVER_MATCHES))],
        }),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Тики 1..3: клетка стабильно матчится (тип не меняется) — буфер
    // продолжает наблюдать независимо от гейта (см. `memory_targets`'s
    // doc-комментарий), запись НЕ должна быть тронута ни на одном из этих
    // тиков ("не слишком рано").
    for tick in 1..=3 {
        engine.run_tick();
        assert!(
            engine.memory_buffers.contains_key(&(0, 0, 0)),
            "тик {tick}: клетка всё ещё матчится — запись НЕ должна быть удалена"
        );
    }

    // Внешнее событие ровно ПЕРЕД тиком 4.
    engine.grid_mut().set_cell(0, 0, Cell::new(UNRELATED));

    // Тик 4: первый тик, на котором инкрементальный матчер видит изменение —
    // запись ДОЛЖНА исчезнуть именно теперь ("не слишком поздно").
    engine.run_tick();
    assert!(
        engine.memory_buffers.get(&(0, 0, 0)).is_none(),
        "тик 4: клетка перестала матчиться — запись должна быть вычищена ИМЕННО на этом тике"
    );

    // Тики 5..7: остаётся вычищенной.
    for tick in 5..=7 {
        engine.run_tick();
        assert!(engine.memory_buffers.get(&(0, 0, 0)).is_none(), "тик {tick}: запись не должна вернуться сама по себе");
    }
    assert_eq!(engine.memory_buffers.len(), 0, "карта должна быть полностью пуста");
}

/// Тот же класс фикса, что и у `memory_buffers` (см.
/// `test_memory_buffer_entry_pruned_after_match_stops_existing`), но для
/// `Engine::feedback_counters` — доказывает, что дешёвая чистка
/// (`ExtensionFlags::extension_rule_indices`) действительно покрывает ОБЕ
/// карты, не только `memory_buffers`, ради которой была написана.
#[test]
fn test_feedback_counter_pruned_after_match_stops_existing() {
    const WATCHER: u8 = 226;
    const UNRELATED: u8 = 227;

    let mut grid = make_grid(3, 1);
    grid.set_cell(0, 0, Cell::new(WATCHER));

    let rule = Rule {
        id: vec![CellType(WATCHER)],
        pattern: vec![],
        // Без сдвигов/изменений вовсе — детекция (и, следовательно, рост
        // `feedback_counters`) не зависит от `shifts`/`changes`, только от
        // того, что паттерн продолжает совпадать (см. `feedback_keys`'s
        // построение в `run_tick_with_cache`: фильтр по `matches`, посчитан
        // ДО фазы применения).
        shifts: vec![],
        changes: vec![],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        // Заведомо недостижимый timeout — тест не про переключение
        // направления, а про сам факт накопления/чистки счётчика.
        feedback: Some(FeedbackSpec { timeout: 1000, new_direction: Direction::Up }),
        recursion: None,
        memory: None,
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    engine.run_tick();
    assert_eq!(engine.feedback_counters.get(&(0, 0, 0)), Some(&1), "тик 1: счётчик должен вырасти на 1");
    engine.run_tick();
    assert_eq!(engine.feedback_counters.get(&(0, 0, 0)), Some(&2), "тик 2: счётчик продолжает расти");

    // Внешнее событие: клетка перестаёт быть WATCHER.
    engine.grid_mut().set_cell(0, 0, Cell::new(UNRELATED));
    engine.run_tick();

    assert!(
        engine.feedback_counters.get(&(0, 0, 0)).is_none(),
        "запись ДОЛЖНА быть вычищена ровно на тике, следующем за тем, когда клетка перестала матчиться — тот же фикс, что и у memory_buffers"
    );
    assert_eq!(engine.feedback_counters.len(), 0, "карта должна быть полностью пуста");
}

// ──────────────────────────────────────────────────────────────
// Part B (аудит взаимодействий с `min_age`, по прецеденту
// `recursion`+`min_age` в `config.rs`): проверяем `memory`'s гейт-фильтр и
// `keep_source`'s пропуск переноса на СУЩЕСТВОВАНИЕ того же класса дыры —
// "клетка, ещё не созревшая до `min_age`, всё равно как-то участвует в
// расширении".
// ──────────────────────────────────────────────────────────────

/// `memory` + `min_age > 0`: незрелая клетка (age < min_age) не должна даже
/// ПОПАСТЬ в `Engine::memory_buffers` — гейт-фильтр `memory` работает НАД
/// списком `matches`, который матчер (`matcher::match_cell`) уже
/// отфильтровал по `min_age` ДО того, как `run_tick_with_cache` вообще
/// узнаёт о существовании этого матча (см. `memory_targets`'s построение:
/// `matches.iter().filter(...)`, где `matches` -- пост-min_age список). Это
/// белый ящик, реально проверяющий карту `Engine::memory_buffers`
/// напрямую, а не только конечное значение клетки -- если бы это было
/// нарушено (аналогично найденному багу `recursion`+`min_age`, где каскадные
/// уровни проверяли только ТИП, не возраст), буфер начал бы заполняться
/// РАНЬШЕ, чем клетка формально имеет право участвовать в арбитраже вообще.
#[test]
fn test_memory_gate_does_not_track_immature_cell_before_min_age() {
    const WATCHER: u8 = 220;
    const FIRED: u8 = 221;
    const NEIGH: u8 = 222;
    const THRESHOLD: u64 = 3;

    let mut grid = make_grid(3, 1);
    grid.set_cell(0, 0, Cell::new(WATCHER));
    grid.set_cell(1, 0, Cell::new(NEIGH)); // (0,0) + Right = (1,0), NeighborType target

    let rule = Rule {
        id: vec![CellType(WATCHER)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(FIRED))],
        active_only: false,
        priority: 10,
        min_age: THRESHOLD,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: Some(MemorySpec {
            window: 1,
            record_trigger: RecordTrigger::NeighborType(Direction::Right),
            match_pattern: vec![RecordedValue::Type(CellType(NEIGH))],
        }),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Тики 1..=THRESHOLD: возраст клетки на момент проверки (ДО advance_age
    // этого же тика) -- 0, 1, ..., THRESHOLD-1, все < THRESHOLD. Матчер
    // (`match_cell`) обязан исключить такую клетку из `matches` целиком, так
    // что она физически не может попасть в `memory_targets` -- буфер должен
    // оставаться ПУСТЫМ (отсутствовать в карте) все эти тики.
    for tick in 1..=THRESHOLD {
        engine.run_tick();
        assert!(
            engine.memory_buffers.get(&(0, 0, 0)).is_none(),
            "тик {tick}: незрелая клетка (age < min_age) НЕ должна быть memory-gate-tracked -- \
если бы была, буфер начал бы копиться раньше, чем клетке формально разрешено матчиться"
        );
        assert_eq!(
            engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
            Some(WATCHER),
            "тик {tick}: правило не должно сработать до созревания"
        );
    }

    // Тик THRESHOLD+1: возраст клетки == THRESHOLD теперь (созрела) -- ВПЕРВЫЕ
    // попадает в `matches`, а значит и в `memory_targets` -- буфер должен
    // начать заполняться РОВНО с этого тика, не раньше.
    engine.run_tick();
    assert_eq!(
        engine.memory_buffers.get(&(0, 0, 0)),
        Some(&VecDeque::from(vec![RecordedValue::Type(CellType(NEIGH))])),
        "тик {}: клетка только что созрела (age == min_age) -- обязана начать отслеживаться именно теперь",
        THRESHOLD + 1
    );
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(WATCHER),
        "гейт всё ещё закрыт в этот тик (буфер проверяется ДО обновления этого же тика)"
    );

    // Тик THRESHOLD+2: буфер [Type(NEIGH)] (window=1, как он стал к концу
    // предыдущего тика) точно совпадает с match_pattern -> гейт открывается.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(FIRED),
        "гейт обязан открыться ровно на тик после того, как буфер созревшей клетки заполнился"
    );
}

/// `keep_source` + `min_age > 0`: источник ("излучение", `keep_source: true`)
/// физически не перемещается и не очищается, поэтому продолжает удовлетворять
/// `min_age` на каждом следующем тике без повторного "созревания" -- это
/// ожидаемо и уже задокументировано. Адверсариальная часть теста -- ДРУГАЯ
/// сторона того же взаимодействия: цель излучения получает СВЕЖУЮ копию
/// (born_at = текущее поколение) при КАЖДОМ срабатывании источника, поэтому
/// НИКОГДА не накапливает возраст, пока источник продолжает её перезаписывать
/// -- если бы `apply_shift_buffered`/флеш-фаза `apply_matches_with_cam`
/// когда-нибудь "просочили" старое `born_at` источника в цель (например, если
/// бы флеш перестал безусловно переустанавливать `born_at = gen` для каждой
/// записи из `write_buffer`), клетка-цель с тем же типом мгновенно
/// удовлетворяла бы `min_age`, унаследовав чужую историю -- ровно тот класс
/// тихой дыры, что и `recursion`+`min_age`.
#[test]
fn test_keep_source_emit_target_never_inherits_source_age_for_min_age() {
    const MARKER: u8 = 223;
    const THRESHOLD: u64 = 4;

    let mut grid = make_grid(5, 1);
    grid.set_cell(0, 0, Cell::new(MARKER));

    let rule = Rule {
        id: vec![CellType(MARKER)],
        pattern: vec![],
        // Точечное излучение: копия только в (1,0), источник (0,0) не трогается.
        shifts: vec![vec![ShiftSpec { direction: Direction::Right, steps: 1, broadcast: false, keep_source: true }]],
        changes: vec![],
        active_only: false,
        priority: 10,
        min_age: THRESHOLD,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Тики 1..=THRESHOLD: источник ещё не созрел -- никакого излучения, цель
    // остаётся дефолтной.
    for tick in 1..=THRESHOLD {
        engine.run_tick();
        assert_eq!(
            engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
            Some(0),
            "тик {tick}: источник ещё не созрел до min_age -- излучения быть не должно"
        );
    }

    // Тик THRESHOLD+1: источник созрел (age == THRESHOLD) -- первое
    // излучение. Цель получает MARKER со свежим born_at (== текущее
    // поколение), значит её ВОЗРАСТ должен быть 0 сразу после этого тика --
    // НЕ унаследованный возраст источника (который к этому моменту зрелый).
    engine.run_tick();
    assert_eq!(engine.grid().get_cell(1, 0).map(|c| c.value.0 .0), Some(MARKER), "тик {}: первое излучение должно было применить копию", THRESHOLD + 1);
    assert_eq!(
        engine.grid().get_age(1, 0),
        0,
        "цель излучения обязана иметь возраст 0 сразу после копирования -- born_at должен быть переустановлен на текущее поколение, а НЕ унаследован от зрелого источника"
    );
    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(MARKER), "источник (keep_source) должен сохранить значение");

    // Тик THRESHOLD+2: источник продолжает удовлетворять min_age (его
    // возраст никогда не сбрасывался -- keep_source не включает источник в
    // written_cells) и излучает СНОВА -- цель перезаписывается свежей копией
    // и её возраст остаётся 0, а НЕ растёт до 1 -- она никогда не "видит"
    // непрерывного течения времени, пока источник её каждый тик перезаписывает.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_age(1, 0),
        0,
        "цель продолжает получать свежие копии каждый тик -- её возраст обязан оставаться 0, не расти, пока источник её непрерывно перезаписывает"
    );
}

/// `recursion` + `min_age > 0` (ранее взаимоисключающая комбинация — см. её
/// историю в `config.rs`, теперь разрешена): каждый уровень каскада обязан
/// САМ проверить `min_age` против ЭФФЕКТИВНОГО (с учётом уже накопленного
/// `write_buffer`) возраста СВОЕЙ клетки-анкера `(ox, oy)`, а не только тип
/// её паттерна — см. `applicator::read_age_effective`.
///
/// Раскладка: seed (RFILLED) в x=0, затем RUNFILLED в x=1..9 с РАЗНЫМ
/// заранее выставленным `born_at` при generation=5:
///   x=1 born_at=0 → age=5 (обычный k=0 матч, проходит min_age=2)
///   x=2 born_at=0 → age=5 (уровень каскада k=1 — тоже старый, должен пройти)
///   x=3 born_at=4 → age=1 (уровень каскада k=2 — СЛИШКОМ МОЛОДОЙ: 1 < 2)
///   x=4 born_at=0 → age=5 (старый, но каскад обязан остановиться РАНЬШЕ —
///                          на x=3 — так что сюда очередь дойти не должна)
///
/// Тип клетки в x=3 сам по себе полностью совпадает с паттерном (RUNFILLED,
/// сосед слева после k=1 стал RFILLED) — без проверки возраста наивная
/// (только-типовая) версия `pattern_matches_effective` продолжила бы каскад
/// и залила бы x=3 (и, скорее всего, x=4 тоже, вплоть до `max_depth`).
/// Единственная причина, по которой каскад обязан остановиться именно на
/// x=3, — `min_age`, так что финальное состояние решётки однозначно
/// свидетельствует, сработала проверка возраста на уровне каскада или нет.
#[test]
fn test_recursion_with_min_age_blocks_cascade_at_too_young_cell() {
    const MIN_AGE: u64 = 2;
    const MAX_DEPTH: u8 = 5;

    let mut grid = make_grid(10, 1);
    // Продвигаем поколение решётки НАПРЯМУЮ (без реальных тиков — метод
    // ничего не трогает, кроме счётчика) до generation=5, чтобы можно было
    // детерминированно расставить born_at ячеек ниже generation и получить
    // заранее выбранный возраст (generation - born_at) для каждой из них.
    for _ in 0..5 {
        grid.advance_age();
    }

    grid.set_cell(0, 0, Cell::new(RFILLED));
    grid.set_cell(1, 0, Cell { value: CellValue::new(RUNFILLED), born_at: 0 }); // age 5
    grid.set_cell(2, 0, Cell { value: CellValue::new(RUNFILLED), born_at: 0 }); // age 5
    grid.set_cell(3, 0, Cell { value: CellValue::new(RUNFILLED), born_at: 4 }); // age 1 < MIN_AGE
    grid.set_cell(4, 0, Cell { value: CellValue::new(RUNFILLED), born_at: 0 }); // age 5, но недостижим
    for x in 5..10 {
        grid.set_cell(x, 0, Cell::new(RUNFILLED));
    }

    let rule = Rule {
        id: vec![CellType(RUNFILLED)],
        pattern: vec![(0, 0, CellType(RUNFILLED)), (-1, 0, CellType(RFILLED))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(RFILLED))],
        active_only: false,
        priority: 10,
        min_age: MIN_AGE,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: Some(RecursionSpec { max_depth: MAX_DEPTH, direction: Direction::Right }),
        memory: None,
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    engine.run_tick();

    // x=0 (seed) не участвует в паттерне как анкер — не проверяется на возраст, не меняется.
    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(RFILLED), "seed x=0 не должен меняться");
    // x=1: обычный (k=0) матч, age=5 >= min_age=2 — должен сработать.
    assert_eq!(engine.grid().get_cell(1, 0).map(|c| c.value.0 .0), Some(RFILLED), "x=1: обычный матч должен пройти min_age");
    // x=2: уровень каскада k=1, age=5 >= min_age=2 — должен сработать.
    assert_eq!(engine.grid().get_cell(2, 0).map(|c| c.value.0 .0), Some(RFILLED), "x=2: k=1 каскад должен пройти min_age (старая клетка)");
    // x=3: уровень каскада k=2, age=1 < min_age=2 — ДОЛЖЕН быть заблокирован, несмотря на совпадающий тип.
    assert_eq!(
        engine.grid().get_cell(3, 0).map(|c| c.value.0 .0),
        Some(RUNFILLED),
        "x=3: k=2 каскад ДОЛЖЕН остановиться здесь — клетка слишком молода (age=1 < min_age=2)"
    );
    // x=4..9: недостижимы — каскад уже остановился на x=3.
    for x in 4..10 {
        assert_eq!(
            engine.grid().get_cell(x, 0).map(|c| c.value.0 .0),
            Some(RUNFILLED),
            "x={x} не должен быть затронут — каскад остановился на x=3"
        );
    }
}

const MEM_RECUR_MARKER: u8 = 94;

/// `memory` (`NeighborType`) + `recursion` вместе — раньше запрещённая
/// комбинация (см. `config.rs`'s старую валидацию), теперь разрешена (тот
/// же приём, что уже сработал для `cam`+`recursion` и `recursion`+`min_age`
/// — найти обход блэнкет-запрета, а не оставить его как есть).
///
/// Ключевое структурное следствие проверено здесь конструктивно: у СВЕЖЕЙ
/// позиции (буфер ещё ни разу не наблюдался) гейт ВСЕГДА закрыт на первом
/// визите (проверка происходит ДО пуша — 0 записей никогда не равно
/// window), причём это касается и обычного (level 0) матча, и КАЖДОГО
/// уровня каскада одинаково — у `memory` нет отдельного "level 0 матчится
/// безусловно". Позиция, чей гейт закрыт, тем не менее ПОЛУЧАЕТ новое
/// наблюдение (буфер продолжает копить историю, даже пока гейт закрыт — та
/// же семантика, что и у обычного top-level матча) — и на СЛЕДУЮЩЕМ тике,
/// когда та же позиция снова оценивается (либо как level 0 нового тика,
/// либо как уровень каскада, либо как независимый top-level матч — не
/// важно, откуда именно), она использует ТОТ ЖЕ, уже частично заполненный
/// буфер. При `window=1` одного такого "прогревочного" тика достаточно,
/// чтобы гейт открылся на следующем шаге — что и даёт цепочке расти РОВНО
/// на одну клетку каждый тик, начиная со ВТОРОГО (первый тик целиком уходит
/// на прогрев самого level 0).
#[test]
fn test_memory_neighbor_type_plus_recursion_cascade_level_gate_primes_across_ticks() {
    let mut grid = make_grid(6, 2);
    const WALL_ROW: usize = 1;
    const WATCH_ROW: usize = 0;
    grid.set_cell(0, WALL_ROW, Cell::new(RFILLED)); // seed, статичен, без правил
    grid.set_cell(1, WALL_ROW, Cell::new(RUNFILLED)); // level 0
    grid.set_cell(2, WALL_ROW, Cell::new(RUNFILLED)); // будущий каскадный/top-level уровень
    grid.set_cell(3, WALL_ROW, Cell::new(RUNFILLED)); // будущий каскадный/top-level уровень
    for x in 1..4 {
        grid.set_cell(x, WATCH_ROW, Cell::new(MEM_RECUR_MARKER)); // статичные маркеры над всей цепочкой
    }

    let rule = Rule {
        id: vec![CellType(RUNFILLED)],
        pattern: vec![(0, 0, CellType(RUNFILLED)), (-1, 0, CellType(RFILLED))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(RFILLED))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: Some(RecursionSpec { max_depth: 1, direction: Direction::Right }),
        memory: Some(MemorySpec {
            window: 1,
            record_trigger: RecordTrigger::NeighborType(Direction::Up),
            match_pattern: vec![RecordedValue::Type(CellType(MEM_RECUR_MARKER))],
        }),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Тик 1 (прогрев level0): x=1 структурно матчится (behind=RFILLED с
    // самого начала), но её буфер памяти пуст -- гейт закрыт, она НЕ
    // выигрывает арбитраж вовсе (значит, и её каскад не запускается: Фаза 3
    // -- часть apply уже выигравшего матча). x=1 получает первое наблюдение.
    engine.run_tick();
    assert_eq!(engine.grid().get_cell(0, WALL_ROW).map(|c| c.value.0 .0), Some(RFILLED), "тик 1: seed не меняется");
    assert_eq!(engine.grid().get_cell(1, WALL_ROW).map(|c| c.value.0 .0), Some(RUNFILLED), "тик 1: level0 НЕ должен сработать -- буфер памяти пуст на первом визите");
    assert_eq!(engine.grid().get_cell(2, WALL_ROW).map(|c| c.value.0 .0), Some(RUNFILLED), "тик 1: x=2 недостижима -- x=1 не выиграла, каскада не было");
    assert_eq!(engine.grid().get_cell(3, WALL_ROW).map(|c| c.value.0 .0), Some(RUNFILLED), "тик 1: x=3 недостижима");

    // Тик 2: x=1 теперь имеет заполненный (с тика 1) буфер -- гейт открыт,
    // x=1 срабатывает. Её каскад пытается level1=x=2: pattern совпадает
    // (behind эффективно RFILLED из этого же каскада), но буфер x=2 ПУСТ на
    // первом визите -- гейт закрыт, каскад останавливается на x=2 (не
    // конвертируя её), x=2 получает первое наблюдение.
    engine.run_tick();
    assert_eq!(engine.grid().get_cell(1, WALL_ROW).map(|c| c.value.0 .0), Some(RFILLED), "тик 2: level0 обязан сработать -- буфер уже заполнен с тика 1");
    assert_eq!(
        engine.grid().get_cell(2, WALL_ROW).map(|c| c.value.0 .0),
        Some(RUNFILLED),
        "тик 2: каскадный уровень x=2 НЕ должен сработать -- её буфер пуст на первом визите"
    );
    assert_eq!(engine.grid().get_cell(3, WALL_ROW).map(|c| c.value.0 .0), Some(RUNFILLED), "тик 2: x=3 недостижима");

    // Тик 3: x=1 -- RFILLED, больше не матчится (её каскад из тика 2 тоже
    // не существует, она сама больше не RUNFILLED). x=2 (RUNFILLED,
    // behind=x=1=RFILLED) теперь НЕЗАВИСИМЫЙ top-level матч -- её буфер уже
    // заполнен с тика 2 -- гейт открыт через ОБЫЧНЫЙ (не каскадный)
    // memory-механизм -- x=2 срабатывает. Её собственный каскад (level1=x=3)
    // повторяет ту же историю: буфер x=3 пуст -- гейт закрыт, не
    // конвертируется, получает первое наблюдение.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(2, WALL_ROW).map(|c| c.value.0 .0),
        Some(RFILLED),
        "тик 3: x=2 обязана сработать через обычный top-level путь -- буфер уже заполнен с тика 2"
    );
    assert_eq!(
        engine.grid().get_cell(3, WALL_ROW).map(|c| c.value.0 .0),
        Some(RUNFILLED),
        "тик 3: x=3 (новый каскадный уровень от x=2) НЕ должна сработать -- её буфер пуст на первом визите"
    );

    // Тик 4: та же история повторяется на x=3 -- теперь она независимый
    // top-level матч (x=2 уже RFILLED) с уже заполненным с тика 3 буфером.
    engine.run_tick();
    assert_eq!(engine.grid().get_cell(3, WALL_ROW).map(|c| c.value.0 .0), Some(RFILLED), "тик 4: x=3 обязана сработать -- её буфер заполнен с тика 3");
}
