use std::cmp::Reverse;
use std::collections::HashMap;

use rayon::prelude::*;

use crate::conflict_analyzer::{get_rule_data, RuleDataCache};
use crate::engine::matcher::CamPositions;
use crate::fast_hash::{FxHashMap, FxHashSet};
use crate::types::{CellType, OverflowAction, Rule, RuleMatch};

/// Счётчики "сколько тиков подряд этот матч проигрывал арбитраж" — см.
/// doc-комментарий `Engine::starvation_counters` и `Rule::starvation_after`.
/// Ключ — тот же `(x, y, rule_idx)`, что и у `CamPositions` (позиция матча +
/// индекс сработавшего правила).
pub(crate) type StarvationCounters = FxHashMap<(u32, u32, usize), u32>;

/// Счётчики "сколько тиков подряд этот матч детектировался" (защёлка, не
/// сбрасывается) — см. `Engine::feedback_counters` и `Rule::feedback`. Тот
/// же ключ `(x, y, rule_idx)`.
pub(crate) type FeedbackCounters = FxHashMap<(u32, u32, usize), u64>;

/// FIFO-буферы наблюдений памяти (см. `Engine::memory_buffers` и
/// `Rule::memory`) — тот же ключ `(x, y, rule_idx)`, что и у остальных
/// per-match Engine-состояний.
pub(crate) type MemoryBuffers = FxHashMap<(u32, u32, usize), std::collections::VecDeque<crate::types::RecordedValue>>;

/// Ниже этого числа матчей накладные расходы rayon (work-stealing,
/// синхронизация пула потоков) не окупаются — та же логика, что и
/// `matcher::PARALLEL_THRESHOLD` (см. её doc-комментарий: там это уже
/// измерено на практике для detect_matches, здесь используем то же число
/// для симметрии — оба места про запуск потоков на маленьком объёме
/// работы).
const PARALLEL_SORT_THRESHOLD: usize = 1024;

/// Модуль для вращения `Rule::tie_break` по поколениям (см. её doc-комментарий
/// в `types.rs`). КРИТИЧНО, что CPU и GPU (`shader.wgsl::TIE_BREAK_MODULUS`)
/// используют ОДНО И ТО ЖЕ число — иначе побитовое совпадение результатов
/// сломается на любом правиле, где `tie_break != 0`.
///
/// Небольшая степень двойки, а не большое простое: для ДВУХ соперничающих
/// правил с residues `a` и `a+d (mod M)` победитель меняется РОВНО в те `d`
/// поколений из каждого периода `M`, где сложение с generation переносит
/// меньший residue через границу модуля раньше большего — то есть частота
/// чередования определяется РАЗНОСТЬЮ `d`, не абсолютным размером `M`.
/// Отсюда рецепт для СТРОГО поровну (50/50) чередования двух правил:
/// расставить их `tie_break` РОВНО на `M/2` друг от друга (например, `0` и
/// `8` при `M=16` — proверено `test_tie_break_rotates_fairly_when_spaced_half_modulus_apart`).
/// Для K-стороннего round-robin — на `M/K` друг от друга. Небольшой модуль
/// делает это наблюдаемым за единицы-десятки тиков (а не сотни/тысячи, как
/// было бы при большом простом), и `& (M-1)` дешевле `%` на GPU, раз `M` —
/// степень двойки.
pub(crate) const TIE_BREAK_MODULUS: u32 = 16;

/// Выбрать непротиворечивый набор совпадений.
///
/// Арбитраж проверяет пересечение РЕАЛЬНЫХ ЗАПИСЕЙ (позиция сдвига +
/// изменения — `RuleData::write_cells`), а не всего паттерна. Это
/// гарантирует, что два совпадения не будут конфликтовать при применении,
/// даже если их паттерны не пересекаются, но их изменения затрагивают одни
/// и те же ячейки — и, симметрично, что два совпадения, чьи паттерны
/// пересекаются (оба читают одну и ту же клетку), но пишут в разные
/// клетки, НЕ считаются конфликтующими: detect_matches всегда читает
/// состояние решётки до тика, так что общее чтение никогда не гонка.
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
    // generation=0: без реального счётчика поколений `tie_break` вырождается
    // в постоянное значение (см. doc-комментарий `Rule::tie_break`) —
    // корректно (не паникует, не расходится с CPU-эталоном), просто не
    // вращается между вызовами; только `run_tick`/`Engine::run_tick` знают
    // настоящее поколение (см. doc-комментарий `arbitrate_with_cam` про ту
    // же причину для `cam_positions`). Пустые `StarvationCounters`/
    // `FeedbackCounters` — та же история: свободная функция не хранит
    // состояние МЕЖДУ вызовами, так что `Rule::starvation_after`/
    // `Rule::feedback` для неё всегда no-op (см. их doc-комментарии).
    arbitrate_with_cam(
        all_matches,
        rule_index,
        rule_cache,
        bounds,
        &CamPositions::default(),
        0,
        &StarvationCounters::default(),
        &FeedbackCounters::default(),
        get_cell_age,
    )
}

/// Как [`arbitrate`], но с картой найденных CAM-позиций (см.
/// `matcher::detect_cam_matches`) — `CamPositions` опирается на
/// `pub(crate) FxHashMap`, так что не может появиться в сигнатуре ПУБЛИЧНОЙ
/// `arbitrate` без утечки приватности типа наружу; только `run_tick`/
/// `Engine::run_tick` имеют что сюда передать (см. doc-комментарий
/// `Engine::detect_matches`), поэтому `arbitrate` остаётся с прежней
/// сигнатурой и просто подставляет пустую карту.
///
/// `generation` — текущее поколение (см. `grid.generation()`), используется
/// ТОЛЬКО для вращения `Rule::tie_break` (см. её doc-комментарий и
/// `TIE_BREAK_MODULUS`); не влияет ни на что другое в арбитраже.
///
/// `starvation_counters` — см. `Rule::starvation_after` и
/// `Engine::starvation_counters`: для матча с `counters[(x,y,rule_idx)] >=
/// rule.starvation_after`, эффективный priority на ЭТОТ тик становится
/// `u32::MAX` (побеждает гарантированно). Обновление счётчиков (инкремент
/// проигравших, сброс выигравших) — забота ВЫЗЫВАЮЩЕЙ стороны
/// (`run_tick_with_cache`), не этой функции: она только ЧИТАЕТ счётчики.
#[allow(clippy::too_many_arguments)]
pub(crate) fn arbitrate_with_cam(
    all_matches: Vec<RuleMatch>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &RuleDataCache,
    bounds: (usize, usize),
    cam_positions: &CamPositions,
    generation: u32,
    starvation_counters: &StarvationCounters,
    feedback_counters: &FeedbackCounters,
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
    // FxHashSet, а не стандартный (SipHash) — чисто внутренняя структура,
    // число записей за тик обычно единицы (см. `fast_hash` модуль).
    let mut used_cells: FxHashSet<(i32, i32)> = FxHashSet::default();

    // Полностью детерминированный тай-брейк: priority → age → rule_id →
    // координаты матча → rule_idx. Раньше при равенстве priority+age порядок
    // определялся тем, в каком порядке detect_matches нашёл матчи —
    // implementation-defined и невоспроизводимо в других реализациях (в
    // частности, в раундовом локальном арбитраже, который эту эквивалентность
    // и мотивировал). Явный тай-брейк даёт identical results с любой другой
    // реализацией, использующей тот же порядок сравнения, а не просто
    // "какой-то одинаково безопасный, но другой" результат.
    //
    // `rule_id` здесь — это полный `Rule::id`, а не `RuleMatch::head` (только
    // голова): несколько правил под одной головой могут иметь разный
    // "хвост" id, и тай-брейк должен различать их, а не тут же вырождаться
    // в сравнение по (x, y, rule_idx). `RuleMatch` больше не хранит клон
    // этого id (см. её doc-комментарий) — восстанавливаем его тем же
    // способом, что и `get_priority`/`get_match_affected_cells`.
    //
    // Ключ считаем один раз на элемент вручную (decorate-sort-undecorate), а
    // сортируем `sort_unstable_by` по уже готовому ключу — не
    // `sort_by_cached_key` (стабильная, медленнее на больших объёмах).
    // Стабильность тут не нужна: (x, y, rule_idx) сам по себе уникален для
    // каждого match'а (одно и то же правило не может совпасть в одной и той
    // же позиции дважды в одном `detect_matches`), так что у полного
    // 6-компонентного ключа никогда не бывает двух РАЗНЫХ элементов с
    // одинаковым значением — то есть ничьих, которые стабильность обязана
    // была бы сохранить, попросту не существует: нестабильная сортировка
    // даёт ТОТ ЖЕ порядок, только быстрее.
    //
    // Пробовал заменить на поразрядную (radix) сортировку по каждому полю
    // отдельно — EMPIRICALLY оказалось МЕДЛЕННЕЕ (даже после починки лишнего
    // клонирования): сравнение-based сортировка тут выигрывает за счёт
    // короткого замыкания (tuple-сравнение останавливается на первом
    // несовпавшем поле — для типичных данных, где priority/age у всех
    // matches одинаковы, это 1-2 поля, а не все 6), тогда как radix обязан
    // честно пройти ВСЕ 5 числовых полей на каждый элемент независимо от
    // того, различают ли они вообще что-то в этом конкретном наборе данных.
    // Урок: не всякий "асимптотически лучше" алгоритм быстрее на практике.
    //
    // Параллельная сортировка из rayon (уже зависимость проекта, используется
    // в matcher.rs) — не самописный алгоритм, а готовая, отточенная
    // реализация; порог, ниже которого не стоит платить за потоки, тот же,
    // что и в matcher.rs (см. `PARALLEL_THRESHOLD`).
    let mut keyed: Vec<_> = all_matches
        .iter()
        .map(|m| {
            let (priority, tie_break, rule_id) = resolve_sort_fields(m, rule_index, starvation_counters);
            let age = get_cell_age(m.x as usize, m.y as usize);
            // Вращаем ОДИН раз здесь (decorate-sort-undecorate), не внутри
            // компаратора сортировки — тот вызывается O(n log n) раз, а
            // повёрнутое значение матча не меняется в течение ОДНОГО вызова
            // arbitrate (одно и то же generation для всех матчей тика).
            let tie_break_rotated = tie_break.wrapping_add(generation) % TIE_BREAK_MODULUS;
            (
                (
                    Reverse(priority),
                    Reverse(age),
                    Reverse(tie_break_rotated),
                    Reverse(rule_id),
                    Reverse(m.x),
                    Reverse(m.y),
                    Reverse(m.rule_idx),
                ),
                *m,
            )
        })
        .collect();
    if keyed.len() >= PARALLEL_SORT_THRESHOLD {
        keyed.par_sort_unstable_by(|a, b| a.0.cmp(&b.0));
    } else {
        keyed.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    }
    let sorted: Vec<RuleMatch> = keyed.into_iter().map(|(_, m)| m).collect();

    // Переиспользуемый буфер вместо свежего Vec на каждый матч (см.
    // doc-комментарий `get_match_affected_cells`).
    let mut affected: Vec<(i32, i32)> = Vec::new();
    for m in sorted {
        // Получаем предвычисленные affected cells из кэша
        get_match_affected_cells(&m, rule_index, rule_cache, bounds, cam_positions, feedback_counters, &mut affected);
        let conflict = affected.iter().any(|coord| used_cells.contains(coord));

        if !conflict {
            used_cells.extend(affected.iter().copied());
            accepted.push(m);
        }
    }

    accepted
}

/// Ниже какого числа матчей разбиение на полосы не окупается — сама
/// сортировка (уже O(M log M), параллельная выше `PARALLEL_SORT_THRESHOLD`)
/// и линейный проход дешевле накладных расходов на классификацию
/// core/boundary плюс rayon-диспетчеризацию нескольких независимых
/// арбитражей вместо одного.
const SPATIAL_THRESHOLD: usize = 4096;

/// Реализация Theorem 6 (`paper2.md` §6.2, "Spatial Decomposition of
/// Arbitration") — параллелит САМ арбитраж (не просто пропускает его, как
/// `Engine::conflict_partners`/`spatial_bypass_split` в `mod.rs`, а именно
/// распределяет по потокам, когда конфликты реально есть и арбитраж не
/// может быть пропущен целиком).
///
/// `reach` — `K` из Definition 4: наибольшее манхэттенское расстояние от
/// позиции совпадения до любой клетки в PatternCells ∪ Affected среди ВСЕХ
/// правил набора (то же самое, что уже вычисляет
/// `engine::compute_conflict_partners`'s `max_radius` — оба места опираются
/// на один и тот же `RuleData::bbox`, который включает и паттерн, и запись).
///
/// Полосы делятся по оси X с запасом `2K` (Definition 5): совпадение
/// core к полосе, если до ОБЕИХ границ полосы ≥ 2K по x — тогда его
/// affected-регион (не более K от центра) физически не может дотянуться ни
/// до соседней полосы, ни до совпадения, пограничного для своей (Lemma 6).
/// Такие core-совпадения из РАЗНЫХ полос гарантированно не пересекаются —
/// их можно арбитрировать независимо и параллельно. Пограничные совпадения
/// (внутри 2K от какой-либо границы полосы) идут одним общим
/// последовательным проходом — как и раньше.
///
/// Полосы выбираются по РЕАЛЬНОМУ разбросу x-координат матчей этого тика
/// (не по номинальной ширине решётки) — на `ChunkStorage` номинальная
/// ширина `usize::MAX`, а сами матчи почти всегда сгруппированы в узкой
/// области; статичное деление "всей" ширины решётки было бы бессмысленным.
#[allow(clippy::too_many_arguments)]
pub fn arbitrate_spatial(
    all_matches: Vec<RuleMatch>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &RuleDataCache,
    bounds: (usize, usize),
    reach: i32,
    get_cell_age: impl Fn(usize, usize) -> u32 + Sync,
) -> Vec<RuleMatch> {
    arbitrate_spatial_with_cam(
        all_matches,
        rule_index,
        rule_cache,
        bounds,
        reach,
        &CamPositions::default(),
        0,
        &StarvationCounters::default(),
        &FeedbackCounters::default(),
        get_cell_age,
    )
}

/// Как [`arbitrate_spatial`], но с картой найденных CAM-позиций — см.
/// doc-комментарий [`arbitrate_with_cam`] про ту же причину раздельных
/// публичной/`pub(crate)` версий. `generation`/`starvation_counters`/
/// `feedback_counters` — см. её doc-комментарий там же.
#[allow(clippy::too_many_arguments)]
pub(crate) fn arbitrate_spatial_with_cam(
    all_matches: Vec<RuleMatch>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &RuleDataCache,
    bounds: (usize, usize),
    reach: i32,
    cam_positions: &CamPositions,
    generation: u32,
    starvation_counters: &StarvationCounters,
    feedback_counters: &FeedbackCounters,
    get_cell_age: impl Fn(usize, usize) -> u32 + Sync,
) -> Vec<RuleMatch> {
    if all_matches.len() < SPATIAL_THRESHOLD || reach <= 0 {
        return arbitrate_with_cam(all_matches, rule_index, rule_cache, bounds, cam_positions, generation, starvation_counters, feedback_counters, get_cell_age);
    }

    let margin = (2 * reach) as u32;
    let (min_x, max_x) = all_matches.iter().fold((u32::MAX, 0u32), |(lo, hi), m| (lo.min(m.x), hi.max(m.x)));
    let spread = max_x.saturating_sub(min_x);

    // Число полос: не больше потоков rayon, и каждая полоса должна быть
    // хотя бы вдвое шире запаса (иначе в ней в принципе не может быть core-
    // совпадений — вся полоса окажется boundary, разбиение того не стоит).
    let max_bands_by_spread = if margin == 0 { usize::MAX } else { (spread / (margin * 2)).max(1) as usize };
    let num_bands = rayon::current_num_threads().min(max_bands_by_spread).max(1);

    if num_bands < 2 {
        return arbitrate_with_cam(all_matches, rule_index, rule_cache, bounds, cam_positions, generation, starvation_counters, feedback_counters, get_cell_age);
    }

    let band_width = (spread / num_bands as u32).max(1);
    let band_of = |x: u32| -> usize {
        (((x - min_x) / band_width) as usize).min(num_bands - 1)
    };
    let band_range = |band: usize| -> (u32, u32) {
        let start = min_x + band as u32 * band_width;
        let end = if band == num_bands - 1 { max_x + 1 } else { min_x + (band as u32 + 1) * band_width };
        (start, end)
    };

    let mut core_by_band: Vec<Vec<RuleMatch>> = (0..num_bands).map(|_| Vec::new()).collect();
    let mut boundary: Vec<RuleMatch> = Vec::new();

    for m in all_matches {
        let band = band_of(m.x);
        let (start, end) = band_range(band);
        // "Расстояние до ОБЕИХ границ полосы ≥ 2K" (Definition 5) — граница
        // полосы это [start, end), расстояние до левой — m.x - start,
        // до правой — (end - 1) - m.x.
        let dist_left = m.x - start;
        let dist_right = (end - 1).saturating_sub(m.x);
        if dist_left >= margin && dist_right >= margin {
            core_by_band[band].push(m);
        } else {
            boundary.push(m);
        }
    }

    // Core-полосы — параллельно (Lemma 6: разные полосы никогда не делят
    // клетки), каждая через тот же детерминированный тотальный порядок.
    let core_results: Vec<RuleMatch> = core_by_band
        .into_par_iter()
        .flat_map(|band_matches| {
            if band_matches.is_empty() {
                Vec::new()
            } else {
                arbitrate_with_cam(band_matches, rule_index, rule_cache, bounds, cam_positions, generation, starvation_counters, feedback_counters, &get_cell_age)
            }
        })
        .collect();

    // Boundary — один общий последовательный проход, как и раньше.
    let mut result = core_results;
    result.extend(arbitrate_with_cam(boundary, rule_index, rule_cache, bounds, cam_positions, generation, starvation_counters, feedback_counters, &get_cell_age));
    result
}

/// Приоритет и id правила, сработавшего в данном match'е, — ОДНИМ поиском
/// в `rule_index`, а не двумя раздельными (как было раньше: `get_priority`
/// и `resolve_rule_id` каждый делали свой `rule_index.get(&m.head)...`).
/// Вызывается один раз на КАЖДЫЙ матч при построении ключа сортировки —
/// на миллионах матчей лишний повторный поиск в HashMap заметен.
/// Использует `rule_idx`, а не поиск по одной лишь `head` — несколько правил
/// могут иметь одинаковую голову, и только `rule_idx` однозначно определяет,
/// какое именно правило сработало.
///
/// `priority` возвращается уже с учётом голодания (см. `Rule::starvation_after`
/// и `Engine::starvation_counters`): если у сработавшего правила это поле
/// установлено И счётчик проигрышей ЭТОГО конкретного `(x, y, rule_idx)` уже
/// достиг порога — возвращается `u32::MAX` вместо номинального `rule.priority`,
/// что гарантирует победу на этот тик (после чего вызывающая сторона сбросит
/// счётчик — см. `run_tick_with_cache`). Без этого поля (`None`, по умолчанию)
/// или пока счётчик ниже порога — обычный номинальный `priority`, без изменений.
fn resolve_sort_fields(
    m: &RuleMatch,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    starvation_counters: &StarvationCounters,
) -> (u32, u32, RuleIdKey) {
    match rule_index.get(&m.head).and_then(|rules| rules.get(m.rule_idx)) {
        Some(rule) => {
            let priority = match rule.starvation_after {
                Some(threshold) if starvation_counters.get(&(m.x, m.y, m.rule_idx)).copied().unwrap_or(0) >= threshold => u32::MAX,
                _ => rule.priority,
            };
            (priority, rule.tie_break, RuleIdKey::from_id(&rule.id))
        }
        None => (0, 0, RuleIdKey::Small([0u8; 16], 0)),
    }
}

/// Компактная, Copy-версия `Rule::id` для тай-брейка арбитража — вместо
/// `Vec<CellType>` (аллокация в куче на каждый матч). Найдено экспериментально
/// (профилирование `arbitrate()` на 4 млн матчей): `sort_by_cached_key`
/// вычисляет ключ ОДИН раз на элемент, но сам АЛГОРИТМ сортировки сравнивает
/// уже вычисленные ключи O(n log n) раз — и Vec-поле в ключе означает
/// разыменование кучи на КАЖДОЕ такое сравнение, а не только один раз при
/// построении. Для реалистичных id (почти всегда ≤16 клеток) `Small` хранит
/// байты прямо в ключе на стеке — сравнения превращаются в сравнение срезов
/// без единого обращения к куче. Id длиннее 16 клеток (на практике не
/// встречается, но встретиться может) — честный fallback на `Vec<u8>`.
///
/// `CellType` — простая обёртка над `u8` с derived `Ord` (сравнение через
/// внутренний байт), так что сравнение по байтам here даёт РОВНО ТУ ЖЕ
/// сортировку, что и лексикографическое сравнение `Vec<CellType>` раньше.
#[derive(Clone, PartialEq, Eq)]
enum RuleIdKey {
    Small([u8; 16], u8),
    Large(Vec<u8>),
}

impl RuleIdKey {
    fn from_id(id: &[CellType]) -> Self {
        if id.len() <= 16 {
            let mut buf = [0u8; 16];
            for (i, ct) in id.iter().enumerate() {
                buf[i] = ct.0;
            }
            RuleIdKey::Small(buf, id.len() as u8)
        } else {
            RuleIdKey::Large(id.iter().map(|ct| ct.0).collect())
        }
    }

    fn as_slice(&self) -> &[u8] {
        match self {
            RuleIdKey::Small(buf, len) => &buf[..*len as usize],
            RuleIdKey::Large(v) => v.as_slice(),
        }
    }
}

impl PartialOrd for RuleIdKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RuleIdKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

/// Вычислить набор реально ЗАПИСЫВАЕМЫХ ячеек для совпадения, используя кэш.
///
/// Берёт предвычисленные относительные `write_cells` из RuleDataCache (не
/// `affected_cells` — тот включает и клетки паттерна, которые матч только
/// ЧИТАЕТ; конфликтовать могут только записи, см. doc-комментарий
/// `RuleData::write_cells`) и сдвигает их на позицию совпадения. Клетка-цель
/// сдвига дополнительно клэмпится на границы решётки, если у правила
/// `OverflowAction::Write` и сдвиг уходит за пределы решётки — это ровно то,
/// что реально делает `apply_shift_buffered` при overflow-записи.
/// Пишет результат в переданный буфер (очищая его сначала), а не возвращает
/// новый `Vec` — вызывается один раз на КАЖДЫЙ матч в горячем цикле
/// `arbitrate()` (найдено экспериментально при профилировании на 4 млн
/// матчей: свежая аллокация на каждый вызов заметна на таком объёме).
/// Буфер переиспользуется вызывающим кодом между итерациями цикла.
fn get_match_affected_cells(
    m: &RuleMatch,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &RuleDataCache,
    bounds: (usize, usize),
    cam_positions: &CamPositions,
    feedback_counters: &FeedbackCounters,
    out: &mut Vec<(i32, i32)>,
) {
    out.clear();
    let head = m.head;
    let matched_rule = rule_index.get(&head).and_then(|rules| rules.get(m.rule_idx));

    // CAM-матч БЕЗ `recursion`: точные affected cells — найденная позиция
    // (не консервативный весь-диск из `RuleData::write_cells`, который
    // годится только для статического графа конфликтов, см. её
    // doc-комментарий в `conflict_analyzer.rs`) плюс сама позиция магнита.
    // `cam_positions` всегда содержит запись для КАЖДОГО CAM-матча,
    // дошедшего сюда — она заполняется в `detect_cam_matches` синхронно с
    // самим `RuleMatch`.
    //
    // CAM-матч С `recursion`: уровни каскада `k = 1..=max_depth` (см.
    // `applicator::apply_cam_buffered`) находят свои клетки ПРОЦЕДУРНО во
    // время apply, читая `write_buffer`, уже накопленный БОЛЕЕ РАННИМИ
    // (по порядку арбитража) матчами того же тика — то есть их реальные
    // affected cells зависят от порядка применения, который на момент
    // арбитража ещё не определён (та же причина, по которой уровень 0
    // сам не знает своей цели без `cam_positions`, только для уровней
    // 1..=max_depth даже такой пост-детект записи нет). Использовать здесь
    // только [found, magnet] уровня 0, как раньше, — это НЕДОоценка: два
    // разных cam+recursion матча, чьи диски уровня 0 не пересекаются, но
    // чьи каскады (уровни 1..=max_depth) МОГУТ physически пересечься,
    // ошибочно считались бы точно (exact-cells) непересекающимися, даже
    // когда статический граф конфликтов (построенный по union дисков всех
    // уровней, см. `compute_rule_data`) уже верно завёл между ними ребро —
    // exact-path тогда молча занижал бы то, что conservative-path верно
    // расширил. Поэтому при наличии `recursion` используем ПОЛНЫЙ
    // консервативный `rule_data.write_cells` (union дисков всех уровней)
    // вместо точных 2 клеток — падаем в общий путь ниже, тот же приём, что
    // уже применяется к обычной (не-cam) рекурсии через `RuleData::write_cells`.
    let cam_has_recursion = matched_rule.is_some_and(|r| r.cam.is_some() && r.recursion.is_some());
    if !cam_has_recursion {
        if let Some(&(fx, fy)) = cam_positions.get(&(m.x, m.y, m.rule_idx)) {
            out.push((fx as i32, fy as i32));
            out.push((m.x as i32, m.y as i32));
            return;
        }
    }

    let rule_data = match get_rule_data(rule_cache, head, m.rule_idx) {
        Some(rd) => rd,
        None => {
            // Правило не найдено в кэше — в норме недостижимо, т.к.
            // rule_cache строится из того же rule_index, что передан сюда,
            // и содержит запись под каждый (head, rule_idx). Консервативный
            // фолбэк: длина id сработавшего правила (или 1, если и его не
            // найти) как ширина паттерна вдоль x, как строился bы паттерн
            // из id по умолчанию (см. `config::load_config`).
            let id_len = rule_index
                .get(&head)
                .and_then(|rules| rules.get(m.rule_idx))
                .map_or(1, |rule| rule.id.len().max(1));
            for i in 0..id_len {
                out.push((m.x as i32 + i as i32, m.y as i32));
            }
            return;
        }
    };

    let (w, h) = (bounds.0 as i32, bounds.1 as i32);
    let overflow = matched_rule.map(|rule| rule.overflow);

    // `Rule::feedback` (Лемма 4, `paper/paper4.md` §8, Corollary C): точное
    // (не union) множество для ЭТОГО КОНКРЕТНОГО матча — зависит от того,
    // защёлкнулся ли счётчик обратной связи, а не от `rule_data.write_cells`
    // (тот всегда union обоих направлений, годится только для статического
    // графа — см. её doc-комментарий). Та же логика раздельного пути, что
    // и у CAM выше, только не early-return: клэмпинг на границу решётки
    // нужен точно так же, как и в общем случае ниже.
    if let Some(spec) = matched_rule.and_then(|rule| rule.feedback) {
        let latched = feedback_counters.get(&(m.x, m.y, m.rule_idx)).copied().unwrap_or(0) >= spec.timeout;
        let (cells, direction) = if latched {
            (&rule_data.feedback_alt_write_cells, spec.new_direction)
        } else {
            let declared = matched_rule.and_then(|rule| rule.shifts.iter().flatten().next()).map(|s| s.direction);
            (&rule_data.feedback_normal_write_cells, declared.unwrap_or(spec.new_direction))
        };
        let target = direction_delta(direction);
        out.extend(cells.iter().map(|&(dx, dy)| clamp_shift_target(m, (dx, dy), (dx, dy) == target, overflow, w, h)));
        return;
    }

    // Правило с несколькими сдвигами реплицирует значение в КАЖДУЮ цель
    // независимо (см. RuleData::shift_targets) — клэмпинг при
    // OverflowAction::Write применим к любой из них, не только к первой.
    out.extend(rule_data.write_cells.iter().map(|&(dx, dy)| {
        clamp_shift_target(m, (dx, dy), rule_data.shift_targets.contains(&(dx, dy)), overflow, w, h)
    }));
}

/// (dx, dy) направления сдвига — та же таблица, что и везде в проекте
/// (`applicator::apply_shift_buffered`, `conflict_analyzer::shift_delta`).
pub(crate) fn direction_delta(direction: crate::types::Direction) -> (i32, i32) {
    use crate::types::Direction;
    match direction {
        Direction::Up => (0, -1),
        Direction::Down => (0, 1),
        Direction::Left => (-1, 0),
        Direction::Right => (1, 0),
    }
}

/// Клэмпинг одной относительной ячейки записи на границу решётки при
/// `OverflowAction::Write`/`WriteLiteral` — общая логика для обычного пути и
/// `Rule::feedback`'а, см. doc-комментарий `get_match_affected_cells`.
fn clamp_shift_target(m: &RuleMatch, (dx, dy): (i32, i32), is_shift_target: bool, overflow: Option<OverflowAction>, w: i32, h: i32) -> (i32, i32) {
    let abs = (m.x as i32 + dx, m.y as i32 + dy);
    if w > 0 && h > 0 && is_shift_target {
        if let Some(OverflowAction::Write(_) | OverflowAction::WriteLiteral(_)) = overflow {
            if abs.0 < 0 || abs.0 >= w || abs.1 < 0 || abs.1 >= h {
                return (abs.0.clamp(0, w - 1), abs.1.clamp(0, h - 1));
            }
        }
    }
    abs
}
