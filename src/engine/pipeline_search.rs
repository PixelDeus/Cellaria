use super::*;

/// Максимальный |смещение| паттерна среди ВСЕХ правил (любой головы).
///
/// Если клетка D изменилась, то любая клетка-центр C, чей паттерн ссылается
/// на D на смещении (dx,dy) (то есть C+(dx,dy) = D, C = D-(dx,dy)), могла
/// изменить статус совпадения — и C лежит в пределах этого радиуса от D
/// (диапазон -radius..=radius симметричен, так что направление роли не
/// играет). Используется для расширения "грязного" множества (см.
/// `Grid::dirty_coords`) до множества кандидатов на пересканирование.
pub(crate) fn max_pattern_radius(rule_index: &HashMap<CellType, Vec<Rule>>) -> i32 {
    pattern_radius(rule_index.values().flatten())
}

/// То же самое, но только по правилам с head=`DEFAULT_CELL_VALUE` (0).
///
/// Нужен отдельно от `max_pattern_radius` для случая, когда кандидатами уже
/// выступают ВСЕ активные клетки (см. использование в `resolve_search_coords_*`
/// при вырожденно большом dirty-множестве): тогда единственная причина хоть
/// что-то расширять — найти head=0 "рождения" рядом с активными клетками
/// (см. `min_age_gated_types`-подобное рассуждение в doc `max_pattern_radius`).
/// Пропагировать охват от активных клеток к их же соседям другого типа
/// (общий случай `max_pattern_radius`) здесь не нужно — сами активные клетки
/// уже все в списке, расширять есть смысл только в сторону ДЕФОЛТНЫХ клеток.
pub(crate) fn zero_head_radius(rule_index: &HashMap<CellType, Vec<Rule>>) -> i32 {
    match rule_index.get(&CellType(DEFAULT_CELL_VALUE)) {
        Some(rules) => pattern_radius(rules.iter()),
        None => 0,
    }
}

pub(crate) fn pattern_radius<'a>(rules: impl Iterator<Item = &'a Rule>) -> i32 {
    let mut max_r = 0i32;
    for rule in rules {
        if rule.pattern.is_empty() {
            // Паттерн из id: смещения 0..id.len()-1 по x.
            max_r = max_r.max(rule.id.len().saturating_sub(1) as i32);
        } else {
            for &(dx, dy, _) in &rule.pattern {
                max_r = max_r.max(dx.unsigned_abs() as i32).max(dy.unsigned_abs() as i32);
            }
        }
        // `cam.radius` — тот же смысл, что и офсет паттерна, симметрично:
        // если клетка-ЦЕЛЬ (тип X) появилась/исчезла в радиусе R от
        // магнита, статус его CAM-совпадения мог измениться, даже если сам
        // магнит не менялся ни разу — без этого dirty-tracking не заметил
        // бы такое изменение (см. doc-комментарий `max_pattern_radius`, та
        // же логика, что уже применена к `min_age`).
        if let Some(cam) = rule.cam {
            max_r = max_r.max(cam.radius as i32);
        }
    }
    max_r
}

/// Типы-головы, у которых есть хотя бы одно правило с `min_age > 0`.
///
/// `min_age` — единственный способ, которым статус совпадения клетки может
/// измениться БЕЗ изменения значения у неё или у соседей — просто течением
/// времени (возраст переходит порог). Dirty-множество это не ловит: клетка
/// может стоять нетронутой сто тиков, а затем "созреть" для правила с
/// `min_age=100`. Поэтому клетки таких типов включаются в кандидатов на
/// КАЖДЫЙ тик безусловно, независимо от dirty-состояния — единственная
/// просадка от инкрементального скана, и она касается только типов, у
/// которых реально есть такие правила (в текущих `configs/` — 2 файла из 37).
pub(crate) fn min_age_gated_types(rule_index: &HashMap<CellType, Vec<Rule>>) -> FxHashSet<CellType> {
    rule_index
        .iter()
        .filter(|(_, rules)| rules.iter().any(|r| r.min_age > 0))
        .map(|(&ct, _)| ct)
        .collect()
}

/// `min_age_gated_types`/`max_pattern_radius`/`zero_head_radius` — все
/// чистые функции ТОЛЬКО от `rule_index`, но раньше пересчитывались заново
/// на каждый вызов `resolve_search_coords_*` — то есть на каждый тик,
/// безусловно, даже когда набор правил месяцами не менялся (найдено при
/// проверке производительности: `min_age_gated_types` — O(всех правил)
/// линейный скан HashMap на пустом месте каждый тик). `Engine` считает это
/// один раз при создании/перестройке (см. `Engine::search_radius_cache`) и
/// переиспользует, как уже делает с `rule_cache`/`group_cache`/
/// `conflict_partners`; свободная функция `run_tick` по-прежнему считает
/// заново на каждый вызов — тот же компромисс, что и везде в этом файле.
pub(crate) struct SearchRadiusCache {
    min_age_gated_types: FxHashSet<CellType>,
    max_pattern_radius: i32,
    zero_head_radius: i32,
}

pub(crate) fn compute_search_radius_cache(rule_index: &HashMap<CellType, Vec<Rule>>) -> SearchRadiusCache {
    SearchRadiusCache {
        min_age_gated_types: min_age_gated_types(rule_index),
        max_pattern_radius: max_pattern_radius(rule_index),
        zero_head_radius: zero_head_radius(rule_index),
    }
}

/// "Использует ли ХОТЬ ОДНО правило набора данное поле" — три флага,
/// посчитанные один раз из `rule_index` (пересчитываются вместе с
/// остальными кэшами при `Engine::rebuild_rule_cache`), а не заново на
/// каждый тик.
///
/// Без этого кэша `run_tick_with_cache` был бы вынужден сканировать ВСЕ
/// `matches` каждый тик отдельным проходом на КАЖДОЕ из полей
/// (`starvation_after`/`feedback`/`memory`) — с HashMap-лукапом
/// (`rule_index.get(&m.head)`) на каждый элемент — ДАЖЕ КОГДА ни одно
/// правило набора это поле не использует. Это противоречило бы заявленному
/// "нулевые накладные расходы для кода, который об этом не просил" (см.
/// doc-комментарии `Rule::starvation_after`/`Rule::feedback`/`Rule::memory`):
/// заявление было честным по НАМЕРЕНИЮ, но не было фактически обеспечено —
/// O(число матчей) работы всё равно платился безусловно. С этим кэшем три
/// скана в `run_tick_with_cache` пропускаются целиком (`Vec::new()`), если
/// соответствующий флаг `false`.
/// Разбивка одного тика по фазам — см. [`Engine::run_tick_profiled`].
/// Три поля, не пять (§5 спецификации описывает пять фаз: Input/Detect/
/// Arbitrate/Apply/Flush): Input не проходит через `run_tick_with_cache`
/// вообще (граничные буферы заполняются `push_input`/`Engine`'s own
/// input-related кодом отдельно, до вызова тика), а Flush (сброс возраста,
/// сбор output) в текущей структуре функции физически внутри той же ветки,
/// что и Apply — вычленить без риска для уже проверенного тик-пайплайна не
/// стал (см. комментарий на месте единственного `mark_phase!(apply)`).
/// `detect` считается ДАЖЕ на тике, где совпадений вообще не нашлось (ранний
/// выход после чистки осиротевших записей) — это тоже реальное время этой
/// фазы, не повод его не засчитывать.
#[derive(Debug, Clone, Copy, Default)]
pub struct TickPhaseTimings {
    pub detect: std::time::Duration,
    pub arbitrate: std::time::Duration,
    pub apply: std::time::Duration,
}

/// Счётчики событий одного тика — заполняется `run_tick_with_cache`, когда
/// вызывающая сторона (см. [`Engine::enable_tick_logging`]) их запросила.
/// Тот же принцип "нулевые накладные расходы, если не просили", что и у
/// [`TickPhaseTimings`]: без записи (`None`) ни один из подсчётов ниже не
/// выполняется — величины (`starving_keys.len()`/`feedback_keys.len()`/
/// `matches.len()`) и так уже посчитаны на своём обычном пути, разница —
/// только в том, копируются ли они куда-то ещё.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TickEventCounts {
    pub(crate) accepted: usize,
    pub(crate) rejected: usize,
    pub(crate) starvation_events: usize,
    pub(crate) feedback_events: usize,
}

/// Одна запись структурированного JSON-лога тиков (п.5, сессия 2026-08-09).
///
/// `accepted`/`rejected` — принятые и отклонённые арбитражем совпадения
/// этого тика (`rejected` = обнаруженные структурным матчером кандидаты,
/// которые арбитраж НЕ принял — необязательно "плохие", могут выиграть на
/// следующем тике). `starvation_events`/`feedback_events` — количество
/// кандидатов этого тика, чьё правило использует `Rule::starvation_after`/
/// `Rule::feedback` соответственно (то есть "под наблюдением" этого
/// механизма на этом тике, а не только "сработавшие" — сам факт наблюдения
/// уже полезен для внешнего мониторинга долго голодающих или часто
/// переключающихся правил).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TickLogEntry {
    pub tick: u64,
    pub accepted: usize,
    pub rejected: usize,
    pub starvation_events: usize,
    pub feedback_events: usize,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ExtensionFlags {
    pub(crate) starvation_after: bool,
    pub(crate) feedback: bool,
    pub(crate) memory: bool,
    /// Хоть одно правило набора использует `Rule::max_activations`. НЕ
    /// участвует в `extension_rule_indices` ниже — тот список только для
    /// per-tick чистки ПОЗИЦИОННЫХ (`x, y, rule_idx`) осиротевших записей;
    /// `activation_counters` ключуется `(head, rule_idx)` без позиции и не
    /// устаревает от того, что клетка перестала совпадать (устаревание
    /// покрыто отдельно — `Engine::invalidate_stale_extension_state`, при
    /// смене состава правил, а не каждый тик).
    pub(crate) max_activations: bool,
    /// Различные `rule_idx` (по ВСЕМ головам сразу, не по одной) среди
    /// правил, у которых задан `feedback`, `memory` ИЛИ `starvation_after` —
    /// используется ТОЛЬКО дешёвой чисткой осиротевших записей в
    /// `run_tick_with_cache` (см. блок "осиротевшие записи" там). Ключи
    /// `feedback_counters`/`memory_buffers`/`starvation_counters` —
    /// `(x, y, rule_idx)`, БЕЗ головы, так что для проверки "не устарела ли
    /// запись в этой позиции" достаточно перебрать этот маленький список (на
    /// практике — единицы, число РАЗЛИЧНЫХ ОПРЕДЕЛЕНИЙ правил с этими
    /// полями, а не пропорционально ни размеру решётки, ни общему числу
    /// правил набора), а не сканировать всю карту (которая растёт
    /// пропорционально числу когда-либо совпавших ПОЗИЦИЙ).
    pub(crate) extension_rule_indices: Vec<usize>,
}

pub(crate) fn compute_extension_flags(rule_index: &HashMap<CellType, Vec<Rule>>) -> ExtensionFlags {
    let mut flags = ExtensionFlags::default();
    // Без раннего выхода (в отличие от прежней версии) — список
    // `extension_rule_indices` обязан собрать ВСЕ подходящие индексы, а не
    // только до первого тройного совпадения флагов; сама функция не на
    // горячем пути (только `Engine::new`/`rebuild_rule_cache`, не каждый
    // тик), так что цена полного прохода незначительна.
    let mut seen_indices: FxHashSet<usize> = FxHashSet::default();
    for rules in rule_index.values() {
        for (idx, rule) in rules.iter().enumerate() {
            flags.starvation_after |= rule.starvation_after.is_some();
            flags.feedback |= rule.feedback.is_some();
            flags.memory |= rule.memory.is_some();
            flags.max_activations |= rule.max_activations.is_some();
            if rule.feedback.is_some() || rule.memory.is_some() || rule.starvation_after.is_some() {
                seen_indices.insert(idx);
            }
        }
    }
    flags.extension_rule_indices = seen_indices.into_iter().collect();
    flags
}

/// Построить кандидатов для detect_matches из уже полученного базового
/// множества (dirty-множество, ЛИБО весь active_coords в вырожденном случае
/// — см. вызывающий код) и заданного радиуса расширения.
pub(crate) fn build_candidates<S: GridStorage>(
    base: Vec<(usize, usize)>,
    radius: i32,
    grid: &Grid<S>,
    cache: &SearchRadiusCache,
) -> Vec<(usize, usize)> {
    // При radius=0 расширять нечего — берём `base` как есть (move), а не
    // через `expand_neighborhood(&base, 0)`: та принимает срез и поэтому
    // ОБЯЗАНА клонировать (`coords.to_vec()`) даже когда возвращает то же
    // самое множество без изменений. Здесь `base` уже во владении —
    // повторное клонирование было бы чистой тратой (при 250 000 элементах —
    // лишний Vec::clone поверх уже сделанного ранее).
    let mut candidates = if radius == 0 {
        base
    } else {
        expand_neighborhood(grid, &base, radius)
    };

    if !cache.min_age_gated_types.is_empty() {
        let mut seen: FxHashSet<(usize, usize)> = candidates.iter().copied().collect();
        for &(x, y) in grid.active_coords() {
            if seen.contains(&(x, y)) {
                continue;
            }
            if let Some(cell) = grid.get_cell(x, y) {
                if cache.min_age_gated_types.contains(&cell.value.0) {
                    seen.insert((x, y));
                    candidates.push((x, y));
                }
            }
        }
    }

    candidates
}

/// Выбрать базовое множество кандидатов и радиус его расширения.
///
/// Если "грязных" клеток сравнимо с числом активных — дешевле и не менее
/// корректно взять весь `active_coords` напрямую (cache-friendly Vec), чем
/// прогонять их через HashSet-конвейер dirty-множества (insert на каждый
/// `set_cell`, drain здесь). На тиках, где изменение разом затрагивает почти
/// всю решётку (единичный массовый эффект, или сценарий, где реально каждая
/// активная клетка меняется каждый тик), dirty вырождается в "почти всё
/// активное" — и его HashSet-механика становится чистым оверхедом.
///
/// В этом вырожденном случае радиус расширения тоже другой и ýже:
/// `active_coords` уже содержит вообще все активные клетки, поэтому
/// пропагировать охват на соседей ради поиска "затронутых нейтральных
/// клеток" (общая цель `max_pattern_radius` для маленького dirty-множества)
/// не нужно — единственное, что ещё может понадобиться найти — это head=0
/// "рождения" рядом с активными клетками (`zero_head_radius`, обычно 0).
/// Использование здесь широкого `max_pattern_radius` было бы чистой тратой:
/// на решётке 500×500 с уже-везде-активными клетками и паттерном шире одной
/// клетки это означало бы прогон `expand_neighborhood` над всеми 250 000
/// координатами без какой-либо дополнительной пользы.
pub(crate) fn dirty_base_and_radius<S: GridStorage>(
    dirty: FxHashSet<(usize, usize)>,
    grid: &Grid<S>,
    cache: &SearchRadiusCache,
) -> (Vec<(usize, usize)>, i32) {
    if dirty.len() * 2 >= grid.active_coords().len() {
        (grid.active_coords().clone(), cache.zero_head_radius)
    } else {
        (dirty.into_iter().collect(), cache.max_pattern_radius)
    }
}

/// "Подглядеть" кандидатов для detect_matches, НЕ потребляя dirty-множество.
///
/// Безопасно вызывать сколько угодно раз подряд без побочных эффектов —
/// например, из `detect_termination` в цикле проверки стабилизации.
/// Потребление (`take_dirty`) здесь недопустимо: если "подглядывающий" вызов
/// очистит dirty-множество, а состояние решётки при этом не изменится
/// (никакой тик не применялся), следующий РЕАЛЬНЫЙ `run_tick` решит, что эти
/// клетки уже проверены, и пропустит реальные совпадения.
pub(crate) fn resolve_search_coords_peek<S: GridStorage>(
    grid: &Grid<S>,
    cache: &SearchRadiusCache,
) -> Vec<(usize, usize)> {
    let dirty = grid.peek_dirty();
    let (base, radius) = dirty_base_and_radius(dirty, grid, cache);
    build_candidates(base, radius, grid, cache)
}

/// Получить кандидатов для detect_matches и ОЧИСТИТЬ dirty-множество.
///
/// Вызывать ровно один раз на каждый реально применяемый тик (`run_tick`,
/// `compose_with`) — после этого вызова следующий тик увидит только то, что
/// изменится начиная с текущего момента (apply_matches этого же тика
/// заполнит dirty-множество заново через `set_cell`).
pub(crate) fn resolve_search_coords_advance<S: GridStorage>(
    grid: &mut Grid<S>,
    cache: &SearchRadiusCache,
) -> Vec<(usize, usize)> {
    let dirty = grid.take_dirty();
    let (base, radius) = dirty_base_and_radius(dirty, grid, cache);
    build_candidates(base, radius, grid, cache)
}

/// Расширить список координат на окрестность заданного радиуса.
/// Используется для обнаружения паттернов вокруг активных ячеек (см.
/// `detect_radius` — радиус 0 означает «расширение не нужно вообще»).
pub(crate) fn expand_neighborhood<S: GridStorage>(
    grid: &Grid<S>,
    coords: &[(usize, usize)],
    radius: i32,
) -> Vec<(usize, usize)> {
    if coords.is_empty() || radius == 0 {
        return coords.to_vec();
    }

    // Если кандидатов после расширения будет сравнимо со всей решёткой —
    // дешевле плотный маркер (прямая запись по индексу без хеширования),
    // чем HashSet<(usize,usize)> с хешированием каждой пары координат.
    let side = (2 * radius + 1) as usize;
    let per_cell = side * side;
    if let Some((w, h)) = grid.storage().bounds() {
        if w > 0 && h > 0 && coords.len().saturating_mul(per_cell) >= w * h {
            let mut seen = vec![false; w * h];
            let mut result = Vec::new();
            for &(x, y) in coords {
                let x0 = x.saturating_sub(radius as usize);
                let x1 = (x + radius as usize).min(w - 1);
                let y0 = y.saturating_sub(radius as usize);
                let y1 = (y + radius as usize).min(h - 1);
                for ny in y0..=y1 {
                    let row = ny * w;
                    for nx in x0..=x1 {
                        let idx = row + nx;
                        if !seen[idx] {
                            seen[idx] = true;
                            result.push((nx, ny));
                        }
                    }
                }
            }
            return result;
        }
    }

    // Малое число кандидатов (типичный случай для разреженных сценариев —
    // движение одной головки Тьюринга или маркера сортировки: 1-4 базовых
    // координаты) — линейный dedup по Vec дешевле, чем HashSet<(usize,usize)>:
    // не нужно ни аллокации хеш-таблицы, ни SipHash каждой пары координат,
    // просто последовательный contains() по маленькому cache-friendly Vec.
    let estimated = coords.len().saturating_mul(per_cell);
    if estimated <= 256 {
        let mut result: Vec<(usize, usize)> = Vec::with_capacity(estimated);
        for &(x, y) in coords {
            for dx in -radius..=radius {
                for dy in -radius..=radius {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx >= 0 && ny >= 0 {
                        let coord = (nx as usize, ny as usize);
                        if !result.contains(&coord) {
                            result.push(coord);
                        }
                    }
                }
            }
        }
        return result;
    }

    let mut set: FxHashSet<(usize, usize)> = FxHashSet::default();
    for &(x, y) in coords {
        for dx in -radius..=radius {
            for dy in -radius..=radius {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx >= 0 && ny >= 0 {
                    set.insert((nx as usize, ny as usize));
                }
            }
        }
    }
    set.into_iter().collect()
}

pub(crate) fn reset_age_for_regions<S: GridStorage>(grid: &mut Grid<S>, regions: &[AffectedRegion]) {
    let gen = grid.generation();
    // По точному списку `written_cells`, а не по прямоугольнику bbox —
    // прямоугольник, охватывающий исходную и целевую позицию сдвига на N>1
    // клеток, включает и клетки МЕЖДУ ними, которые сдвиг не трогает вовсе
    // (найдено экспериментально: клетка между позициями получала обнулённый
    // возраст, хотя сама не менялась). `written_cells` — ровно то, что было
    // вставлено в write-буфер при применении, без лишнего.
    //
    // `grid.set_cell(...)`, а не `grid.storage.set(...)` напрямую — раньше
    // было наоборот (в обход `set_cell`, единственной документированной
    // точки мутации), из-за чего `active_coords`/`dirty_coords` не
    // синхронизировались с этой записью; это уже вызывало реальный баг в
    // связке с `is_default()`-логикой `set_cell` (см. её doc-комментарий
    // про `was_in_active`) — тот баг был закрыт СО СТОРОНЫ `set_cell`
    // (сделан устойчивым к обходу), а не устранением самого обхода. Раз
    // `value` не меняется, единственный реальный эффект здесь —
    // корректная отметка `dirty_coords`; `active_coords`-переход
    // практически никогда не срабатывает (клетка уже отмечена активной
    // записью value на этом же тике), так что цена этого перехода на
    // `set_cell` пренебрежимо мала.
    for region in regions {
        for &(x, y) in &region.written_cells {
            if let Some(cell) = grid.get_cell(x as usize, y as usize) {
                grid.set_cell(
                    x as usize,
                    y as usize,
                    Cell {
                        value: cell.value,
                        born_at: gen,
                    },
                );
            }
        }
    }
}
