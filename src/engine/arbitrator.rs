use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use crate::conflict_analyzer::compute_affected_cells;
use crate::types::{CellType, Rule, RuleMatch};

/// Выбрать непротиворечивый набор совпадений.
///
/// Арбитраж проверяет пересечение ПОЛНЫХ affected regions (паттерн +
/// позиция сдвига + изменения), а не только паттерна. Это гарантирует,
/// что два совпадения не будут конфликтовать при применении, даже если
/// их паттерны не пересекаются, но их изменения затрагивают одни и те же
/// ячейки.
pub fn arbitrate(
    all_matches: Vec<RuleMatch>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    get_cell_age: impl Fn(usize, usize) -> u32,
) -> Vec<RuleMatch> {
    if all_matches.is_empty() {
        return Vec::new();
    }

    let mut accepted: Vec<RuleMatch> = Vec::new();
    let mut used_cells: HashSet<(u32, u32)> = HashSet::new();

    let mut sorted = all_matches;
    sorted.sort_by_key(|m| {
        let priority = get_priority(&m.rule_id, rule_index);
        let age = get_cell_age(m.x as usize, m.y as usize);
        (Reverse(priority), Reverse(age))
    });

    for m in sorted {
        // Вычисляем полный набор affected cells для этого совпадения
        let affected = get_match_affected_cells(&m, rule_index);
        let mut conflict = false;

        for &(px, py) in &affected {
            if px >= 0 && py >= 0 {
                let coord = (px as u32, py as u32);
                if used_cells.contains(&coord) {
                    conflict = true;
                    break;
                }
            }
        }

        if !conflict {
            for &(px, py) in &affected {
                if px >= 0 && py >= 0 {
                    used_cells.insert((px as u32, py as u32));
                }
            }
            accepted.push(m);
        }
    }

    accepted
}

/// Получить приоритет правила по его ID.
fn get_priority(rule_id: &[CellType], rule_index: &HashMap<CellType, Vec<Rule>>) -> u32 {
    if let Some(first) = rule_id.first() {
        if let Some(rules) = rule_index.get(first) {
            for rule in rules {
                if rule.id == rule_id {
                    return rule.priority;
                }
            }
        }
    }
    0
}

/// Вычислить полный набор affected cells для совпадения.
///
/// Uses `compute_affected_cells` from conflict_analyzer for relative coordinates,
/// then shifts them by the match position.
fn get_match_affected_cells(
    m: &RuleMatch,
    rule_index: &HashMap<CellType, Vec<Rule>>,
) -> Vec<(i32, i32)> {
    let rule = find_rule(&m.rule_id, rule_index);
    if let Some(rule) = rule {
        let relative = compute_affected_cells(rule);
        relative
            .iter()
            .map(|&(dx, dy)| (m.x as i32 + dx, m.y as i32 + dy))
            .collect()
    } else {
        // Если правило не найдено, используем только паттерн
        let mut cells = Vec::new();
        for (i, _) in m.rule_id.iter().enumerate() {
            cells.push((m.x as i32 + i as i32, m.y as i32));
        }
        cells
    }
}

/// Найти правило по его ID.
fn find_rule<'a>(
    rule_id: &[CellType],
    rule_index: &'a HashMap<CellType, Vec<Rule>>,
) -> Option<&'a Rule> {
    if let Some(first) = rule_id.first() {
        if let Some(rules) = rule_index.get(first) {
            for rule in rules {
                if rule.id == rule_id {
                    return Some(rule);
                }
            }
        }
    }
    None
}