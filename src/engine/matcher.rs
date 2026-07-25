use std::collections::HashMap;

use crate::grid::Grid;
use crate::storage::GridStorage;
use crate::types::{Cell, CellType, Rule, RuleMatch};

/// Обнаружить все совпадения правил на решётке.
///
/// `coords` — список координат для проверки (уже расширенный на окрестность ±2
/// caller'ом через `expand_neighborhood`).
pub fn detect_matches<S: GridStorage>(
    grid: &Grid<S>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    coords: &[(usize, usize)],
) -> Vec<RuleMatch> {
    let mut matches: Vec<RuleMatch> = Vec::new();

    for (cell_type, rules) in rule_index {
        for rule in rules {
            for &(cx, cy) in coords {
                if let Some(center_cell) = grid.get_cell(cx, cy) {
                    if center_cell.value.0 != *cell_type {
                        continue;
                    }
                    if center_cell.age < rule.min_age {
                        continue;
                    }
                } else {
                    continue;
                }

                if rule.active_only {
                    let center_cell = grid
                        .get_cell(cx, cy)
                        .expect("center cell should exist after checking");
                    let default_cell = Cell::default();
                    if center_cell.value == default_cell.value && center_cell.age == 0 {
                        continue;
                    }
                }

                let rule_id: Vec<CellType> = rule.id.clone();

                // Если pattern пуст — строим из id (обратная совместимость)
                let effective_pattern: Vec<(i8, i8, CellType)> = if !rule.pattern.is_empty() {
                    rule.pattern.clone()
                } else {
                    rule_id.iter().enumerate()
                        .map(|(i, &ct)| (i as i8, 0i8, ct))
                        .collect()
                };

                // Проверяем по двумерному паттерну
                let mut matched = true;

                for (dx, dy, expected_type) in &effective_pattern {
                    let nx = cx.wrapping_add_signed(*dx as isize);
                    let ny = cy.wrapping_add_signed(*dy as isize);

                    // Проверяем границы
                    if let Some((bw, bh)) = grid.storage.bounds() {
                        if nx >= bw || ny >= bh {
                            matched = false;
                            break;
                        }
                    }

                    let cell = grid.get_cell(nx, ny);
                    match cell {
                        Some(c) if c.value.0 == *expected_type => {}
                        _ => {
                            matched = false;
                            break;
                        }
                    }
                }

                if matched {
                    // Строим pattern для RuleMatch (одномерный для совместимости)
                    let pattern: Vec<Vec<u8>> = vec![rule_id.iter().map(|ct| ct.0).collect()];

                    matches.push(RuleMatch {
                        x: cx as u32,
                        y: cy as u32,
                        pattern,
                        rule_id: rule_id.clone(),
                    });
                }
            }
        }
    }

    matches
}
