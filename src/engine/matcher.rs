use std::collections::{HashMap, HashSet};
use rayon::prelude::*;

use crate::grid::Grid;
use crate::storage::GridStorage;
use crate::types::{Cell, CellType, Rule, RuleMatch};

/// Предвычисленные данные для группы правил с общим head-типом.
/// Строится один раз до параллельной фазы — дёшево, O(число head-типов).
struct GroupData<'a> {
    rules: &'a [Rule],
    effective_patterns: Vec<Vec<(i8, i8, CellType)>>,
    all_offsets: Vec<(i8, i8)>,
    offset_map: HashMap<(i8, i8), usize>,
    /// Упакованные паттерны (u64) для паттернов ≤ 8 ячеек. Пусто, если
    /// в группе есть паттерн длиннее — тогда используется fallback-цикл.
    packed_patterns: Vec<(u64, u64)>,
}

fn build_group_data(rule_index: &HashMap<CellType, Vec<Rule>>) -> HashMap<CellType, GroupData<'_>> {
    rule_index
        .iter()
        .map(|(&cell_type, rules)| {
            // Эффективные паттерны: явный rule.pattern либо (обратная совместимость)
            // построенный из id как (0,0,id[0]), (1,0,id[1]), ...
            let effective_patterns: Vec<Vec<(i8, i8, CellType)>> = rules
                .iter()
                .map(|rule| {
                    if !rule.pattern.is_empty() {
                        rule.pattern.clone()
                    } else {
                        rule.id
                            .iter()
                            .enumerate()
                            .map(|(i, &ct)| (i as i8, 0i8, ct))
                            .collect()
                    }
                })
                .collect();

            // Собираем все уникальные смещения (dx, dy) для группы правил
            let mut all_offsets: Vec<(i8, i8)> = Vec::new();
            let mut offset_set: HashSet<(i8, i8)> = HashSet::new();
            for pat in &effective_patterns {
                for &(dx, dy, _) in pat {
                    if offset_set.insert((dx, dy)) {
                        all_offsets.push((dx, dy));
                    }
                }
            }

            // Карта: смещение → индекс в all_offsets / cache
            let offset_map: HashMap<(i8, i8), usize> = all_offsets
                .iter()
                .enumerate()
                .map(|(i, &o)| (o, i))
                .collect();

            // ─── Предвычисляем упакованные паттерны (u64) ───
            // Каждый байт в u64 — это значение CellType для одного смещения.
            // Позиция байта = индекс смещения в all_offsets.
            // Маска указывает, какие байты нужно проверять.
            // Для паттернов > 8 ячеек — fallback на цикл.
            let packed_patterns: Vec<(u64, u64)> = if all_offsets.len() <= 8 {
                effective_patterns
                    .iter()
                    .map(|pat| {
                        let mut packed = 0u64;
                        let mut mask = 0u64;
                        for &(dx, dy, expected) in pat {
                            if let Some(&idx) = offset_map.get(&(dx, dy)) {
                                let shift = (idx as u64) * 8;
                                packed |= (expected.0 as u64) << shift;
                                mask |= 0xFFu64 << shift;
                            }
                        }
                        (packed, mask)
                    })
                    .collect()
            } else {
                Vec::new()
            };

            (
                cell_type,
                GroupData {
                    rules: rules.as_slice(),
                    effective_patterns,
                    all_offsets,
                    offset_map,
                    packed_patterns,
                },
            )
        })
        .collect()
}

/// Обнаружить все совпадения правил на решётке.
///
/// # Оптимизации
///
/// 1. **Per-head-type precompute** — эффективные паттерны, смещения и
///    упакованные (u64) паттерны считаются один раз на head-тип (`GroupData`),
///    а не на ячейку и не на группу внутри параллельной фазы.
///
/// 2. **Neighborhood cache** — для каждой ячейки один раз загружаем значения всех
///    соседей, необходимых для паттернов её группы. Проверка каждого правила
///    идёт по кэшу, а не через `grid.get_cell`.
///
/// 3. **Параллелизация по активным ячейкам (rayon)** — в отличие от
///    параллелизации по группам правил (по head-типам), это масштабируется
///    независимо от того, сколько различных head-типов реально активно.
///    Если типов мало (например, 1-3 — Game of Life, Wireworld), деление
///    работы по группам оставляло почти всю работу одному потоку; деление по
///    ячейкам всегда использует все доступные ядра при достаточном числе
///    активных ячеек.
///
/// 4. **Упаковка паттерна в u64** — для паттернов ≤ 8 ячеек сравнение выполняется
///    одной инструкцией `(cache_u64 & mask) == pattern` вместо цикла.
pub fn detect_matches<S: GridStorage + Sync>(
    grid: &Grid<S>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    active_coords: &Vec<(usize, usize)>,
) -> Vec<RuleMatch> {
    let group_data = build_group_data(rule_index);
    let bounds = grid.storage.bounds();
    let default_cell = Cell::default();

    // Отфильтровываем клетки, чей тип вообще не имеет привязанных правил.
    // Раньше это делал `cells_by_type` (группировка по head-типу): клетки
    // без своей группы правил никогда не попадали в основной цикл. Без этого
    // шага параллельная фаза ниже проходила бы по ВСЕМ активным клеткам —
    // включая заведомо нерелевантные (например, "чужой" тип в шахматном
    // паттерне 1A: половина активных клеток вообще не может ни с чем
    // совпасть, но раньше на них тратился полный проход с поиском в
    // group_data и загрузкой ячейки).
    let relevant_cells: Vec<(usize, usize)> = active_coords
        .par_iter()
        .copied()
        .filter(|&(cx, cy)| {
            grid.get_cell(cx, cy)
                .map_or(false, |c| group_data.contains_key(&c.value.0))
        })
        .collect();

    relevant_cells
        .par_iter()
        .fold(
            // Аккумулятор на rayon-поток: (переиспользуемый буфер "хвоста"
            // кэша соседей >8 ячеек, накопленные матчи).
            || (Vec::<CellType>::new(), Vec::<RuleMatch>::new()),
            |(mut cache_overflow, mut local_matches), &(cx, cy)| {
                let center_cell = match grid.get_cell(cx, cy) {
                    Some(c) => c,
                    None => return (cache_overflow, local_matches),
                };
                let gd = match group_data.get(&center_cell.value.0) {
                    Some(gd) => gd,
                    None => return (cache_overflow, local_matches),
                };
                let center_age = grid.get_age(cx, cy);

                // ─── Загружаем кэш соседей ───
                // Быстрый путь (≤8 смещений, подавляющее большинство правил) —
                // на стеке, без аллокации. "Хвост" (>8) — в переиспользуемый
                // на весь поток Vec (буфер живёт в аккумуляторе fold, не
                // аллоцируется заново на каждую ячейку).
                let mut cache_arr: [CellType; 8] = [CellType(0); 8];
                cache_overflow.clear();
                let mut cache_valid = true;

                for (i, &(dx, dy)) in gd.all_offsets.iter().enumerate() {
                    let nx = cx.wrapping_add_signed(dx as isize);
                    let ny = cy.wrapping_add_signed(dy as isize);

                    if let Some((bw, bh)) = bounds {
                        if nx >= bw || ny >= bh {
                            cache_valid = false;
                            break;
                        }
                    }

                    // Отсутствующие (дефолтные) ячейки считаются CellType(0)
                    let ct = grid.get_cell(nx, ny).map_or(CellType(0), |c| c.value.0);
                    if i < 8 {
                        cache_arr[i] = ct;
                    } else {
                        cache_overflow.push(ct);
                    }
                }

                if !cache_valid {
                    return (cache_overflow, local_matches);
                }

                // Упаковываем кэш в u64 для быстрого сравнения
                let cache_u64 = if gd.all_offsets.len() <= 8 {
                    let mut packed = 0u64;
                    for (i, ct) in cache_arr.iter().enumerate().take(gd.all_offsets.len()) {
                        packed |= (ct.0 as u64) << ((i as u64) * 8);
                    }
                    Some(packed)
                } else {
                    None
                };

                // Доступ к кэшу по индексу — прозрачно поверх стека и "хвоста".
                let cache_at = |idx: usize| -> CellType {
                    if idx < 8 {
                        cache_arr[idx]
                    } else {
                        cache_overflow[idx - 8]
                    }
                };

                // ─── Проверяем каждое правило группы по кэшу ───
                for (rule_idx, rule) in gd.rules.iter().enumerate() {
                    if center_age < rule.min_age {
                        continue;
                    }

                    if rule.active_only
                        && center_cell.value == default_cell.value
                        && center_age == 0
                    {
                        continue;
                    }

                    let matched = if let Some(cache_packed) = cache_u64 {
                        // SIMD-сравнение: одна инструкция вместо цикла по паттерну
                        let (packed_pattern, packed_mask) = gd.packed_patterns[rule_idx];
                        (cache_packed & packed_mask) == packed_pattern
                    } else {
                        // Fallback: цикл по паттерну (для больших паттернов > 8 ячеек)
                        let pat = &gd.effective_patterns[rule_idx];
                        let mut m = true;
                        for &(dx, dy, expected) in pat {
                            let idx = match gd.offset_map.get(&(dx, dy)) {
                                Some(&i) => i,
                                None => {
                                    m = false;
                                    break;
                                }
                            };
                            if cache_at(idx) != expected {
                                m = false;
                                break;
                            }
                        }
                        m
                    };

                    if matched {
                        let pattern: Vec<Vec<u8>> =
                            vec![rule.id.iter().map(|ct| ct.0).collect()];
                        local_matches.push(RuleMatch {
                            x: cx as u32,
                            y: cy as u32,
                            pattern,
                            rule_id: rule.id.clone(),
                            rule_idx,
                        });
                    }
                }

                (cache_overflow, local_matches)
            },
        )
        .map(|(_, local_matches)| local_matches)
        .reduce(Vec::new, |mut a, b| {
            a.extend(b);
            a
        })
}