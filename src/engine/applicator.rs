use std::collections::HashMap;

use crate::conflict_analyzer::{get_rule_data, RuleDataCache};
use crate::engine::matcher::CamPositions;
use crate::fast_hash::FxHashMap;
use crate::grid::Grid;
use crate::storage::GridStorage;
use crate::types::{
    AffectedRegion, Cell, CellType, CellValue, ChangeValue, Direction, OverflowAction,
    RecordTrigger, RecordedValue, Rule, RuleMatch, ShiftSpec,
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
///
/// `pub(crate)`, не приватный — `Engine` (сессия 2026-08-09, список
/// "вопросы" п.4) держит один персистентный экземпляр как поле, вместо
/// того чтобы `apply_matches_with_cam` создавал `WriteBuffer::default()`
/// заново на каждый тик: микробенчмарк (generic `HashMap`, не
/// специфично для `FxHashMap`) показал ~58.6% экономии от переиспользования
/// `.clear()` вместо пересоздания на реалистичном объёме записей на тик.
pub(crate) type WriteBuffer = FxHashMap<(u32, u32), Cell>;

// Сессия 2026-08-09: `VecSink`/`WriteSink`-абстракция для safe-ветки
// ("путь (а)") реализована, дважды исправлена (dyn -> generics, лишняя
// аллокация в finalize) и в итоге ОТКАЧЕНА -- контролируемый A/B на реальном
// CF-сценарии (apply_matches_with_cam, тот же код, разница только в
// safe_len) показал РЕГРЕССИЮ (-19.9% даже после обоих исправлений, -66-104%
// в промежуточных вариантах), а не выигрыш. Причина: изначальный
// микробенчмарк, обосновавший идею (Vec+sort дешевле HashMap-вставки),
// использовал синтетические УЖЕ ОТСОРТИРОВАННЫЕ по координате данные
// (`for i in 0..N { push((i,0,...)) }`) -- лучший случай почти для любого
// сортировщика. Реальный порядок записи (порядок арбитража/детекта) не
// отсортирован, и настоящая O(N log N) стоимость сортировки на реальных
// данных перевешивает то, что экономится на отказе от хэширования. Урок тот
// же, что уже был найден в этой сессии для radix-сортировки в
// `arbitrator.rs`: не всякий "теоретически дешевле" вариант быстрее на
// практике — и синтетический микробенчмарк с нереалистичным порядком данных
// может убедительно, но ошибочно, подтвердить неверную гипотезу.

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
    // Свежие буферы на каждый вызов — эта свободная функция, как и
    // `run_tick`, не хранит состояния между вызовами (см. doc-комментарий
    // `detect_matches`); персистентные буферы есть только у `Engine`
    // (`Engine::write_buffer`/`Engine::pattern_buffer`), которые и
    // переиспользуются в горячем пути `run_tick_with_cache`.
    let mut write_buffer = WriteBuffer::default();
    let mut pattern_buffer = Vec::new();
    apply_matches_with_cam(
        grid,
        &matches,
        rule_index,
        rule_cache,
        &CamPositions::default(),
        &mut crate::engine::arbitrator::FeedbackCounters::default(),
        &mut crate::engine::arbitrator::MemoryBuffers::default(),
        &mut write_buffer,
        &mut pattern_buffer,
    )
}

/// Как [`apply_matches`], но с картой найденных CAM-позиций (см.
/// `matcher::detect_cam_matches`) и счётчиками обратной связи (см.
/// `Engine::feedback_counters`/`Rule::feedback`) — только `run_tick`/
/// `Engine::run_tick` имеют что туда передать (см. doc-комментарий
/// `Engine::detect_matches`), поэтому `apply_matches` остаётся публичным с
/// прежней сигнатурой и просто подставляет пустые карты. `feedback_counters`
/// — `&mut`, а не `&`: маркер физически ДВИГАЕТСЯ при сдвиге, так что запись
/// счётчика нужно ПЕРЕНОСИТЬ на новую позицию (см. doc-комментарий
/// `apply_shift_buffered`), а не только читать.
///
/// `matches: &[RuleMatch]`, не `Vec<RuleMatch>` (сессия 2026-08-09, список
/// "вопросы", продолжение) — `RuleMatch` `Copy` (13 байт), эта функция
/// никогда не нуждалась во владении, только в чтении; раньше сигнатура
/// требовала владения, из-за чего единственный вызывающий (`run_tick_with_cache`)
/// был вынужден передавать `accepted.clone()` -- полную копию `Vec<RuleMatch>`
/// на КАЖДЫЙ тик с принятыми матчами -- лишь потому, что `accepted` нужен
/// ещё раз ПОСЛЕ этого вызова (обновление бюджета активаций, `TickEventCounts`,
/// собственный возврат `run_tick_with_cache`). Слайс убирает клон целиком.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_matches_with_cam<S: GridStorage>(
    grid: &mut Grid<S>,
    matches: &[RuleMatch],
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &RuleDataCache,
    cam_positions: &CamPositions,
    feedback_counters: &mut crate::engine::arbitrator::FeedbackCounters,
    memory_buffers: &mut crate::engine::arbitrator::MemoryBuffers,
    write_buffer: &mut WriteBuffer,
    pattern_buffer: &mut Vec<CellValue>,
) -> (Vec<AffectedRegion>, Vec<(u32, Cell)>) {
    let mut pending_boundary: Vec<(u32, Cell)> = Vec::new();
    let mut regions: Vec<AffectedRegion> = Vec::new();
    // Общий буфер для всех match'ей этого тика — переданный вызывающей
    // стороной (см. doc-комментарий `WriteBuffer`), а не создаваемый здесь;
    // очищаем сами (не полагаемся на то, что вызывающая сторона это
    // сделала) — так же, как `rebuild_rule_cache` сам вызывается изнутри
    // `set_rule_index`, а не по соглашению снаружи.
    write_buffer.clear();
    // Массив-lookup вместо `rule_index.get()` — см. doc-комментарий
    // `HeadRuleIndex`/`build_head_index` (сессия 2026-08-09, "фантазия" п.1).
    let head_index = crate::engine::build_head_index(rule_index);

    for &m in matches {
        // Резолвим по rule_idx, а не поиском по id: несколько правил могут
        // иметь одинаковую голову (недетерминированный выбор), и только
        // rule_idx однозначно определяет, какое именно правило сработало
        // для этого match'а.
        //
        // Без `.cloned()`: `apply_rule_buffered` использует правило только по
        // ссылке, а клонирование целого `Rule` (вложенные `Vec` в pattern/
        // changes/shifts/id) на каждый match каждого тика было чистой тратой.
        let rule = crate::engine::lookup_rule(&head_index, m.head, m.rule_idx);
        if let Some(rule) = rule {
            if let Some(cam) = rule.cam {
                let gen = grid.generation();
                let region = apply_cam_buffered(&*grid, &m, rule, cam, cam_positions, gen, write_buffer);
                regions.push(region);
                continue;
            }
            let rule_data = get_rule_data(rule_cache, m.head, m.rule_idx);
            // Пишем сразу в общий буфер, а не в свой локальный внутри
            // `apply_rule_buffered` с последующим `extend` — arbitration уже
            // гарантирует отсутствие пересечения записей между matches, так
            // что объединять нечего, а лишний HashMap на каждый match — это
            // лишняя аллокация и хэширование на пустом месте.
            let (region, outputs) = apply_rule_buffered(grid, &m, rule, rule_data, write_buffer, feedback_counters, memory_buffers, pattern_buffer);
            regions.push(region);
            pending_boundary.extend(outputs);
        }
    }

    // Фаза flush: атомарно применяем все изменения из буфера в решётку.
    // `.drain()`, не `.into_iter()` (`write_buffer` теперь `&mut`, не
    // владеющий) — попутно оставляет буфер пустым сразу после использования,
    // а не только при следующем `.clear()` в начале функции.
    let gen = grid.generation();
    for ((x, y), cell) in write_buffer.drain() {
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

/// Применить CAM-матч (`types::CamSearch`) с буферизацией — притягивает
/// значение найденной клетки на позицию магнита: найденная клетка
/// становится дефолтной (очищается), магнит получает `cam.target_type`.
/// Без `rule.recursion` — ровно 2 записанные клетки, всегда, не зависит от
/// `rule.changes`/`shifts` (CAM-правило их не имеет, см. валидацию в
/// `config.rs`).
///
/// С `rule.recursion` (см. Corollary D в `conflict_analyzer::compute_rule_data`'s
/// doc-комментарий и `paper/paper4.md` §8/§9): после притяжения уровня 0
/// (выше) — цепочка НЕЗАВИСИМЫХ магнитов вдоль `recursion.direction`,
/// каждый на фиксированном смещении `k × direction` от исходного матча,
/// `k = 1..=max_depth`. Уровень продолжается, только если клетка на его
/// позиции имеет тип головы правила (`rule.id[0]`, гарантированно есть —
/// у cam-правила `pattern` пуст по валидации, значит `id` не пуст по общей
/// валидации "id or pattern must not be empty") И в её собственном диске
/// радиуса `cam.radius` находится `cam.target_type` — иначе каскад
/// останавливается на первом же несовпадении, та же граница, что и у
/// обычной (не-cam) рекурсии. Чтение — ЭФФЕКТИВНОЕ (`read_cell_effective`/
/// `search_nearest_effective`, учитывает уже накопленный этим же каскадом
/// `write_buffer`), не устаревший срез "до тика" — по той же причине, что и
/// у обычной рекурсии (см. её doc-комментарий в `types.rs`): это ЕДИНСТВЕННЫЙ
/// способ не дать двум соседним уровням каскада независимо "найти" и
/// притянуть одну и ту же физическую клетку дважды (без эффективного чтения
/// уровень k+1 не увидел бы, что уровень k уже очистил найденную клетку).
fn apply_cam_buffered<S: GridStorage>(
    grid: &Grid<S>,
    m: &RuleMatch,
    rule: &Rule,
    cam: crate::types::CamSearch,
    cam_positions: &CamPositions,
    gen: u64,
    write_buffer: &mut WriteBuffer,
) -> AffectedRegion {
    // `cam_positions` ОБЯЗАНА содержать запись для КАЖДОГО CAM-матча,
    // дошедшего до `apply` (заполняется в `detect_cam_matches` синхронно с
    // самим `RuleMatch`, см. её doc-комментарий) — падаем явно, а не
    // подставляем (magnet, magnet) молча: такой fallback раньше маскировал
    // реальный баг (CAM-правило дублировалось через обычный `match_cell`,
    // не только через `detect_cam_matches` — см. doc-комментарий
    // `GroupData::rule_meta`) — молчаливая подстановка "нашли самого себя"
    // заставляла магнит без цели в радиусе всё равно "успешно" становиться
    // target_type, найдено настоящим тестом (`test_cam_target_outside_radius_no_match`).
    let (fx, fy) = *cam_positions.get(&(m.x, m.y, m.rule_idx)).expect(
        "apply_cam_buffered: matched CAM RuleMatch without a corresponding cam_positions entry — \
         detect_cam_matches and match_cell have gone out of sync (see GroupData::rule_meta doc-comment)",
    );
    let magnet = (m.x, m.y);
    let found = (fx, fy);

    write_buffer.insert(found, Cell { value: CellValue::default(), born_at: gen });
    write_buffer.insert(magnet, Cell { value: CellValue(cam.target_type), born_at: gen });

    let mut xs = vec![found.0, magnet.0];
    let mut ys = vec![found.1, magnet.1];
    let mut written_cells = vec![found, magnet];

    if let Some(spec) = rule.recursion {
        // Гарантировано валидацией `config::load_config`: `cam` требует
        // пустой `pattern`, а общая валидация требует непустым хотя бы один
        // из `id`/`pattern` — значит `id` здесь непуст.
        let head_type = rule.id.first().copied().expect(
            "apply_cam_buffered: cam rule with empty `pattern` must have non-empty `id` (see config::load_config validation)",
        );
        let (ddx, ddy) = match spec.direction {
            Direction::Up => (0, -1),
            Direction::Down => (0, 1),
            Direction::Left => (-1, 0),
            Direction::Right => (1, 0),
        };
        let mut cx = m.x as i32;
        let mut cy = m.y as i32;
        for _ in 1..=spec.max_depth {
            cx += ddx;
            cy += ddy;
            if cx < 0 || cy < 0 {
                break;
            }
            if read_cell_effective(grid, write_buffer, cx, cy).0 != head_type {
                break;
            }
            let Some((nfx, nfy)) = search_nearest_effective(grid, write_buffer, cx, cy, cam.radius, cam.target_type)
            else {
                break;
            };
            let level_magnet = (cx as u32, cy as u32);
            let level_found = (nfx as u32, nfy as u32);
            write_buffer.insert(level_found, Cell { value: CellValue::default(), born_at: gen });
            write_buffer.insert(level_magnet, Cell { value: CellValue(cam.target_type), born_at: gen });
            xs.push(level_found.0);
            xs.push(level_magnet.0);
            ys.push(level_found.1);
            ys.push(level_magnet.1);
            written_cells.push(level_found);
            written_cells.push(level_magnet);
        }
    }

    AffectedRegion {
        x_start: *xs.iter().min().expect("xs always has at least the level-0 pair, never empty"),
        x_end: *xs.iter().max().expect("xs always has at least the level-0 pair, never empty"),
        y_start: *ys.iter().min().expect("ys always has at least the level-0 pair, never empty"),
        y_end: *ys.iter().max().expect("ys always has at least the level-0 pair, never empty"),
        has_changes: false,
        written_cells,
    }
}

/// Ближайшая клетка типа `target` в Chebyshev-радиусе `radius` вокруг
/// (cx, cy), читая ЭФФЕКТИВНОЕ состояние (`read_cell_effective` — grid плюс
/// уже накопленный `write_buffer` этого же тика) — вариант
/// `matcher::search_nearest` для каскада `cam`+`recursion` (см.
/// `apply_cam_buffered`'s doc-комментарий), которому, в отличие от обычного
/// CAM-детекта, нужно видеть изменения, сделанные более ранними уровнями
/// ТОГО ЖЕ каскада. Тай-брейк идентичен `search_nearest`: минимум по
/// (dist, y, x).
fn search_nearest_effective<S: GridStorage>(
    grid: &Grid<S>,
    write_buffer: &WriteBuffer,
    cx: i32,
    cy: i32,
    radius: u8,
    target: CellType,
) -> Option<(i32, i32)> {
    let r = radius as i32;
    let mut best: Option<(i32, i32, i32)> = None; // (dist, y, x) — минимум лексикографически
    for dy in -r..=r {
        let ny = cy + dy;
        if ny < 0 {
            continue;
        }
        for dx in -r..=r {
            let nx = cx + dx;
            if nx < 0 || (nx, ny) == (cx, cy) {
                continue;
            }
            if read_cell_effective(grid, write_buffer, nx, ny).0 != target {
                continue;
            }
            let dist = dx.abs().max(dy.abs());
            let candidate = (dist, ny, nx);
            if best.is_none_or(|b| candidate < b) {
                best = Some(candidate);
            }
        }
    }
    best.map(|(_, y, x)| (x, y))
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
#[allow(clippy::too_many_arguments)]
fn apply_rule_buffered<S: GridStorage>(
    grid: &mut Grid<S>,
    m: &RuleMatch,
    rule: &Rule,
    rule_data: Option<&crate::conflict_analyzer::RuleData>,
    write_buffer: &mut WriteBuffer,
    feedback_counters: &mut crate::engine::arbitrator::FeedbackCounters,
    memory_buffers: &mut crate::engine::arbitrator::MemoryBuffers,
    pattern_buffer: &mut Vec<CellValue>,
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

    // Буфер значений паттерна для динамических ссылок ($0, $1, ...) —
    // переданный вызывающей стороной (`Engine::pattern_buffer`, п.3 в
    // "третьей оптимизации того же класса", сессия 2026-08-09), а не
    // `Vec::new()` заново на каждый match: раньше это была свежая аллокация
    // на КАЖДЫЙ match с непустым `pattern` (правила с пустым pattern,
    // например чистые сдвиги без чтения соседей, эту цену не платили —
    // `Vec::new()` сам по себе не аллоцирует, пока в него не запушили).
    pattern_buffer.clear();
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
    //
    // `Rule::feedback` (см. её doc-комментарий и Лемму 4, `paper/paper4.md`
    // §8): если счётчик для ЭТОГО конкретного матча уже защёлкнут (достиг
    // `timeout`), реальный применённый сдвиг — `new_direction`, а не
    // декларированное направление. Правило с `feedback` гарантированно
    // имеет РОВНО один сдвиг (см. валидацию в `config::load_config`), так
    // что подмена направления применима к единственной итерации цикла ниже.
    let feedback_override = rule.feedback.and_then(|spec| {
        let latched = feedback_counters.get(&(m.x, m.y, m.rule_idx)).copied().unwrap_or(0) >= spec.timeout;
        latched.then_some(spec.new_direction)
    });
    // Инкремент защёлки — СРАЗУ ПОСЛЕ чтения выше, ДО применения сдвига
    // ниже. Обязательно именно здесь, а не отдельным проходом ПОСЛЕ всего
    // apply (как для starvation_after): `apply_shift_buffered` следующим
    // шагом может РЕЛОЦИРОВАТЬ эту же запись на новую позицию (remove
    // старого ключа + insert нового) — если бы инкремент шёл отдельным
    // проходом позже по СТАРОЙ позиции этого матча, для ВЫИГРАВШИХ и
    // сдвинувшихся матчей он создавал бы ПОСТОРОННЮЮ свежую запись с 0 на
    // уже покинутой позиции вместо инкремента РЕАЛЬНОЙ, уже перенесённой
    // записи — защёлка никогда не достигала бы `timeout` (найдено
    // эмпирически: маркер уезжал за край решётки, ни разу не переключившись).
    // Проигравшие арбитраж матчи сюда не попадают (эта функция вызывается
    // только для `accepted`) — они инкрементируются отдельно, в
    // `run_tick_with_cache`, по своей (гарантированно НЕ релоцированной,
    // раз apply для них не запускался) позиции.
    if rule.feedback.is_some() {
        let counter = feedback_counters.entry((m.x, m.y, m.rule_idx)).or_insert(0);
        *counter = counter.saturating_add(1);
    }
    for shift_group in &rule.shifts {
        for shift in shift_group {
            let effective_shift = match feedback_override {
                Some(direction) => ShiftSpec { direction, ..*shift },
                None => *shift,
            };
            apply_shift_buffered(
                grid, cx, cy, &effective_shift, rule, m.rule_idx, feedback_counters, memory_buffers, &mut affected,
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
            apply_changes_at(rule, pattern_buffer.as_slice(), cx, cy, (0, 0), grid, gen, write_buffer, &mut affected);
        } else {
            for &target in shift_targets {
                apply_changes_at(rule, pattern_buffer.as_slice(), cx, cy, target, grid, gen, write_buffer, &mut affected);
            }
        }
    }

    // Фаза 3: `Rule::recursion` — каскад ВНУТРИ ОДНОГО тика (см. её
    // doc-комментарий и Лемму 4, `paper/paper4.md` §8, Corollary B).
    // Каждый уровень k=1..=max_depth заново проверяет ТОТ ЖЕ `pattern`,
    // сдвинутый на k×direction, читая уже НАКОПЛЕННЫЙ этим же каскадом
    // `write_buffer` (не устаревший срез grid "до тика") — единственное
    // осознанное исключение из общего правила detect-читает-pre-tick,
    // ограниченное ОДНИМ выигравшим арбитраж матчем (см. её doc-комментарий
    // в `types.rs`). Останавливается на первом уровне, где паттерн не
    // совпал — естественная граница каскада, как и у заливки.
    if let Some(spec) = rule.recursion {
        let (ddx, ddy) = match spec.direction {
            Direction::Up => (0, -1),
            Direction::Down => (0, 1),
            Direction::Left => (-1, 0),
            Direction::Right => (1, 0),
        };
        for k in 1..=spec.max_depth as i32 {
            let ox = cx + ddx * k;
            let oy = cy + ddy * k;
            if !pattern_matches_effective(grid, write_buffer, &rule.pattern, ox, oy, rule.min_age, gen) {
                break;
            }
            // `memory` + `recursion` (только `NeighborType` — см.
            // `config.rs`'s валидацию: `RuleOutcome` остаётся запрещённым,
            // семантика "Applied/Missed" для уровня каскада, у которого нет
            // отдельного арбитража, попросту не определена). Уровень k —
            // САМОСТОЯТЕЛЬНАЯ позиция со своим собственным буфером (ключ
            // `(ox, oy, rule_idx)`, НЕ позиция исходного матча) — та же
            // клетка на будущих тиках может независимо стать top-level
            // матчем и продолжить ТОТ ЖЕ буфер (см. push-логику в
            // `engine/mod.rs`, тот же ключ). Гейт проверяется по буферу
            // КАК ОН БЫЛ до этого тика (та же семантика, что и у top-level
            // матчей), затем — НОВОЕ наблюдение пушится безусловно (буфер
            // продолжает наблюдать даже когда гейт закрыт), и только ПОСЛЕ
            // ЭТОГО закрытый гейт останавливает каскад (тот же класс
            // границы, что и несовпавший pattern/min_age выше).
            if let Some(mem_spec) = &rule.memory {
                if let RecordTrigger::NeighborType(dir) = mem_spec.record_trigger {
                    let key = (ox as u32, oy as u32, m.rule_idx);
                    let gate_open = memory_buffers
                        .get(&key)
                        .is_some_and(|buf| buf.len() == mem_spec.window && buf.iter().eq(mem_spec.match_pattern.iter()));
                    let (ndx, ndy) = crate::engine::arbitrator::direction_delta(dir);
                    let neighbor_type = read_cell_effective(grid, write_buffer, ox + ndx, oy + ndy).0;
                    let buf = memory_buffers.entry(key).or_default();
                    buf.push_back(RecordedValue::Type(neighbor_type));
                    while buf.len() > mem_spec.window {
                        buf.pop_front();
                    }
                    if !gate_open {
                        break;
                    }
                }
            }
            let level_pattern_buffer = read_pattern_buffer_effective(grid, write_buffer, &rule.pattern, ox, oy);
            apply_changes_at(rule, &level_pattern_buffer, ox, oy, (0, 0), grid, gen, write_buffer, &mut affected);
        }
    }

    (affected, pending_outputs)
}

/// Прочитать одну ячейку, учитывая уже накопленный `write_buffer` (если
/// клетка уже записана ЭТИМ ЖЕ тиком — вернуть её, иначе честно прочитать
/// grid) — используется ТОЛЬКО каскадом `Rule::recursion`, см. её
/// doc-комментарий про единственное осознанное исключение из
/// detect-читает-pre-tick.
fn read_cell_effective<S: GridStorage>(grid: &Grid<S>, write_buffer: &WriteBuffer, x: i32, y: i32) -> CellValue {
    if x < 0 || y < 0 {
        return CellValue::default();
    }
    if let Some(cell) = write_buffer.get(&(x as u32, y as u32)) {
        return cell.value;
    }
    grid.get_cell(x as usize, y as usize).map(|c| c.value).unwrap_or_default()
}

/// Вычислить возраст ОДНОЙ ячейки, учитывая уже накопленный `write_buffer` —
/// аналог `read_cell_effective`, но для возраста, а не значения. Используется
/// ТОЛЬКО каскадом `Rule::recursion`, чтобы каждый уровень каскада мог
/// применить `rule.min_age` к СВОЕЙ клетке-анкеру `(ox, oy)`, той же ролью,
/// которую `center_age`/`(cx, cy)` играют в обычном `matcher::match_cell`.
///
/// Если клетка уже записана этим же тиком (есть в `write_buffer`), её
/// эффективный `born_at` — РОВНО `gen`: и `apply_changes_at`/
/// `apply_shift_buffered`/`apply_overflow_write`/`apply_cam_buffered` (Фазы
/// 1-2 и предыдущие уровни каскада), и финальный flush в
/// `apply_matches_with_cam` пишут `born_at: gen` для любой записи буфера —
/// значит `gen - born_at == 0` ВСЕГДА для буферных записей. Это не костыль, а
/// ровно то же самое, что `Grid::get_age` вернула бы для этой клетки, если
/// бы её прочитали из решётки СРАЗУ после `set_cell` в этот же тик (до
/// `advance_age`) — клетка, которую каскад только что создал/очистил в этом
/// тике, имеет возраст 0 для следующего уровня, точно как любая другая
/// свежезаписанная клетка имеет возраст 0 до конца текущего тика. Не
/// переизобретаем `is_default`-ветку `Grid::get_age` собственной логикой —
/// копируем её дословно на случай, если будущий код когда-нибудь положит в
/// буфер запись с born_at ≠ gen.
fn read_age_effective<S: GridStorage>(grid: &Grid<S>, write_buffer: &WriteBuffer, x: i32, y: i32, gen: u64) -> u64 {
    if x < 0 || y < 0 {
        return 0;
    }
    if let Some(cell) = write_buffer.get(&(x as u32, y as u32)) {
        return if cell.is_default() { 0 } else { gen.saturating_sub(cell.born_at) };
    }
    grid.get_age(x as usize, y as usize)
}

/// Проверить, совпадает ли `pattern` (тот же список смещений, что и у
/// исходного матча) в позиции `(ox, oy)`, читая эффективное (с учётом
/// каскада) состояние — см. `read_cell_effective` — И удовлетворяет ли эта
/// позиция-анкер `min_age` (см. `read_age_effective`). `(ox, oy)` играет ТУ
/// ЖЕ роль, что `(cx, cy)` играет в `matcher::match_cell`: обычный путь
/// проверяет `min_age` ОДИН РАЗ, против возраста клетки-анкера самого матча,
/// а не против каждой клетки паттерна по отдельности — здесь то же самое,
/// только анкер каждого уровня каскада — это `(ox, oy)`, а не исходные
/// `(cx, cy)` матча.
fn pattern_matches_effective<S: GridStorage>(
    grid: &Grid<S>,
    write_buffer: &WriteBuffer,
    pattern: &[(i8, i8, CellType)],
    ox: i32,
    oy: i32,
    min_age: u64,
    gen: u64,
) -> bool {
    if min_age > 0 && read_age_effective(grid, write_buffer, ox, oy, gen) < min_age {
        return false;
    }
    pattern.iter().all(|&(dx, dy, expected)| read_cell_effective(grid, write_buffer, ox + dx as i32, oy + dy as i32).0 == expected)
}

/// Пересобрать буфер значений паттерна (для `$N`-ссылок в `changes`) для
/// ОДНОГО уровня каскада — та же логика, что и исходный `pattern_buffer` в
/// `apply_rule_buffered`, но с эффективным (с учётом каскада) чтением и
/// относительно НОВОЙ (сдвинутой) позиции этого уровня, а не исходного матча.
fn read_pattern_buffer_effective<S: GridStorage>(
    grid: &Grid<S>,
    write_buffer: &WriteBuffer,
    pattern: &[(i8, i8, CellType)],
    ox: i32,
    oy: i32,
) -> Vec<CellValue> {
    pattern.iter().map(|&(dx, dy, _)| read_cell_effective(grid, write_buffer, ox + dx as i32, oy + dy as i32)).collect()
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
/// Читает головку из grid (старое значение), пишет очистку (0,0) — если не
/// `shift.keep_source` (см. её doc-комментарий, "излучение") — и запись в
/// (nx,ny) в буфер.
#[allow(clippy::too_many_arguments)]
fn apply_shift_buffered<S: GridStorage>(
    grid: &mut Grid<S>,
    cx: i32,
    cy: i32,
    shift: &ShiftSpec,
    rule: &Rule,
    rule_idx: usize,
    feedback_counters: &mut crate::engine::arbitrator::FeedbackCounters,
    memory_buffers: &mut crate::engine::arbitrator::MemoryBuffers,
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

    // Перенос счётчиков feedback/memory на новую позицию — ТОЛЬКО когда
    // источник реально освобождается (`!shift.keep_source`). С
    // `keep_source: true` ("излучение") исходная клетка НЕ исчезает: она
    // по-прежнему тот же самый маркер на той же позиции и должна сохранить
    // СВОЙ счётчик как есть — перенос (remove+insert) стёр бы его историю,
    // хотя оригинал физически никуда не делся. Новая позиция при этом
    // получает копию ЗНАЧЕНИЯ клетки, но не копию счётчика — это НОВЫЙ,
    // независимый экземпляр, его собственный счётчик естественно начнётся с
    // нуля на следующем тике (обычный `.entry(key).or_insert(0)` в
    // пост-арбитражном апдейте), а не унаследует историю оригинала.
    if !shift.keep_source {
        // `Rule::feedback`: счётчик отслеживает "сколько тиков подряд ЭТОТ
        // конкретный маркер пытается", а не "сколько тиков подряд что-то
        // происходило в ЭТОЙ клетке" — маркер физически ДВИГАЕТСЯ каждый
        // тик (в отличие от `starvation_after`, где матч перепроверяет ОДНУ
        // и ту же клетку), так что запись нужно ПЕРЕНЕСТИ на новую позицию
        // вместе со сдвигом, а не оставлять сиротой на старой (там счётчик
        // просто исчезнет из карты) и стартовать с нуля на новой. Перенос —
        // независимо от того, какое направление сейчас эффективно
        // применяется (обычное или `new_direction`): счётчик всегда следует
        // за физическим положением головки.
        if rule.feedback.is_some() && nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
            if let Some(val) = feedback_counters.remove(&(ox as u32, oy as u32, rule_idx)) {
                feedback_counters.insert((nx as u32, ny as u32, rule_idx), val);
            }
        }

        // `Rule::memory`: тот же перенос и по той же причине, что и у
        // `feedback` выше — буфер наблюдений отслеживает КОНКРЕТНЫЙ маркер,
        // а не клетку; правило с `memory` и сдвигом (0 или 1, см. валидацию
        // в `config::load_config`) физически двигается, так что запись
        // нужно перенести на новую позицию, иначе буфер обнулялся бы
        // (терялся из карты) на каждом шагу — тот же класс бага, что уже
        // найден для `feedback` (см. её doc-комментарий).
        if rule.memory.is_some() && nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
            if let Some(val) = memory_buffers.remove(&(ox as u32, oy as u32, rule_idx)) {
                memory_buffers.insert((nx as u32, ny as u32, rule_idx), val);
            }
        }

        // Include original position in affected region
        affected.x_start = affected.x_start.min(ox as u32);
        affected.x_end = affected.x_end.max(ox as u32 + 1);
        affected.y_start = affected.y_start.min(oy as u32);
        affected.y_end = affected.y_end.max(oy as u32 + 1);

        // Clear original position (write to buffer, not grid)
        write_buffer.insert((ox as u32, oy as u32), Cell::default());
        affected.written_cells.push((ox as u32, oy as u32));
    }

    if !shift.broadcast {
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
        return;
    }

    // Broadcast (см. `ShiftSpec::broadcast`): КАЖДАЯ клетка пути (k=1..steps)
    // получает копию значения головки, не только финальная (nx,ny). Путь
    // монотонен (фиксированное направление, фиксированный шаг) — как только
    // k-я позиция выходит за границу решётки, ВСЕ последующие тоже вне
    // границ, значит `overflow` применяется РОВНО ОДИН РАЗ в точке выхода,
    // дальше цикл останавливается — не "теряем" уже записанные клетки пути
    // до этой точки, просто не продолжаем размножение за край.
    for k in 1..=steps {
        let px = ox + dx * k;
        let py = oy + dy * k;
        if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
            write_buffer.insert((px as u32, py as u32), head_cell);
            affected.written_cells.push((px as u32, py as u32));
            affected.x_start = affected.x_start.min(px as u32);
            affected.x_end = affected.x_end.max(px as u32 + 1);
            affected.y_start = affected.y_start.min(py as u32);
            affected.y_end = affected.y_end.max(py as u32 + 1);
        } else {
            let bx = if px < 0 { 0 } else { (px as usize).min(w.saturating_sub(1)) };
            let by = if py < 0 { 0 } else { (py as usize).min(h.saturating_sub(1)) };
            match rule.overflow {
                OverflowAction::Discard => {
                    // head cell is lost at this point of the path
                }
                OverflowAction::Write(value) => {
                    let output_value = if value != 0 { Some(value) } else { None };
                    apply_overflow_write(grid, bx, by, output_value, head_cell, gen, write_buffer, affected, pending_outputs);
                }
                OverflowAction::WriteLiteral(value) => {
                    apply_overflow_write(grid, bx, by, Some(value), head_cell, gen, write_buffer, affected, pending_outputs);
                }
            }
            break; // монотонный путь — дальше тоже вне границ
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