use super::*;
use crate::grid::Grid;
use crate::storage::VecStorage;
use crate::types::{BoundaryBuffer, Cell, CellType, CellValue};
use std::collections::HashSet;

fn make_grid_from_vec(width: usize, height: usize) -> Grid<VecStorage> {
    let storage = VecStorage {
        cells: vec![Cell::default(); width * height],
        width,
        height,
    };
    Grid::new(storage, HashSet::new())
}

#[test]
fn test_deserialize_add_rule() {
    // AddRule: [priority=10, id_len=1, type=5, 255]
    let packet = vec![10, 1, 5, 255];
    let idx = find_terminator(&packet).unwrap();
    let data = &packet[..idx];
    let op = deserialize_packet(data).unwrap();
    match op {
        RuleOp::AddRule(rule) => {
            assert_eq!(rule.id, vec![CellType(5)]);
            assert_eq!(rule.priority, 10);
        }
        _ => panic!("Expected AddRule"),
    }
}

#[test]
fn test_deserialize_remove_rule() {
    // RemoveRule: [0xF0, id_len=1, rule_id=42, 255]
    let packet = vec![0xF0, 1, 42, 255];
    let idx = find_terminator(&packet).unwrap();
    let data = &packet[..idx];
    let op = deserialize_packet(data).unwrap();
    assert_eq!(op, RuleOp::RemoveRule(vec![CellType(42)]));
}

#[test]
fn test_deserialize_clear_all() {
    let packet = vec![0xF1, 255];
    let idx = find_terminator(&packet).unwrap();
    let data = &packet[..idx];
    let op = deserialize_packet(data).unwrap();
    assert_eq!(op, RuleOp::ClearAll);
}

fn make_rule(id: Vec<CellType>, priority: u32, changes: Vec<(i32, i32, crate::types::ChangeValue)>) -> Rule {
    Rule {
        id,
        pattern: vec![],
        shifts: vec![],
        changes,
        active_only: false,
        priority,
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

#[test]
fn test_rule_store_apply_add() {
    let mut store = RuleStore::new();
    let rule = make_rule(
        vec![CellType(1)],
        10,
        vec![(0, 0, crate::types::ChangeValue::Literal(2))],
    );
    assert!(store.apply(CompletedOp {
        op: RuleOp::AddRule(rule)
    }));
    assert_eq!(store.rules().len(), 1);
    assert!(store.dirty);
}

#[test]
fn test_rule_store_apply_remove() {
    let rule = make_rule(
        vec![CellType(1)],
        10,
        vec![(0, 0, crate::types::ChangeValue::Literal(2))],
    );
    let mut store = RuleStore::with_rules(vec![rule]);
    store.dirty = false;
    assert!(store.apply(CompletedOp {
        op: RuleOp::RemoveRule(vec![CellType(1)])
    }));
    assert_eq!(store.rules().len(), 0);
}

#[test]
fn test_rule_store_apply_clear() {
    let rules = vec![
        make_rule(
            vec![CellType(1)],
            10,
            vec![(0, 0, crate::types::ChangeValue::Literal(2))],
        ),
        make_rule(
            vec![CellType(3)],
            5,
            vec![(0, 0, crate::types::ChangeValue::Literal(4))],
        ),
    ];
    let mut store = RuleStore::with_rules(rules);
    store.dirty = false;
    assert!(store.apply(CompletedOp { op: RuleOp::ClearAll }));
    assert_eq!(store.rules().len(), 0);
}

#[test]
fn test_get_index_rebuilds_when_dirty() {
    let rule = make_rule(
        vec![CellType(5)],
        10,
        vec![(0, 0, crate::types::ChangeValue::Literal(6))],
    );
    let mut store = RuleStore::with_rules(vec![rule]);
    store.get_index();

    let new_rule = make_rule(
        vec![CellType(7)],
        5,
        vec![(0, 0, crate::types::ChangeValue::Literal(8))],
    );
    store.apply(CompletedOp {
        op: RuleOp::AddRule(new_rule),
    });
    assert!(store.dirty, "dirty should be set after apply");

    let index = store.get_index();
    assert!(index.contains_key(&CellType(5)), "Index should include original rule");
    assert!(index.contains_key(&CellType(7)), "Index should include new rule");
    assert!(!store.dirty, "dirty should be false after rebuild");
}

#[test]
fn test_deserialize_rejects_255_in_id() {
    // data = [priority=10, id_len=1, type=255]
    let data = vec![10, 1, 0xFF];
    let result = deserialize_packet(&data);
    assert!(result.is_err(), "Should reject 255 in id");
}

#[test]
fn test_drain_rule_channel_basic() {
    let mut grid = make_grid_from_vec(1, 1);
    let mut bb = BoundaryBuffer::new();
    bb.direction = "output".to_string();
    grid.set_boundary(0, 0, bb);
    // Симулируем вывод в граничный буфер (через канал 0)
    if let Some(buf) = grid.get_boundary_mut(0, 0) {
        buf.enqueue(
            0,
            Cell {
                value: CellValue(CellType(10)),
                born_at: 0,
            },
        );
        buf.enqueue(
            0,
            Cell {
                value: CellValue(CellType(1)),
                born_at: 0,
            },
        );
        buf.enqueue(
            0,
            Cell {
                value: CellValue(CellType(5)),
                born_at: 0,
            },
        );
        buf.enqueue(
            0,
            Cell {
                value: CellValue(CellType(255)),
                born_at: 0,
            },
        );
    }

    let mut store = RuleStore::new();
    let ops = store.drain_rule_channel(&mut grid);

    assert_eq!(ops.len(), 1, "Should decode one packet");
    match &ops[0].op {
        RuleOp::AddRule(rule) => {
            assert_eq!(rule.priority, 10);
            assert_eq!(rule.id, vec![CellType(5)]);
        }
        _ => panic!("Expected AddRule"),
    }
}

#[test]
fn test_drain_rule_channel_keeps_independent_boundaries_separate() {
    // Two independent output ports transmitting bytes gradually, with their
    // transmissions overlapping in time (port A's 2nd byte and port B's 1st
    // byte arrive in the same `drain_rule_channel` call). A single shared
    // accumulator (keyed only by channel) would interleave the two byte
    // streams and corrupt both packets — accumulators must be kept separate
    // per physical boundary.
    let mut grid = make_grid_from_vec(10, 1);
    let mut out_a = BoundaryBuffer::new();
    out_a.direction = "output".to_string();
    grid.set_boundary(0, 0, out_a);
    let mut out_b = BoundaryBuffer::new();
    out_b.direction = "output".to_string();
    grid.set_boundary(5, 0, out_b);

    let mut store = RuleStore::new();
    // Packet A: [priority=10, id_len=1, id=50, terminator]
    let packet_a = [10u8, 1, 50, 255];
    // Packet B: [priority=20, id_len=1, id=60, terminator]
    let packet_b = [20u8, 1, 60, 255];

    let mut all_ops = Vec::new();
    for i in 0..packet_a.len() + 1 {
        if i < packet_a.len() {
            grid.get_boundary_mut(0, 0).unwrap().enqueue(
                0,
                Cell {
                    value: CellValue(CellType(packet_a[i])),
                    born_at: 0,
                },
            );
        }
        if i >= 1 && i - 1 < packet_b.len() {
            grid.get_boundary_mut(5, 0).unwrap().enqueue(
                0,
                Cell {
                    value: CellValue(CellType(packet_b[i - 1])),
                    born_at: 0,
                },
            );
        }
        all_ops.extend(store.drain_rule_channel(&mut grid));
    }

    assert_eq!(store.error_stats(), 0, "Neither stream should have been corrupted");
    assert!(
        all_ops
            .iter()
            .any(|o| matches!(&o.op, RuleOp::AddRule(r) if r.id == vec![CellType(50)] && r.priority == 10)),
        "Packet A must decode correctly despite overlapping with B"
    );
    assert!(
        all_ops
            .iter()
            .any(|o| matches!(&o.op, RuleOp::AddRule(r) if r.id == vec![CellType(60)] && r.priority == 20)),
        "Packet B must decode correctly despite overlapping with A"
    );
}

#[test]
fn test_decode_errors_increments_on_bad_packet() {
    let mut grid = make_grid_from_vec(1, 1);
    let mut bb = BoundaryBuffer::new();
    bb.direction = "output".to_string();
    grid.set_boundary(0, 0, bb);
    // Corrupted data: just 255 (empty packet = error)
    if let Some(buf) = grid.get_boundary_mut(0, 0) {
        buf.enqueue(
            0,
            Cell {
                value: CellValue(CellType(255)),
                born_at: 0,
            },
        );
    }

    let mut store = RuleStore::new();
    let ops = store.drain_rule_channel(&mut grid);
    assert!(ops.is_empty(), "No valid packets should be decoded");
    assert_eq!(store.error_stats(), 1, "decode_errors should increment");
}

#[test]
fn test_default_rule_store() {
    let store = RuleStore::default();
    assert_eq!(store.rules().len(), 0);
    assert_eq!(store.error_stats(), 0);
}

#[test]
fn test_error_stats() {
    let store = RuleStore::new();
    assert_eq!(store.error_stats(), 0);
}

// ====================================================================
// Интеграционный тест: RuleStore + Engine (самомодификация)
// ====================================================================

#[test]
fn test_integration_self_modification() {
    use crate::types::ChangeValue;

    // 1) Создаём решётку 3×3 с VecStorage
    let mut grid = make_grid_from_vec(3, 3);

    // 2) Правило 1: id=[7], priority=10, changes=[(0,0,0)]
    let rule1 = Rule {
        id: vec![CellType(7)],
        pattern: vec![],
        shifts: vec![],
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

    // 3) Правило 2: id=[9], priority=5,
    let _rule2 = Rule {
        id: vec![CellType(9)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(42))],
        active_only: false,
        priority: 5,
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

    // 4) RuleStore с правилом 1
    let mut store = RuleStore::with_rules(vec![rule1]);

    // 5) Симулируем внешний пакет: кладём rule2 в граничный буфер
    if grid.get_boundary(0, 0).is_none() {
        let mut bb = BoundaryBuffer::new();
        bb.direction = "output".to_string();
        grid.set_boundary(0, 0, bb);
    }
    if let Some(buf) = grid.get_boundary_mut(0, 0) {
        buf.enqueue(
            0,
            Cell {
                value: CellValue(CellType(5)),
                born_at: 0,
            },
        );
        buf.enqueue(
            0,
            Cell {
                value: CellValue(CellType(1)),
                born_at: 0,
            },
        );
        buf.enqueue(
            0,
            Cell {
                value: CellValue(CellType(9)),
                born_at: 0,
            },
        );
        buf.enqueue(
            0,
            Cell {
                value: CellValue(CellType(255)),
                born_at: 0,
            },
        );
    }

    // 6) Дренируем канал и применяем
    let ops = store.drain_rule_channel(&mut grid);
    assert_eq!(ops.len(), 1, "should decode one AddRule packet");
    for op in ops {
        store.apply(op);
    }

    // 7) Проверяем, что в store теперь два правила
    assert_eq!(store.rules().len(), 2, "should have 2 rules after self-modification");

    // 8) Проверяем, что индекс перестроился и включает новое правило
    let idx = store.get_index();
    assert!(idx.contains_key(&CellType(7)), "index should contain rule1");
    assert!(
        idx.contains_key(&CellType(9)),
        "index should contain rule2 after self-modification"
    );
    assert_eq!(
        idx[&CellType(9)].len(),
        1,
        "index should have exactly 1 rule for CellType(9)"
    );
}

// ====================================================================
// Part A: `feedback`/`recursion`/`memory`/`keep_source` через RuleStore.
//
// Precedent (`ChangeValue::Ref`, see `deserialize_packet`'s doc-comment):
// the packet format is a FIXED byte layout with no
// byte position reserved for these fields, so a rule transmitted through the
// self-modification channel can never carry them -- this is a structural
// impossibility to encode, not a silent loss of already-encoded intent (no
// serializer in this codebase ever attempts to write them out either -- byte
// packets are hand-assembled by the sender, e.g.
// `examples/strength_self_modification*.rs`). These tests prove that the
// SAME honest treatment holds for the four extensions built this session,
// by constructing adversarial packets that push every byte the format has
// room for (including a shift, which is the one place `keep_source` could
// in principle have hidden a bit) and confirming the decoded `Rule` always
// comes back with these fields at their inert defaults -- never silently
// "on" with lost parameters.
// ====================================================================

#[test]
fn test_add_rule_protocol_cannot_express_feedback_recursion_memory_or_keep_source() {
    // AddRule with a shift: [priority=7, id_len=1, id=3, SHIFT_FLAG, dir=3(Right), steps=1, 255]
    // This is the ONLY place in the packet format where a per-shift flag
    // (like `keep_source`) could conceivably be smuggled in -- there is no
    // spare byte/bit anywhere else in the shift triplet.
    let packet = vec![7u8, 1, 3, SHIFT_FLAG, 3, 1, 255];
    let idx = find_terminator(&packet).unwrap();
    let op = deserialize_packet(&packet[..idx]).expect("well-formed AddRule packet must decode");

    let RuleOp::AddRule(rule) = op else {
        panic!("expected AddRule")
    };

    assert!(rule.feedback.is_none(), "protocol has no bytes for FeedbackSpec::timeout/new_direction -- must decode to None, not a corrupted/default Some(_)");
    assert!(
        rule.recursion.is_none(),
        "protocol has no bytes for RecursionSpec::max_depth/direction -- must decode to None"
    );
    assert!(rule.memory.is_none(), "protocol has no bytes for MemorySpec (which is even variable-length: window + match_pattern) -- must decode to None");

    assert_eq!(rule.shifts.len(), 1, "one shift group decoded");
    assert_eq!(rule.shifts[0].len(), 1, "one shift in the group");
    assert!(!rule.shifts[0][0].keep_source, "protocol's shift triplet [SHIFT_FLAG, dir_byte, steps] has no bit for keep_source -- must decode to false ('источник очищается', old behavior), never silently true");
    assert!(!rule.shifts[0][0].broadcast, "same structural limitation as keep_source -- broadcast has no encoding either (already documented above deserialize_packet's shift-parsing loop)");
}

#[test]
fn test_add_rule_protocol_ignores_trailing_bytes_after_shift_rather_than_smuggling_extension_fields() {
    // Adversarial: what if a sender tried to tack extra bytes onto a shift
    // triplet hoping a future/careless parser revision would read them as
    // `keep_source`/`feedback` flags? Extra bytes here are parsed as the
    // START of a `changes` triplet (dx, dy, value) instead -- there is no
    // silent field-stealing, just the ordinary change-parsing path, exactly
    // as documented. This pins down that behavior so it can't regress into
    // an accidental extension-field backdoor.
    // [priority=7, id_len=1, id=3, SHIFT_FLAG, dir=0(Up), steps=2, dx=1, dy=0, value=9, 255]
    let packet = vec![7u8, 1, 3, SHIFT_FLAG, 0, 2, 1, 0, 9, 255];
    let idx = find_terminator(&packet).unwrap();
    let op = deserialize_packet(&packet[..idx]).expect("well-formed AddRule packet must decode");
    let RuleOp::AddRule(rule) = op else {
        panic!("expected AddRule")
    };

    assert!(rule.feedback.is_none() && rule.recursion.is_none() && rule.memory.is_none());
    assert!(!rule.shifts[0][0].keep_source && !rule.shifts[0][0].broadcast);
    assert_eq!(
        rule.changes,
        vec![(1i32, 0i32, crate::types::ChangeValue::Literal(9))],
        "trailing bytes after the shift triplet are parsed as an ordinary `changes` entry, not stolen by any extension field"
    );
}

/// End-to-end through the actual self-modification pipeline (not just the
/// packet parser in isolation): a rule with `feedback`/`recursion`/`memory`/
/// `keep_source` set is transmitted byte-for-byte the same way
/// `Engine::enable_self_modification` would receive it from a boundary
/// buffer, and the rule that lands in `RuleStore`/`rule_index` is checked to
/// confirm none of the four fields survived the round trip -- proving the
/// gap (if any) is at the wire format, not somewhere else in the pipeline
/// that might independently reintroduce them.
#[test]
fn test_self_modification_pipeline_never_materializes_the_four_extensions_on_a_received_rule() {
    let mut grid = make_grid_from_vec(1, 1);
    let mut bb = BoundaryBuffer::new();
    bb.direction = "output".to_string();
    grid.set_boundary(0, 0, bb);

    // Same adversarial packet as above (priority=7, id=[3], one Right shift).
    let packet_bytes: [u8; 7] = [7, 1, 3, SHIFT_FLAG, 3, 1, 255];
    if let Some(buf) = grid.get_boundary_mut(0, 0) {
        for &b in &packet_bytes {
            buf.enqueue(
                0,
                Cell {
                    value: CellValue(CellType(b)),
                    born_at: 0,
                },
            );
        }
    }

    let mut store = RuleStore::new();
    let ops = store.drain_rule_channel(&mut grid);
    assert_eq!(ops.len(), 1, "should decode exactly one AddRule packet");
    for op in ops {
        store.apply(op);
    }

    let idx = store.get_index();
    let received = idx
        .get(&CellType(3))
        .and_then(|rules| rules.first())
        .expect("received rule must be indexed under its head type");

    assert!(
        received.feedback.is_none(),
        "no rule materialized through the self-mod pipeline can carry `feedback` -- the wire format never encoded it"
    );
    assert!(received.recursion.is_none(), "same for `recursion`");
    assert!(received.memory.is_none(), "same for `memory`");
    assert!(!received.shifts[0][0].keep_source, "same for `keep_source`");
}

// ====================================================================
// Part B: extending the wire protocol — `broadcast`, `ChangeValue::Ref`,
// `cam` are now expressible over the self-modification channel.
//
// Before this session, `cam`/`ShiftSpec::broadcast`/`ChangeValue::Ref` were
// structurally unencodable: the packet format had no reserved byte for any
// of them (see the old version of this file's `test_deserialize_...`
// tests, which only ever produced `broadcast: false`/`cam: None`/
// `Literal`-only changes). Two new reserved flag bytes
// (`SHIFT_EXT_FLAG`=0xFD for `broadcast`, `CHANGE_REF_FLAG`=0xFC for `Ref`)
// and one new op-code (`OP_ADD_EXT`=0xF2, for `cam`, which needs a whole
// extra sub-record rather than a single bit) close that gap. These tests
// prove the extension actually round-trips AND that it does not disturb
// decoding of packets that predate it. `feedback`/`recursion`/`memory`/
// `keep_source` remain unencodable (see Part A above) — out of scope for
// this extension, not silently fixed by it.
// ====================================================================

#[test]
fn test_old_hardcoded_packets_still_decode_exactly_as_before() {
    // The exact byte sequences already hardcoded in
    // `examples/strength_self_modification.rs` and
    // `examples/proof_guarded_self_modification.rs` before this session's
    // changes -- confirms neither example needs to change.
    let shift_packet: [u8; 6] = [10, 1, 55, SHIFT_FLAG, 3, 1]; // + terminator handled by caller
    let op = deserialize_packet(&shift_packet).unwrap();
    match op {
        RuleOp::AddRule(rule) => {
            assert_eq!(rule.priority, 10);
            assert_eq!(rule.id, vec![CellType(55)]);
            assert_eq!(rule.shifts.len(), 1);
            assert_eq!(rule.shifts[0].len(), 1);
            assert_eq!(rule.shifts[0][0].steps, 1);
            assert!(!rule.shifts[0][0].broadcast, "old packets never set broadcast");
            assert!(rule.cam.is_none(), "old packets never set cam");
            assert!(rule.changes.is_empty());
        }
        _ => panic!("expected AddRule"),
    }

    let change_packet: [u8; 6] = [10, 1, 50, 0, 0, 77]; // dx=0, dy=0, value=77
    let op2 = deserialize_packet(&change_packet).unwrap();
    match op2 {
        RuleOp::AddRule(rule) => {
            assert_eq!(rule.changes, vec![(0i32, 0i32, crate::types::ChangeValue::Literal(77))]);
        }
        _ => panic!("expected AddRule"),
    }
}

#[test]
fn test_broadcast_shift_roundtrip_via_serializer() {
    let rule = Rule {
        id: vec![CellType(9)],
        pattern: vec![],
        shifts: vec![vec![crate::types::ShiftSpec {
            direction: crate::types::Direction::Right,
            steps: 4,
            broadcast: true,
            keep_source: false,
        }]],
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
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let packet = serialize_add_rule(&rule).expect("broadcast shift should serialize");
    // No `cam`, priority doesn't collide with any op-code -> stays in the
    // compact (non-Extended) AddRule format; broadcast alone never needs
    // OP_ADD_EXT.
    assert_eq!(packet[0], 20, "should use the plain AddRule format, not OP_ADD_EXT");

    let idx = find_terminator(&packet).unwrap();
    let op = deserialize_packet(&packet[..idx]).unwrap();
    match op {
        RuleOp::AddRule(decoded) => {
            assert_eq!(decoded.id, rule.id);
            assert_eq!(decoded.priority, rule.priority);
            assert_eq!(
                decoded.shifts, rule.shifts,
                "broadcast flag must survive the round trip"
            );
        }
        _ => panic!("expected AddRule"),
    }
}

#[test]
fn test_broadcast_shift_packet_hand_assembled() {
    // [priority=7, id_len=1, id=3, SHIFT_EXT_FLAG, dir=3(Right), steps=2, flags=0b1(broadcast), 255]
    let packet = vec![7u8, 1, 3, SHIFT_EXT_FLAG, 3, 2, 0b0000_0001, 255];
    let idx = find_terminator(&packet).unwrap();
    let op = deserialize_packet(&packet[..idx]).unwrap();
    let RuleOp::AddRule(rule) = op else {
        panic!("expected AddRule")
    };
    assert_eq!(rule.shifts.len(), 1);
    assert!(
        rule.shifts[0][0].broadcast,
        "flags bit0 set -> broadcast must decode true"
    );
    assert_eq!(rule.shifts[0][0].steps, 2);
}

#[test]
fn test_change_ref_roundtrip_via_serializer() {
    let rule = Rule {
        id: vec![CellType(4)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![
            (1, 0, crate::types::ChangeValue::Ref(0)),
            (2, 0, crate::types::ChangeValue::Literal(9)),
        ],
        active_only: false,
        priority: 11,
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
    let packet = serialize_add_rule(&rule).expect("Ref change should serialize");
    assert_eq!(packet[0], 11, "Ref alone stays in the plain AddRule format");

    let idx = find_terminator(&packet).unwrap();
    let op = deserialize_packet(&packet[..idx]).unwrap();
    match op {
        RuleOp::AddRule(decoded) => {
            assert_eq!(
                decoded.changes, rule.changes,
                "Ref and Literal changes must both survive the round trip, in order"
            );
        }
        _ => panic!("expected AddRule"),
    }
}

#[test]
fn test_change_ref_packet_hand_assembled() {
    // [priority=7, id_len=1, id=3, CHANGE_REF_FLAG, dx=1, dy=0, ref_index=2, 255]
    let packet = vec![7u8, 1, 3, CHANGE_REF_FLAG, 1, 0, 2, 255];
    let idx = find_terminator(&packet).unwrap();
    let op = deserialize_packet(&packet[..idx]).unwrap();
    let RuleOp::AddRule(rule) = op else {
        panic!("expected AddRule")
    };
    assert_eq!(rule.changes, vec![(1i32, 0i32, crate::types::ChangeValue::Ref(2))]);
}

#[test]
fn test_cam_roundtrip_via_serializer_uses_extended_format() {
    let rule = Rule {
        id: vec![CellType(6)],
        pattern: vec![(0, 0, CellType(6))],
        shifts: vec![],
        changes: vec![],
        active_only: false,
        priority: 30,
        min_age: 0,
        overflow: Default::default(),
        cam: Some(crate::types::CamSearch {
            radius: 5,
            target_type: CellType(2),
        }),
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let packet = serialize_add_rule(&rule).expect("cam rule should serialize");
    assert_eq!(packet[0], OP_ADD_EXT, "cam requires the AddRuleExtended op-code");

    let idx = find_terminator(&packet).unwrap();
    let op = deserialize_packet(&packet[..idx]).unwrap();
    match op {
        RuleOp::AddRule(decoded) => {
            assert_eq!(decoded.priority, 30);
            assert_eq!(decoded.id, vec![CellType(6)]);
            assert_eq!(
                decoded.cam,
                Some(crate::types::CamSearch {
                    radius: 5,
                    target_type: CellType(2)
                })
            );
            assert!(decoded.shifts.is_empty());
        }
        _ => panic!("expected AddRule"),
    }
}

#[test]
fn test_add_rule_extended_hand_assembled_cam_only() {
    // [OP_ADD_EXT, priority=15, id_len=1, id=8, ext_flags=0b1(has_cam), radius=3, target=CellType(1), 255]
    let packet = vec![OP_ADD_EXT, 15, 1, 8, 0b0000_0001, 3, 1, 255];
    let idx = find_terminator(&packet).unwrap();
    let op = deserialize_packet(&packet[..idx]).unwrap();
    let RuleOp::AddRule(rule) = op else {
        panic!("expected AddRule")
    };
    assert_eq!(rule.priority, 15);
    assert_eq!(rule.id, vec![CellType(8)]);
    assert_eq!(
        rule.cam,
        Some(crate::types::CamSearch {
            radius: 3,
            target_type: CellType(1)
        })
    );
    assert!(rule.shifts.is_empty());
    assert!(rule.changes.is_empty());
}

#[test]
fn test_add_rule_extended_cam_with_shifts_rejected() {
    // ext_flags has_cam=1, but a shift also follows -- must be rejected,
    // mirroring config::load_config's "`cam` must not also have `shifts`".
    let packet = vec![OP_ADD_EXT, 15, 1, 8, 0b0000_0001, 3, 1, SHIFT_FLAG, 0, 1, 255];
    let idx = find_terminator(&packet).unwrap();
    let result = deserialize_packet(&packet[..idx]);
    assert!(result.is_err(), "cam + shifts must be rejected, not silently accepted");
}

#[test]
fn test_add_rule_extended_cam_with_multi_id_rejected() {
    // id_len=2 with has_cam=1 -- rejected: the wire protocol always derives
    // `pattern` from the full `id`, so a multi-element id would give a
    // cam-rule a non-trivial pattern, unlike the YAML path's explicit
    // "pattern must be empty" check.
    let packet = vec![OP_ADD_EXT, 15, 2, 8, 9, 0b0000_0001, 3, 1, 255];
    let idx = find_terminator(&packet).unwrap();
    let result = deserialize_packet(&packet[..idx]);
    assert!(result.is_err(), "cam with id_len != 1 must be rejected");
}

#[test]
fn test_recursion_roundtrip_via_serializer_uses_extended_format() {
    let rule = Rule {
        id: vec![CellType(6)],
        pattern: vec![(0, 0, CellType(6))],
        shifts: vec![],
        changes: vec![(1, 0, crate::types::ChangeValue::Literal(9))],
        active_only: false,
        priority: 30,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: Some(crate::types::RecursionSpec {
            max_depth: 5,
            direction: crate::types::Direction::Right,
        }),
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let packet = serialize_add_rule(&rule).expect("recursion rule should serialize");
    assert_eq!(packet[0], OP_ADD_EXT, "recursion requires the AddRuleExtended op-code");

    let idx = find_terminator(&packet).unwrap();
    let op = deserialize_packet(&packet[..idx]).unwrap();
    match op {
        RuleOp::AddRule(decoded) => {
            assert_eq!(decoded.priority, 30);
            assert_eq!(decoded.id, vec![CellType(6)]);
            assert_eq!(
                decoded.recursion,
                Some(crate::types::RecursionSpec {
                    max_depth: 5,
                    direction: crate::types::Direction::Right
                })
            );
            assert!(decoded.shifts.is_empty());
            assert_eq!(
                decoded.changes,
                vec![(1i32, 0i32, crate::types::ChangeValue::Literal(9))]
            );
        }
        _ => panic!("expected AddRule"),
    }
}

#[test]
fn test_add_rule_extended_hand_assembled_recursion_only() {
    // [OP_ADD_EXT, priority=15, id_len=1, id=8, ext_flags=0b10(has_recursion),
    //  max_depth=3, direction=3(Right), 255]
    let packet = vec![OP_ADD_EXT, 15, 1, 8, 0b0000_0010, 3, 3, 255];
    let idx = find_terminator(&packet).unwrap();
    let op = deserialize_packet(&packet[..idx]).unwrap();
    let RuleOp::AddRule(rule) = op else {
        panic!("expected AddRule")
    };
    assert_eq!(rule.priority, 15);
    assert_eq!(rule.id, vec![CellType(8)]);
    assert_eq!(
        rule.recursion,
        Some(crate::types::RecursionSpec {
            max_depth: 3,
            direction: crate::types::Direction::Right
        })
    );
    assert!(rule.shifts.is_empty());
    assert!(rule.changes.is_empty());
}

#[test]
fn test_add_rule_extended_cam_and_recursion_together_hand_assembled() {
    // Both bits set: ext_flags=0b11 -- cam bytes FIRST, then recursion bytes
    // (see OP_ADD_EXT's doc-comment on ordering). This combination is
    // ALLOWED on this wire path (unlike GPU, see `CamRecursionUnsupported`'s
    // doc-comment) -- mirrors CPU `applicator::apply_cam_buffered`'s support
    // for `cam`+`recursion` together.
    // [OP_ADD_EXT, priority=15, id_len=1, id=8, ext_flags=0b11,
    //  cam_radius=3, cam_target=1, recursion_max_depth=2, recursion_dir=0(Up), 255]
    let packet = vec![OP_ADD_EXT, 15, 1, 8, 0b0000_0011, 3, 1, 2, 0, 255];
    let idx = find_terminator(&packet).unwrap();
    let op = deserialize_packet(&packet[..idx]).unwrap();
    let RuleOp::AddRule(rule) = op else {
        panic!("expected AddRule")
    };
    assert_eq!(
        rule.cam,
        Some(crate::types::CamSearch {
            radius: 3,
            target_type: CellType(1)
        })
    );
    assert_eq!(
        rule.recursion,
        Some(crate::types::RecursionSpec {
            max_depth: 2,
            direction: crate::types::Direction::Up
        })
    );
}

#[test]
fn test_add_rule_extended_recursion_with_shifts_rejected() {
    // ext_flags has_recursion=1, but a shift also follows -- must be
    // rejected, mirroring config::load_config's "`recursion` must have no
    // shifts".
    let packet = vec![OP_ADD_EXT, 15, 1, 8, 0b0000_0010, 3, 3, SHIFT_FLAG, 0, 1, 255];
    let idx = find_terminator(&packet).unwrap();
    let result = deserialize_packet(&packet[..idx]);
    assert!(
        result.is_err(),
        "recursion + shifts must be rejected, not silently accepted"
    );
}

#[test]
fn test_add_rule_extended_recursion_max_depth_255_rejected() {
    // max_depth byte == 255 collides with the stream-level terminator --
    // must be rejected at parse time (mirrors cam's radius/target_type==255
    // check), not silently decoded as a huge depth. Calls deserialize_packet
    // DIRECTLY on the exact 7 intended bytes (bypassing find_terminator,
    // which would otherwise treat the max_depth=255 byte itself as the
    // stream terminator and truncate the packet before it even reaches
    // deserialize_packet) -- isolates the ext_flags/max_depth validation.
    let packet: [u8; 7] = [OP_ADD_EXT, 15, 1, 8, 0b0000_0010, 255, 3];
    let result = deserialize_packet(&packet);
    assert!(
        result.is_err(),
        "recursion max_depth of 255 must be rejected, not decoded as a valid depth"
    );
}

#[test]
fn test_serializer_rejects_recursion_with_shifts() {
    let rule = Rule {
        id: vec![CellType(6)],
        pattern: vec![],
        shifts: vec![vec![crate::types::ShiftSpec::new(crate::types::Direction::Up, 1)]],
        changes: vec![],
        active_only: false,
        priority: 30,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: Some(crate::types::RecursionSpec {
            max_depth: 2,
            direction: crate::types::Direction::Right,
        }),
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let result = serialize_add_rule(&rule);
    assert!(
        result.is_err(),
        "recursion + shifts must be rejected by the serializer, matching the deserializer's own check"
    );
}

#[test]
fn test_serializer_rejects_recursion_max_depth_255() {
    let rule = Rule {
        id: vec![CellType(6)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![],
        active_only: false,
        priority: 30,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: Some(crate::types::RecursionSpec {
            max_depth: 255,
            direction: crate::types::Direction::Right,
        }),
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let result = serialize_add_rule(&rule);
    assert!(
        result.is_err(),
        "recursion max_depth of 255 is unreachable (terminator collision) and must be rejected"
    );
}

/// Проверяет, что recursion-правило, пришедшее по каналу самомодификации
/// (не напрямую через `deserialize_packet`, а через полный
/// `RuleStore::drain_rule_channel` + `apply` конвейер), действительно
/// материализуется с `recursion: Some(_)` и реально работает — не просто
/// "парсер не упал", а "движок реально применяет каскад".
#[test]
fn test_add_rule_extended_recursion_end_to_end_through_self_modification_pipeline() {
    let mut grid = make_grid_from_vec(3, 1);
    let mut bb = BoundaryBuffer::new();
    bb.direction = "output".to_string();
    grid.set_boundary(0, 0, bb);

    let rule = Rule {
        id: vec![CellType(9)],
        pattern: vec![(0, 0, CellType(9)), (-1, 0, CellType(10))],
        shifts: vec![],
        changes: vec![(0, 0, crate::types::ChangeValue::Literal(10))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: Some(crate::types::RecursionSpec {
            max_depth: 2,
            direction: crate::types::Direction::Right,
        }),
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let packet = serialize_add_rule(&rule).expect("recursion rule should serialize");
    if let Some(buf) = grid.get_boundary_mut(0, 0) {
        for &b in &packet {
            buf.enqueue(
                0,
                Cell {
                    value: CellValue(CellType(b)),
                    born_at: 0,
                },
            );
        }
    }

    let mut store = RuleStore::new();
    let ops = store.drain_rule_channel(&mut grid);
    assert_eq!(ops.len(), 1, "should decode exactly one AddRuleExtended packet");
    for op in ops {
        store.apply(op);
    }

    let idx = store.get_index();
    let received = idx
        .get(&CellType(9))
        .and_then(|rules| rules.first())
        .expect("received rule must be indexed under its head type");
    assert_eq!(
        received.recursion,
        Some(crate::types::RecursionSpec {
            max_depth: 2,
            direction: crate::types::Direction::Right
        }),
        "recursion rule delivered through the self-modification channel must materialize with its RecursionSpec intact"
    );
}

#[test]
fn test_serializer_rejects_cam_with_shifts() {
    let rule = Rule {
        id: vec![CellType(6)],
        pattern: vec![],
        shifts: vec![vec![crate::types::ShiftSpec::new(crate::types::Direction::Up, 1)]],
        changes: vec![],
        active_only: false,
        priority: 30,
        min_age: 0,
        overflow: Default::default(),
        cam: Some(crate::types::CamSearch {
            radius: 5,
            target_type: CellType(2),
        }),
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    assert!(
        serialize_add_rule(&rule).is_err(),
        "cam + shifts must be rejected by the serializer too, not just the parser"
    );
}

#[test]
fn test_serializer_auto_switches_to_extended_format_for_reserved_priorities() {
    // priority 240/241/242 collide with OP_REMOVE/OP_CLEAR/OP_ADD_EXT as the
    // FIRST byte of a plain AddRule packet -- the serializer must never
    // emit a plain-format packet with one of these as the leading byte
    // (that would silently become a different operation on decode). It
    // switches to AddRuleExtended, whose leading byte is always OP_ADD_EXT
    // and priority moves to the second byte instead.
    for reserved_priority in [240u32, 241, 242] {
        let rule = Rule {
            id: vec![CellType(1)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![],
            active_only: false,
            priority: reserved_priority,
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
        let packet = serialize_add_rule(&rule).unwrap();
        assert_eq!(
            packet[0], OP_ADD_EXT,
            "priority {reserved_priority} must be carried via AddRuleExtended, not as the packet's first byte"
        );
        assert_eq!(packet[1], reserved_priority as u8);

        let idx = find_terminator(&packet).unwrap();
        let op = deserialize_packet(&packet[..idx]).unwrap();
        match op {
            RuleOp::AddRule(decoded) => assert_eq!(decoded.priority, reserved_priority),
            _ => panic!("expected AddRule"),
        }
    }
}

#[test]
fn test_serializer_rejects_priority_255() {
    let rule = make_rule(vec![CellType(1)], 255, vec![]);
    let result = serialize_add_rule(&rule);
    assert!(
        result.is_err(),
        "priority 255 is unreachable -- would be eaten by the stream-level terminator scan"
    );
}

#[test]
fn test_serializer_rejects_unreachable_change_bytes() {
    use crate::types::ChangeValue;
    // dx = -1 (0xFF byte) -- would collide with the stream-level terminator.
    let rule_dx = make_rule(vec![CellType(1)], 10, vec![(-1, 0, ChangeValue::Literal(5))]);
    assert!(
        serialize_add_rule(&rule_dx).is_err(),
        "dx=-1 (0xFF byte) must be rejected"
    );

    // value = 255 -- same reason.
    let rule_val = make_rule(vec![CellType(1)], 10, vec![(0, 0, ChangeValue::Literal(255))]);
    assert!(serialize_add_rule(&rule_val).is_err(), "value=255 must be rejected");

    // dx = -4 (0xFC byte) -- collides with CHANGE_REF_FLAG.
    let rule_ref_collision = make_rule(vec![CellType(1)], 10, vec![(-4, 0, ChangeValue::Literal(5))]);
    assert!(
        serialize_add_rule(&rule_ref_collision).is_err(),
        "dx=-4 (0xFC byte) collides with CHANGE_REF_FLAG and must be rejected"
    );
}

#[test]
fn test_serializer_rejects_first_change_colliding_with_shift_flags() {
    use crate::types::ChangeValue;
    // No shifts, first (only) change has dx=-2 (0xFE) -- collides with
    // SHIFT_FLAG at the exact position the shift-parsing loop checks.
    let rule = make_rule(vec![CellType(1)], 10, vec![(-2, 0, ChangeValue::Literal(5))]);
    assert!(
        serialize_add_rule(&rule).is_err(),
        "first change dx=-2 (0xFE) collides with SHIFT_FLAG and must be rejected"
    );

    // Same for dx=-3 (0xFD), colliding with SHIFT_EXT_FLAG.
    let rule2 = make_rule(vec![CellType(1)], 10, vec![(-3, 0, ChangeValue::Literal(5))]);
    assert!(
        serialize_add_rule(&rule2).is_err(),
        "first change dx=-3 (0xFD) collides with SHIFT_EXT_FLAG and must be rejected"
    );
}

/// `ChangeValue::Add`/`Sub` — вне подмножества протокола (см.
/// `push_change`'s doc-комментарий в `rule_store.rs` про то, почему это
/// именно `Err`, а не тихий пропуск, как у `feedback`/`memory`/`keep_source`).
#[test]
fn test_serializer_rejects_arithmetic_change() {
    use crate::types::ChangeValue;
    let rule_add = make_rule(
        vec![CellType(1)],
        10,
        vec![(
            0,
            0,
            ChangeValue::Add(Box::new(ChangeValue::Ref(0)), Box::new(ChangeValue::Literal(1))),
        )],
    );
    assert!(
        serialize_add_rule(&rule_add).is_err(),
        "ChangeValue::Add must be rejected -- the protocol has no byte encoding for it"
    );

    let rule_sub = make_rule(
        vec![CellType(1)],
        10,
        vec![(
            0,
            0,
            ChangeValue::Sub(Box::new(ChangeValue::Literal(5)), Box::new(ChangeValue::Ref(0))),
        )],
    );
    assert!(
        serialize_add_rule(&rule_sub).is_err(),
        "ChangeValue::Sub must be rejected -- same reason as Add"
    );
}

#[test]
fn test_serialize_remove_and_clear_roundtrip() {
    let id = vec![CellType(7), CellType(8)];
    let packet = serialize_remove_rule(&id).unwrap();
    let idx = find_terminator(&packet).unwrap();
    let op = deserialize_packet(&packet[..idx]).unwrap();
    assert_eq!(op, RuleOp::RemoveRule(id));

    let clear_packet = serialize_clear_all();
    let idx2 = find_terminator(&clear_packet).unwrap();
    let op2 = deserialize_packet(&clear_packet[..idx2]).unwrap();
    assert_eq!(op2, RuleOp::ClearAll);
}

#[test]
fn test_add_rule_extended_end_to_end_through_self_modification_pipeline() {
    // Same style as `test_integration_self_modification` above, but the
    // injected packet uses OP_ADD_EXT to carry a `cam` rule -- proves `cam`
    // now actually reaches `RuleStore`/`get_index` through the real
    // boundary-buffer channel, not just through direct `deserialize_packet`
    // calls.
    let mut grid = make_grid_from_vec(1, 1);
    let mut bb = BoundaryBuffer::new();
    bb.direction = "output".to_string();
    grid.set_boundary(0, 0, bb);

    let rule = Rule {
        id: vec![CellType(6)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![],
        active_only: false,
        priority: 12,
        min_age: 0,
        overflow: Default::default(),
        cam: Some(crate::types::CamSearch {
            radius: 2,
            target_type: CellType(3),
        }),
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let packet_bytes = serialize_add_rule(&rule).unwrap();

    if let Some(buf) = grid.get_boundary_mut(0, 0) {
        for &b in &packet_bytes {
            buf.enqueue(
                0,
                Cell {
                    value: CellValue(CellType(b)),
                    born_at: 0,
                },
            );
        }
    }

    let mut store = RuleStore::new();
    let ops = store.drain_rule_channel(&mut grid);
    assert_eq!(ops.len(), 1, "should decode exactly one AddRule packet");
    for op in ops {
        store.apply(op);
    }

    let idx = store.get_index();
    let received = idx
        .get(&CellType(6))
        .and_then(|rules| rules.first())
        .expect("received rule must be indexed under its head type");
    assert_eq!(
        received.cam,
        Some(crate::types::CamSearch {
            radius: 2,
            target_type: CellType(3)
        }),
        "cam must survive the full boundary-buffer -> RuleStore pipeline, not just direct deserialize_packet calls"
    );
}

// ============================================================================
// Адверсариальная проверка: `deserialize_packet`/`drain_rule_channel`
// разбирают байты, которые в реальном сценарии (канал 0 = ввод от ВНЕШНЕГО
// источника, не обязательно доверенного) могут быть чем угодно. Найденный
// в этой же сессии баг с `ChunkStorage` показал, что
// "должно быть безопасно по построению" стоит проверять конструктивно, а
// не только по чтению кода — здесь то же самое, но через `proptest`
// вместо одной руками собранной адверсариальной клетки.
// ============================================================================

use proptest::prelude::*;

proptest! {
    /// `deserialize_packet` обязана НИКОГДА не паниковать, какие бы байты
    /// ей ни дали, — только `Ok(RuleOp)` или `Err(String)`. Это низкоуровневый
    /// парсер потенциально недоверенного канального ввода (см. `CellariaError::Protocol`
    /// в спецификации), не внутренняя функция с доверенными вызывающими.
    #[test]
    fn prop_deserialize_packet_never_panics(data in proptest::collection::vec(any::<u8>(), 0..128)) {
        let _ = deserialize_packet(&data);
    }

    /// Тот же инвариант, но через ПОЛНЫЙ публичный путь (не напрямую
    /// внутреннюю `deserialize_packet`): произвольные байты кладутся в
    /// канал 0 boundary-буфера, как это сделал бы реальный внешний
    /// источник ввода, и прогоняются через `RuleStore::drain_rule_channel`
    /// -- включая `find_terminator`'s накопление в `accum_buffers` и
    /// `MAX_BUFFER_SIZE`'s гейт переполнения, которые `deserialize_packet`
    /// в одиночку не задействует.
    #[test]
    fn prop_drain_rule_channel_never_panics_on_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let mut grid = make_grid_from_vec(4, 4);
        grid.set_boundary(0, 0, BoundaryBuffer { direction: "output".to_string(), ..Default::default() });
        if let Some(buf) = grid.get_boundary_mut(0, 0) {
            for &b in &bytes {
                buf.enqueue(0, Cell { value: CellValue(CellType(b)), born_at: 0 });
            }
        }
        let mut store = RuleStore::new();
        let ops = store.drain_rule_channel(&mut grid);
        // Дошедшие Ok-операции обязаны тоже безопасно применяться -- `apply`
        // на мусорных, но структурно валидных операциях не должен паниковать.
        for op in ops {
            store.apply(op);
        }
    }
}
