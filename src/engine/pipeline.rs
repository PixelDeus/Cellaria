use super::*;

// Хелперы разрешения поисковых координат/кэшей расширений — отдельно от
// самого алгоритма тика (`run_tick_with_cache` выше), см. `pipeline_search.rs`.
// Реэкспорт явным списком (не `*`) — глоб конфликтует с `use super::*` выше
// (тот же `TickLogEntry`/`TickPhaseTimings` уже реэкспортированы обратно
// через `engine::mod`, компилятор не может сам решить неоднозначность двух
// путей к одному и тому же элементу).
#[path = "pipeline_search.rs"]
mod pipeline_search;
pub use pipeline_search::{TickLogEntry, TickPhaseTimings};
pub(crate) use pipeline_search::{
    compute_extension_flags, compute_search_radius_cache, reset_age_for_regions, resolve_search_coords_advance,
    resolve_search_coords_peek, ExtensionFlags, SearchRadiusCache, TickEventCounts,
};

/// Теоремой `ConflictGraph` определить, какие ГОЛОВЫ структурно МОГЛИ БЫ
/// столкнуться друг с другом (включая с самими собой — self-loop), и
/// наибольший "радиус" (bbox affected-региона) среди всех правил набора —
/// см. doc-комментарии `Engine::conflict_partners`/`Engine::max_affected_radius`.
/// Считается один раз при создании/перестройке `Engine`, не на каждый тик —
/// `ConflictGraph::build` сам по себе не бесплатен (O(N²·K²) от числа
/// правил), но правила меняются на порядки реже, чем тикает движок. Само
/// решение "нужен ли арбитраж" для конкретного совпадения принимается
/// заново каждый тик в `spatial_bypass_split`, используя эти структурные
/// данные как вход, а не как готовый ответ.
pub(crate) fn compute_conflict_partners(
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &crate::conflict_analyzer::RuleDataCache,
) -> (FxHashMap<CellType, FxHashSet<CellType>>, i32) {
    let mut rules: Vec<Rule> = Vec::new();
    let mut head_of_rule: Vec<CellType> = Vec::new();
    for (&head, rs) in rule_index {
        for r in rs {
            rules.push(r.clone());
            head_of_rule.push(head);
        }
    }
    let graph = crate::conflict_analyzer::ConflictGraph::build(&rules);
    let mut partners: FxHashMap<CellType, FxHashSet<CellType>> = FxHashMap::default();
    for &(i, j) in graph.potential_conflicts() {
        let (hi, hj) = (head_of_rule[i], head_of_rule[j]);
        partners.entry(hi).or_default().insert(hj);
        partners.entry(hj).or_default().insert(hi);
    }

    let max_radius = rule_cache
        .iter()
        .filter_map(|opt| opt.as_ref())
        .flat_map(|rules| rules.iter())
        .map(|data| {
            let (min_x, max_x, min_y, max_y) = data.bbox;
            min_x
                .unsigned_abs()
                .max(max_x.unsigned_abs())
                .max(min_y.unsigned_abs())
                .max(max_y.unsigned_abs()) as i32
        })
        .max()
        .unwrap_or(0);

    (partners, max_radius)
}

/// Структурный вход для `spatial_bypass_split` — см. doc-комментарии
/// `Engine::conflict_partners`/`Engine::max_affected_radius`.
pub(crate) struct ConflictContext<'a> {
    pub(crate) partners: &'a FxHashMap<CellType, FxHashSet<CellType>>,
    pub(crate) max_radius: i32,
}

/// Выполнить один тик симуляции (свободная функция).
///
/// Пересобирает `rule_cache`/`group_cache` заново на каждый вызов — эта
/// функция не хранит состояния между тиками (в отличие от [`Engine`],
/// который кэширует их в `self.rule_cache`/`self.group_cache`). Для
/// небольшого набора правил стоимость пересборки пренебрежимо мала; для
/// конфигов с десятками-сотнями правил и частыми тиками в цикле
/// предпочтительнее держать `Engine` и звать `Engine::run_tick`.
///
/// НЕ вычисляет `conflict_partners`/`max_affected_radius` (в отличие от
/// `Engine`, где это считается один раз и кэшируется) — `ConflictGraph::build`
/// сам по себе O(N²·K²) от числа правил и размера паттернов, и на наборе в
/// сотни правил (например, полный Game of Life — 228 правил) эта проверка
/// сама по себе дороже целого тика. Пересчитывать её на КАЖДЫЙ вызов
/// свободной функции — не "пренебрежимо мало", как rule_cache/group_cache, а
/// реальная регрессия (найдено экспериментально: наивная версия этой
/// оптимизации замедлила GoL на порядки).
///
/// `Rule::starvation_after`/`Rule::feedback`/`Rule::memory`/
/// `Rule::max_activations` для этого пути всегда no-op — см. их
/// doc-комментарии: нужна память МЕЖДУ вызовами, а эта функция её не хранит
/// (свежие пустые `StarvationCounters`/`FeedbackCounters`/`MemoryBuffers`/
/// `ActivationCounters` на каждый вызов, как и `CamPositions` выше — буфер
/// памяти никогда не наполнится, гейт никогда не откроется/не закроется).
pub fn run_tick<S: GridStorage>(
    grid: &mut Grid<S>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
) -> (Vec<RuleMatch>, Vec<(u32, Cell)>) {
    let rule_cache = crate::conflict_analyzer::build_rule_data_cache(rule_index);
    let group_cache = build_group_data(rule_index);
    let search_radius_cache = compute_search_radius_cache(rule_index);
    let extension_flags = compute_extension_flags(rule_index);
    let mut state = RuleStateStore::default();
    let mut write_buffer = WriteBuffer::default();
    let mut pattern_buffer = Vec::new();
    run_tick_with_cache(
        grid,
        rule_index,
        &rule_cache,
        &group_cache,
        &search_radius_cache,
        &extension_flags,
        None,
        &mut state,
        None,
        None,
        &mut write_buffer,
        &mut pattern_buffer,
        None,
    )
}

/// Общая логика одного тика, параметризованная источниками `rule_cache`/
/// `group_cache` — общий код для свободной функции `run_tick` (строит кэши
/// каждый раз) и `Engine::run_tick` (переиспользует `self.rule_cache`/
/// `self.group_cache`).
///
/// `conflict_ctx: None` — оптимизация выключена, всегда полный арбитраж (путь
/// свободной функции `run_tick`, где считать граф конфликтов заново на
/// каждый вызов не оправдано). `Some(ctx)` — матчи голов, у которых в этом
/// тике физически нет рядом ни одного структурного конфликт-партнёра,
/// принимаются напрямую, без единого сравнения; остальные — через обычный
/// арбитраж (см. `spatial_bypass_split`). Это не эвристика — та же теорема
/// уже проверена тяжёлыми property-тестами
/// (`prop_conflict_free_rules_accept_everything`): для матча, рядом с которым
/// нет ни одного потенциального конфликт-партнёра, арбитраж и так принял бы
/// его в 100% случаев, просто дороже.
/// Разделить матчи на "точно безопасные в этом тике" (принять напрямую) и
/// "нужен арбитраж" — используя `ctx.partners` (структурно: какие головы
/// МОГЛИ БЫ столкнуться) вместе с РЕАЛЬНЫМИ позициями совпадений этого тика.
///
/// Голова, отсутствующая в `ctx.partners` как ключ, безусловно безопасна —
/// без единого сравнения позиций (эквивалент старого "глобально безопасной
/// головы"). Голова-ключ безопасна ТОЛЬКО если ни один из её партнёров не
/// совпал в пределах `2 × ctx.max_radius` по x И по y — это ВЕРХНЯЯ граница
/// на дальность, на которую affected-регион ЛЮБОЙ пары правил может
/// пересечься (см. doc-комментарий `Engine::max_affected_radius`), поэтому
/// проверка консервативна (может послать в арбитраж чуть больше, чем
/// строго нужно), но никогда не бывает наоборот.
///
/// Пространственный отсев — стандартный spatial hashing: решётка совпадений
/// делится на квадратные корзины стороной `bucket = 2 × max_radius`, и для
/// каждого совпадения проверяются только 3×3 соседние корзины — этого
/// достаточно, потому что две точки дальше `bucket` друг от друга по любой
/// оси не могут оказаться в соседних (или той же) корзине.
/// Быстрый lookup правил по head-типу — массив вместо `HashMap` (сессия
/// 2026-08-09, "фантазия" п.1). `CellType` оборачивает `u8` (256
/// возможных значений) — прямая индексация категорически дешевле
/// хэширования: замерено 94% экономии на реальной последовательности
/// head-значений тика (не синтетика — урок предыдущих раундов этой сессии).
///
/// Строится ОДИН РАЗ за вызов горячей функции (не персистентно у `Engine` —
/// `&'a Vec<Rule>` заимствует у `rule_index`, который сам живёт в `Engine`;
/// хранить такое заимствование полем того же `Engine` — самоссылающаяся
/// структура, потребовала бы unsafe/`ouroboros`, той же ценой, что уже
/// сознательно отвергали раньше в этой сессии для похожего случая), из уже
/// имеющегося параметра `rule_index` — дёшево, O(число голов), а не O(256).
pub(crate) type HeadRuleIndex<'a> = [Option<&'a Vec<Rule>>; 256];

pub(crate) fn build_head_index(rule_index: &HashMap<CellType, Vec<Rule>>) -> HeadRuleIndex<'_> {
    let mut index: HeadRuleIndex = [None; 256];
    for (&head, rules) in rule_index {
        index[head.0 as usize] = Some(rules);
    }
    index
}

/// Замена `rule_index.get(&head).and_then(|rules| rules.get(rule_idx))` —
/// та же семантика Option-цепочки, через [`HeadRuleIndex`] вместо `HashMap`.
pub(crate) fn lookup_rule<'a>(head_index: &HeadRuleIndex<'a>, head: CellType, rule_idx: usize) -> Option<&'a Rule> {
    head_index[head.0 as usize].and_then(|rules| rules.get(rule_idx))
}

pub(crate) fn spatial_bypass_split(matches: Vec<RuleMatch>, ctx: &ConflictContext) -> (Vec<RuleMatch>, Vec<RuleMatch>) {
    let (mut safe, candidates): (Vec<RuleMatch>, Vec<RuleMatch>) =
        matches.into_iter().partition(|m| !ctx.partners.contains_key(&m.head));

    if candidates.is_empty() {
        return (safe, candidates);
    }

    let bucket = (2 * ctx.max_radius).max(1);
    let mut buckets: FxHashMap<(i32, i32), Vec<usize>> = FxHashMap::default();
    for (idx, m) in candidates.iter().enumerate() {
        let key = ((m.x as i32).div_euclid(bucket), (m.y as i32).div_euclid(bucket));
        buckets.entry(key).or_default().push(idx);
    }

    let mut needs_arbitration = vec![false; candidates.len()];
    for idx in 0..candidates.len() {
        if needs_arbitration[idx] {
            continue;
        }
        let m = &candidates[idx];
        let Some(my_partners) = ctx.partners.get(&m.head) else {
            continue;
        };
        let (bx, by) = ((m.x as i32).div_euclid(bucket), (m.y as i32).div_euclid(bucket));
        'neighbors: for dbx in -1..=1 {
            for dby in -1..=1 {
                let Some(members) = buckets.get(&(bx + dbx, by + dby)) else {
                    continue;
                };
                for &other in members {
                    if other != idx && my_partners.contains(&candidates[other].head) {
                        needs_arbitration[idx] = true;
                        needs_arbitration[other] = true;
                        break 'neighbors;
                    }
                }
            }
        }
    }

    let mut unsafe_matches = Vec::new();
    for (idx, m) in candidates.into_iter().enumerate() {
        if needs_arbitration[idx] {
            unsafe_matches.push(m);
        } else {
            safe.push(m);
        }
    }
    (safe, unsafe_matches)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_tick_with_cache<S: GridStorage>(
    grid: &mut Grid<S>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &crate::conflict_analyzer::RuleDataCache,
    group_cache: &GroupCache,
    search_radius_cache: &SearchRadiusCache,
    extension_flags: &ExtensionFlags,
    conflict_ctx: Option<&ConflictContext>,
    state: &mut RuleStateStore,
    mut timings: Option<&mut TickPhaseTimings>,
    mut counts: Option<&mut TickEventCounts>,
    write_buffer: &mut WriteBuffer,
    pattern_buffer: &mut Vec<CellValue>,
    // `LayeredEngine`'s cross-layer гейт (см. её doc-комментарий) -- `None`
    // для ВСЕХ обычных вызовов (`Engine::run_tick`/`run_tick_profiled`,
    // свободная функция `run_tick`), нулевая цена. Применяется СРАЗУ после
    // слияния cam-матчей и ДО раннего выхода на пустом `matches` -- то есть
    // ДО memory/max_activations гейтов и до starvation/feedback-списков
    // ниже: кандидат, отфильтрованный этим гейтом, для ВСЕГО остального
    // тика (наблюдение памяти, бюджет активаций, счётчики голодания/
    // feedback, сам арбитраж) структурно НЕ существовал вовсе -- та же
    // семантика, что и у непройденного `pattern` (см. doc-комментарий
    // `Rule::cross_layer_reads` в `types.rs`), а не частичный гейт поверх
    // уже засчитанного наблюдения.
    cross_layer_filter: Option<&dyn Fn(&RuleMatch) -> bool>,
) -> (Vec<RuleMatch>, Vec<(u32, Cell)>) {
    // `Instant::now()` только когда `timings` реально запрошен (`Some`) —
    // ветка ниже, а не безусловный замер: `Instant::now()` сам по себе
    // недорог, но не бесплатен, а этот путь — самый горячий во всём
    // движке (каждый тик). См. `Engine::run_tick_profiled`.
    let mut phase_start = timings.is_some().then(std::time::Instant::now);
    macro_rules! mark_phase {
        ($field:ident) => {
            if let (Some(start), Some(t)) = (phase_start, timings.as_mut()) {
                t.$field += start.elapsed();
                // Последний вызов (перед `apply`) переприсваивает значение,
                // которое больше никто не читает — безвредно, дешевле, чем
                // отдельный вариант макроса для "последней" фазы.
                #[allow(unused_assignments)]
                {
                    phase_start = Some(std::time::Instant::now());
                }
            }
        };
    }

    let search_coords = resolve_search_coords_advance(grid, search_radius_cache);

    let mut matches = detect_matches_with_group_data(grid, group_cache, &search_coords);
    // CAM-детекция (см. её doc-комментарий в `matcher.rs`) — изолированный
    // проход, ноль стоимости без `cam`-правил в конфиге (ранний выход
    // внутри `detect_cam_matches`). `search_coords` — тот же кандидатный
    // список, что и у обычной детекции: `max_pattern_radius`/`pattern_radius`
    // уже учитывают `cam.radius` в своём расширении (см. её doc-комментарий).
    let (cam_matches, cam_positions) = detect_cam_matches(grid, rule_index, &search_coords);
    matches.extend(cam_matches);

    if let Some(filter) = cross_layer_filter {
        matches.retain(|m| filter(m));
    }

    // Осиротевшие записи `feedback_counters`/`memory_buffers`/`starvation_counters`:
    // позиция, которая раньше совпала и завела запись в одной из карт, но с
    // тех пор перестала совпадать с ЭТИМ конкретным `rule_idx` (тип клетки
    // сменился внешне — проиграла конфликт другому правилу, была
    // перезаписана напрямую, и т.п., БЕЗ участия собственного
    // `apply_shift_buffered` правила, который уже переносит запись при
    // обычном сдвиге) — иначе запись остаётся в карте НАВСЕГДА (было принято
    // как "приемлемый компромисс" — растёт пропорционально числу РАЗЛИЧНЫХ
    // позиций решётки, когда-либо совпавших с extension-правилом, что
    // технически ограничено размером решётки × числом правил, но не
    // пропорционально ничему более узкому).
    //
    // `starvation_counters` изначально (см. историю сессии) НЕ входил сюда —
    // только `feedback`/`memory` — реальный, найденный при аудите
    // GPU-портирования `starvation_after` баг: без чистки замороженный
    // счётчик "К проигрышей подряд" при исчезновении и повторном появлении
    // ТОГО ЖЕ матча продолжал считать с застывшего значения, а не с нуля,
    // давая правилу выиграть РАНЬШЕ положенного по его же гарантии — см.
    // `test_starvation_counter_resets_after_match_disappears_and_reappears`.
    //
    // Дёшево и корректно ровно ПОТОМУ, что `search_coords` (посчитан в самом
    // начале функции) — это уже тот же самый dirty-based инвариант, на
    // котором держится весь инкрементальный матчер (см.
    // `resolve_search_coords_advance`): `Grid::set_cell` безусловно метит
    // клетку "грязной" при ЛЮБОЙ записи, а `search_coords` уже включает и
    // саму грязную клетку, и её соседей в пределах `max_pattern_radius`.
    // Значит если матч для `(x, y, rule_idx)` перестал выполняться, `(x, y)`
    // ГАРАНТИРОВАННО присутствует в `search_coords` этого тика — тот же
    // инвариант, что уже обеспечивает корректность самой детекции матчей, не
    // новое предположение. Проверка НЕ требует ни полного скана карты
    // (O(размер карты) каждый тик — именно то, чего этот подход избегает),
    // ни хранения снимка кандидатного множества прошлого тика (единственная
    // альтернатива с тем же результатом, но с постоянным доп. расходом
    // памяти — см. `ExtensionFlags::extension_rule_indices`): она проходит
    // по уже оплаченному `search_coords` (O(размер кандидатов этого тика),
    // та же величина, что и сама детекция матчей) и для каждой позиции
    // проверяет лишь маленький, посчитанный один раз список
    // `extension_flags.extension_rule_indices`.
    //
    // ВАЖНО: этот блок обязан стоять ДО раннего выхода `matches.is_empty()`
    // ниже — если на этом тике не нашлось вообще ни одного совпадения НИ
    // ДЛЯ ОДНОГО правила, это ОСОБЕННО важный случай для очистки (последний
    // живой матч только что исчез), а не повод его пропустить. Расположение
    // ПОСЛЕ раннего выхода было найденным, но не исправленным багом
    // (см. историю в памяти сессии) — чистка молча никогда не срабатывала
    // именно тогда, когда была нужнее всего.
    //
    // Источник "актуально ли ещё" — `matches` В СЫРОМ ВИДЕ, ДО применения
    // memory-гейта ниже: тот же выбор, что уже сделан для `memory_targets`
    // ниже (буфер обязан продолжать наблюдать, даже когда гейт закрыт).
    // Если бы здесь вместо этого использовался `feedback_keys` (считается
    // НИЖЕ, ПОСЛЕ гейта) — совмещённое `feedback`+`memory` правило с
    // временно закрытым гейтом было бы ошибочно сочтено "переставшим
    // совпадать" и вычищено, хотя структурный паттерн физически всё ещё
    // совпадает, просто временно не участвует в арбитраже (сломало бы
    // `test_emit_preserves_feedback_and_memory_state_at_source_across_ticks`-подобный
    // сценарий).
    if !extension_flags.extension_rule_indices.is_empty() {
        let prune_targets: FxHashSet<(u32, u32, usize)> = matches
            .iter()
            .filter(|m| {
                rule_index
                    .get(&m.head)
                    .and_then(|rules| rules.get(m.rule_idx))
                    .is_some_and(|r| r.feedback.is_some() || r.memory.is_some() || r.starvation_after.is_some())
            })
            .map(|m| (m.x, m.y, m.rule_idx))
            .collect();
        // `mutate()`, не `snapshot()` -- технически это запись (remove), но
        // безопасная относительно 2.2.1: ключи, которые чистит этот блок,
        // структурно НЕ совпадают ни с одним матчем этого тика (иначе они
        // были бы в `prune_targets`), значит никакое чтение этого же тика их
        // не увидит — см. doc-комментарий выше про сам механизм чистки.
        let mut w = state.mutate();
        for &(x, y) in &search_coords {
            let (xu, yu) = (x as u32, y as u32);
            for &r in &extension_flags.extension_rule_indices {
                let key = (xu, yu, r);
                if !prune_targets.contains(&key) {
                    w.feedback_counters_mut().remove(&key);
                    w.memory_buffers_mut().remove(&key);
                    w.starvation_counters_mut().remove(&key);
                }
            }
        }
    }

    if matches.is_empty() {
        // Время всё равно идёт: без этого симуляция, где на каком-то тике не
        // нашлось ни одного совпадения (например, поле держит `min_age`-
        // клетку, которая ещё не "созрела", и больше ничего не происходит),
        // навсегда замораживает generation — а с ним и возраст, который
        // `min_age` только и проверяет. Раньше `advance_age()` вызывался
        // только на пути с реально применённым тиком, из-за чего вызов
        // `run_tick()` N раз не гарантировал N реально прошедших тиков.
        grid.advance_age();
        mark_phase!(detect);
        if let Some(c) = counts.as_mut() {
            **c = TickEventCounts::default();
        }
        return (Vec::new(), Vec::new());
    }

    // Помечаем "грязными" позиции ВСЕХ найденных совпадений — не только тех,
    // что примет арбитраж. Проигравшее арбитраж совпадение — это НЕ
    // исчезнувшее условие: клетка не изменилась, паттерн по-прежнему
    // совпадает, и конфликт может разрешиться иначе на следующем тике (если
    // победитель освободит клетку или сам станет неактуален). Если не
    // помечать проигравших, они выпадают из dirty-множества навсегда, хотя
    // полный скан продолжал бы находить и переоценивать их каждый тик.
    for m in &matches {
        grid.mark_dirty(m.x as usize, m.y as usize);
    }

    // Массив-lookup вместо `rule_index.get()` для всего блока ниже — см.
    // doc-комментарий `HeadRuleIndex`/`build_head_index`/`lookup_rule`.
    let head_index = build_head_index(rule_index);

    // Матчи правил с `Rule::memory` — список нужен из ПОЛНОГО (ещё не
    // гейтованного) набора: буфер обязан продолжать наблюдать, даже пока
    // гейт этого правила закрыт, иначе искомая последовательность никогда
    // бы не накопилась (см. `Engine::memory_buffers`'s doc-комментарий).
    // Скан целиком пропускается (см. `ExtensionFlags`'s doc-комментарий),
    // если НИ ОДНО правило набора не использует `memory` — иначе платили бы
    // O(число матчей) лукапов каждый тик безусловно, вопреки заявленным
    // "нулевым накладным расходам".
    let memory_targets: Vec<(u32, u32, usize, CellType)> = if extension_flags.memory {
        matches
            .iter()
            .filter(|m| lookup_rule(&head_index, m.head, m.rule_idx).is_some_and(|r| r.memory.is_some()))
            .map(|m| (m.x, m.y, m.rule_idx, m.head))
            .collect()
    } else {
        Vec::new()
    };

    // Гейт-фильтр памяти: убирает из `matches` кандидатов, чьё правило имеет
    // `memory`, но буфер (каким он был НА КОНЕЦ ПРЕДЫДУЩЕГО тика — этот тик
    // ещё не писал в него, см. обновление буферов ниже) ещё не полон или не
    // совпадает с `match_pattern` поэлементно. Трактуется так же, как если
    // бы `pattern` не совпал вовсе — starvation_after/feedback-списки ниже и
    // сам арбитраж никогда не увидят такого кандидата на этом тике. Чисто
    // runtime-фильтр кандидатов: не меняет заявленную зону записи правила,
    // поэтому `conflict_analyzer` не требует изменений (Лемма 4 тут не
    // нужна — см. `types::MemorySpec`'s doc-комментарий).
    if !memory_targets.is_empty() {
        let snap = state.snapshot();
        matches.retain(|m| {
            let Some(spec) = lookup_rule(&head_index, m.head, m.rule_idx).and_then(|r| r.memory.as_ref()) else {
                return true;
            };
            snap.memory_buffers()
                .get(&(m.x, m.y, m.rule_idx))
                .is_some_and(|buf| buf.len() == spec.window && buf.iter().eq(spec.match_pattern.iter()))
        });
    }

    // Гейт-фильтр бюджета активаций (`Rule::max_activations`, см. её
    // doc-комментарий) — убирает из `matches` кандидатов, чьё правило уже
    // исчерпало ГЛОБАЛЬНЫЙ (не по позиции) бюджет побед. Счётчик читается
    // КАК ОН БЫЛ НА КОНЕЦ ПРЕДЫДУЩЕГО тика (2.2.1) — этот тик ещё не писал
    // в него (инкремент ниже, после apply). В отличие от memory-гейта, не
    // нужен отдельный "сырой" список целей до фильтра — запись счётчика не
    // привязана к позиции и не нуждается в непрерывном наблюдении, когда
    // гейт закрыт (нечему наблюдать — как только правило исчерпало бюджет,
    // оно исчерпало его НАВСЕГДА, дальше проверять нечего).
    if extension_flags.max_activations {
        let snap = state.snapshot();
        matches.retain(|m| {
            let Some(limit) = lookup_rule(&head_index, m.head, m.rule_idx).and_then(|r| r.max_activations) else {
                return true;
            };
            snap.activation_counters()
                .get(&(m.head, m.rule_idx))
                .copied()
                .unwrap_or(0)
                < limit
        });
    }

    // Матчи правил с `Rule::starvation_after`/`Rule::feedback` — единственные,
    // за которыми вообще стоит следить (см. doc-комментарии
    // `Engine::starvation_counters`/`Engine::feedback_counters`); списки нужны
    // ДО того, как `matches` уйдёт по значению в арбитраж ниже. Считаются
    // ПОСЛЕ гейт-фильтра памяти — гейтованный кандидат этот тик не участвует
    // ни в чём, как будто не детектировался. Каждый скан пропускается
    // целиком, если соответствующий флаг `ExtensionFlags` ложный — та же
    // причина, что и у `memory_targets` выше.
    let starving_keys: Vec<(u32, u32, usize)> = if extension_flags.starvation_after {
        matches
            .iter()
            .filter(|m| lookup_rule(&head_index, m.head, m.rule_idx).is_some_and(|r| r.starvation_after.is_some()))
            .map(|m| (m.x, m.y, m.rule_idx))
            .collect()
    } else {
        Vec::new()
    };
    let feedback_keys: Vec<(u32, u32, usize)> = if extension_flags.feedback {
        matches
            .iter()
            .filter(|m| lookup_rule(&head_index, m.head, m.rule_idx).is_some_and(|r| r.feedback.is_some()))
            .map(|m| (m.x, m.y, m.rule_idx))
            .collect()
    } else {
        Vec::new()
    };

    // Снимок для отчёта (п.5) -- ДО того, как `matches` уйдёт по значению в
    // арбитраж ниже; `starving_keys`/`feedback_keys` тоже считаются здесь на
    // случай, если сами векторы позже будут перемещены/изменены.
    let candidate_count = matches.len();
    let starvation_candidate_count = starving_keys.len();
    let feedback_candidate_count = feedback_keys.len();

    mark_phase!(detect);

    // Арбитраж: матчи, у которых в ЭТОМ тике физически нет рядом ни одного
    // структурного конфликт-партнёра, принимаются напрямую, без единого
    // сравнения (см. doc-комментарий функции); остальные — через обычный
    // арбитраж.
    let generation = grid.generation() as u32;
    // `tie_break_decided` -- ключи принятых матчей, чья победа НЕ решена
    // priority/age (см. `arbitrator::TieBreakDecidedWins`), нужны ниже для
    // корректного обновления `starvation_counters` (5.2/2.2.1: победа "по
    // жребию" не должна сбрасывать счётчик голодания так же, как решительная
    // победа). `safe`-ветка (`spatial_bypass_split`) сюда не попадает вообще
    // -- у нeё по построению нет ни одного структурного конфликт-партнёра
    // рядом в этом тике, значит и тай-брейка не было, победа заведомо
    // решительна.
    let (accepted, tie_break_decided): (Vec<RuleMatch>, FxHashSet<(u32, u32, usize)>) = {
        // Снимок держится, пока arbitrate читает счётчики -- borrow checker
        // не даст получить `state.mutate()` (нужен для обновлений ниже),
        // пока этот `snap` жив, ровно то формальное ограничение из
        // doc-комментария `rule_state`, которое раньше держалось только
        // дисциплиной.
        let snap = state.snapshot();
        match conflict_ctx {
            None => arbitrate_with_cam(
                matches,
                rule_index,
                rule_cache,
                (grid.width(), grid.height()),
                &cam_positions,
                generation,
                snap.starvation_counters(),
                snap.feedback_counters(),
                &[],
                |x, y| grid.get_age(x, y) as u32,
            ),
            Some(ctx) => {
                let (safe, unsafe_matches) = spatial_bypass_split(matches, ctx);
                if unsafe_matches.is_empty() {
                    (safe, FxHashSet::default())
                } else {
                    let (unsafe_accepted, tie_break_decided) = arbitrate_spatial_with_cam(
                        unsafe_matches,
                        rule_index,
                        rule_cache,
                        (grid.width(), grid.height()),
                        ctx.max_radius,
                        &cam_positions,
                        generation,
                        snap.starvation_counters(),
                        snap.feedback_counters(),
                        |x, y| grid.get_age(x, y) as u32,
                    );
                    let mut accepted = safe;
                    accepted.extend(unsafe_accepted);
                    (accepted, tie_break_decided)
                }
            }
        }
    };
    mark_phase!(arbitrate);

    // Голодание и память (при триггере `RuleOutcome`) оба смотрят "кто
    // выиграл арбитраж" — считаем этот набор один раз, а не дважды.
    // `feedback_keys` тоже нужен этот набор (см. её doc-комментарий ниже) —
    // добавлен в условие наравне со starving_keys/memory_targets.
    let accepted_keys: FxHashSet<(u32, u32, usize)> =
        if starving_keys.is_empty() && memory_targets.is_empty() && feedback_keys.is_empty() {
            FxHashSet::default()
        } else {
            accepted.iter().map(|m| (m.x, m.y, m.rule_idx)).collect()
        };

    // Обновление счётчиков голодания: выигравшие сбрасываются (запись
    // удаляется), проигравшие растут на 1 (saturating — см. doc-комментарий
    // поля). Делается ПОСЛЕ арбитража, а не во время — сам арбитраж только
    // ЧИТАЕТ счётчики (см. `resolve_sort_fields`), обновление их же в
    // процессе сортировки было бы порядко-зависимым UB-по-смыслу.
    {
        let mut w = state.mutate();
        for key in starving_keys {
            if accepted_keys.contains(&key) {
                // Победа "по жребию" (tie_break, не priority/age) не сбрасывает
                // счётчик голодания -- см. doc-комментарий `tie_break_decided`
                // выше и `arbitrator::TieBreakDecidedWins`. Счётчик остаётся КАК
                // ЕСТЬ: не растёт (это всё-таки победа, не проигрыш), но и не
                // обнуляется (реального превосходства не было -- следующий тик
                // должен считаться от того же накопленного значения, иначе
                // правило, побеждающее только жребием, никогда не докопит до
                // `starvation_after` даже суммарно проигрывая чаще, чем выигрывая).
                if !tie_break_decided.contains(&key) {
                    w.starvation_counters_mut().remove(&key);
                }
            } else {
                let counter = w.starvation_counters_mut().entry(key).or_insert(0);
                *counter = counter.saturating_add(1);
            }
        }
    }

    // Обновление буферов памяти (см. `Engine::memory_buffers`): `NeighborType`
    // пишет значение, известное уже ДО арбитража (тип соседа — читаем ТЕКУЩЕЕ
    // pre-tick состояние решётки, apply ещё не произошёл); `RuleOutcome`
    // пишет исход АРБИТРАЖА этого тика (`accepted_keys`, уже посчитан выше).
    // FIFO: новое значение — в конец, при переполнении `window` — старое
    // вылетает с начала.
    {
        let mut w = state.mutate();
        for (x, y, rule_idx, head) in memory_targets {
            let Some(spec) = lookup_rule(&head_index, head, rule_idx).and_then(|r| r.memory.as_ref()) else {
                continue;
            };
            let value = match spec.record_trigger {
                RecordTrigger::NeighborType(dir) => {
                    let (dx, dy) = arbitrator::direction_delta(dir);
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    let cell_type = if nx < 0 || ny < 0 {
                        CellType::new(DEFAULT_CELL_VALUE)
                    } else {
                        grid.get_cell(nx as usize, ny as usize)
                            .map(|c| c.value.0)
                            .unwrap_or(CellType::new(DEFAULT_CELL_VALUE))
                    };
                    RecordedValue::Type(cell_type)
                }
                RecordTrigger::RuleOutcome => {
                    if accepted_keys.contains(&(x, y, rule_idx)) {
                        RecordedValue::Applied
                    } else {
                        RecordedValue::Missed
                    }
                }
            };
            let window = spec.window;
            let buf = w.memory_buffers_mut().entry((x, y, rule_idx)).or_default();
            buf.push_back(value);
            while buf.len() > window {
                buf.pop_front();
            }
        }
    }

    let outputs = if accepted.is_empty() {
        // См. комментарий выше — то же самое: тик "случился", даже если всё
        // отклонено арбитражем, время должно пройти.
        grid.advance_age();
        Vec::new()
    } else {
        // Применение. `RuleStateWriter`, не `snapshot()`: `apply_rule_buffered`
        // (внутри `apply_matches_with_cam`) читает `feedback_counters` для
        // `feedback_override` И инкрементирует его -- единая атомарная
        // операция для победителя (см. doc-комментарий `rule_state`'s
        // модуля про единственное осознанное исключение).
        //
        // `&accepted`, не `accepted.clone()` (было раньше) -- `RuleMatch`
        // (`Copy`, 13 байт) не требует владения для `apply_matches_with_cam`
        // (см. её doc-комментарий): полная копия `Vec<RuleMatch>` на КАЖДЫЙ
        // тик с принятыми матчами была не нужна вообще, `accepted` и так
        // используется ниже (обновление бюджета активаций, `counts`, и
        // собственный возврат функции) -- клон существовал только потому,
        // что старая сигнатура требовала владения, а не потому, что данные
        // реально нужно было дублировать.
        let mut w = state.mutate();
        let (feedback_counters, memory_buffers) = w.feedback_and_memory_mut();
        let (regions, outputs) = apply_matches_with_cam(
            grid,
            &accepted,
            rule_index,
            rule_cache,
            &cam_positions,
            feedback_counters,
            memory_buffers,
            write_buffer,
            pattern_buffer,
        );

        // Старение
        grid.advance_age();
        reset_age_for_regions(grid, &regions);
        outputs
    };

    // Обновление счётчиков обратной связи ДЛЯ ПРОИГРАВШИХ арбитраж матчей —
    // ЗАЩЁЛКА: растёт на КАЖДЫЙ тик, где матч детектируется (независимо от
    // исхода арбитража — считаются попытки, не победы), никогда не
    // сбрасывается. Читается (арбитражем и apply) КАК ОНА БЫЛА на конец
    // предыдущего тика — та же дисциплина, что уже соблюдает
    // `starvation_counters`; переключение на `new_direction` вступает в
    // силу СО СЛЕДУЮЩЕГО тика после того, как порог достигнут, не в ТОТ ЖЕ
    // тик. GPU (`shader.wgsl`) зеркалит эту семантику без какой-либо
    // "+1"-поправки — читает persistent-счётчик напрямую, ровно как уже
    // делает `starvation_counters`.
    //
    // ВЫИГРАВШИЕ матчи (в `accepted_keys`) сюда НЕ входят — их инкремент
    // уже сделан ВНУТРИ `applicator::apply_rule_buffered`, СРАЗУ после
    // чтения счётчика для `feedback_override` и ДО вызова
    // `apply_shift_buffered`. Это не то же самое место, что у голодания
    // (которое обновляется единым проходом здесь, после арбитража) —
    // критичная разница: `apply_shift_buffered` может РЕЛОЦИРОВАТЬ запись
    // этого же матча на новую позицию (remove старого ключа + insert
    // нового), так что единый пост-apply проход по СТАРЫМ позициям
    // (`feedback_keys`, посчитанным ДО арбитража) создавал бы для КАЖДОГО
    // выигравшего и сдвинувшегося матча ПОСТОРОННЮЮ свежую запись на уже
    // покинутой позиции вместо инкремента РЕАЛЬНОЙ, уже перенесённой записи
    // — защёлка никогда не достигала бы `timeout` (найдено эмпирически:
    // маркер уезжал за край решётки, ни разу не переключившись). Матчи,
    // проигравшие арбитраж, никогда не вызывают apply вообще — их позиция
    // гарантированно НЕ релоцирована, инкремент по ней здесь безопасен.
    {
        let mut w = state.mutate();
        for key in &feedback_keys {
            if !accepted_keys.contains(key) {
                let counter = w.feedback_counters_mut().entry(*key).or_insert(0);
                *counter = counter.saturating_add(1);
            }
        }
    }

    // Обновление бюджета активаций: инкремент для КАЖДОГО выигравшего матча,
    // чьё правило использует `max_activations` — ключ `(head, rule_idx)` не
    // привязан к позиции (см. её doc-комментарий), поэтому, в отличие от
    // `feedback_keys`, нет проблемы релокации записи при сдвиге — можно
    // просто пройти по уже посчитанному `accepted` напрямую, без отдельного
    // pre-arbitration списка ключей.
    if extension_flags.max_activations {
        let mut w = state.mutate();
        for m in &accepted {
            if lookup_rule(&head_index, m.head, m.rule_idx).is_some_and(|r| r.max_activations.is_some()) {
                let counter = w.activation_counters_mut().entry((m.head, m.rule_idx)).or_insert(0);
                *counter = counter.saturating_add(1);
            }
        }
    }

    // "apply" здесь включает и Flush (5.5) -- `advance_age`/сбор output
    // происходят ВНУТРИ ветки `accepted.is_empty()` выше, не отдельным
    // блоком; текущая структура функции не даёт их вычленить без более
    // рискованной переделки уже проверенного тик-пайплайна (см. doc-
    // комментарий `TickPhaseTimings::apply`).
    mark_phase!(apply);

    if let Some(c) = counts.as_mut() {
        **c = TickEventCounts {
            accepted: accepted.len(),
            rejected: candidate_count - accepted.len(),
            starvation_events: starvation_candidate_count,
            feedback_events: feedback_candidate_count,
        };
    }

    (accepted, outputs)
}
