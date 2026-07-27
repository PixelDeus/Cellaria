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
    let op = deserialize_packet(data, 100).unwrap();
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
    // RemoveRule: [0xF0, rule_id=42, 255]
    let packet = vec![0xF0, 42, 255];
    let idx = find_terminator(&packet).unwrap();
    let data = &packet[..idx];
    let op = deserialize_packet(data, 0).unwrap();
    assert_eq!(op, RuleOp::RemoveRule(vec![CellType(42)]));
}

#[test]
fn test_deserialize_clear_all() {
    let packet = vec![0xF1, 255];
    let idx = find_terminator(&packet).unwrap();
    let data = &packet[..idx];
    let op = deserialize_packet(data, 0).unwrap();
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
    }
}

#[test]
fn test_rule_store_apply_add() {
    let mut store = RuleStore::new();
    let rule = make_rule(vec![CellType(1)], 10, vec![(0, 0, crate::types::ChangeValue::Literal(2))]);
    assert!(store.apply(CompletedOp {
        op: RuleOp::AddRule(rule)
    }));
    assert_eq!(store.rules().len(), 1);
    assert!(store.dirty);
}

#[test]
fn test_rule_store_apply_remove() {
    let rule = make_rule(vec![CellType(1)], 10, vec![(0, 0, crate::types::ChangeValue::Literal(2))]);
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
        make_rule(vec![CellType(1)], 10, vec![(0, 0, crate::types::ChangeValue::Literal(2))]),
        make_rule(vec![CellType(3)], 5, vec![(0, 0, crate::types::ChangeValue::Literal(4))]),
    ];
    let mut store = RuleStore::with_rules(rules);
    store.dirty = false;
    assert!(store.apply(CompletedOp {
        op: RuleOp::ClearAll
    }));
    assert_eq!(store.rules().len(), 0);
}

#[test]
fn test_get_index_rebuilds_when_dirty() {
    let rule = make_rule(vec![CellType(5)], 10, vec![(0, 0, crate::types::ChangeValue::Literal(6))]);
    let mut store = RuleStore::with_rules(vec![rule]);
    store.get_index();

    let new_rule = make_rule(vec![CellType(7)], 5, vec![(0, 0, crate::types::ChangeValue::Literal(8))]);
    store.apply(CompletedOp {
        op: RuleOp::AddRule(new_rule),
    });
    assert!(store.dirty, "dirty should be set after apply");

    let index = store.get_index();
    assert!(
        index.contains_key(&CellType(5)),
        "Index should include original rule"
    );
    assert!(
        index.contains_key(&CellType(7)),
        "Index should include new rule"
    );
    assert!(!store.dirty, "dirty should be false after rebuild");
}

#[test]
fn test_deserialize_rejects_255_in_id() {
    // data = [priority=10, id_len=1, type=255]
    let data = vec![10, 1, 0xFF];
    let result = deserialize_packet(&data, 100);
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
        buf.enqueue(0, Cell {
            value: CellValue(CellType(10)),
            born_at: 0,
        });
        buf.enqueue(0, Cell {
            value: CellValue(CellType(1)),
            born_at: 0,
        });
        buf.enqueue(0, Cell {
            value: CellValue(CellType(5)),
            born_at: 0,
        });
        buf.enqueue(0, Cell {
            value: CellValue(CellType(255)),
            born_at: 0,
        });
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
fn test_decode_errors_increments_on_bad_packet() {
    let mut grid = make_grid_from_vec(1, 1);
    let mut bb = BoundaryBuffer::new();
    bb.direction = "output".to_string();
    grid.set_boundary(0, 0, bb);
    // Corrupted data: just 255 (empty packet = error)
    if let Some(buf) = grid.get_boundary_mut(0, 0) {
        buf.enqueue(0, Cell {
            value: CellValue(CellType(255)),
            born_at: 0,
        });
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
        buf.enqueue(0, Cell { value: CellValue(CellType(5)), born_at: 0 });
        buf.enqueue(0, Cell { value: CellValue(CellType(1)), born_at: 0 });
        buf.enqueue(0, Cell { value: CellValue(CellType(9)), born_at: 0 });
        buf.enqueue(0, Cell { value: CellValue(CellType(255)), born_at: 0 });
    }

    // 6) Дренируем канал и применяем
    let ops = store.drain_rule_channel(&mut grid);
    assert_eq!(ops.len(), 1, "should decode one AddRule packet");
    for op in ops {
        store.apply(op);
    }

    // 7) Проверяем, что в store теперь два правила
    assert_eq!(
        store.rules().len(),
        2,
        "should have 2 rules after self-modification"
    );

    // 8) Проверяем, что индекс перестроился и включает новое правило
    let idx = store.get_index();
    assert!(
        idx.contains_key(&CellType(7)),
        "index should contain rule1"
    );
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
