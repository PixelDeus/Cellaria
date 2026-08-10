use super::*;
use crate::types::{CellType, ChangeValue, Rule};

/// Смешанная группа: одна голова, два правила — одно ПОЛНОСТЬЮ
/// специфицирует соседа (годится для `exact_lookup`), другое — id-fallback
/// без паттерна (wildcard относительно объединённого `all_offsets` группы,
/// остаётся в `fallback_rules`). До фикса v0.6.0 единственного wildcard-
/// правила в группе хватало, чтобы отключить `exact_lookup` для ВСЕЙ
/// группы целиком — см. doc-комментарий `GroupData::exact_lookup`.
fn mixed_group_rule_index() -> HashMap<CellType, Vec<Rule>> {
    let full = Rule {
        id: vec![CellType(10)],
        pattern: vec![(0, 0, CellType(10)), (1, 0, CellType(20))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(11))],
        active_only: false,
        priority: 20,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
    };
    let wildcard = Rule {
        id: vec![CellType(10)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(12))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
    };
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    idx.insert(CellType(10), vec![full, wildcard]);
    idx
}

#[test]
fn test_mixed_group_exact_lookup_covers_fully_specified_rule() {
    let group_cache = build_group_data(&mixed_group_rule_index());
    let gd = group_cache.get(&CellType(10)).expect("head 10 must have GroupData");

    assert!(
        gd.exact_lookup.is_some(),
        "полностью специфицированное правило должно попасть в exact_lookup, даже рядом с wildcard-соседом"
    );
    let lookup = gd.exact_lookup.as_ref().unwrap();
    let total_exact_rules: usize = lookup.values().map(|v| v.len()).sum();
    assert_eq!(total_exact_rules, 1, "ровно одно (rule_idx=0) правило должно быть покрыто exact_lookup");
}

#[test]
fn test_mixed_group_fallback_rules_covers_only_wildcard() {
    let group_cache = build_group_data(&mixed_group_rule_index());
    let gd = group_cache.get(&CellType(10)).expect("head 10 must have GroupData");

    assert_eq!(
        gd.fallback_rules, vec![1],
        "только wildcard-правило (rule_idx=1) должно остаться в fallback_rules"
    );
}

#[test]
fn test_mixed_group_matches_correctly_despite_split() {
    // Сквозная проверка: обе клетки-сценарии (сосед совпадает / сосед не
    // совпадает с полным правилом) должны находить ПРАВИЛЬНОЕ совпадающее
    // правило — раскол на exact_lookup/fallback_rules не должен путать,
    // какое правило реально сработало.
    use crate::grid::Grid;
    use crate::storage::VecStorage;
    use crate::types::{Cell, CellValue};

    let rule_index = mixed_group_rule_index();

    // Сценарий 1: сосед = 20 -> оба правила совпадают (полное — потому что
    // условие на соседа выполнено, wildcard — оно вообще не смотрит на
    // соседа, значит совпадает всегда).
    let mut grid1 = Grid::new(VecStorage::new(2, 1), Default::default());
    grid1.set_cell(0, 0, Cell { value: CellValue(CellType(10)), born_at: 0 });
    grid1.set_cell(1, 0, Cell { value: CellValue(CellType(20)), born_at: 0 });
    let mut rule_idxs1: Vec<usize> = detect_matches(&grid1, &rule_index, &vec![(0, 0)])
        .iter().map(|m| m.rule_idx).collect();
    rule_idxs1.sort_unstable();
    assert_eq!(rule_idxs1, vec![0, 1], "сосед=20: и точное (0), и wildcard (1) правило должны совпасть");

    // Сценарий 2: сосед НЕ 20 -> только wildcard-правило (rule_idx=1) совпадает,
    // точное (rule_idx=0) требует конкретно 20 и не срабатывает.
    let mut grid2 = Grid::new(VecStorage::new(2, 1), Default::default());
    grid2.set_cell(0, 0, Cell { value: CellValue(CellType(10)), born_at: 0 });
    grid2.set_cell(1, 0, Cell { value: CellValue(CellType(99)), born_at: 0 });
    let matches2 = detect_matches(&grid2, &rule_index, &vec![(0, 0)]);
    assert_eq!(matches2.len(), 1);
    assert_eq!(matches2[0].rule_idx, 1, "сосед!=20 должен совпасть только с wildcard-правилом (rule_idx=1)");
}
