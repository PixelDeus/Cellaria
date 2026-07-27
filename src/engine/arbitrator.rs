use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use crate::conflict_analyzer::{get_rule_data, RuleDataCache};
use crate::types::{CellType, OverflowAction, Rule, RuleMatch};

/// Выбрать непротиворечивый набор совпадений.
///
/// Арбитраж проверяет пересечение ПОЛНЫХ affected regions (паттерн +
/// позиция сдвига + изменения), а не только паттерна. Это гарантирует,
/// что два совпадения не будут конфликтовать при применении, даже если
/// их паттерны не пересекаются, но их изменения затрагивают одни и те же
/// ячейки.
///
/// `bounds` — (width, height) решётки. Нужны для корректного учёта
/// `OverflowAction::Write`: реальная запись при выходе сдвига за границу
/// клэмпится на край решётки (см. `apply_shift_buffered`), а не остаётся на
/// исходной (возможно, отрицательной или запредельной) абстрактной позиции.
/// Без этого два матча — один с обычным сдвигом в пределах решётки, другой
/// с переполняющимся сдвигом — могут писать в одну и ту же реальную клетку,
/// оставаясь "непересекающимися" в абстрактных координатах.
///
/// Использует предвычисленный RuleDataCache для быстрого доступа к
/// affected cells без повторного вычисления.
pub fn arbitrate(
    all_matches: Vec<RuleMatch>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &RuleDataCache,
    bounds: (usize, usize),
    get_cell_age: impl Fn(usize, usize) -> u32,
) -> Vec<RuleMatch> {
    if all_matches.is_empty() {
        return Vec::new();
    }

    let mut accepted: Vec<RuleMatch> = Vec::new();
    // Ключ — (i32, i32), а не (u32, u32): affected cells считаются в
    // абстрактных координатах относительно позиции матча и могут уходить в
    // отрицательные значения (например, у сдвигов/changes с отрицательным
    // смещением рядом с (0,0)). Раньше здесь стоял guard `px >= 0 && py >= 0`,
    // из-за которого такие ячейки вообще не попадали ни в проверку конфликта,
    // ни в used_cells — два матча, оба уходящие в отрицательные координаты,
    // могли пройти арбитраж вместе, хотя их affected regions пересекались.
    // Это особенно опасно с OverflowAction::Write, где реальная запись при
    // overflow клэмпится на границу решётки (уже неотрицательную позицию),
    // а анализ конфликтов смотрел на исходную (отброшенную) координату.
    let mut used_cells: HashSet<(i32, i32)> = HashSet::new();

    // Полностью детерминированный тай-брейк: priority → age → rule_id →
    // координаты матча → rule_idx. Раньше при равенстве priority+age порядок
    // определялся тем, в каком порядке detect_matches нашёл матчи —
    // implementation-defined и невоспроизводимо в других реализациях (в
    // частности, в раундовом локальном арбитраже, который эту эквивалентность
    // и мотивировал). Явный тай-брейк даёт identical results с любой другой
    // реализацией, использующей тот же порядок сравнения, а не просто
    // "какой-то одинаково безопасный, но другой" результат.
    //
    // sort_by_cached_key, а не sort_by_key: ключ теперь клонирует rule_id
    // (Vec<CellType>), и без кэширования это могло бы вычисляться несколько
    // раз на элемент за один сорт.
    let mut sorted = all_matches;
    sorted.sort_by_cached_key(|m| {
        let priority = get_priority(m, rule_index);
        let age = get_cell_age(m.x as usize, m.y as usize);
        (
            Reverse(priority),
            Reverse(age),
            Reverse(m.rule_id.clone()),
            Reverse(m.x),
            Reverse(m.y),
            Reverse(m.rule_idx),
        )
    });

    for m in sorted {
        // Получаем предвычисленные affected cells из кэша
        let affected = get_match_affected_cells(&m, rule_index, rule_cache, bounds);
        let conflict = affected.iter().any(|coord| used_cells.contains(coord));

        if !conflict {
            used_cells.extend(affected.iter().copied());
            accepted.push(m);
        }
    }

    accepted
}

/// Получить приоритет правила, сработавшего в данном match'е.
/// Использует `rule_idx`, а не поиск по `rule_id` — несколько правил
/// могут иметь одинаковый id, и только `rule_idx` однозначно определяет,
/// какое именно правило сработало.
fn get_priority(m: &RuleMatch, rule_index: &HashMap<CellType, Vec<Rule>>) -> u32 {
    m.rule_id
        .first()
        .and_then(|first| rule_index.get(first))
        .and_then(|rules| rules.get(m.rule_idx))
        .map_or(0, |rule| rule.priority)
}

/// Вычислить полный набор affected cells для совпадения, используя кэш.
///
/// Берёт предвычисленные относительные affected cells из RuleDataCache
/// и сдвигает их на позицию совпадения. Клетка-цель сдвига дополнительно
/// клэмпится на границы решётки, если у правила `OverflowAction::Write` и
/// сдвиг уходит за пределы решётки — это ровно то, что реально делает
/// `apply_shift_buffered` при overflow-записи.
fn get_match_affected_cells(
    m: &RuleMatch,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &RuleDataCache,
    bounds: (usize, usize),
) -> Vec<(i32, i32)> {
    let head = match m.rule_id.first() {
        Some(&h) => h,
        None => return Vec::new(),
    };

    let rule_data = match get_rule_data(rule_cache, head, m.rule_idx) {
        Some(rd) => rd,
        None => {
            // Если правило не найдено в кэше, используем только паттерн
            let mut cells = Vec::new();
            for (i, _) in m.rule_id.iter().enumerate() {
                cells.push((m.x as i32 + i as i32, m.y as i32));
            }
            return cells;
        }
    };

    let (w, h) = (bounds.0 as i32, bounds.1 as i32);
    // Правило с несколькими сдвигами реплицирует значение в КАЖДУЮ цель
    // независимо (см. RuleData::shift_targets) — клэмпинг при
    // OverflowAction::Write применим к любой из них, не только к первой.
    let has_shift = !rule_data.shift_targets.is_empty();
    let overflow: Option<OverflowAction> = if has_shift {
        rule_index
            .get(&head)
            .and_then(|rules| rules.get(m.rule_idx))
            .map(|rule| rule.overflow)
    } else {
        None
    };

    rule_data
        .affected_cells
        .iter()
        .map(|&(dx, dy)| {
            let abs = (m.x as i32 + dx, m.y as i32 + dy);
            if w > 0 && h > 0 && rule_data.shift_targets.contains(&(dx, dy)) {
                if let Some(OverflowAction::Write(_)) = overflow {
                    if abs.0 < 0 || abs.0 >= w || abs.1 < 0 || abs.1 >= h {
                        return (abs.0.clamp(0, w - 1), abs.1.clamp(0, h - 1));
                    }
                }
            }
            abs
        })
        .collect()
}
