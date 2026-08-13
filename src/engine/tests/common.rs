//! Общие хелперы, используемые тестами в нескольких из соседних файлов
//! этого модуля. Каждый файл подключает их через `use super::common::*;`.

use super::super::*;
use crate::types::{CellType, ChangeValue, Rule};
use crate::VecStorage;

pub(super) fn make_grid(w: usize, h: usize) -> Grid<VecStorage> {
    Grid::from_storage(VecStorage::new(w, h))
}

pub(super) fn make_rule_index(rules: Vec<Rule>) -> HashMap<CellType, Vec<Rule>> {
    let mut index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        if let Some(first) = rule.id.first() {
            index.entry(*first).or_default().push(rule);
        }
    }
    index
}

/// Используется в `fairness.rs` (собственные тесты голодания) И
/// `persistence.rs` (snapshot/replay-тесты используют голодание как
/// удобный источник персистентного состояния для проверки) -- вынесено
/// сюда при разбиении `engine/tests.rs`, а не продублировано в обоих.
pub(super) fn make_starvation_rules(low_starvation_after: Option<u32>) -> HashMap<CellType, Vec<Rule>> {
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
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
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
        starvation_after: low_starvation_after,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    make_rule_index(vec![rule_high, rule_low])
}
