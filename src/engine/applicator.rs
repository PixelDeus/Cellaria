use std::collections::HashMap;

use crate::conflict_analyzer::{get_rule_data, RuleDataCache};
use crate::grid::Grid;
use crate::storage::GridStorage;
use crate::types::{
    AffectedRegion, Cell, CellType, CellValue, ChangeValue, Direction, OverflowAction, Rule,
    RuleMatch, ShiftSpec,
};

/// Буфер изменений: координата → новое значение ячейки.
/// Все изменения читают старые значения из решётки, но пишут в буфер.
/// После обработки всех match'ей буфер атомарно применяется к решётке,
/// что даёт клеточно-автоматную семантику (все изменения параллельны).
type WriteBuffer = HashMap<(u32, u32), Cell>;

/// Применить набор совпадений к решётке с буферизацией.
///
/// Все изменения читают исходное состояние решётки, затем применяются
/// атомарно. Это гарантирует детерминированное клеточно-автоматное
/// поведение: все правила видят одни и те же старые значения,
/// независимо от порядка правил.
pub fn apply_matches<S: GridStorage>(
    grid: &mut Grid<S>,
    matches: Vec<RuleMatch>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &RuleDataCache,
) -> (Vec<AffectedRegion>, Vec<(u32, Cell)>) {
    let mut pending_boundary: Vec<(u32, Cell)> = Vec::new();
    let mut regions: Vec<AffectedRegion> = Vec::new();
    // Общий буфер для всех match'ей
    let mut write_buffer: WriteBuffer = HashMap::new();

    for m in matches {
        // Резолвим по rule_idx, а не поиском по id: несколько правил могут
        // иметь одинаковый id (недетерминированный выбор), и только rule_idx
        // однозначно определяет, какое именно правило сработало для этого match'а.
        let rule = m
            .rule_id
            .first()
            .and_then(|first| rule_index.get(first))
            .and_then(|rules| rules.get(m.rule_idx))
            .cloned();
        if let Some(rule) = rule {
            let rule_data = m
                .rule_id
                .first()
                .and_then(|&head_type| get_rule_data(rule_cache, head_type, m.rule_idx));
            let (region, outputs, rule_buf) = apply_rule_buffered(grid, &m, &rule, rule_data);
            regions.push(region);
            pending_boundary.extend(outputs);
            // Объединяем буфер правила в общий буфер.
            // Так как arbitration уже гарантирует непротиворечивость,
            // перезапись одной клетки разными правилами исключена.
            write_buffer.extend(rule_buf);
        }
    }

    // Фаза flush: атомарно применяем все изменения из буфера в решётку
    let gen = grid.generation();
    for ((x, y), cell) in write_buffer {
        let w = grid.width();
        let h = grid.height();
        if x < w as u32 && y < h as u32 {
            // born_at выставляется уже в apply_rule_buffered,
            // но перепроверяем для безопасности
            let final_cell = Cell {
                value: cell.value,
                born_at: gen,
            };
            grid.set_cell(x as usize, y as usize, final_cell);
        }
    }

    (regions, pending_boundary)
}

/// Применить одно правило к ячейке с буферизацией.
///
/// Семантика (соответствует оригинальной sequential):
///   1. Сдвиги — перемещают головку, очищают старую позицию.
///   2. Changes — модифицируют ячейки ОТНОСИТЕЛЬНО НОВОЙ позиции головки
///      (после сдвига). Читают из grid (старое состояние), но пишут
///      со смещением total_shift так, чтобы изменения применились
///      к правильным клеткам после сдвига.
///
/// В буфере сдвиги записываются первыми, а changes — вторыми,
/// перезаписывая любые конфликтующие записи. Это даёт sequential-семантику
/// в пределах одного правила (сдвиг → change), но CA-семантику между
/// разными правилами.
fn apply_rule_buffered<S: GridStorage>(
    grid: &mut Grid<S>,
    m: &RuleMatch,
    rule: &Rule,
    rule_data: Option<&crate::conflict_analyzer::RuleData>,
) -> (AffectedRegion, Vec<(u32, Cell)>, WriteBuffer) {
    let cx = m.x as i32;
    let cy = m.y as i32;

    // Определяем bounding box affected region по паттерну
    let (min_px, max_px, min_py, max_py) = if let Some(data) = rule_data {
        let mut min_px = i32::MAX;
        let mut max_px = i32::MIN;
        let mut min_py = i32::MAX;
        let mut max_py = i32::MIN;
        for &(dx, dy) in &data.pattern_cells {
            let px = cx + dx;
            let py = cy + dy;
            min_px = min_px.min(px);
            max_px = max_px.max(px);
            min_py = min_py.min(py);
            max_py = max_py.max(py);
        }
        (min_px, max_px, min_py, max_py)
    } else {
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
        (min_px, max_px, min_py, max_py)
    };
    let mut affected = AffectedRegion {
        x_start: min_px.max(0) as u32,
        x_end: (max_px + 1).max(0) as u32,
        y_start: min_py.max(0) as u32,
        y_end: (max_py + 1).max(0) as u32,
        has_changes: false,
    };

    // Буфер значений паттерна для динамических ссылок ($0, $1, ...)
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

    let mut write_buffer: WriteBuffer = HashMap::new();
    let mut pending_outputs: Vec<(u32, Cell)> = Vec::new();
    let gen = grid.generation();

    // Фаза 1: сдвиги — перемещают головку.
    // Читают из grid (старое состояние), пишут в буфер.
    for shift_group in &rule.shifts {
        for shift in shift_group {
            apply_shift_buffered(
                grid, cx, cy, shift, rule, &mut affected,
                &mut write_buffer, &mut pending_outputs, gen,
            );
        }
    }

    // Фаза 2: изменения (changes) — ПЕРЕЗАПИСЫВАЮТ сдвиги при конфликте.
    //
    // Применяются относительно КАЖДОЙ реальной цели сдвига (не относительно
    // их суммы) — правило с несколькими сдвигами реплицирует значение
    // головки в каждую цель независимо (см. apply_shift_buffered выше), а
    // не двигает его по цепочке, поэтому единой "новой позиции головки"
    // при 2+ сдвигах не существует. При ровно одном сдвиге это сохраняет
    // прежнюю sequential-семантику (сдвиг → change поверх новой позиции);
    // без сдвигов — changes применяются относительно исходной позиции (0,0).
    if !rule.changes.is_empty() {
        affected.has_changes = true;

        let fallback_targets;
        let shift_targets: &[(i32, i32)] = if let Some(data) = rule_data {
            &data.shift_targets
        } else {
            fallback_targets = compute_shift_targets_fallback(rule);
            &fallback_targets
        };

        if shift_targets.is_empty() {
            apply_changes_at(rule, &pattern_buffer, cx, cy, (0, 0), grid, gen, &mut write_buffer, &mut affected);
        } else {
            for &target in shift_targets {
                apply_changes_at(rule, &pattern_buffer, cx, cy, target, grid, gen, &mut write_buffer, &mut affected);
            }
        }
    }

    (affected, pending_outputs, write_buffer)
}

/// Применить `rule.changes` относительно одной цели сдвига (или (0,0), если
/// сдвигов нет). Вынесено отдельно, потому что правило с несколькими
/// сдвигами вызывает это один раз на каждую цель (см. вызывающий код).
#[allow(clippy::too_many_arguments)]
fn apply_changes_at<S: GridStorage>(
    rule: &Rule,
    pattern_buffer: &[CellValue],
    cx: i32,
    cy: i32,
    (base_dx, base_dy): (i32, i32),
    grid: &Grid<S>,
    gen: u64,
    write_buffer: &mut WriteBuffer,
    affected: &mut AffectedRegion,
) {
    for &(dx, dy, ref value) in &rule.changes {
        let nx = cx + base_dx + dx;
        let ny = cy + base_dy + dy;
        if nx >= 0 && ny >= 0 {
            let ux = nx as u32;
            let uy = ny as u32;
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

                let cell = Cell {
                    value: new_val,
                    born_at: gen,
                };
                // insert перезаписывает — changes побеждают сдвиги
                write_buffer.insert((ux, uy), cell);

                affected.x_start = affected.x_start.min(ux);
                affected.x_end = affected.x_end.max(ux + 1);
                affected.y_start = affected.y_start.min(uy);
                affected.y_end = affected.y_end.max(uy + 1);
            }
        }
    }
}

/// Применить цепочечный сдвиг с буферизацией.
///
/// Читает головку из grid (старое значение), пишет очистку (0,0)
/// и запись в (nx,ny) в буфер.
fn apply_shift_buffered<S: GridStorage>(
    grid: &mut Grid<S>,
    cx: i32,
    cy: i32,
    shift: &ShiftSpec,
    rule: &Rule,
    affected: &mut AffectedRegion,
    write_buffer: &mut WriteBuffer,
    pending_outputs: &mut Vec<(u32, Cell)>,
    gen: u64,
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
    // Читаем головку из grid (старое значение)
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

    // Clear original position (write to buffer, not grid)
    write_buffer.insert((ox as u32, oy as u32), Cell::default());

    if nx >= 0 && nx < w && ny >= 0 && ny < h {
        // Normal shift — move head
        write_buffer.insert((nx as u32, ny as u32), head_cell);
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
                            born_at: gen,
                        }
                    } else {
                        head_cell
                    };
                    buf.enqueue(0, output_cell);
                    pending_outputs.push((bx as u32, output_cell));
                } else {
                    // Fallback-запись в решётку при overflow
                    let out_val = CellValue(CellType::new(value));
                    let cell = Cell {
                        value: out_val,
                        born_at: gen,
                    };
                    write_buffer.insert((bx as u32, by as u32), cell);
                    affected.x_start = affected.x_start.min(bx as u32);
                    affected.x_end = affected.x_end.max(bx as u32 + 1);
                    affected.y_start = affected.y_start.min(by as u32);
                    affected.y_end = affected.y_end.max(by as u32 + 1);
                }
            }
        }
    }
}

/// Вычислить shift_targets fallback (без кэша) — цель КАЖДОГО отдельного
/// сдвига, а не суммарная (см. `RuleData::shift_targets`).
fn compute_shift_targets_fallback(rule: &Rule) -> Vec<(i32, i32)> {
    let mut targets = Vec::new();
    for group in &rule.shifts {
        for shift in group {
            let (dx, dy) = match shift.direction {
                Direction::Up => (0, -(shift.steps as i32)),
                Direction::Down => (0, shift.steps as i32),
                Direction::Left => (-(shift.steps as i32), 0),
                Direction::Right => (shift.steps as i32, 0),
            };
            targets.push((dx, dy));
        }
    }
    targets
}