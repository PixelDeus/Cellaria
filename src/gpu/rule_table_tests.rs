use std::collections::HashMap;

use super::*;
use crate::types::{Direction, OverflowAction, ShiftSpec};

fn base_rule(id: Vec<u8>, pattern: Vec<(i8, i8, u8)>, changes: Vec<(i32, i32, ChangeValue)>) -> Rule {
    Rule {
        id: id.into_iter().map(CellType).collect(),
        pattern: pattern.into_iter().map(|(dx, dy, t)| (dx, dy, CellType(t))).collect(),
        shifts: Vec::new(),
        changes,
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
    }
}

/// Классический Game of Life: голова "жива" (1), паттерн — 8 соседей,
/// правило пишет новое состояние в саму себя.
fn gol_index() -> HashMap<CellType, Vec<Rule>> {
    let mut idx = HashMap::new();
    idx.insert(
        CellType(1),
        vec![base_rule(
            vec![1],
            vec![(0, 0, 1), (1, 0, 1), (-1, 0, 1)],
            vec![(0, 0, ChangeValue::Literal(1))],
        )],
    );
    idx
}

#[test]
fn test_build_gpu_rule_table_accepts_gol_shaped_config() {
    let idx = gol_index();
    let table = build_gpu_rule_table(&idx).expect("GoL-shaped config must be within the supported subset");

    let slot = table.head_slots[1];
    assert_eq!(slot.rules_count, 1);
    let rule = table.rules[slot.rules_start as usize];
    assert_eq!(rule.pattern_len, 3);
    assert_eq!(rule.shift_count, 0);
    assert_eq!(rule.change_count, 1);
    assert_eq!((rule.change_dx0, rule.change_dy0, rule.change_val0), (0, 0, 1));
    assert_eq!(rule.id_len, 1);
    assert_eq!(rule.id_b0, 1);
    assert_eq!(rule.rule_idx, 0);
    assert!(!table.needs_arbitration, "self-only changes must not require arbitration");
    assert_eq!(table.margin, 0, "margin is unused (and must be 0) when arbitration isn't needed");
    assert_eq!(table.max_matches_per_cell, 0, "max_matches_per_cell is unused (and must be 0) when arbitration isn't needed");

    let offsets = &table.pattern_offsets[rule.pattern_start as usize..(rule.pattern_start + rule.pattern_len) as usize];
    assert_eq!(offsets.len(), 3);
    assert_eq!((offsets[0].dx, offsets[0].dy, offsets[0].expected), (0, 0, 1));

    // union офсетов головы должен покрывать все 3 офсета этого единственного правила.
    assert_eq!(slot.offsets_count, 3);
    let union = &table.head_offsets[slot.offsets_start as usize..(slot.offsets_start + slot.offsets_count) as usize];
    let mut union_pairs: Vec<(i32, i32)> = union.iter().map(|o| (o.dx, o.dy)).collect();
    union_pairs.sort();
    assert_eq!(union_pairs, vec![(-1, 0), (0, 0), (1, 0)]);
}

#[test]
fn test_build_gpu_rule_table_other_heads_have_zero_slot() {
    let idx = gol_index();
    let table = build_gpu_rule_table(&idx).unwrap();
    assert_eq!(table.head_slots[0].rules_count, 0);
    assert_eq!(table.head_slots[255].rules_count, 0);
}

#[test]
fn test_build_gpu_rule_table_union_offsets_dedup_across_rules() {
    // Два правила одной головы, оба читают (0,0), но разные "хвосты" —
    // union должен содержать (0,0) один раз, плюс (1,0) и (2,0).
    let mut idx = HashMap::new();
    idx.insert(
        CellType(1),
        vec![
            base_rule(vec![1], vec![(0, 0, 1), (1, 0, 1)], vec![(0, 0, ChangeValue::Literal(2))]),
            base_rule(vec![1], vec![(0, 0, 1), (2, 0, 1)], vec![(0, 0, ChangeValue::Literal(3))]),
        ],
    );
    let table = build_gpu_rule_table(&idx).unwrap();
    let slot = table.head_slots[1];
    assert_eq!(slot.rules_count, 2);
    assert_eq!(slot.offsets_count, 3);
    let union = &table.head_offsets[slot.offsets_start as usize..(slot.offsets_start + slot.offsets_count) as usize];
    let mut union_pairs: Vec<(i32, i32)> = union.iter().map(|o| (o.dx, o.dy)).collect();
    union_pairs.sort();
    assert_eq!(union_pairs, vec![(0, 0), (1, 0), (2, 0)]);
}

#[test]
fn test_build_gpu_rule_table_last_self_change_wins() {
    // Совпадает с семантикой applicator::apply_changes_at: несколько
    // changes на один и тот же (0,0) — побеждает последний в списке (это
    // просто change_count==2, а не 1 — резолвится в шейдере тем же
    // порядком применения; здесь проверяем только, что оба закодированы
    // как есть, в исходном порядке).
    let mut idx = HashMap::new();
    idx.insert(
        CellType(1),
        vec![base_rule(
            vec![1],
            vec![(0, 0, 1)],
            vec![(0, 0, ChangeValue::Literal(5)), (0, 0, ChangeValue::Literal(9))],
        )],
    );
    let table = build_gpu_rule_table(&idx).unwrap();
    let rule = table.rules[table.head_slots[1].rules_start as usize];
    assert_eq!(rule.change_count, 2);
    assert_eq!(rule.change_val0, 5);
    assert_eq!(rule.change_val1, 9);
}

#[test]
fn test_build_gpu_rule_table_accepts_shift_and_flags_arbitration() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![(0, 0, ChangeValue::Literal(9))]);
    rule.shifts = vec![vec![ShiftSpec::new(Direction::Right, 1)]];
    idx.insert(CellType(1), vec![rule]);

    let table = build_gpu_rule_table(&idx).expect("a single Discard shift is within the supported subset");
    assert!(table.needs_arbitration);
    let r = table.rules[table.head_slots[1].rules_start as usize];
    assert_eq!(r.shift_count, 1);
    assert_eq!((r.shift_dx0, r.shift_dy0), (1, 0));
    // Сдвиг на 1 клетку + self-change (0,0) -> margin = 1 (shift_reach) + 0 (change_reach) = 1,
    // а НЕ статический потолок MAX_MARGIN — см. doc-комментарий GpuRuleTable::margin.
    assert_eq!(table.margin, 1);
    // Одна голова, одно правило -> max_matches_per_cell = 1, а НЕ MAX_MATCHES_PER_CELL(8).
    assert_eq!(table.max_matches_per_cell, 1);
}

#[test]
fn test_build_gpu_rule_table_max_matches_per_cell_is_max_over_heads() {
    // Голова 1 — три правила, голова 2 — одно; max_matches_per_cell должен
    // взять максимум по ВСЕМ головам (3), а не по последней обработанной.
    let mut idx = HashMap::new();
    idx.insert(
        CellType(1),
        vec![
            base_rule(vec![1], vec![(0, 0, 1)], vec![(1, 0, ChangeValue::Literal(1))]),
            base_rule(vec![1], vec![(0, 0, 1)], vec![(1, 0, ChangeValue::Literal(2))]),
            base_rule(vec![1], vec![(0, 0, 1)], vec![(1, 0, ChangeValue::Literal(3))]),
        ],
    );
    idx.insert(
        CellType(2),
        vec![base_rule(vec![2], vec![(0, 0, 2)], vec![(1, 0, ChangeValue::Literal(4))])],
    );

    let table = build_gpu_rule_table(&idx).expect("non-self changes within MAX_CHANGE_REACH");
    assert_eq!(table.head_slots[1].rules_count, 3);
    assert_eq!(table.head_slots[2].rules_count, 1);
    assert_eq!(table.max_matches_per_cell, 3);
}

#[test]
fn test_build_gpu_rule_table_margin_is_max_over_all_rules() {
    // Реальный охват — МАКСИМУМ по всем правилам, а не просто по последнему
    // обработанному: правило A (сдвиг на 3) должно "победить" даже если
    // правило B (сдвиг на 1, но с change на 2 клетки, суммарно тоже 3)
    // окажется где-то не первым в итерации HashMap.
    let mut idx = HashMap::new();
    let mut rule_a = base_rule(vec![1], vec![(0, 0, 1)], vec![]);
    rule_a.shifts = vec![vec![ShiftSpec::new(Direction::Right, 3)]];
    let mut rule_b = base_rule(vec![1], vec![(0, 0, 1)], vec![(2, 0, ChangeValue::Literal(1))]);
    rule_b.shifts = vec![vec![ShiftSpec::new(Direction::Up, 1)]];
    idx.insert(CellType(1), vec![rule_a, rule_b]);

    let table = build_gpu_rule_table(&idx).expect("shifts/changes within MAX_SHIFT_REACH/MAX_CHANGE_REACH");
    // rule_a: shift_reach=3, change_reach=0 -> margin=3.
    // rule_b: shift_reach=1, change_reach=2 -> margin=3.
    // max(3,3) = 3.
    assert_eq!(table.margin, 3);
}

#[test]
fn test_build_gpu_rule_table_margin_from_non_self_change_without_shift() {
    let mut idx = HashMap::new();
    idx.insert(
        CellType(1),
        vec![base_rule(vec![1], vec![(0, 0, 1)], vec![(2, -1, ChangeValue::Literal(9))])],
    );
    let table = build_gpu_rule_table(&idx).expect("non-self change within MAX_CHANGE_REACH");
    assert!(table.needs_arbitration);
    // Без сдвигов margin = change_reach напрямую = max(|2|,|-1|) = 2.
    assert_eq!(table.margin, 2);
}

#[test]
fn test_build_gpu_rule_table_accepts_non_self_change_and_flags_arbitration() {
    let mut idx = HashMap::new();
    idx.insert(
        CellType(1),
        vec![base_rule(vec![1], vec![(0, 0, 1)], vec![(1, 0, ChangeValue::Literal(2))])],
    );
    let table = build_gpu_rule_table(&idx).expect("a non-self change without shifts is within the supported subset");
    assert!(table.needs_arbitration);
}

#[test]
fn test_build_gpu_rule_table_rejects_no_effect_rule() {
    let mut idx = HashMap::new();
    idx.insert(CellType(1), vec![base_rule(vec![1], vec![(0, 0, 1)], vec![])]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::NoEffect { head: 1, rule_idx: 0 });
}

#[test]
fn test_build_gpu_rule_table_rejects_ref_change() {
    let mut idx = HashMap::new();
    idx.insert(
        CellType(1),
        vec![base_rule(vec![1], vec![(0, 0, 1)], vec![(0, 0, ChangeValue::Ref(0))])],
    );

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::ChangeIsRef { head: 1, rule_idx: 0 });
}

#[test]
fn test_build_gpu_rule_table_rejects_starvation_guard() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![]);
    rule.starvation_after = Some(3);
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::StarvationGuardUnsupported { head: 1, rule_idx: 0 });
}

#[test]
fn test_build_gpu_rule_table_rejects_overflow_write_on_shift() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![]);
    rule.shifts = vec![vec![ShiftSpec::new(Direction::Right, 1)]];
    rule.overflow = OverflowAction::WriteLiteral(5);
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::OverflowNotDiscard { head: 1, rule_idx: 0 });
}

#[test]
fn test_build_gpu_rule_table_rejects_too_many_shifts() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![]);
    rule.shifts = (0..(MAX_SHIFTS + 1)).map(|_| vec![ShiftSpec::new(Direction::Right, 1)]).collect();
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::TooManyShifts { head: 1, rule_idx: 0, len: MAX_SHIFTS + 1 });
}

#[test]
fn test_build_gpu_rule_table_rejects_too_many_changes() {
    let mut idx = HashMap::new();
    let changes: Vec<(i32, i32, ChangeValue)> = (0..(MAX_CHANGES as i32 + 1)).map(|i| (i, 0, ChangeValue::Literal(1))).collect();
    idx.insert(CellType(1), vec![base_rule(vec![1], vec![(0, 0, 1)], changes)]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::TooManyChanges { head: 1, rule_idx: 0, len: MAX_CHANGES + 1 });
}

#[test]
fn test_build_gpu_rule_table_rejects_oversized_pattern() {
    let mut idx = HashMap::new();
    let pattern: Vec<(i8, i8, u8)> = (0..(MAX_PATTERN_OFFSETS as i8 + 1)).map(|i| (i, 0, 1)).collect();
    idx.insert(
        CellType(1),
        vec![base_rule(vec![1], pattern, vec![(0, 0, ChangeValue::Literal(1))])],
    );

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(
        err,
        GpuUnsupportedReason::PatternTooLarge { head: 1, rule_idx: 0, len: MAX_PATTERN_OFFSETS + 1 }
    );
}

#[test]
fn test_build_gpu_rule_table_rejects_oversized_id() {
    let mut idx = HashMap::new();
    let id: Vec<u8> = (0..(MAX_ID_BYTES as u8 + 1)).collect();
    let len = id.len();
    idx.insert(
        CellType(1),
        vec![base_rule(id, vec![(0, 0, 1)], vec![(0, 0, ChangeValue::Literal(1))])],
    );

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::RuleIdTooLong { head: 1, rule_idx: 0, len });
}

#[test]
fn test_build_gpu_rule_table_rejects_too_many_rules_for_arbitration() {
    // needs_arbitration=true (одно правило со сдвигом) + голова с
    // MAX_MATCHES_PER_CELL+1 правилами — должно быть отклонено, хотя каждое
    // правило по отдельности в рамках подмножества.
    let mut idx = HashMap::new();
    let mut group: Vec<Rule> = (0..(MAX_MATCHES_PER_CELL + 1))
        .map(|i| base_rule(vec![1], vec![(0, 0, 1)], vec![(0, 0, ChangeValue::Literal(i as u8))]))
        .collect();
    let mut shifting = base_rule(vec![1], vec![(0, 0, 1)], vec![]);
    shifting.shifts = vec![vec![ShiftSpec::new(Direction::Right, 1)]];
    group.push(shifting);
    idx.insert(CellType(1), group);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::TooManyRulesForArbitration { head: 1, len: MAX_MATCHES_PER_CELL + 2 });
}

#[test]
fn test_build_gpu_rule_table_uses_pattern_fallback_from_id_when_pattern_empty() {
    // pattern пуст → effective_pattern строится из id, как в matcher.rs.
    let mut idx = HashMap::new();
    idx.insert(
        CellType(1),
        vec![base_rule(vec![1, 2], vec![], vec![(0, 0, ChangeValue::Literal(3))])],
    );
    let table = build_gpu_rule_table(&idx).unwrap();
    let rule = table.rules[table.head_slots[1].rules_start as usize];
    assert_eq!(rule.pattern_len, 2);
    let offsets = &table.pattern_offsets[rule.pattern_start as usize..(rule.pattern_start + rule.pattern_len) as usize];
    assert_eq!((offsets[0].dx, offsets[0].dy, offsets[0].expected), (0, 0, 1));
    assert_eq!((offsets[1].dx, offsets[1].dy, offsets[1].expected), (1, 0, 2));
}
