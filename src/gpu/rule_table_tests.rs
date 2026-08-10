use std::collections::HashMap;

use super::*;
use crate::types::{Direction, FeedbackSpec, MemorySpec, OverflowAction, RecordTrigger, ShiftSpec};

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
        starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
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

/// `starvation_after` ТЕПЕРЬ поддерживается (см. `GpuRuleTable::needs_starvation`'s
/// doc-комментарий) — раньше это правило отвергалось блэнкет-
/// `StarvationGuardUnsupported`; теперь строится успешно, форсирует
/// Arbitrated-пайплайн (голодание осмысленно только под конкуренцией) и
/// корректно кодирует `has_starvation`/`starvation_threshold`.
#[test]
fn test_build_gpu_rule_table_accepts_starvation_guard() {
    let mut idx = HashMap::new();
    // Self-write-only иначе (self-change) -- проверяет, что starvation
    // САМА форсирует арбитраж, а не просто "уже была нужна по другой причине".
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![(0, 0, ChangeValue::Literal(1))]);
    rule.starvation_after = Some(3);
    idx.insert(CellType(1), vec![rule]);

    let table = build_gpu_rule_table(&idx).expect("starvation_after must be accepted");
    assert!(table.needs_arbitration, "starvation_after must force the arbitrated pipeline even for an otherwise self-write-only rule");
    assert!(table.needs_starvation);
    let r = table.rules[table.head_slots[1].rules_start as usize];
    assert_eq!(r.has_starvation, 1);
    assert_eq!(r.starvation_threshold, 3);
}

/// Угловой случай, найденный при реализации: `starvation_after: Some(0)` —
/// РЕАЛЬНОЕ, отличное от "не установлено" значение (побеждает через
/// голодание СРАЗУ, см. `rule_table::GpuRule::has_starvation`'s
/// doc-комментарий) — не должно тихо схлопнуться в "выключено" только
/// потому, что 0 — естественный сентинел для ДРУГИХ полей
/// (`cam_radius`/`recursion_max_depth`).
#[test]
fn test_build_gpu_rule_table_starvation_threshold_zero_is_not_disabled() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![(0, 0, ChangeValue::Literal(1))]);
    rule.starvation_after = Some(0);
    idx.insert(CellType(1), vec![rule]);

    let table = build_gpu_rule_table(&idx).expect("starvation_after: Some(0) must be accepted");
    let r = table.rules[table.head_slots[1].rules_start as usize];
    assert_eq!(r.has_starvation, 1, "has_starvation must be 1 even though the threshold itself is 0");
    assert_eq!(r.starvation_threshold, 0);
}

/// `feedback` (не-broadcast) ТЕПЕРЬ поддерживается (см.
/// `GpuRuleTable::needs_feedback`'s doc-комментарий) — раньше отвергалось
/// блэнкет-`FeedbackUnsupported`; теперь строится успешно и корректно
/// кодирует альтернативное направление.
#[test]
fn test_build_gpu_rule_table_accepts_feedback() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![]);
    rule.shifts = vec![vec![ShiftSpec::new(Direction::Right, 1)]];
    rule.feedback = Some(FeedbackSpec { timeout: 5, new_direction: Direction::Up });
    idx.insert(CellType(1), vec![rule]);

    let table = build_gpu_rule_table(&idx).expect("non-broadcast feedback must be accepted");
    assert!(table.needs_feedback);
    assert!(table.needs_arbitration, "feedback rules always have a shift, which already forces arbitration");
    let r = table.rules[table.head_slots[1].rules_start as usize];
    assert_eq!(r.has_feedback, 1);
    assert_eq!(r.feedback_timeout, 5);
    assert_eq!((r.feedback_alt_dx, r.feedback_alt_dy), (0, -1), "new_direction=Up must encode to (0,-1)");
}

/// `feedback` + `broadcast` ВМЕСТЕ — вне GPU-подмножества (см.
/// `GpuUnsupportedReason::FeedbackBroadcastUnsupported`'s doc-комментарий:
/// перенос счётчика читает `cells[1]` как "новая позиция", неверно для
/// broadcast-пути, который пишет весь путь, не одну клетку).
#[test]
fn test_build_gpu_rule_table_rejects_feedback_with_broadcast() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![]);
    rule.shifts = vec![vec![ShiftSpec { direction: Direction::Right, steps: 1, broadcast: true, keep_source: false }]];
    rule.feedback = Some(FeedbackSpec { timeout: 5, new_direction: Direction::Up });
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::FeedbackBroadcastUnsupported { head: 1, rule_idx: 0 });
}

/// Защитная проверка (та же философия, что и `cam`'s `id_len == 1`):
/// правило с `feedback`, но НЕ ровно одним сдвигом, пришедшее мимо
/// `config::load_config`'s собственной валидации (например, напрямую через
/// Rust API), не должно тихо ломать однократное предположение
/// `feedback_alt_dx/dy`'s кодирования.
#[test]
fn test_build_gpu_rule_table_rejects_feedback_without_exactly_one_shift() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![]);
    rule.shifts = vec![];
    rule.feedback = Some(FeedbackSpec { timeout: 5, new_direction: Direction::Up });
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::TooManyShifts { head: 1, rule_idx: 0, len: 0 });
}

/// `recursion` в пределах `MAX_RECURSION_DEPTH` ПОДДЕРЖИВАЕТСЯ (см.
/// `MAX_RECURSION_DEPTH`'s doc-комментарий) — раньше это правило отвергалось
/// блэнкет-`RecursionUnsupported`; теперь строится успешно, и матч уходит
/// через needs_arbitration (каскад пишет вне self-клетки).
#[test]
fn test_build_gpu_rule_table_accepts_recursion_within_depth_limit() {
    use crate::types::RecursionSpec;

    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![(0, 0, ChangeValue::Literal(2))]);
    rule.recursion = Some(RecursionSpec { max_depth: 2, direction: Direction::Right });
    idx.insert(CellType(1), vec![rule]);

    let table = build_gpu_rule_table(&idx).expect("recursion within MAX_RECURSION_DEPTH must be accepted");
    assert!(table.needs_arbitration, "recursion cascade writes outside the self cell, must force the arbitrated pipeline");
    assert_eq!(table.rules[0].recursion_max_depth, 2);
    assert_eq!((table.rules[0].recursion_dx, table.rules[0].recursion_dy), (1, 0));
}

#[test]
fn test_build_gpu_rule_table_rejects_recursion_depth_too_large() {
    use crate::types::RecursionSpec;

    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![(0, 0, ChangeValue::Literal(2))]);
    rule.recursion = Some(RecursionSpec { max_depth: MAX_RECURSION_DEPTH + 1, direction: Direction::Right });
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(
        err,
        GpuUnsupportedReason::RecursionDepthTooLarge { head: 1, rule_idx: 0, max_depth: MAX_RECURSION_DEPTH + 1 }
    );
}

/// `cam` + `recursion` вместе — поддерживается на CPU (см. `applicator.rs`),
/// но НЕ на GPU (см. `CamRecursionUnsupported`'s doc-комментарий: CAM-каскад
/// нуждается в рантайм-поиске на каждом уровне, не в статическом офсете).
#[test]
fn test_build_gpu_rule_table_rejects_cam_with_recursion() {
    use crate::types::{CamSearch, RecursionSpec};

    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![], vec![]);
    rule.cam = Some(CamSearch { radius: 3, target_type: CellType(2) });
    rule.recursion = Some(RecursionSpec { max_depth: 1, direction: Direction::Right });
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::CamRecursionUnsupported { head: 1, rule_idx: 0 });
}

/// `ShiftSpec::keep_source` без `feedback`/`memory` — ПОДДЕРЖИВАЕТСЯ (см.
/// `GpuMatch::keep_age_mask` в `shader.wgsl`): плоский `build_gpu_rule_table`
/// не должен отвергать эту комбинацию.
#[test]
fn test_build_gpu_rule_table_accepts_plain_keep_source() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![]);
    rule.shifts = vec![vec![ShiftSpec { direction: Direction::Right, steps: 1, broadcast: false, keep_source: true }]];
    idx.insert(CellType(1), vec![rule]);

    assert!(build_gpu_rule_table(&idx).is_ok(), "keep_source alone (no feedback/memory) must be GPU-supported");
}

/// "Излучение" (`broadcast` + `keep_source` вместе) — тоже ПОДДЕРЖИВАЕТСЯ,
/// исходный мотивирующий случай.
#[test]
fn test_build_gpu_rule_table_accepts_emit_broadcast_plus_keep_source() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![]);
    rule.shifts = vec![vec![ShiftSpec { direction: Direction::Right, steps: 3, broadcast: true, keep_source: true }]];
    idx.insert(CellType(1), vec![rule]);

    assert!(build_gpu_rule_table(&idx).is_ok(), "broadcast+keep_source (emit) must be GPU-supported");
}

/// `keep_source` + `feedback` вместе — вне GPU-подмножества (перенос
/// счётчика не портирован для случая, когда источник не освобождается).
#[test]
fn test_build_gpu_rule_table_rejects_feedback_plus_keep_source() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![]);
    rule.shifts = vec![vec![ShiftSpec { direction: Direction::Right, steps: 1, broadcast: false, keep_source: true }]];
    rule.feedback = Some(FeedbackSpec { timeout: 3, new_direction: Direction::Down });
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::FeedbackKeepSourceUnsupported { head: 1, rule_idx: 0 });
}

/// `keep_source` + `memory` вместе — та же причина, вне GPU-подмножества.
#[test]
fn test_build_gpu_rule_table_rejects_memory_plus_keep_source() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![]);
    rule.shifts = vec![vec![ShiftSpec { direction: Direction::Right, steps: 1, broadcast: false, keep_source: true }]];
    rule.memory = Some(MemorySpec {
        window: 1,
        record_trigger: RecordTrigger::NeighborType(Direction::Right),
        match_pattern: vec![RecordedValue::Type(CellType(9))],
    });
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::MemoryKeepSourceUnsupported { head: 1, rule_idx: 0 });
}

/// `Rule::max_activations` — вне GPU-подмножества (см. её doc-комментарий:
/// ключ `(head, rule_idx)` без позиции, нет готового GPU-буфера такой формы).
#[test]
fn test_build_gpu_rule_table_rejects_max_activations() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![]);
    rule.max_activations = Some(3);
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::MaxActivationsUnsupported { head: 1, rule_idx: 0 });
}

/// `Rule::memory` — ПОДДЕРЖИВАЕТСЯ (см. `GpuRuleTable::needs_memory`'s
/// doc-комментарий: та же persistent-storage техника, что уже применена к
/// `starvation_after`/`feedback`, снимает главное препятствие "GPU не
/// хранит состояние между тиками"). `RuleOutcome`-триггер, без сдвига —
/// простейший случай, окно в пределах потолка.
#[test]
fn test_build_gpu_rule_table_accepts_memory_rule_outcome() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![(0, 0, ChangeValue::Literal(2))]);
    rule.memory = Some(MemorySpec {
        window: 1,
        record_trigger: RecordTrigger::RuleOutcome,
        match_pattern: vec![crate::types::RecordedValue::Missed],
    });
    idx.insert(CellType(1), vec![rule]);

    let table = build_gpu_rule_table(&idx).expect("memory (RuleOutcome, no shift, window within cap) is within the v2 subset");
    assert!(table.needs_memory, "needs_memory must be true when a rule uses Rule::memory");
    assert!(table.needs_arbitration, "memory must force the Arbitrated pipeline even for an otherwise self-write-only rule");
    let r = &table.rules[0];
    assert_eq!(r.has_memory, 1);
    assert_eq!(r.memory_window, 1);
    assert_eq!(r.memory_trigger, 1, "RuleOutcome must encode as 1");
    assert_eq!(r.memory_pattern0, 257, "Missed must encode as 257");
    assert_eq!(r.memory_has_shift, 0);
}

/// `NeighborType`-триггер, С сдвигом (буфер обязан переезжать — см.
/// `memory_has_shift`).
#[test]
fn test_build_gpu_rule_table_accepts_memory_neighbor_type_with_shift() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![]);
    rule.shifts = vec![vec![ShiftSpec::new(Direction::Right, 1)]];
    rule.memory = Some(MemorySpec {
        window: 2,
        record_trigger: RecordTrigger::NeighborType(Direction::Up),
        match_pattern: vec![crate::types::RecordedValue::Type(CellType(9)), crate::types::RecordedValue::Type(CellType(3))],
    });
    idx.insert(CellType(1), vec![rule]);

    let table = build_gpu_rule_table(&idx).expect("memory (NeighborType, one non-broadcast shift, window within cap) is within the v2 subset");
    let r = &table.rules[0];
    assert_eq!(r.has_memory, 1);
    assert_eq!(r.memory_window, 2);
    assert_eq!(r.memory_trigger, 0, "NeighborType must encode as 0");
    assert_eq!((r.memory_dx, r.memory_dy), (0, -1), "Direction::Up must encode as (0,-1)");
    assert_eq!(r.memory_pattern0, 9);
    assert_eq!(r.memory_pattern1, 3);
    assert_eq!(r.memory_has_shift, 1, "a rule with exactly one shift must set memory_has_shift");
}

/// `memory`+`recursion` — CPU-side поддерживает `NeighborType`+`recursion`
/// (см. `applicator.rs`'s каскадный гейт), но GPU отвергает ЛЮБУЮ
/// комбинацию `memory`+`recursion`, независимо от триггера — каскадный
/// per-level гейт не реализован здесь (см. `MemoryRecursionUnsupported`'s
/// doc-комментарий).
#[test]
fn test_build_gpu_rule_table_rejects_memory_with_recursion() {
    use crate::types::RecursionSpec;

    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![(0, 0, ChangeValue::Literal(2))]);
    rule.recursion = Some(RecursionSpec { max_depth: 1, direction: Direction::Right });
    rule.memory = Some(MemorySpec {
        window: 1,
        record_trigger: RecordTrigger::NeighborType(Direction::Up),
        match_pattern: vec![crate::types::RecordedValue::Type(CellType(9))],
    });
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::MemoryRecursionUnsupported { head: 1, rule_idx: 0 });
}

#[test]
fn test_build_gpu_rule_table_rejects_memory_with_cam() {
    use crate::types::CamSearch;

    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![], vec![]);
    rule.cam = Some(CamSearch { radius: 3, target_type: CellType(9) });
    rule.memory = Some(MemorySpec {
        window: 1,
        record_trigger: RecordTrigger::RuleOutcome,
        match_pattern: vec![crate::types::RecordedValue::Applied],
    });
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::MemoryCamUnsupported { head: 1, rule_idx: 0 });
}

#[test]
fn test_build_gpu_rule_table_rejects_memory_window_too_large() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![(0, 0, ChangeValue::Literal(2))]);
    let window = super::MAX_MEMORY_WINDOW + 1;
    rule.memory = Some(MemorySpec {
        window,
        record_trigger: RecordTrigger::RuleOutcome,
        match_pattern: vec![crate::types::RecordedValue::Applied; window],
    });
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::MemoryWindowTooLarge { head: 1, rule_idx: 0, window });
}

#[test]
fn test_build_gpu_rule_table_rejects_memory_with_broadcast_shift() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![]);
    rule.shifts = vec![vec![ShiftSpec::broadcast(Direction::Right, 2)]];
    rule.memory = Some(MemorySpec {
        window: 1,
        record_trigger: RecordTrigger::NeighborType(Direction::Up),
        match_pattern: vec![crate::types::RecordedValue::Type(CellType(9))],
    });
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::MemoryBroadcastUnsupported { head: 1, rule_idx: 0 });
}

/// `feedback` + `cam` — не имеет собственной защиты
/// (`GpuUnsupportedReason::MemoryCamUnsupported` защищает только `memory`),
/// но должна быть отвергнута ЗАЩИТНО через существующую проверку "ровно
/// один сдвиг" (`feedback` требует `shift_count == 1`, а CAM-правило по
/// построению имеет `shift_count == 0` — см. `config.rs`'s "CAM это
/// единственный эффект правила"). Тест проверяет это ПРЕДПОЛОЖЕНИЕ, а не
/// принимает его на веру.
#[test]
fn test_build_gpu_rule_table_rejects_feedback_with_cam() {
    use crate::types::CamSearch;

    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![], vec![]);
    rule.cam = Some(CamSearch { radius: 3, target_type: CellType(9) });
    rule.feedback = Some(FeedbackSpec { timeout: 5, new_direction: Direction::Down });
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::TooManyShifts { head: 1, rule_idx: 0, len: 0 });
}

/// `feedback` + `changes` при том же смещении, что и объявленный сдвиг —
/// CPU-семантика "changes побеждают shifts" значит реальный тип на новой
/// позиции может оказаться НЕ `me.value`, ломая предположение переноса
/// счётчика (`update_feedback_relocate_pass`'s `slot_in_cell`-reuse). См.
/// `GpuUnsupportedReason::FeedbackChangeCollidesWithShiftTarget`'s
/// doc-комментарий.
#[test]
fn test_build_gpu_rule_table_rejects_feedback_change_colliding_with_shift_target() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![(1, 0, ChangeValue::Literal(99))]);
    rule.shifts = vec![vec![ShiftSpec::new(Direction::Right, 1)]];
    rule.feedback = Some(FeedbackSpec { timeout: 5, new_direction: Direction::Down });
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::FeedbackChangeCollidesWithShiftTarget { head: 1, rule_idx: 0 });
}

/// Тот же класс коллизии, но через `new_direction` (альтернативное
/// направление), а не декларированное — тоже обязано быть отвергнуто,
/// поскольку `feedback` переключается между ними в рантайме одного и того
/// же правила.
#[test]
fn test_build_gpu_rule_table_rejects_feedback_change_colliding_with_alt_direction() {
    let mut idx = HashMap::new();
    // Декларированный сдвиг — Right(1) => (1,0); alt-направление — Down =>
    // (0,1); change коллидирует с alt, не с декларированным.
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![(0, 1, ChangeValue::Literal(99))]);
    rule.shifts = vec![vec![ShiftSpec::new(Direction::Right, 1)]];
    rule.feedback = Some(FeedbackSpec { timeout: 5, new_direction: Direction::Down });
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::FeedbackChangeCollidesWithShiftTarget { head: 1, rule_idx: 0 });
}

/// То же для `memory` + сдвиг — см.
/// `GpuUnsupportedReason::MemoryChangeCollidesWithShiftTarget`'s
/// doc-комментарий.
#[test]
fn test_build_gpu_rule_table_rejects_memory_change_colliding_with_shift_target() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![(1, 0, ChangeValue::Literal(99))]);
    rule.shifts = vec![vec![ShiftSpec::new(Direction::Right, 1)]];
    rule.memory = Some(MemorySpec {
        window: 1,
        record_trigger: RecordTrigger::NeighborType(Direction::Up),
        match_pattern: vec![crate::types::RecordedValue::Type(CellType(9))],
    });
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::MemoryChangeCollidesWithShiftTarget { head: 1, rule_idx: 0 });
}

/// Non-collision sanity check — `changes` at a DIFFERENT offset than the
/// shift target must still be accepted (the new validation must not be
/// overly broad).
#[test]
fn test_build_gpu_rule_table_accepts_feedback_change_at_different_offset() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![(0, 2, ChangeValue::Literal(99))]);
    rule.shifts = vec![vec![ShiftSpec::new(Direction::Right, 1)]];
    rule.feedback = Some(FeedbackSpec { timeout: 5, new_direction: Direction::Down });
    idx.insert(CellType(1), vec![rule]);

    build_gpu_rule_table(&idx).expect("change at an unrelated offset must not be rejected");
}

/// A `changes` entry at the SOURCE position (0,0) — a legitimate,
/// DIFFERENT pattern (leave a marker behind at the vacated source cell
/// instead of a plain clear) — must NOT be rejected by the new
/// shift-target-collision check: the collision that matters is with the
/// shift's TARGET, not its source.
#[test]
fn test_build_gpu_rule_table_accepts_feedback_change_at_source_position() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![(0, 0, ChangeValue::Literal(77))]);
    rule.shifts = vec![vec![ShiftSpec::new(Direction::Right, 1)]];
    rule.feedback = Some(FeedbackSpec { timeout: 5, new_direction: Direction::Down });
    idx.insert(CellType(1), vec![rule]);

    build_gpu_rule_table(&idx).expect("change at the source (0,0), not the shift target, must not be rejected");
}

#[test]
fn test_build_gpu_rule_table_rejects_memory_with_more_than_one_shift() {
    let mut idx = HashMap::new();
    let mut rule = base_rule(vec![1], vec![(0, 0, 1)], vec![]);
    rule.shifts = vec![vec![ShiftSpec::new(Direction::Right, 1), ShiftSpec::new(Direction::Down, 1)]];
    rule.memory = Some(MemorySpec {
        window: 1,
        record_trigger: RecordTrigger::NeighborType(Direction::Up),
        match_pattern: vec![crate::types::RecordedValue::Type(CellType(9))],
    });
    idx.insert(CellType(1), vec![rule]);

    let err = build_gpu_rule_table(&idx).unwrap_err();
    assert_eq!(err, GpuUnsupportedReason::TooManyShifts { head: 1, rule_idx: 0, len: 2 });
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
