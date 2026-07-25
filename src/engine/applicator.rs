use std::collections::HashMap;

use crate::grid::Grid;
use crate::storage::GridStorage;
use crate::types::{
    AffectedRegion, Cell, CellType, CellValue, ChangeValue, Direction, OverflowAction, Rule,
    RuleMatch, ShiftSpec,
};

/// Применить набор совпадений к решётке.
pub fn apply_matches<S: GridStorage>(
    grid: &mut Grid<S>,
    matches: Vec<RuleMatch>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
) -> (Vec<AffectedRegion>, Vec<(u32, Cell)>) {
    let mut pending_boundary: Vec<(u32, Cell)> = Vec::new();
    let mut regions: Vec<AffectedRegion> = Vec::new();

    for m in matches {
        // Inline lookup: find rule by id in the index
        let rule = m.rule_id.first().and_then(|first| {
            rule_index.get(first).and_then(|rules| {
                rules.iter().find(|r| r.id == m.rule_id)
            })
        }).cloned();
        if let Some(rule) = rule {
            let (region, outputs) = apply_rule(grid, &m, &rule);
            regions.push(region);
            pending_boundary.extend(outputs);
        }
    }

    (regions, pending_boundary)
}

/// Применить одно правило к ячейке.
fn apply_rule<S: GridStorage>(
    grid: &mut Grid<S>,
    m: &RuleMatch,
    rule: &Rule,
) -> (AffectedRegion, Vec<(u32, Cell)>) {
    let cx = m.x as i32;
    let cy = m.y as i32;

    // Определяем bounding box affected region по паттерну
    let mut min_px = i32::MAX;
    let mut max_px = i32::MIN;
    let mut min_py = i32::MAX;
    let mut max_py = i32::MIN;
    for (dx, dy, _ct) in &rule.pattern {
        let px = cx + *dx as i32;
        let py = cy + *dy as i32;
        min_px = min_px.min(px);
        max_px = max_px.max(px);
        min_py = min_py.min(py);
        max_py = max_py.max(py);
    }
    let mut affected = AffectedRegion {
        x_start: min_px.max(0) as u32,
        x_end: (max_px + 1).max(0) as u32,
        y_start: min_py.max(0) as u32,
        y_end: (max_py + 1).max(0) as u32,
        has_changes: false,
    };

    // Фаза 0: буфер значений паттерна для динамических ссылок ($0, $1, ...)
    // Собираем в порядке rule.pattern
    let mut pattern_buffer: Vec<CellValue> = Vec::new();
    for (dx, dy, _ct) in &rule.pattern {
        let px = m.x as i32 + *dx as i32;
        let py = m.y as i32 + *dy as i32;
        let val = if px >= 0 && py >= 0 {
            grid.get_cell(px as usize, py as usize)
                .copied()
                .map(|c| c.value)
                .unwrap_or_default()
        } else {
            CellValue::default()
        };
        pattern_buffer.push(val);
    }

    let mut pending_outputs: Vec<(u32, Cell)> = Vec::new();

    // Фаза 1: сдвиги
    for shift_group in &rule.shifts {
        for shift in shift_group {
            apply_shift(grid, cx, cy, shift, &mut affected, rule, &mut pending_outputs);
        }
    }

    // Фаза 2: изменения — координаты корректируются суммарным total_dx/total_dy по всем группам сдвигов
    if !rule.changes.is_empty() {
        affected.has_changes = true;

        let (total_dx, total_dy) = {
            let mut dx = 0i32;
            let mut dy = 0i32;
            for group in &rule.shifts {
                for shift in group {
                    match shift.direction {
                        Direction::Up => dy -= shift.steps as i32,
                        Direction::Down => dy += shift.steps as i32,
                        Direction::Left => dx -= shift.steps as i32,
                        Direction::Right => dx += shift.steps as i32,
                    }
                }
            }
            (dx, dy)
        };

        for &(dx, dy, ref value) in &rule.changes {
            let nx = cx + total_dx + dx;
            let ny = cy + total_dy + dy;
            if nx >= 0 && ny >= 0 {
                let ux = nx as usize;
                let uy = ny as usize;
                let w = grid.width() as i32;
                let h = grid.height() as i32;

                if nx < w && ny < h {
                    let new_val = match *value {
                        ChangeValue::Literal(v) => CellValue(CellType::new(v)),
                        ChangeValue::Ref(i) => {
                            if i < pattern_buffer.len() {
                                pattern_buffer[i]
                            } else {
                                CellValue::default()
                            }
                        }
                    };

                    grid.set_cell(
                        ux,
                        uy,
                        Cell {
                            value: new_val,
                            age: 0,
                        },
                    );

                    affected.x_start = affected.x_start.min(ux as u32);
                    affected.x_end = affected.x_end.max(ux as u32 + 1);
                    affected.y_start = affected.y_start.min(uy as u32);
                    affected.y_end = affected.y_end.max(uy as u32 + 1);
                }
            }
        }
    }

    (affected, pending_outputs)
}

/// Применить цепочечный сдвиг — перемещает ТОЛЬКО первую ячейку паттерна (головку).
fn apply_shift<S: GridStorage>(
    grid: &mut Grid<S>,
    cx: i32,
    cy: i32,
    shift: &ShiftSpec,
    affected: &mut AffectedRegion,
    rule: &Rule,
    // Used by GUI/CLI for output collection
    #[allow(unused_variables)]
    pending_outputs: &mut Vec<(u32, Cell)>,
) {
    let w = grid.width() as i32;
    let h = grid.height() as i32;

    let (dx, dy) = match shift.direction {
        Direction::Up => (0, -1),
        Direction::Down => (0, 1),
        Direction::Left => (-1, 0),
        Direction::Right => (1, 0),
    };
    let steps = shift.steps as i32;

    let ox = cx;
    let oy = cy;
    if ox < 0 || ox >= w || oy < 0 || oy >= h {
        return;
    }
    let head_cell = match grid.get_cell(ox as usize, oy as usize).copied() {
        Some(cell) => cell,
        None => return,
    };

    let nx = ox + dx * steps;
    let ny = oy + dy * steps;

    // Include original position in affected region
    affected.x_start = affected.x_start.min(ox as u32);
    affected.x_end = affected.x_end.max(ox as u32 + 1);
    affected.y_start = affected.y_start.min(oy as u32);
    affected.y_end = affected.y_end.max(oy as u32 + 1);

    // Clear original position FIRST (before overflow write to same cell)
    grid.set_cell(ox as usize, oy as usize, Cell::default());

    if nx >= 0 && nx < w && ny >= 0 && ny < h {
        // Normal shift — move head
        grid.set_cell(nx as usize, ny as usize, head_cell);
        affected.x_start = affected.x_start.min(nx as u32);
        affected.x_end = affected.x_end.max(nx as u32 + 1);
        affected.y_start = affected.y_start.min(ny as u32);
        affected.y_end = affected.y_end.max(ny as u32 + 1);
    } else {
        // Overflow
        let bx = nx.clamp(0, w - 1) as usize;
        let by = ny.clamp(0, h - 1) as usize;
        match rule.overflow {
            OverflowAction::Discard => {
                // head cell is lost
            }
            OverflowAction::Write(value) => {
                if let Some(buf) = grid.get_boundary_mut(bx, by) {
                    let output_cell = if value != 0 {
                        Cell {
                            value: CellValue(CellType::new(value)),
                            age: head_cell.age,
                        }
                    } else {
                        head_cell
                    };
                    buf.enqueue(0, output_cell);
                } else {
                    // Fallback-запись в решётку при overflow: если граничный буфер не настроен,
                    // значение записывается напрямую в решётку на последнюю клетку перед границей.
                    // Это гарантирует, что данные не теряются даже без буфера.
                    let out_val = CellValue(CellType::new(value));
                    grid.set_cell(
                        bx,
                        by,
                        Cell {
                            value: out_val,
                            age: 0,
                        },
                    );
                    affected.x_start = affected.x_start.min(bx as u32);
                    affected.x_end = affected.x_end.max(bx as u32 + 1);
                    affected.y_start = affected.y_start.min(by as u32);
                    affected.y_end = affected.y_end.max(by as u32 + 1);
                }
            }
        }
    }
}