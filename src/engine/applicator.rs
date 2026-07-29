use std::collections::HashMap;

use crate::conflict_analyzer::{get_rule_data, RuleDataCache};
use crate::fast_hash::FxHashMap;
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
///
/// `FxHashMap`, а не стандартный `HashMap` (SipHash): чисто внутренний тип
/// (не часть публичного API), число записей за тик обычно единицы —
/// SipHash-инициализация и раунды сжатия на таком объёме заметно дороже
/// самой записи (см. `fast_hash` модуль).
type WriteBuffer = FxHashMap<(u32, u32), Cell>;

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
    let mut write_buffer: WriteBuffer = WriteBuffer::default();

    for m in matches {
        // Резолвим по rule_idx, а не поиском по id: несколько правил могут
        // иметь одинаковую голову (недетерминированный выбор), и только
        // rule_idx однозначно определяет, какое именно правило сработало
        // для этого match'а.
        //
        // Без `.cloned()`: `apply_rule_buffered` использует правило только по
        // ссылке, а клонирование целого `Rule` (вложенные `Vec` в pattern/
        // changes/shifts/id) на каждый match каждого тика было чистой тратой.
        let rule = rule_index
            .get(&m.head)
            .and_then(|rules| rules.get(m.rule_idx));
        if let Some(rule) = rule {
            let rule_data = get_rule_data(rule_cache, m.head, m.rule_idx);
            // Пишем сразу в общий буфер, а не в свой локальный внутри
            // `apply_rule_buffered` с последующим `extend` — arbitration уже
            // гарантирует отсутствие пересечения записей между matches, так
            // что объединять нечего, а лишний HashMap на каждый match — это
            // лишняя аллокация и хэширование на пустом месте.
            let (region, outputs) = apply_rule_buffered(grid, &m, rule, rule_data, &mut write_buffer);
            regions.push(region);
            pending_boundary.extend(outputs);
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
    write_buffer: &mut WriteBuffer,
) -> (AffectedRegion, Vec<(u32, Cell)>) {
    let cx = m.x as i32;
    let cy = m.y as i32;

    // Bounding box стартует ПУСТЫМ (никаких клеток паттерна!), а не от
    // паттерна — паттерн только ЧИТАЕТСЯ, а `AffectedRegion` из этой функции
    // используется ровно одним потребителем: `reset_age_for_regions`,
    // которая сбрасывает возраст (born_at) КАЖДОЙ клетки внутри bbox. Если
    // бы bbox включал клетки паттерна, возраст соседа, которого правило
    // только ПРОЧИТАЛО (не записало), сбрасывался бы в 0 каждый раз, когда
    // рядом срабатывает чужое правило — ломая `min_age` для этого соседа
    // навсегда, даже если сам он не менялся (найдено экспериментально: сосед
    // в паттерне держал age=0 бесконечно). Ниже `apply_shift_buffered`/
    // `apply_changes_at` сами расширяют bbox по РЕАЛЬНЫМ целям записи —
    // этого достаточно, читать паттерн заново не нужно.
    let mut affected = AffectedRegion {
        x_start: u32::MAX,
        x_end: 0,
        y_start: u32::MAX,
        y_end: 0,
        has_changes: false,
        written_cells: Vec::new(),
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

    let mut pending_outputs: Vec<(u32, Cell)> = Vec::new();
    let gen = grid.generation();

    // Фаза 1: сдвиги — перемещают головку.
    // Читают из grid (старое состояние), пишут в буфер.
    for shift_group in &rule.shifts {
        for shift in shift_group {
            apply_shift_buffered(
                grid, cx, cy, shift, rule, &mut affected,
                write_buffer, &mut pending_outputs, gen,
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
            apply_changes_at(rule, &pattern_buffer, cx, cy, (0, 0), grid, gen, write_buffer, &mut affected);
        } else {
            for &target in shift_targets {
                apply_changes_at(rule, &pattern_buffer, cx, cy, target, grid, gen, write_buffer, &mut affected);
            }
        }
    }

    (affected, pending_outputs)
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

            // Сравнение через usize, а не `grid.width() as i32`: у
            // ChunkStorage (безграничная решётка) width()/height() —
            // usize::MAX, а `as i32` от usize::MAX даёт -1 (переполнение
            // при усечении), из-за чего `nx < w` было ложным ВСЕГДА —
            // changes на ChunkStorage не применялись НИ РАЗУ ни при каких
            // координатах. Найдено экспериментально: простейшее "1 -> 2"
            // без паттерна и сдвигов не срабатывало вообще.
            if (nx as usize) < grid.width() && (ny as usize) < grid.height() {
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
                affected.written_cells.push((ux, uy));

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
    // usize, а не `grid.width() as i32` — см. подробный комментарий в
    // `apply_changes_at` про то же самое усечение usize::MAX -> -1 на
    // ChunkStorage, из-за которого сдвиги на безграничной решётке тоже
    // никогда не применялись (функция всегда возвращалась на строке ниже).
    let w = grid.width();
    let h = grid.height();

    let (dx, dy) = match shift.direction {
        Direction::Up => (0, -1),
        Direction::Down => (0, 1),
        Direction::Left => (-1, 0),
        Direction::Right => (1, 0),
    };
    let steps = shift.steps as i32;

    let ox = cx;
    let oy = cy;
    if ox < 0 || oy < 0 || (ox as usize) >= w || (oy as usize) >= h {
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
    affected.written_cells.push((ox as u32, oy as u32));

    if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
        // Normal shift — move head
        write_buffer.insert((nx as u32, ny as u32), head_cell);
        affected.written_cells.push((nx as u32, ny as u32));
        affected.x_start = affected.x_start.min(nx as u32);
        affected.x_end = affected.x_end.max(nx as u32 + 1);
        affected.y_start = affected.y_start.min(ny as u32);
        affected.y_end = affected.y_end.max(ny as u32 + 1);
    } else {
        // Overflow (уходит за границу ЛИБО меньше нуля, ЛИБО (только на
        // конечных решётках) не меньше ширины/высоты — на ChunkStorage
        // верхняя граница практически недостижима, реальный overflow там
        // означает только уход в отрицательные координаты).
        let bx = if nx < 0 { 0 } else { (nx as usize).min(w.saturating_sub(1)) };
        let by = if ny < 0 { 0 } else { (ny as usize).min(h.saturating_sub(1)) };
        match rule.overflow {
            OverflowAction::Discard => {
                // head cell is lost
            }
            OverflowAction::Write(value) => {
                let output_value = if value != 0 { Some(value) } else { None };
                apply_overflow_write(grid, bx, by, output_value, head_cell, gen, write_buffer, affected, pending_outputs);
            }
            OverflowAction::WriteLiteral(value) => {
                apply_overflow_write(grid, bx, by, Some(value), head_cell, gen, write_buffer, affected, pending_outputs);
            }
        }
    }
}

/// Общая часть `Write`/`WriteLiteral`: `output_value = Some(v)` — записать
/// буквально `v`; `None` — пронести собственное значение головки
/// (`head_cell`) как есть. Раньше это различение (литерал против "своего
/// значения") было закодировано неявно через `value != 0` внутри одного
/// варианта `Write` — из-за чего буквальный литерал `0` был невыразим:
/// `Write(0)` всегда означал "пронести своё", никогда "запиши 0". `WriteLiteral`
/// снимает это ограничение явным вторым вариантом вместо перегрузки нуля.
#[allow(clippy::too_many_arguments)]
fn apply_overflow_write<S: GridStorage>(
    grid: &mut Grid<S>,
    bx: usize,
    by: usize,
    output_value: Option<u8>,
    head_cell: Cell,
    gen: u64,
    write_buffer: &mut WriteBuffer,
    affected: &mut AffectedRegion,
    pending_outputs: &mut Vec<(u32, Cell)>,
) {
    if let Some(buf) = grid.get_boundary_mut(bx, by) {
        let output_cell = match output_value {
            Some(value) => Cell {
                value: CellValue(CellType::new(value)),
                born_at: gen,
            },
            None => head_cell,
        };
        buf.enqueue(0, output_cell);
        pending_outputs.push((bx as u32, output_cell));
    } else {
        // Fallback-запись в решётку при overflow
        let value = output_value.unwrap_or(head_cell.value.0 .0);
        let cell = Cell {
            value: CellValue(CellType::new(value)),
            born_at: gen,
        };
        write_buffer.insert((bx as u32, by as u32), cell);
        affected.written_cells.push((bx as u32, by as u32));
        affected.x_start = affected.x_start.min(bx as u32);
        affected.x_end = affected.x_end.max(bx as u32 + 1);
        affected.y_start = affected.y_start.min(by as u32);
        affected.y_end = affected.y_end.max(by as u32 + 1);
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