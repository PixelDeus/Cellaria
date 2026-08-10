use std::collections::HashMap;
use rayon::prelude::*;

use crate::fast_hash::{FxHashMap, FxHashSet};
use crate::grid::Grid;
use crate::storage::GridStorage;
use crate::types::{Cell, CellType, Rule, RuleMatch};

/// Найденная позиция CAM-совпадения, ключ — (x, y, rule_idx) магнита (то же
/// самое, что уникально идентифицирует матч в тай-брейке арбитража, см.
/// `arbitrator::arbitrate`'s doc-комментарий про (x,y,rule_idx)) — переживает
/// сортировку/фильтрацию матчей в `arbitrate`, в отличие от позиционного
/// индекса в исходном Vec.
pub(crate) type CamPositions = FxHashMap<(u32, u32, usize), (u32, u32)>;

/// Предвычисленные данные для группы правил с общим head-типом.
/// Строится один раз до параллельной фазы — дёшево, O(число head-типов).
///
/// Без времени жизни (в отличие от исходной версии, где было поле
/// `rules: &'a [Rule]`) — вместо ссылки на правила хранит только то, что
/// реально читается из них в `match_cell` (`min_age`, `active_only`), уже
/// скопированным. Это позволяет закэшировать `GroupCache` внутри `Engine`
/// (см. `Engine::group_cache`) без самоссылающейся структуры: поле,
/// заимствующее соседнее поле того же `Engine`, потребовало бы unsafe или
/// отдельного крейта (`ouroboros`/`self_cell`) — цена, не оправданная тем,
/// что сюда нужно от `Rule` всего два маленьких Copy-поля.
pub(crate) struct GroupData {
    /// (min_age, active_only) каждого правила группы, в том же порядке,
    /// что и `rule_index[head]` — используется вместо `&[Rule]`.
    /// (min_age, active_only, is_cam). `is_cam` — правило с `Rule::cam`
    /// (см. её doc-комментарий в `types.rs`) детектируется ОТДЕЛЬНЫМ
    /// проходом (`detect_cam_matches`), не здесь — этот флаг нужен только
    /// чтобы такое правило НИКОГДА не попало в обычный `match_cell` тоже
    /// (иначе получится дубликат: фиктивный "пустой" матч без сдвигов/
    /// changes от обычного пути И реальный от CAM-пути с тем же
    /// (x,y,rule_idx) — оба бы попали в один и тот же слот `cam_positions`,
    /// и обычный (никогда не находивший цель) мог бы случайно применить
    /// притяжение через fallback в `apply_cam_buffered`, даже когда CAM
    /// ничего не нашёл — найдено экспериментально настоящим тестом).
    rule_meta: Vec<(u64, bool, bool)>,
    effective_patterns: Vec<Vec<(i8, i8, CellType)>>,
    all_offsets: Vec<(i8, i8)>,
    /// `FxHashMap`/`FxHashSet` здесь и ниже (`exact_lookup`, `GroupCache`) —
    /// не стандартный `HashMap` (SipHash): всё это горячий путь `match_cell`
    /// (задевается на КАЖДУЮ проверяемую клетку каждый тик), ключи — offset'ы
    /// или упакованные окрестности, не из недоверенного источника (см.
    /// `fast_hash`).
    offset_map: FxHashMap<(i8, i8), usize>,
    /// Упакованные паттерны (u128) для паттернов ≤ 16 ячеек. Пусто, если
    /// в группе есть паттерн длиннее — тогда используется fallback-цикл.
    /// 16, а не 8: паттерн Game of Life (центр + 8 соседей) — это уже 9
    /// клеток, не влезающих в u64 — такие паттерны раньше всегда шли по
    /// медленному общему циклу вместо однокомандного сравнения.
    packed_patterns: Vec<(u128, u128)>,
    /// Точный lookup "упакованное значение окрестности → индексы правил" —
    /// для ПОДМНОЖЕСТВА правил группы, которые полностью специфицируют все
    /// `all_offsets` (маска покрывает их все, ни одного wildcard-байта).
    /// Такие правила взаимоисключающи по построению — под конкретную
    /// окрестность подходит максимум одна точная упакованная комбинация — и
    /// вместо линейного перебора ВСЕХ правил группы (Game of Life: до 172 на
    /// голову — все 256 масок соседей минус не меняющие состояние) можно
    /// найти совпадение(я) одним хеш-lookup. Индексы внутри — ОРИГИНАЛЬНЫЕ
    /// `rule_idx` (те же, что в `rule_meta`/`packed_patterns`), не позиция
    /// внутри подмножества. `None`, если ни одно правило группы не подходит
    /// (все — wildcard, либо паттерн > 16 клеток).
    ///
    /// Раньше одного-единственного wildcard-правила в группе хватало, чтобы
    /// отключить lookup для ВСЕЙ группы целиком — например, у "челнока", где
    /// одно правило головы полностью читает "я + сосед-стена", а другое
    /// (просто сдвиг) читает только себя, быстрый путь терялся ЦЕЛИКОМ, хотя
    /// первое правило само по себе полностью специфицировано. Теперь то, что
    /// можно проверить одним lookup, проверяется им; для оставшихся
    /// (`fallback_rules`) — как и раньше, `packed_patterns`/`effective_patterns`.
    exact_lookup: Option<FxHashMap<u128, Vec<usize>>>,
    /// Индексы правил группы, НЕ покрытых `exact_lookup` (wildcard-паттерн
    /// либо паттерн > 16 клеток) — единственные, кому всё ещё нужен перебор.
    /// Пусто, если группа целиком покрыта `exact_lookup`.
    fallback_rules: Vec<usize>,
}

/// Кэш `GroupData` по head-типу — аналог `RuleDataCache`, но для матчера.
/// Построение (`build_group_data`) не бесплатно (клонирует паттерны,
/// строит offset-карты) — `Engine` строит это один раз и переиспользует
/// между тиками (см. `Engine::group_cache`), как уже делает с `rule_cache`.
pub(crate) type GroupCache = FxHashMap<CellType, GroupData>;

pub(crate) fn build_group_data(rule_index: &HashMap<CellType, Vec<Rule>>) -> GroupCache {
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

            let rule_meta: Vec<(u64, bool, bool)> =
                rules.iter().map(|r| (r.min_age, r.active_only, r.cam.is_some())).collect();

            // Собираем все уникальные смещения (dx, dy) для группы правил
            let mut all_offsets: Vec<(i8, i8)> = Vec::new();
            let mut offset_set: FxHashSet<(i8, i8)> = FxHashSet::default();
            for pat in &effective_patterns {
                for &(dx, dy, _) in pat {
                    if offset_set.insert((dx, dy)) {
                        all_offsets.push((dx, dy));
                    }
                }
            }

            // Карта: смещение → индекс в all_offsets / cache
            let offset_map: FxHashMap<(i8, i8), usize> = all_offsets
                .iter()
                .enumerate()
                .map(|(i, &o)| (o, i))
                .collect();

            // ─── Предвычисляем упакованные паттерны (u128) ───
            // Каждый байт в u128 — это значение CellType для одного смещения.
            // Позиция байта = индекс смещения в all_offsets.
            // Маска указывает, какие байты нужно проверять.
            // Для паттернов > 16 ячеек — fallback на цикл.
            let packed_patterns: Vec<(u128, u128)> = if all_offsets.len() <= 16 {
                effective_patterns
                    .iter()
                    .map(|pat| {
                        let mut packed = 0u128;
                        let mut mask = 0u128;
                        for &(dx, dy, expected) in pat {
                            if let Some(&idx) = offset_map.get(&(dx, dy)) {
                                let shift = (idx as u32) * 8;
                                packed |= (expected.0 as u128) << shift;
                                mask |= 0xFFu128 << shift;
                            }
                        }
                        (packed, mask)
                    })
                    .collect()
            } else {
                Vec::new()
            };

            // Точный lookup — по подмножеству правил, которые полностью
            // специфицируют все `all_offsets` (маска = "полная" для их
            // числа); остальные (wildcard, либо паттерн > 16 клеток —
            // `packed_patterns` тогда вообще пуст) идут в `fallback_rules`.
            let (exact_lookup, fallback_rules) = if !packed_patterns.is_empty() {
                let full_mask: u128 = if all_offsets.len() >= 16 {
                    !0u128
                } else {
                    (1u128 << (8 * all_offsets.len() as u32)) - 1
                };
                let mut lookup: FxHashMap<u128, Vec<usize>> = FxHashMap::default();
                let mut fallback = Vec::new();
                for (idx, &(packed, mask)) in packed_patterns.iter().enumerate() {
                    if mask == full_mask {
                        lookup.entry(packed).or_default().push(idx);
                    } else {
                        fallback.push(idx);
                    }
                }
                (if lookup.is_empty() { None } else { Some(lookup) }, fallback)
            } else {
                (None, (0..rules.len()).collect())
            };

            (
                cell_type,
                GroupData {
                    rule_meta,
                    effective_patterns,
                    all_offsets,
                    offset_map,
                    packed_patterns,
                    exact_lookup,
                    fallback_rules,
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
///    упакованные (u128) паттерны считаются один раз на head-тип (`GroupData`),
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
/// 4. **Упаковка паттерна в u128** — для паттернов ≤ 16 ячеек (включая полный
///    паттерн Game of Life из 9 клеток) сравнение выполняется одной
///    инструкцией `(cache_u128 & mask) == pattern` вместо цикла.
/// Ниже какого числа кандидатов не стоит платить за rayon (work-stealing,
/// синхронизация пула потоков) — при малом числе клеток (типичный тик у
/// разреженных сценариев вроде движения одной головки машины Тьюринга или
/// маркера сортировки: 1-20 кандидатов) эти накладные расходы на порядки
/// превышают саму работу (замерено: ~45 мкс параллельных накладных расходов
/// против <1 мкс полезной работы на 24-ядерной машине). Выше порога
/// параллелизация уже окупается (подтверждено на плотных сценариях вроде
/// Game of Life с тысячами активных клеток за тик).
const PARALLEL_THRESHOLD: usize = 1024;

/// Свободная функция: пересобирает `GroupCache` на каждый вызов — как и
/// `run_tick`, эта функция не хранит состояния между тиками. Для одной-двух
/// голов это недорого; в горячем цикле по многу тиков предпочтительнее
/// `Engine::detect_matches`, которая переиспользует `Engine::group_cache`
/// (аналогично `Engine::rule_cache` — см. его комментарий).
pub fn detect_matches<S: GridStorage + Sync>(
    grid: &Grid<S>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    active_coords: &Vec<(usize, usize)>,
) -> Vec<RuleMatch> {
    let group_data = build_group_data(rule_index);
    detect_matches_with_group_data(grid, &group_data, active_coords)
}

/// Общая логика обнаружения совпадений, параметризованная источником
/// `GroupCache` — используется свободной функцией `detect_matches` (строит
/// кэш каждый раз) и `Engine::detect_matches`/`Engine::run_tick` и т.п.
/// (переиспользуют `self.group_cache`).
pub(crate) fn detect_matches_with_group_data<S: GridStorage + Sync>(
    grid: &Grid<S>,
    group_data: &GroupCache,
    active_coords: &Vec<(usize, usize)>,
) -> Vec<RuleMatch> {
    let bounds = grid.storage.bounds();
    let default_cell = Cell::default();

    // Отфильтровываем клетки, чей тип вообще не имеет привязанных правил, —
    // ПРЯМО ВНУТРИ прохода ниже, а не отдельным `filter().collect()` до него
    // (как было раньше). Раньше это был отдельный проход (изначально —
    // `cells_by_type`, группировка по head-типу; клетки без своей группы
    // правил никогда не попадали в основной цикл), затем — отдельный
    // `par_iter().filter().collect()` перед `fold`. Замер (сессия
    // 2026-08-09, список "вопросы" п.1): слияние в один проход даёт ~37-43%
    // на 300×300 (и на сценарии с большой долей нерелевантных клеток типа
    // шахматки 1A, и на сценарии с полностью релевантными) — сама проверка
    // типа не становится дешевле, но пропадает ВТОРОЙ rayon dispatch/reduce
    // и промежуточная `Vec<(usize,usize)>` под отфильтрованный список,
    // которых при слиянии просто нет.
    //
    // Порог параллелизации теперь только один — по `active_coords.len()`
    // (раньше проверялся дважды: до фильтра и после) — раз проверка
    // релевантности уже не отдельный проход, применять порог к
    // "уже отфильтрованному" списку было бы нечем: он не существует отдельно.
    let is_relevant = |&(cx, cy): &(usize, usize)| -> bool {
        grid.get_cell(cx, cy).map_or(false, |c| group_data.contains_key(&c.value.0))
    };

    if active_coords.len() >= PARALLEL_THRESHOLD {
        active_coords
            .par_iter()
            .fold(
                // Аккумулятор на rayon-поток: (переиспользуемый буфер "хвоста"
                // кэша соседей >8 ячеек, накопленные матчи).
                || (Vec::<CellType>::new(), Vec::<RuleMatch>::new()),
                |(mut cache_overflow, mut local_matches), coord @ &(cx, cy)| {
                    if is_relevant(coord) {
                        match_cell(grid, group_data, bounds, default_cell, cx, cy, &mut cache_overflow, &mut local_matches);
                    }
                    (cache_overflow, local_matches)
                },
            )
            .map(|(_, local_matches)| local_matches)
            .reduce(Vec::new, |mut a, b| {
                a.extend(b);
                a
            })
    } else {
        // Последовательный путь: то же самое тело на клетку (`match_cell`),
        // без rayon — см. doc-комментарий `PARALLEL_THRESHOLD`. Порядок
        // найденных matches значения не имеет: `arbitrate()` сортирует их по
        // полностью детерминированному тай-брейку независимо от порядка на
        // входе (см. её doc-комментарий), так что смена параллельного пути
        // на последовательный не может повлиять на результат.
        let mut cache_overflow = Vec::new();
        let mut local_matches = Vec::new();
        for coord @ &(cx, cy) in active_coords.iter() {
            if is_relevant(coord) {
                match_cell(grid, group_data, bounds, default_cell, cx, cy, &mut cache_overflow, &mut local_matches);
            }
        }
        local_matches
    }
}

/// Проверить все правила группы head-типа клетки (cx, cy) и добавить найденные
/// совпадения в `local_matches`. Общее тело для параллельного (rayon fold) и
/// последовательного путей `detect_matches` — см. `PARALLEL_THRESHOLD`.
#[allow(clippy::too_many_arguments)]
fn match_cell<S: GridStorage>(
    grid: &Grid<S>,
    group_data: &GroupCache,
    bounds: Option<(usize, usize)>,
    default_cell: Cell,
    cx: usize,
    cy: usize,
    cache_overflow: &mut Vec<CellType>,
    local_matches: &mut Vec<RuleMatch>,
) {
    let center_cell = match grid.get_cell(cx, cy) {
        Some(c) => c,
        None => return,
    };
    let gd = match group_data.get(&center_cell.value.0) {
        Some(gd) => gd,
        None => return,
    };
    let center_age = grid.get_age(cx, cy);

    // ─── Загружаем кэш соседей ───
    // Быстрый путь (≤16 смещений, подавляющее большинство правил,
    // включая полный паттерн Game of Life из 9 клеток) — на
    // стеке, без аллокации. "Хвост" (>16) — в переиспользуемый
    // на весь поток Vec (буфер живёт в аккумуляторе fold, не
    // аллоцируется заново на каждую ячейку).
    let mut cache_arr: [CellType; 16] = [CellType(0); 16];
    cache_overflow.clear();
    let mut cache_valid = true;

    for (i, &(dx, dy)) in gd.all_offsets.iter().enumerate() {
        // checked_add_signed, а не wrapping_add_signed: координаты — usize,
        // отрицательных не бывает, так что смещение, уходящее "левее" (0,0)
        // или "выше" (0,0), обязано считаться недостижимым — вне решётки.
        // wrapping-версия молча заворачивала бы такое смещение в
        // usize::MAX (переполнение снизу), а на решётках БЕЗ явных границ
        // (`ChunkStorage::bounds() == None`, единственная проверка `bounds`
        // ниже просто пропускалась) это означало бы, что клетка у (0,0)
        // читает как "соседа слева" совершенно не связанную клетку где-то
        // у координаты usize::MAX — то есть ложное совпадение по фантомному
        // соседу вместо честного "здесь ничего нет". Найдено экспериментально
        // на ChunkStorage с паттерном, читающим (-1,0) у клетки в (0,0).
        let (nx, ny) = match (cx.checked_add_signed(dx as isize), cy.checked_add_signed(dy as isize)) {
            (Some(nx), Some(ny)) => (nx, ny),
            _ => {
                cache_valid = false;
                break;
            }
        };

        if let Some((bw, bh)) = bounds {
            if nx >= bw || ny >= bh {
                cache_valid = false;
                break;
            }
        }

        // Отсутствующие (дефолтные) ячейки считаются CellType(0)
        let ct = grid.get_cell(nx, ny).map_or(CellType(0), |c| c.value.0);
        if i < 16 {
            cache_arr[i] = ct;
        } else {
            cache_overflow.push(ct);
        }
    }

    if !cache_valid {
        return;
    }

    // Упаковываем кэш в u128 для быстрого сравнения
    let cache_u128 = if gd.all_offsets.len() <= 16 {
        let mut packed = 0u128;
        for (i, ct) in cache_arr.iter().enumerate().take(gd.all_offsets.len()) {
            packed |= (ct.0 as u128) << ((i as u32) * 8);
        }
        Some(packed)
    } else {
        None
    };

    // Доступ к кэшу по индексу — прозрачно поверх стека и "хвоста".
    let cache_at = |idx: usize| -> CellType {
        if idx < 16 {
            cache_arr[idx]
        } else {
            cache_overflow[idx - 16]
        }
    };

    // ─── Проверяем правила группы по кэшу ───
    //
    // Быстрый путь: для правил из `exact_lookup` (полностью специфицируют
    // все `all_offsets` — см. её doc-комментарий) достаточно ОДНОГО
    // хеш-lookup по упакованному значению окрестности вместо перебора.
    // Остальные правила группы (`fallback_rules` — wildcard-паттерн, либо
    // паттерн > 16 клеток) по-прежнему проверяются ниже, но ТОЛЬКО они, а
    // не вся группа целиком — раньше один-единственный wildcard в группе
    // включал полный перебор для всех, даже для соседей, которые сами по
    // себе были полностью специфицированы (см. doc-комментарий `exact_lookup`).
    if let (Some(lookup), Some(cache_packed)) = (&gd.exact_lookup, cache_u128) {
        if let Some(candidates) = lookup.get(&cache_packed) {
            for &rule_idx in candidates {
                let (min_age, active_only, is_cam) = gd.rule_meta[rule_idx];
                if is_cam {
                    continue; // детектируется отдельно, см. doc-комментарий `rule_meta`
                }
                if center_age < min_age {
                    continue;
                }
                if active_only && center_cell.value == default_cell.value && center_age == 0 {
                    continue;
                }
                local_matches.push(RuleMatch {
                    x: cx as u32,
                    y: cy as u32,
                    head: center_cell.value.0,
                    rule_idx,
                });
            }
        }
    }

    for &rule_idx in &gd.fallback_rules {
        let (min_age, active_only, is_cam) = gd.rule_meta[rule_idx];
        if is_cam {
            continue; // детектируется отдельно, см. doc-комментарий `rule_meta`
        }
        if center_age < min_age {
            continue;
        }

        if active_only
            && center_cell.value == default_cell.value
            && center_age == 0
        {
            continue;
        }

        let matched = if let Some(cache_packed) = cache_u128 {
            // SIMD-сравнение: одна инструкция вместо цикла по паттерну
            let (packed_pattern, packed_mask) = gd.packed_patterns[rule_idx];
            (cache_packed & packed_mask) == packed_pattern
        } else {
            // Fallback: цикл по паттерну (для больших паттернов > 16 ячеек)
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
            local_matches.push(RuleMatch {
                x: cx as u32,
                y: cy as u32,
                // `center_cell.value.0`, а не `rule.id.first()`:
                // это ровно тот же тип, под которым `gd` найден
                // в `group_data` выше, и он корректен даже когда
                // у правила пустой `id`, а голова определена
                // через `pattern` (см. `config::load_config`'s
                // `center_type` fallback) — тогда `rule.id`
                // просто пуст.
                head: center_cell.value.0,
                rule_idx,
            });
        }
    }
}

/// Изолированный проход детекции CAM-совпадений (`Rule::cam`, см. её
/// doc-комментарий в `types.rs`) — намеренно ОТДЕЛЬНЫЙ от
/// `detect_matches_with_group_data`/`match_cell` (packed-паттерны и
/// exact-lookup там заточены под ФИКСИРОВАННЫЕ офсеты; CAM — принципиально
/// другой алгоритм, скан диска в поисках типа). Цена для конфигов БЕЗ
/// `cam`-правил — один быстрый ранний выход (`has_cam`), ноль лишней
/// работы в горячем пути.
///
/// Возвращает обычные `RuleMatch` (участвуют в арбитраже наравне со всеми —
/// тот же тай-брейк priority→age→id→x→y→rule_idx) плюс побочную карту
/// найденных позиций — `get_match_affected_cells`/`apply_rule_buffered`
/// используют её вместо статических `RuleData::write_cells` (которые для
/// `cam`-правил хранят консервативную границу — весь диск, см.
/// `conflict_analyzer::compute_rule_data` — а не точную найденную клетку).
pub(crate) fn detect_cam_matches<S: GridStorage>(
    grid: &Grid<S>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    active_coords: &[(usize, usize)],
) -> (Vec<RuleMatch>, CamPositions) {
    let has_cam = rule_index.values().flatten().any(|r| r.cam.is_some());
    if !has_cam {
        return (Vec::new(), FxHashMap::default());
    }

    let mut matches = Vec::new();
    let mut positions = FxHashMap::default();

    for &(cx, cy) in active_coords {
        let Some(center_cell) = grid.get_cell(cx, cy) else { continue };
        let Some(rules) = rule_index.get(&center_cell.value.0) else { continue };
        let center_age = grid.get_age(cx, cy);

        for (rule_idx, rule) in rules.iter().enumerate() {
            let Some(cam) = rule.cam else { continue };
            if center_age < rule.min_age {
                continue;
            }
            if rule.active_only && center_cell.value == Cell::default().value && center_age == 0 {
                continue;
            }

            if let Some((fx, fy)) = search_nearest(grid, cx, cy, cam.radius, cam.target_type) {
                matches.push(RuleMatch { x: cx as u32, y: cy as u32, head: center_cell.value.0, rule_idx });
                positions.insert((cx as u32, cy as u32, rule_idx), (fx as u32, fy as u32));
            }
        }
    }

    (matches, positions)
}

/// Ближайшая клетка типа `target` в Chebyshev-радиусе `radius` вокруг
/// (cx, cy) — по состоянию решётки ДО тика (то же чтение, что и обычный
/// `pattern_matches`, никогда не видит изменений этого же тика). При
/// равном расстоянии — детерминированный тай-брейк по (y, x), не зависит
/// от порядка сканирования.
fn search_nearest<S: GridStorage>(
    grid: &Grid<S>,
    cx: usize,
    cy: usize,
    radius: u8,
    target: CellType,
) -> Option<(usize, usize)> {
    let r = radius as i64;
    let mut best: Option<(i64, i64, i64)> = None; // (dist, y, x) — минимум лексикографически
    for dy in -r..=r {
        let ny = cy as i64 + dy;
        if ny < 0 {
            continue;
        }
        for dx in -r..=r {
            let nx = cx as i64 + dx;
            if nx < 0 || (nx as usize, ny as usize) == (cx, cy) {
                continue;
            }
            let Some(cell) = grid.get_cell(nx as usize, ny as usize) else { continue };
            if cell.value.0 != target {
                continue;
            }
            let dist = dx.abs().max(dy.abs());
            let candidate = (dist, ny, nx);
            if best.is_none_or(|b| candidate < b) {
                best = Some(candidate);
            }
        }
    }
    best.map(|(_, y, x)| (x as usize, y as usize))
}

#[cfg(test)]
#[path = "matcher_tests.rs"]
mod tests;