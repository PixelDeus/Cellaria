//! CPU-сторона: превращает `HashMap<CellType, Vec<Rule>>` в плоскую таблицу,
//! пригодную для загрузки в GPU-буферы (`wgpu::Buffer`).
//!
//! Поддерживаемое подмножество (см. план): без `ChangeValue::Ref` и без
//! `OverflowAction::Write`/`WriteLiteral` (боковой ввод-вывод через
//! `BoundaryBuffer` остаётся CPU-only — отдельная задача поверх рабочего
//! базового движка) — но ТЕПЕРЬ, в отличие от первой версии, СО сдвигами и
//! `changes` на произвольном смещении (не только self). Любое правило вне
//! подмножества делает ВЕСЬ конфиг непригодным для GPU целиком, а не только
//! само это правило — `GpuEngine` должен либо точно воспроизводить
//! CPU-семантику конфига, либо отказаться строиться над ним (см.
//! [`GpuUnsupportedReason`]), а не молча отбрасывать часть правил.
//!
//! [`GpuRuleTable::needs_arbitration`] говорит `GpuEngine`, какой пайплайн
//! использовать: если НИ ОДНО правило конфига не имеет сдвигов и КАЖДОЕ
//! `changes` пишет только в саму клетку (0,0) — как классический Game of
//! Life — конфликтов записи между потоками в принципе быть не может (каждый
//! поток пишет только свою собственную клетку), и однопроходный self-write
//! шейдер (`shader.wgsl::main`, см. её doc-комментарий) достаточен и
//! быстрее. Иначе (есть хоть один сдвиг или запись в соседа) нужен полный
//! многораундовый арбитраж (`shader.wgsl::claim_pass`/`resolve_pass`).
//!
//! Без упаковки паттерна в u128 (в отличие от `engine::matcher::GroupData`,
//! `packed_patterns`) — в WGSL нет 128-битного целого, а корректность здесь
//! приоритетнее повторения CPU-оптимизаций; шейдер сравнивает паттерн
//! офсет-за-офсетом (что и так `O(len)` ≤ `MAX_PATTERN_OFFSETS`).

use std::collections::HashMap;

use crate::fast_hash::FxHashSet;
use crate::types::{CellType, ChangeValue, Direction, OverflowAction, RecordTrigger, RecordedValue, Rule};

/// Тот же потолок, что у `matcher::GroupData::packed_patterns` (см. её
/// doc-комментарий) — сохраняем для единообразия, хотя сама упаковка здесь
/// не используется.
pub const MAX_PATTERN_OFFSETS: usize = 16;

/// Потолок длины `Rule::id`, участвующей в тай-брейке арбитража на GPU (см.
/// `GpuRule::id_b0`). Правила длиннее — вне подмножества: длинные id на
/// практике не встречаются (см. аналогичный комментарий у
/// `arbitrator::RuleIdKey::Small`), а раздувать тай-брейк-ключ под
/// гипотетический случай — не оправдано.
pub const MAX_ID_BYTES: usize = 8;

/// Потолок числа НЕЗАВИСИМЫХ сдвигов на правило (`rule.shifts.iter().flatten()`
/// — вложенность в группы, как и на CPU, не влияет на применение, см.
/// `RuleData::shift_targets`). 2 сдвига (реплицировать в 2 направления
/// одновременно) покрывают подавляющее большинство реальных правил
/// (`shuttle_memory`, wireworld и т.п. используют максимум 1); больше — вне
/// подмножества.
pub const MAX_SHIFTS: usize = 2;

/// Потолок числа `changes` на правило — та же логика, что и `MAX_SHIFTS`.
pub const MAX_CHANGES: usize = 4;

/// Потолок |dx|/|dy| одного сдвига (`ShiftSpec::steps`, спроецированный на
/// ось направления).
pub const MAX_SHIFT_REACH: i32 = 12;

/// Потолок |dx|/|dy| одного `changes`-смещения.
pub const MAX_CHANGE_REACH: i32 = 4;

/// Потолок `ShiftSpec::steps` СПЕЦИФИЧНО для `broadcast: true` — отдельный
/// (более узкий), НЕ равный [`MAX_SHIFT_REACH`], константа для обычных
/// (не-broadcast) сдвигов. Почему отдельный: broadcast-путь пишет ВСЕ
/// промежуточные клетки (`k=1..steps`), не только конечную точку (см.
/// doc-комментарий `ShiftSpec` в `types.rs` и `applicator::apply_shift_buffered`),
/// значит число ячеек записи ОДНОГО broadcast-сдвига растёт линейно с
/// `steps`, а не константно (=1), как у обычного сдвига. [`MAX_WRITE_CELLS`]
/// ниже — потолок ячеек записи ОДНОГО матча — это compile-time размер
/// массива `GpuMatch::cells`/`values` в `shader.wgsl` (WGSL требует
/// константный размер массива внутри структуры), общий для ВСЕХ матчей
/// ВСЕХ конфигов сразу (шейдер компилируется ОДИН раз статически, см.
/// `GpuEngine::init`'s `include_str!` — не перекомпилируется под
/// конкретный `GpuRuleTable`, в отличие от `margin`/`max_matches_per_cell`,
/// которые всего лишь границы циклов/размеры буферов через uniform, а не
/// размеры полей структуры). Значит цена любого увеличения
/// `MAX_WRITE_CELLS` платится БЕЗУСЛОВНО КАЖДЫМ arbitrated-конфигом, даже
/// не использующим broadcast вовсе — если бы broadcast использовал тот же
/// потолок, что обычный сдвиг (12), `MAX_WRITE_CELLS` пришлось бы поднять
/// до 1+2*(12+4)=33 (рост в 3× от текущих 11) ради свойства, которым
/// пользуется меньшинство конфигов. 4 — тот же порядок величины, что и
/// [`MAX_CHANGES`]/[`MAX_SHIFTS`] в этом файле (скромный, но достаточный
/// для подавляющего большинства реальных сценариев — "луч"/"провод"
/// длиной несколько клеток), даёт `MAX_WRITE_CELLS`=17 (рост всего 55% от
/// 11) — сценарии длиннее 4 клеток вне подмножества (см.
/// `GpuUnsupportedReason::BroadcastPathTooLong`), а не молча урезаются.
pub const MAX_BROADCAST_REACH: i32 = 4;

/// Потолок радиуса `CamSearch` на GPU — в отличие от CPU (`CamSearch::radius:
/// u8`, до 255, без искусственного потолка), GPU-версия сканирует диск
/// (2R+1)² на клетку-кандидата КАЖДЫЙ тик (см. `shader.wgsl::cam_search`) и
/// раздувает дополненную сетку арбитража на `2×radius` по каждой оси (тот же
/// механизм, что и `margin` для сдвигов) — оба растут квадратично/линейно с
/// R, так что здесь нужен реальный потолок, а не только желание. 16 — вдвое
/// больше `MAX_SHIFT_REACH`, разумный запас для "найди ближайшую цель
/// поблизости" сценариев без превращения диска в скан половины решётки.
pub const MAX_CAM_RADIUS: u8 = 16;

/// Потолок `RecursionSpec::max_depth` на GPU. `Rule::recursion` ПОДДЕРЖИВАЕТСЯ
/// (см. `GpuUnsupportedReason::RecursionDepthTooLarge` вместо старого
/// блэнкет-отказа) — ключевое наблюдение: КАЖДЫЙ уровень каскада `k =
/// 1..=max_depth` читает клетку на СТАТИЧЕСКИ известном смещении `k ×
/// direction` от исходного матча и пишет свои `changes` относительно НЕЁ,
/// используя эффективное чтение ТОЛЬКО клеток, УЖЕ ЗАПИСАННЫХ этим же
/// каскадом (см. `read_cell_effective_local`/`read_age_effective_local` в
/// `shader.wgsl`, зеркалящих CPU `applicator::read_cell_effective`/
/// `read_age_effective`) — НИКОГДА клеток, записанных ДРУГИМ потоком/матчем
/// (по построению: `recursion` требует пустые `shifts`, `changes`-цели
/// каждого уровня уже включены в консервативную статическую границу
/// `conflict_analyzer::compute_rule_data`'s recursion-ветку, так что любой
/// РЕАЛЬНО конфликтующий чужой матч был бы исключён арбитражем ДО того, как
/// этот каскад вообще начал выполняться). Значит ВЕСЬ каскад одного матча —
/// чисто ЛОКАЛЬНОЕ вычисление ОДНОГО потока (ровно как путь broadcast-сдвига,
/// см. [`MAX_BROADCAST_REACH`]), без какой-либо межпоточной синхронизации —
/// именно ЭТО делает `recursion` GPU-совместимым, в отличие от `feedback`/
/// `memory`/`starvation_after` (требуют персистентное МЕЖДУ тиками
/// CPU-состояние, которого у `GpuEngine` в принципе нет).
///
/// 4 — тот же выбор, что и [`MAX_BROADCAST_REACH`] (см. её doc-комментарий):
/// скромный, но достаточный для подавляющего большинства сценариев каскада,
/// не раздувающий [`MAX_WRITE_CELLS`] сверх необходимого. Более глубокие
/// каскады — вне подмножества, явный отказ, а не молчаливое усечение.
pub const MAX_RECURSION_DEPTH: u8 = 4;

/// Потолок `MemorySpec::window` на GPU. Как и `memory_pattern0..3` в
/// [`GpuRule`] (плоские поля, не массив — см. `GpuRule`'s doc-комментарий
/// про ограничение naga "may only be indexed by a constant" для значений),
/// сам буфер-в-персистентном-storage (`shader.wgsl`'s `memory_buffers`
/// binding) устроен как `array<atomic<u32>>`, индексируемый ОДНИМ плоским
/// индексом `m * MAX_MEMORY_WINDOW + i` — это НЕ поле-массив внутри
/// значения, загруженного динамическим индексом (тот случай, который
/// действительно запрещён), а прямая индексация top-level storage-массива,
/// тот же паттерн, что уже используют `matches[m]`/`starvation_counters[m]`
/// — так что переменная длина здесь ограничивается только явным потолком
/// (не техническим ограничением WGSL). 4 — тот же выбор, что и
/// [`MAX_CHANGES`]/[`MAX_RECURSION_DEPTH`] (скромный, но достаточный для
/// подавляющего большинства реальных последовательностей-триггеров).
pub const MAX_MEMORY_WINDOW: usize = 4;

/// Максимальная дистанция (по любой оси) от клетки-источника матча до
/// ЛЮБОЙ клетки, которую он потенциально затрагивает для целей арбитража
/// (не только реальной записи — см. следующий абзац) — `MAX_SHIFT_REACH +
/// MAX_CHANGE_REACH`. `GpuEngine` резервирует под арбитраж "дополненную"
/// (padded) координатную сетку шириной/высотой решётки + `2×MARGIN` именно
/// под эту дистанцию (см. `shader.wgsl::padded_idx`).
///
/// ПОЧЕМУ это нужно: `arbitrator::get_match_affected_cells` на CPU (см. её
/// реализацию) при `OverflowAction::Discard` НЕ клэмпит и НЕ отбрасывает
/// уходящие за границу решётки относительные ячейки записи — клэмпинг там
/// применяется ТОЛЬКО для `OverflowAction::Write`/`WriteLiteral`. Значит
/// `arbitrate()` учитывает конфликт даже между двумя матчами, чьи "цели"
/// уходят ЗА пределы решётки, если эти уходящие координаты СОВПАДАЮТ —
/// хотя физически туда ничего не пишется ни для одного из них (реальная
/// запись всё равно останавливается на границе, см. `apply_shift_buffered`/
/// `apply_changes_at`'s собственные bounds-check). Найдено экспериментально
/// (`tests/gpu_v2_correctness.rs`'s property-тест): без этого GPU и CPU
/// расходились у самого края решётки — GPU принимал матч, который CPU
/// отклонял из-за "фантомного" конфликта вне видимой части решётки.
pub const MAX_MARGIN: i32 = MAX_SHIFT_REACH + MAX_CHANGE_REACH;

/// Потолок числа правил ОДНОЙ головы, когда конфигу нужен полный арбитраж
/// (`GpuRuleTable::needs_arbitration == true`) — см. `GpuMatch` в
/// `shader.wgsl`: там каждой клетке решётки статически резервируется ровно
/// `MAX_MATCHES_PER_CELL` слотов кандидатов (по числу правил её головы),
/// так что общий размер буфера матчей — `width*height*MAX_MATCHES_PER_CELL`,
/// не зависящий от решётки список. НЕ применяется к self-write-only
/// конфигам (`needs_arbitration == false`, например Game of Life с его
/// 172 правилами на голову) — там весь перебор идёт внутри ОДНОГО потока
/// на клетку, без отдельного буфера матчей вообще.
pub const MAX_MATCHES_PER_CELL: usize = 8;

/// Один офсет паттерна: (dx, dy, ожидаемый тип).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuPatternOffset {
    pub dx: i32,
    pub dy: i32,
    pub expected: u32,
    pub _pad: u32,
}

/// Одно правило в плоской таблице.
///
/// `id_b0..id_b7`/`id_len` кодируют `Rule::id` для тай-брейка на GPU —
/// побайтовое лексикографическое сравнение `id_b0..id_b7` даёт тот же
/// порядок, что `Reverse(RuleIdKey)` в `arbitrator::arbitrate`, ЗА
/// ИСКЛЮЧЕНИЕМ случая, когда один id — собственный префикс другого (тогда
/// `Vec<CellType>::cmp` учитывает ещё и длину, а побайтовое сравнение с
/// нулевым паддингом — нет); этот угловой случай задокументирован как
/// принятое упрощение, а не забытый баг.
///
/// Поля — плоские (`id_b0..id_b7`, `shift_dx0/shift_dy0`, `change_dx0` и
/// т.д.), а не массивы: избегает индексации массива внутри
/// struct-элемента storage-буфера в WGSL по динамическому индексу — на
/// прошлом шаге (см. `v1_check`/`v2_arbitrate_check` в scratch-прототипах)
/// это упиралось в ограничение naga "may only be indexed by a constant" для
/// значений (не указателей). Порядок полей здесь ОБЯЗАН совпадать со
/// `struct GpuRule` в `shader.wgsl` 1-в-1 — это то, что `bytemuck::cast_slice`
/// льёт в буфер напрямую, без явной сериализации.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuRule {
    /// Смещение первого офсета правила в общем `pattern_offsets`.
    pub pattern_start: u32,
    pub pattern_len: u32,
    pub priority: u32,
    /// `Rule::min_age: u64`, клэмпнутый к `u32::MAX` — упрощение: счётчик
    /// поколений реалистичного прогона не достигает 4 млрд тиков раньше,
    /// чем закончится сама демонстрация GPU-бэкенда.
    pub min_age: u32,
    pub active_only: u32,
    pub id_len: u32,
    pub id_b0: u32,
    pub id_b1: u32,
    pub id_b2: u32,
    pub id_b3: u32,
    pub id_b4: u32,
    pub id_b5: u32,
    pub id_b6: u32,
    pub id_b7: u32,
    /// Позиция правила в исходном `rule_index[head]` — последний уровень
    /// тай-брейка, как `RuleMatch::rule_idx` у CPU-арбитража.
    pub rule_idx: u32,
    /// Число сдвигов (≤ [`MAX_SHIFTS`]) — 0 у self-write-only правил
    /// (v1-подмножество: раньше отдельного поля не было, `new_value`
    /// кодировался напрямую; теперь это `change_count==1` с
    /// `change_dx0==change_dy0==0`, см. doc-комментарий модуля).
    pub shift_count: u32,
    pub shift_dx0: i32,
    pub shift_dy0: i32,
    pub shift_dx1: i32,
    pub shift_dy1: i32,
    /// `ShiftSpec::broadcast` соответствующего сдвига (1 — broadcast, 0 —
    /// обычный) — см. doc-комментарий [`MAX_BROADCAST_REACH`]. Осмыслено
    /// только при `shift_count >= 1`/`>= 2` соответственно.
    pub shift_broadcast0: u32,
    pub shift_broadcast1: u32,
    /// `ShiftSpec::keep_source` соответствующего сдвига (1/0) — см.
    /// doc-комментарий `GpuMatch::keep_age_mask` в `shader.wgsl`. Осмыслено
    /// только при `shift_count >= 1`/`>= 2` соответственно.
    pub shift_keep_source0: u32,
    pub shift_keep_source1: u32,
    /// Число `changes` (≤ [`MAX_CHANGES`]).
    pub change_count: u32,
    pub change_dx0: i32,
    pub change_dy0: i32,
    pub change_val0: u32,
    pub change_dx1: i32,
    pub change_dy1: i32,
    pub change_val1: u32,
    pub change_dx2: i32,
    pub change_dy2: i32,
    pub change_val2: u32,
    pub change_dx3: i32,
    pub change_dy3: i32,
    pub change_val3: u32,
    /// `CamSearch::radius` — 0, если правило НЕ использует CAM (обычное
    /// поведение). `shift_count`/`change_count` у CAM-правила всегда 0 (см.
    /// валидацию в `config.rs`/`build_gpu_rule_table` — CAM это
    /// единственный эффект правила, не довесок к сдвигам/changes).
    pub cam_radius: u32,
    /// `CamSearch::target_type` — осмыслен только если `cam_radius > 0`.
    pub cam_target_type: u32,
    /// `Rule::tie_break` — см. её doc-комментарий в `types.rs` и
    /// `arbitrator::TIE_BREAK_MODULUS`. Прямое (не повёрнутое) значение;
    /// вращение `(tie_break + generation) % M` делается в шейдере при
    /// записи матча (`params.generation` уже доступен там), не здесь.
    pub tie_break: u32,
    /// `RecursionSpec::max_depth` — 0, если правило НЕ использует recursion
    /// (обычное поведение). См. [`MAX_RECURSION_DEPTH`]'s doc-комментарий
    /// про то, почему каскад — чисто локальное, однопоточное вычисление.
    pub recursion_max_depth: u32,
    /// (dx, dy) единичного шага `RecursionSpec::direction` — тот же
    /// формат, что и `shift_dx0/dy0`. Осмыслены только при
    /// `recursion_max_depth > 0`.
    pub recursion_dx: i32,
    pub recursion_dy: i32,
    /// 1, если у правила задан `Rule::starvation_after`, иначе 0. ОТДЕЛЬНЫЙ
    /// булев флаг, а НЕ "0 в `starvation_threshold` = выключено" (как у
    /// `cam_radius`/`recursion_max_depth` выше) — намеренно: в отличие от
    /// тех двух (где 0 — по-настоящему вырожденное, ничего не делающее
    /// значение: радиус 0 ничего не находит, глубина 0 никуда не
    /// каскадирует), `Rule::starvation_after: Option<u32>` со значением
    /// `Some(0)` — РЕАЛЬНЫЙ, отличный от "выключено" случай: порог 0
    /// означает "матч побеждает через голодание СРАЗУ, с первого же тика"
    /// (счётчик отсутствующего в HashMap ключа читается как 0, `0 >= 0`).
    /// Кодировать это как "0 = выключено" тихо превратило бы
    /// `starvation_after: 0` в `starvation_after: None` — расхождение
    /// GPU/CPU ровно на этом угловом случае.
    pub has_starvation: u32,
    /// Осмыслен только при `has_starvation == 1`.
    pub starvation_threshold: u32,
    /// 1, если у правила задан `Rule::feedback` (гарантированно ровно один
    /// сдвиг, без `broadcast` — см. `GpuUnsupportedReason::FeedbackBroadcastUnsupported`
    /// и `TooManyShifts`'s защитную проверку в `build_gpu_rule_table`).
    pub has_feedback: u32,
    /// `FeedbackSpec::timeout: u64`, клэмпнутый к `u32::MAX` — то же
    /// упрощение, что и у `GpuRule::min_age`.
    pub feedback_timeout: u32,
    /// (dx, dy) `FeedbackSpec::new_direction` — тот же формат, что и
    /// `shift_dx0/dy0`, но АЛЬТЕРНАТИВНОЕ направление, применяемое вместо
    /// декларированного (`shift_dx0/dy0`), когда persistent-счётчик
    /// `feedback_counters[m]` (см. `shader.wgsl`'s binding) достиг
    /// `feedback_timeout`. Осмыслены только при `has_feedback == 1`.
    pub feedback_alt_dx: i32,
    pub feedback_alt_dy: i32,
    /// 1, если у правила задан `Rule::memory` — см. `GpuRuleTable::needs_memory`'s
    /// doc-комментарий. Гарантированно исключает `recursion`/`cam`
    /// (`GpuUnsupportedReason::MemoryRecursionUnsupported`/
    /// `MemoryCamUnsupported`) и не-broadcast, ≤1 сдвиг (та же защита, что
    /// и у `has_feedback`).
    pub has_memory: u32,
    /// `MemorySpec::window` — 1..=[`MAX_MEMORY_WINDOW`]. Осмыслен только при
    /// `has_memory == 1`.
    pub memory_window: u32,
    /// `RecordTrigger` — 0 = `NeighborType`, 1 = `RuleOutcome`.
    pub memory_trigger: u32,
    /// (dx, dy) `RecordTrigger::NeighborType`'s направления — тот же формат,
    /// что `shift_dx0/dy0`. Осмыслены только при `memory_trigger == 0`
    /// (нули при `RuleOutcome`, там наблюдение не привязано к соседу).
    pub memory_dx: i32,
    pub memory_dy: i32,
    /// 1, если у правила с `memory` есть РОВНО один сдвиг (значит буфер
    /// физически переезжает на новую позицию при выигранном сдвиге — см.
    /// `update_memory_relocate_pass` в `shader.wgsl`, зеркалит
    /// `applicator.rs`'s перенос `Engine::memory_buffers` при
    /// `apply_shift_buffered`), 0 — если сдвига нет вовсе (буфер живёт на
    /// фиксированной позиции, никогда не переезжает). `MemorySpec` по
    /// построению допускает только 0 или 1 сдвиг (см. `config::load_config`
    /// и защитную проверку в `build_gpu_rule_table` ниже) — здесь плоский
    /// булев, а не "число сдвигов", ровно потому что других значений тут
    /// быть не может.
    pub memory_has_shift: u32,
    /// Целевая последовательность `MemorySpec::match_pattern`, поэлементно,
    /// от старого к новому — плоские поля вместо массива (см. `GpuRule`'s
    /// doc-комментарий про ограничение naga), значимы только первые
    /// `memory_window` из них. Кодировка ОДНОГО `RecordedValue` (общая с
    /// рантайм-записью буфера в `shader.wgsl`, см. `encode_recorded_value`
    /// ниже): `0..=255` = `Type(CellType)` (значение — сам код типа
    /// клетки), `256` = `Applied`, `257` = `Missed` — без коллизий,
    /// `CellType` — `u8`.
    pub memory_pattern0: u32,
    pub memory_pattern1: u32,
    pub memory_pattern2: u32,
    pub memory_pattern3: u32,
}

/// Один офсет union-проверки границ (см. `GpuHeadSlot::offsets_start`) —
/// только dx/dy, без `expected`: используется исключительно для
/// bounds-check, не для сравнения значений.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuOffset {
    pub dx: i32,
    pub dy: i32,
}

/// Слот плоской 256-элементной таблицы, индексируемой `CellType` (`u8`).
///
/// `rules_start`/`rules_count` — диапазон в `GpuRuleTable::rules`.
///
/// `offsets_start`/`offsets_count` — диапазон в `GpuRuleTable::head_offsets`:
/// ОБЪЕДИНЕНИЕ офсетов всех правил этой головы (как `matcher::GroupData::all_offsets`
/// на CPU). Нужен для отдельного, ПЕРЕД перебором правил, bounds-check:
/// `matcher::match_cell` грузит соседский кэш ОДИН раз на клетку по всему
/// `all_offsets` группы и, если хоть один офсет (даже нужный только ДРУГОМУ
/// правилу той же головы) уходит за границу решётки, прерывает загрузку и
/// не проверяет НИ ОДНО правило группы на этой клетке вообще (см. её
/// подробный doc-комментарий) — это и есть причина, по которой граничное
/// кольцо решётки у Game of Life никогда ни с чем не совпадает (см.
/// `examples/flagship_gol.rs`). Без этого шейдер (который иначе трактует
/// офсет каждого правила независимо, подставляя `default_cell_type` для
/// офсетов вне границ) давал бы РАЗНЫЙ результат от CPU на любой клетке
/// в пределах `max(|dx|,|dy|)` от края решётки — найдено экспериментально
/// при первой попытке сравнить `GpuEngine` с `engine::run_tick` в лоб.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuHeadSlot {
    pub rules_start: u32,
    pub rules_count: u32,
    pub offsets_start: u32,
    pub offsets_count: u32,
}

/// Потолок ячеек записи ОДНОГО матча: max(путь сдвигов/changes, каскад
/// recursion). Путь сдвигов: source-clear (1, только если есть сдвиг) + на
/// каждый сдвиг — путь сдвига (обычный сдвиг пишет 1 ячейку — конечную
/// точку; broadcast-сдвиг пишет до [`MAX_BROADCAST_REACH`] ячеек — см. её
/// doc-комментарий про то, почему это отдельный, более узкий потолок, чем
/// [`MAX_SHIFT_REACH`]) + его `changes`. Каскад recursion (взаимоисключим со
/// сдвигами по валидации `config.rs`, см. [`MAX_RECURSION_DEPTH`]'s
/// doc-комментарий): `MAX_RECURSION_DEPTH + 1` уровней (уровень 0 + каскад),
/// каждый пишет до [`MAX_CHANGES`] ячеек. Зеркалит `shader.wgsl`'s
/// `MAX_WRITE_CELLS` — держать их синхронно ОБЯЗАТЕЛЬНО (см.
/// doc-комментарий [`GpuMatchLayout`]).
pub const MAX_WRITE_CELLS: usize = {
    let shift_path = 1 + MAX_SHIFTS * (MAX_BROADCAST_REACH as usize + MAX_CHANGES);
    let recursion_path = (MAX_RECURSION_DEPTH as usize + 1) * MAX_CHANGES;
    if shift_path > recursion_path { shift_path } else { recursion_path }
};

/// Раскладка `GpuMatch` из `shader.wgsl`, ТОЛЬКО для `std::mem::size_of`
/// при выделении буфера матчей в `GpuEngine` — сам буфер целиком
/// заполняется GPU (`detect_pass`), CPU в него ничего не пишет и с этим
/// типом напрямую не работает. Порядок/состав полей должен зеркалить
/// `struct GpuMatch` в `shader.wgsl` 1-в-1 (тот же принцип, что и у
/// [`GpuRule`]), иначе размер буфера будет неверным.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuMatchLayout {
    pub priority: u32,
    pub age: u32,
    /// Уже повёрнутое `(tie_break + generation) % TIE_BREAK_MODULUS` — см.
    /// `GpuRule::tie_break` и `shader.wgsl::TIE_BREAK_MODULUS`. Должно идти
    /// СРАЗУ после `age`, ровно как в `struct GpuMatch` в шейдере.
    pub tie_break: u32,
    pub id: [u32; MAX_ID_BYTES],
    pub x: u32,
    pub y: u32,
    pub rule_idx: u32,
    pub cell_count: u32,
    /// См. `shader.wgsl::GpuMatch::keep_age_mask`'s doc-комментарий. ДОЛЖНО
    /// идти СРАЗУ после `cell_count` и ДО `cells`, ровно как в шейдере —
    /// иначе `bytemuck::cast_slice` в `cpu_fallback_resolve` читает
    /// `cells`/`values` со сдвигом на 4 байта относительно реального
    /// GPU-буфера (найдено этим же багом при добавлении поля — забыл
    /// продублировать сюда, `matches_readback_buf`'s parity-тест это
    /// сразу поймал).
    pub keep_age_mask: u32,
    pub cells: [u32; MAX_WRITE_CELLS],
    pub values: [u32; MAX_WRITE_CELLS],
    /// См. `shader.wgsl::GpuMatch::matched`'s doc-комментарий.
    pub matched: u32,
    /// См. `shader.wgsl::GpuMatch::structural`'s doc-комментарий — нужен
    /// ТОЛЬКО для `Rule::memory`'s буфера: в отличие от `matched` (который
    /// для memory-правил означает "гейт открыт", т.е. финальный статус
    /// кандидата), `structural` означает "структурно совпало, независимо
    /// от гейта" — буфер обязан продолжать наблюдать, даже пока гейт
    /// закрыт (зеркалит CPU `memory_targets`, взятый из ПОЛНОГО, ещё не
    /// гейтованного списка матчей).
    pub structural: u32,
}

/// Итог кодирования — то, что льётся в GPU-буферы как есть (`bytemuck::cast_slice`).
#[derive(Debug)]
pub struct GpuRuleTable {
    pub head_slots: [GpuHeadSlot; 256],
    pub rules: Vec<GpuRule>,
    pub pattern_offsets: Vec<GpuPatternOffset>,
    pub head_offsets: Vec<GpuOffset>,
    /// См. doc-комментарий модуля — выбирает между self-write-only
    /// (быстрый, однопроходный) и полным арбитражным (медленнее,
    /// многораундовый) пайплайном в `GpuEngine`.
    pub needs_arbitration: bool,
    /// РЕАЛЬНЫЙ максимальный охват (см. doc-комментарий [`MAX_MARGIN`]) —
    /// максимум по ВСЕМ правилам конфига от `shift_reach + change_reach`
    /// (0, если `needs_arbitration == false`: тогда отступ вообще не
    /// используется). В отличие от [`MAX_MARGIN`] (статический потолок,
    /// используемый ТОЛЬКО для валидации на входе), это фактическая нужная
    /// величина для ЭТОГО конкретного набора правил — типичные конфиги
    /// (сдвиг на 1 клетку, без длинных change-смещений) дают `margin` в
    /// районе 1-2, а не 16. `GpuEngine` резервирует под арбитраж
    /// дополненную сетку размером `(width + 2×margin) × (height + 2×margin)`
    /// — раздувать её до потолка ради конфигов, которым это не нужно,
    /// было бы чистой тратой (особенно заметно на маленьких решётках, где
    /// накладные расходы `clear_locked`/`clear_claims` над "фантомным"
    /// ободом могли в разы превышать саму решётку).
    pub margin: i32,
    /// РЕАЛЬНЫЙ максимум `rules_count` по всем `head_slots` (0, если
    /// `needs_arbitration == false` — Simple-пайплайн вообще не заводит
    /// буфер матчей). В отличие от [`MAX_MATCHES_PER_CELL`] (статический
    /// потолок, используемый ТОЛЬКО для валидации), это фактическое число
    /// правил у самой "многоголовой" head ЭТОГО конкретного конфига —
    /// `GpuEngine` резервирует `detect_pass`/`claim_pass`/`resolve_pass`
    /// ровно на `width×height×max_matches_per_cell` потоков, а не всегда
    /// на потолочные ×8: типичный конфиг (1 правило на голову, как
    /// `examples/flagship_shifts.rs`'s движущиеся частицы) даёт здесь 1,
    /// а не 8 — 8-кратное сокращение лишних потоков, которые иначе только
    /// проверяли бы "слот пуст" и сразу выходили.
    pub max_matches_per_cell: u32,
    /// Максимум `|dx|.max(|dy|)` по ВСЕМ офсетам паттерна ВСЕХ правил
    /// конфига (0, если у всех правил только self-офсет `(0,0)`) —
    /// используется ТОЛЬКО Simple-пайплайном (`needs_arbitration == false`),
    /// чтобы решить, можно ли закешировать соседей клетки в shared-памяти
    /// workgroup'а (`shader.wgsl::main_tiled`, halo радиуса 1) вместо
    /// раздельного чтения `current[]` из глобальной памяти на каждое
    /// сравнение офсета — см. её doc-комментарий. При `pattern_reach > 1`
    /// `GpuEngine` использует обычный `main` (общий случай, без tiling).
    pub pattern_reach: i32,
    /// `true`, если ХОТЯ БЫ одно правило конфига использует `starvation_after`
    /// — `GpuEngine` аллоцирует persistent storage-буфер счётчиков голодания
    /// и подключает дополнительный `update_starvation_pass` ТОЛЬКО в этом
    /// случае (нулевые накладные расходы для конфигов, которым это не нужно
    /// — та же философия, что и `ExtensionFlags` на CPU-стороне). Всегда
    /// подразумевает `needs_arbitration == true` (см. её вычисление ниже) —
    /// голодание осмысленно только когда есть реальная конкуренция за
    /// клетку, которую нужно арбитражировать.
    pub needs_starvation: bool,
    /// `true`, если ХОТЯ БЫ одно правило использует `feedback` —
    /// `GpuEngine` аллоцирует persistent storage-буфер счётчиков обратной
    /// связи и подключает `update_feedback_pass` ТОЛЬКО в этом случае, тот
    /// же приём, что и `needs_starvation`. Всегда подразумевает
    /// `needs_arbitration == true` (сдвиг — уже само по себе запись в
    /// соседа, требующая арбитража, независимо от `feedback`).
    pub needs_feedback: bool,
    /// `true`, если ХОТЯ БЫ одно правило использует `memory` — `GpuEngine`
    /// аллоцирует ДВА persistent storage-буфера (`memory_buffers`,
    /// `memory_len`) и подключает `update_memory_push_pass`/
    /// `update_memory_relocate_pass` ТОЛЬКО в этом случае, тот же приём,
    /// что и `needs_starvation`/`needs_feedback`. Всегда подразумевает
    /// `needs_arbitration == true` — ДАЖЕ для правила, которое иначе (без
    /// `memory`) получило бы Simple-пайплайн (self-write, без сдвигов и
    /// записи в соседа): гейт и persistent-буферы существуют только в
    /// инфраструктуре Arbitrated-пайплайна (`matches[m]`/`match_state` и
    /// т.д.), Simple-пайплайн их вообще не заводит.
    pub needs_memory: bool,
}

/// Почему конфиг целиком не влезает в поддерживаемое подмножество GPU-бэкенда.
/// `head`/`rule_idx` указывают на конкретное правило-нарушитель — этого
/// достаточно, чтобы пользователь библиотеки нашёл его в своём YAML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuUnsupportedReason {
    /// `changes` пуст И `shifts` пуст — правило совпадает, но ничего не
    /// делает; вне минимального осмысленного сценария.
    NoEffect { head: u8, rule_idx: usize },
    /// `ChangeValue::Ref` — вне подмножества (нужен буфер значений
    /// паттерна на клетку, отдельная, более сложная задача).
    ChangeIsRef { head: u8, rule_idx: usize },
    /// `OverflowAction::Write`/`WriteLiteral` у правила со сдвигом — требует
    /// синхронизации с `BoundaryBuffer` на хосте; вне подмножества
    /// (поддерживается только `Discard` — уходящая за край клетка теряется).
    OverflowNotDiscard { head: u8, rule_idx: usize },
    /// Число сдвигов правила (после разворачивания групп) больше [`MAX_SHIFTS`].
    TooManyShifts { head: u8, rule_idx: usize, len: usize },
    /// Число `changes` правила больше [`MAX_CHANGES`].
    TooManyChanges { head: u8, rule_idx: usize, len: usize },
    /// Паттерн длиннее [`MAX_PATTERN_OFFSETS`].
    PatternTooLarge { head: u8, rule_idx: usize, len: usize },
    /// `Rule::id` длиннее [`MAX_ID_BYTES`] — не влезает в тай-брейк-ключ.
    RuleIdTooLong { head: u8, rule_idx: usize, len: usize },
    /// Конфигу нужен арбитраж (`needs_arbitration`), но у какой-то головы
    /// больше [`MAX_MATCHES_PER_CELL`] правил — см. её doc-комментарий.
    TooManyRulesForArbitration { head: u8, len: usize },
    /// `|dx|`/`|dy|` сдвига больше [`MAX_SHIFT_REACH`] — см. doc-комментарий
    /// [`MAX_MARGIN`].
    ShiftTooFar { head: u8, rule_idx: usize, dx: i32, dy: i32 },
    /// `|dx|`/`|dy`| `changes`-смещения больше [`MAX_CHANGE_REACH`].
    ChangeTooFar { head: u8, rule_idx: usize, dx: i32, dy: i32 },
    /// `CamSearch::radius` больше [`MAX_CAM_RADIUS`] — см. её doc-комментарий.
    /// CPU-версия не имеет такого потолка (только GPU: диск (2R+1)² на
    /// клетку-кандидата каждый тик и раздутие дополненной сетки на `2×radius`
    /// делают ОЧЕНЬ большой радиус реально дорогим именно на GPU).
    CamRadiusTooFar { head: u8, rule_idx: usize, radius: u8 },
    /// `ShiftSpec::broadcast` с `steps` больше [`MAX_BROADCAST_REACH`] — вне
    /// подмножества: см. её doc-комментарий про то, почему потолок
    /// broadcast-пути отдельный (более узкий), чем [`MAX_SHIFT_REACH`]
    /// обычных сдвигов. Короткие broadcast-сдвиги (`steps` в пределах
    /// потолка) ПОДДЕРЖИВАЮТСЯ — см. `build_gpu_rule_table`.
    BroadcastPathTooLong { head: u8, rule_idx: usize, steps: i32 },
    /// `ShiftSpec::keep_source` + `Rule::feedback` ВМЕСТЕ — вне
    /// GPU-подмножества (обычный, без `feedback`, `keep_source` теперь
    /// ПОДДЕРЖИВАЕТСЯ, см. `GpuMatch::keep_age_mask` в `shader.wgsl`).
    /// Причина: `update_feedback_relocate_pass` не портирован для случая,
    /// когда источник НЕ освобождается — на CPU перенос счётчика вообще
    /// пропускается при `keep_source`, а GPU-версия такого условия не
    /// знает и релоцировала бы счётчик даже когда оригинал физически
    /// остался на месте (найдено бы порчей данных, не крашем).
    FeedbackKeepSourceUnsupported { head: u8, rule_idx: usize },
    /// `ShiftSpec::keep_source` + `Rule::memory` ВМЕСТЕ — вне
    /// GPU-подмножества, ТА ЖЕ причина, что и `FeedbackKeepSourceUnsupported`.
    MemoryKeepSourceUnsupported { head: u8, rule_idx: usize },
    /// `Rule::feedback` + `ShiftSpec::broadcast` ВМЕСТЕ — вне GPU-подмножества
    /// (обычный, не-broadcast `feedback` ПОДДЕРЖИВАЕТСЯ, см.
    /// `GpuRuleTable::needs_feedback`). Причина: перенос persistent-счётчика
    /// на новую позицию (см. её doc-комментарий) читает КОНКРЕТНУЮ клетку
    /// `matches[m].cells[1]` как "новую позицию головки" — верно для
    /// обычного сдвига (ровно 2 записи: source-clear + цель), но
    /// broadcast-сдвиг пишет ВЕСЬ путь (см. `MAX_BROADCAST_REACH`'s
    /// doc-комментарий), так что "куда физически переместился маркер" —
    /// это ПОСЛЕДНЯЯ клетка пути, а не `cells[1]` — нужна отдельная,
    /// не сделанная здесь логика поиска конца пути.
    FeedbackBroadcastUnsupported { head: u8, rule_idx: usize },
    /// `RecursionSpec::max_depth` больше [`MAX_RECURSION_DEPTH`] — вне
    /// подмножества, тот же приём, что и `BroadcastPathTooLong`. Каскад в
    /// пределах потолка ПОДДЕРЖИВАЕТСЯ (см. [`MAX_RECURSION_DEPTH`]'s
    /// doc-комментарий про то, почему это чисто локальное однопоточное
    /// вычисление, а не блэнкет-отказ, как считалось раньше).
    RecursionDepthTooLarge { head: u8, rule_idx: usize, max_depth: u8 },
    /// `cam` + `recursion` ВМЕСТЕ — вне GPU-подмножества (в отличие от CPU,
    /// где эта комбинация поддерживается, см. `applicator::apply_cam_buffered`'s
    /// doc-комментарий): CAM-каскад нуждается в РАНТАЙМ-поиске (`cam_search`)
    /// на КАЖДОМ уровне, а не в статически известном офсете `k × direction`,
    /// как обычная (не-cam) рекурсия — это другой, ещё не реализованный
    /// путь, не покрываемый нынешним локальным-каскадным расширением.
    CamRecursionUnsupported { head: u8, rule_idx: usize },
    /// `MemorySpec::window` больше [`MAX_MEMORY_WINDOW`] — тот же приём,
    /// что `RecursionDepthTooLarge`/`BroadcastPathTooLong`. Окна в пределах
    /// потолка ПОДДЕРЖИВАЮТСЯ (см. `GpuRuleTable::needs_memory`'s
    /// doc-комментарий) — раньше `Rule::memory` отвергался целиком, теперь
    /// (как и `starvation_after`/`feedback` до него) персистентный
    /// storage-буфер снял главное препятствие "GPU не хранит состояние
    /// между тиками".
    MemoryWindowTooLarge { head: u8, rule_idx: usize, window: usize },
    /// `Rule::memory` + `Rule::recursion` ВМЕСТЕ — вне GPU-подмножества
    /// (в отличие от CPU, где `NeighborType`+`recursion` поддерживается,
    /// см. `applicator.rs`'s каскадный гейт). Причина: гейт каждого уровня
    /// каскада требует своего собственного per-level буфера/наблюдения
    /// (`applicator.rs`'s Фаза 3 проверяет гейт заново на каждом `k`) —
    /// отдельная, не реализованная здесь логика, тот же класс границы, что
    /// и `CamRecursionUnsupported`.
    MemoryRecursionUnsupported { head: u8, rule_idx: usize },
    /// `Rule::memory` + `Rule::cam` ВМЕСТЕ — вне GPU-подмножества. CPU не
    /// запрещает эту комбинацию явно, но `detect_pass`'s CAM-ветка — отдельный,
    /// более ранний код-путь (возвращается до того места, где включается
    /// гейт-проверка памяти) — не подключена сюда, тот же нереализованный
    /// класс, что и `MemoryRecursionUnsupported`.
    MemoryCamUnsupported { head: u8, rule_idx: usize },
    /// `Rule::memory` + `ShiftSpec::broadcast` ВМЕСТЕ — вне GPU-подмножества,
    /// ТА ЖЕ причина, что и `FeedbackBroadcastUnsupported`: перенос буфера
    /// на новую позицию читает `matches[m].cells[1]` как "новую позицию
    /// маркера" — верно только для обычного (не-broadcast) сдвига.
    MemoryBroadcastUnsupported { head: u8, rule_idx: usize },
    /// `Rule::feedback` + `changes` на ТОМ ЖЕ относительном смещении, что и
    /// (эффективный) сдвиг — вне GPU-подмножества. Причина: `apply_pass`
    /// зеркалит CPU-семантику "changes ПОБЕЖДАЮТ shifts при конфликте
    /// клетки" (см. `applicator.rs`'s "Фаза 1/Фаза 2"), значит РЕАЛЬНО
    /// записанное на новой позиции значение может оказаться НЕ `me.value`
    /// (собственный тип головы маркера), а литерал `changes`. Перенос
    /// счётчика (`update_feedback_relocate_pass`) вычисляет `new_m` через
    /// `slot_in_cell`, ПРЕДПОЛАГАЯ, что тип на новой позиции — снова
    /// `me.value` (та же голова, тот же список правил, тот же слот) — если
    /// это предположение нарушено, счётчик уезжает в слот, который либо
    /// никогда не читается (безвредная, но некорректная утечка), либо (в
    /// худшем случае) СОВПАДАЕТ со слотом СОВЕРШЕННО ДРУГОГО правила у
    /// новой головы, чей persistent-счётчик был бы тихо испорчен чужим
    /// значением. CPU-сторона свободна от этого риска: её ключ переноса —
    /// `(x, y, rule_idx)`, полностью самоописывающийся независимо от
    /// фактического типа клетки на новой позиции (см. `applicator.rs`'s
    /// `apply_shift_buffered`). Проверяется по ОБОИМ возможным эффективным
    /// смещениям (`shift_dx0/dy0` и `feedback_alt_dx/dy`), так как
    /// `feedback` переключается между ними в рантайме одного и того же
    /// правила.
    FeedbackChangeCollidesWithShiftTarget { head: u8, rule_idx: usize },
    /// `Rule::memory` + `changes` на ТОМ ЖЕ относительном смещении, что и
    /// сдвиг — вне GPU-подмножества. ТА ЖЕ причина и то же решение, что и
    /// `FeedbackChangeCollidesWithShiftTarget`, только без "альтернативного"
    /// направления (у `memory` его просто нет).
    MemoryChangeCollidesWithShiftTarget { head: u8, rule_idx: usize },
    /// `Rule::max_activations` — вне GPU-подмножества. Счётчик ключуется
    /// `(head, rule_idx)`, БЕЗ позиции (см. её doc-комментарий в `types.rs`)
    /// — персистентное состояние ВНЕ формы, для которой уже существует
    /// GPU-буфер (`starvation_counters_buf`/`feedback_counters_buf` оба
    /// индексированы по клетке решётки, `padded_idx`-совместимо; глобальный
    /// счётчик на `rule_idx` потребовал бы отдельного, третьего вида
    /// буфера и отдельного гейт-прохода в `detect_pass`). Не тронуто
    /// намеренно — см. §13.4 спецификации.
    MaxActivationsUnsupported { head: u8, rule_idx: usize },
}

/// Собрать `effective_pattern` ровно так же, как `matcher::build_group_data`
/// (явный `rule.pattern`, либо fallback из `rule.id` при пустом паттерне) —
/// чтобы GPU видел ТЕ ЖЕ паттерны, что и CPU-матчер для того же правила.
fn effective_pattern(rule: &Rule) -> Vec<(i8, i8, CellType)> {
    if !rule.pattern.is_empty() {
        rule.pattern.clone()
    } else {
        rule.id
            .iter()
            .enumerate()
            .map(|(i, &ct)| (i as i8, 0i8, ct))
            .collect()
    }
}

/// Дельта одного сдвига — зеркалит `conflict_analyzer::shift_delta`.
fn shift_delta(shift: &crate::types::ShiftSpec) -> (i32, i32) {
    let steps = shift.steps as i32;
    match shift.direction {
        Direction::Up => (0, -steps),
        Direction::Down => (0, steps),
        Direction::Left => (-steps, 0),
        Direction::Right => (steps, 0),
    }
}

/// Кодировка ОДНОГО `RecordedValue` в `u32` — см. `GpuRule::memory_pattern0..3`'s
/// doc-комментарий. Общая функция для CPU-стороны (кодирование
/// `MemorySpec::match_pattern` в `GpuRule`) и рантайм-записи буфера в
/// `shader.wgsl` (`update_memory_push_pass`) — ОБЯЗАНЫ использовать
/// идентичную кодировку, иначе гейт-сравнение в `detect_pass` никогда не
/// совпадёт ни с одним реально записанным значением.
fn encode_recorded_value(value: RecordedValue) -> u32 {
    match value {
        RecordedValue::Type(ct) => ct.0 as u32,
        RecordedValue::Applied => 256,
        RecordedValue::Missed => 257,
    }
}

/// Единичный шаг `RecursionSpec::direction` — зеркалит
/// `conflict_analyzer::direction_unit_delta`/`applicator`'s инлайн-match в
/// каскаде `Rule::recursion`.
fn recursion_direction_delta(direction: Direction) -> (i32, i32) {
    match direction {
        Direction::Up => (0, -1),
        Direction::Down => (0, 1),
        Direction::Left => (-1, 0),
        Direction::Right => (1, 0),
    }
}

/// Построить GPU-таблицу правил из того же `rule_index`, что использует
/// `Engine`/`detect_matches`. `Err`, если хотя бы одно правило выходит за
/// поддерживаемое подмножество — см. doc-комментарий модуля про то, почему
/// это ошибка для ВСЕГО конфига, а не молчаливый пропуск одного правила.
pub fn build_gpu_rule_table(
    rule_index: &HashMap<CellType, Vec<Rule>>,
) -> Result<GpuRuleTable, GpuUnsupportedReason> {
    let mut head_slots = [GpuHeadSlot { rules_start: 0, rules_count: 0, offsets_start: 0, offsets_count: 0 }; 256];
    let mut rules: Vec<GpuRule> = Vec::new();
    let mut pattern_offsets: Vec<GpuPatternOffset> = Vec::new();
    let mut head_offsets: Vec<GpuOffset> = Vec::new();
    let mut needs_arbitration = false;
    let mut needs_starvation = false;
    let mut needs_feedback = false;
    let mut needs_memory = false;
    let mut margin: i32 = 0;
    let mut pattern_reach: i32 = 0;

    // Порядок обхода `HashMap` не детерминирован, но это не важно: каждый
    // head пишет в СВОЙ слот `head_slots[head]`, слоты друг другу не мешают
    // независимо от порядка, а `rule_idx` внутри головы считается по позиции
    // в `rules` конкретного head (`enumerate()` ниже), не по общему счётчику.
    for (&head, group) in rule_index.iter() {
        let rules_start = rules.len() as u32;
        // Union офсетов ВСЕХ правил группы, в порядке первого появления —
        // как `matcher::build_group_data`'s `all_offsets`/`offset_set` (см.
        // doc-комментарий `GpuHeadSlot::offsets_start`). Дедуп нужен только
        // для компактности буфера — порядок и повторы на корректность
        // bounds-check не влияют, шейдер просто проверяет каждый офсет.
        let mut union_offsets: Vec<(i8, i8)> = Vec::new();
        let mut union_seen: FxHashSet<(i8, i8)> = FxHashSet::default();

        for (rule_idx, rule) in group.iter().enumerate() {
            if rule.max_activations.is_some() {
                return Err(GpuUnsupportedReason::MaxActivationsUnsupported { head: head.0, rule_idx });
            }
            if let Some(spec) = rule.recursion {
                if rule.cam.is_some() {
                    return Err(GpuUnsupportedReason::CamRecursionUnsupported { head: head.0, rule_idx });
                }
                if spec.max_depth > MAX_RECURSION_DEPTH {
                    return Err(GpuUnsupportedReason::RecursionDepthTooLarge { head: head.0, rule_idx, max_depth: spec.max_depth });
                }
            }
            if let Some(spec) = &rule.memory {
                if rule.recursion.is_some() {
                    return Err(GpuUnsupportedReason::MemoryRecursionUnsupported { head: head.0, rule_idx });
                }
                if rule.cam.is_some() {
                    return Err(GpuUnsupportedReason::MemoryCamUnsupported { head: head.0, rule_idx });
                }
                if spec.window > MAX_MEMORY_WINDOW {
                    return Err(GpuUnsupportedReason::MemoryWindowTooLarge { head: head.0, rule_idx, window: spec.window });
                }
            }
            // Флаг `broadcast`/`keep_source` КАЖДОГО сдвига, в том же
            // порядке/индексации, что `shift_deltas` — используется ниже и
            // при заполнении `GpuRule::shift_broadcast0/1`/`shift_keep_source0/1`.
            let shift_broadcasts: Vec<bool> = rule.shifts.iter().flatten().map(|s| s.broadcast).collect();
            let shift_keep_sources: Vec<bool> = rule.shifts.iter().flatten().map(|s| s.keep_source).collect();
            let shift_deltas: Vec<(i32, i32)> = rule.shifts.iter().flatten().map(shift_delta).collect();
            if shift_deltas.len() > MAX_SHIFTS {
                return Err(GpuUnsupportedReason::TooManyShifts { head: head.0, rule_idx, len: shift_deltas.len() });
            }
            if rule.feedback.is_some() {
                // `config::load_config` уже требует РОВНО один сдвиг для
                // `feedback` — здесь та же проверка защитно (см. её же
                // паттерн у `cam`'s `id_len == 1` выше): правило, пришедшее
                // мимо YAML-пути (напрямую через Rust API), не должно
                // тихо ломать `GpuRule::feedback_alt_dx/dy`'s единственное
                // предположение "ровно один сдвиг".
                if shift_deltas.len() != 1 {
                    return Err(GpuUnsupportedReason::TooManyShifts { head: head.0, rule_idx, len: shift_deltas.len() });
                }
                if shift_broadcasts[0] {
                    return Err(GpuUnsupportedReason::FeedbackBroadcastUnsupported { head: head.0, rule_idx });
                }
                if shift_keep_sources[0] {
                    // Перенос счётчика (`update_feedback_relocate_pass`) не
                    // портирован для случая, когда источник НЕ освобождается
                    // — на CPU перенос вообще пропускается при `keep_source`
                    // (`applicator.rs`: "перенос — ТОЛЬКО когда источник
                    // реально освобождается"), GPU-версия такого условия не
                    // знает и релоцировала бы счётчик даже когда оригинал
                    // физически остался на месте. Отдельная, более узкая
                    // задача от `keep_age_mask` (тот отвечает только за
                    // возраст клетки, не за feedback-счётчик).
                    return Err(GpuUnsupportedReason::FeedbackKeepSourceUnsupported { head: head.0, rule_idx });
                }
                // Перенос счётчика (см. `FeedbackChangeCollidesWithShiftTarget`'s
                // doc-комментарий) предполагает, что тип клетки на новой
                // позиции останется `me.value` — ложно, если `changes`
                // ЗАПИСЫВАЕТ туда что-то другое (CPU-семантика "changes
                // побеждают shifts при конфликте клетки"). Проверяем ОБА
                // возможных эффективных смещения — декларированное и
                // альтернативное (`new_direction`), поскольку `feedback`
                // переключается между ними в рантайме одного и того же
                // правила, а эта проверка статическая (на этапе сборки
                // таблицы).
                if let Some(spec) = rule.feedback {
                    let alt = recursion_direction_delta(spec.new_direction);
                    if rule.changes.iter().any(|&(dx, dy, _)| (dx, dy) == shift_deltas[0] || (dx, dy) == alt) {
                        return Err(GpuUnsupportedReason::FeedbackChangeCollidesWithShiftTarget { head: head.0, rule_idx });
                    }
                }
            }
            if rule.memory.is_some() {
                // `config::load_config` уже требует 0 или 1 сдвиг для
                // `memory` — та же защитная re-проверка, что и у `feedback`
                // выше, на случай конфига, собранного мимо YAML-пути.
                if shift_deltas.len() > 1 {
                    return Err(GpuUnsupportedReason::TooManyShifts { head: head.0, rule_idx, len: shift_deltas.len() });
                }
                if shift_deltas.len() == 1 && shift_broadcasts[0] {
                    return Err(GpuUnsupportedReason::MemoryBroadcastUnsupported { head: head.0, rule_idx });
                }
                if shift_deltas.len() == 1 && shift_keep_sources[0] {
                    // Та же причина, что и `FeedbackKeepSourceUnsupported`
                    // выше — перенос буфера памяти на GPU не портирован для
                    // случая, когда источник не освобождается.
                    return Err(GpuUnsupportedReason::MemoryKeepSourceUnsupported { head: head.0, rule_idx });
                }
                // Та же причина, что и `FeedbackChangeCollidesWithShiftTarget`
                // выше — `memory` без "альтернативного" направления, только
                // одно возможное эффективное смещение.
                if shift_deltas.len() == 1 && rule.changes.iter().any(|&(dx, dy, _)| (dx, dy) == shift_deltas[0]) {
                    return Err(GpuUnsupportedReason::MemoryChangeCollidesWithShiftTarget { head: head.0, rule_idx });
                }
            }
            for &(dx, dy) in &shift_deltas {
                if dx.abs() > MAX_SHIFT_REACH || dy.abs() > MAX_SHIFT_REACH {
                    return Err(GpuUnsupportedReason::ShiftTooFar { head: head.0, rule_idx, dx, dy });
                }
            }
            // Broadcast — ПОДДЕРЖИВАЕТСЯ (см. `push_write_cell`-цикл в
            // `detect_pass`, который теперь пишет ВЕСЬ путь, не только
            // финальную точку), но `steps` ограничен отдельным, более узким
            // потолком [`MAX_BROADCAST_REACH`] — см. её doc-комментарий про
            // то, почему это НЕ то же самое, что [`MAX_SHIFT_REACH`] обычных
            // сдвигов (число ячеек записи растёт с `steps`, не константно).
            for (&(dx, dy), &is_broadcast) in shift_deltas.iter().zip(shift_broadcasts.iter()) {
                if is_broadcast {
                    let steps = dx.abs().max(dy.abs());
                    if steps > MAX_BROADCAST_REACH {
                        return Err(GpuUnsupportedReason::BroadcastPathTooLong { head: head.0, rule_idx, steps });
                    }
                }
            }
            if rule.changes.len() > MAX_CHANGES {
                return Err(GpuUnsupportedReason::TooManyChanges { head: head.0, rule_idx, len: rule.changes.len() });
            }
            for &(dx, dy, _) in &rule.changes {
                if dx.abs() > MAX_CHANGE_REACH || dy.abs() > MAX_CHANGE_REACH {
                    return Err(GpuUnsupportedReason::ChangeTooFar { head: head.0, rule_idx, dx, dy });
                }
            }
            if shift_deltas.is_empty() && rule.changes.is_empty() && rule.cam.is_none() {
                return Err(GpuUnsupportedReason::NoEffect { head: head.0, rule_idx });
            }
            if let Some(cam) = rule.cam {
                if cam.radius > MAX_CAM_RADIUS {
                    return Err(GpuUnsupportedReason::CamRadiusTooFar { head: head.0, rule_idx, radius: cam.radius });
                }
                // CAM всегда нуждается в арбитраже (две клетки-магнита могут
                // притянуть одну и ту же цель) — см. doc-комментарий
                // `needs_arbitration` ниже.
                needs_arbitration = true;
            }

            // Реальный охват ЭТОГО правила — та же формула, что и у
            // `MAX_MARGIN` (shift_reach + change_reach), но по фактическим
            // (а не предельно допустимым) значениям, см. doc-комментарий
            // `GpuRuleTable::margin`. `cam.radius` — тот же смысл: диск
            // радиуса R вокруг магнита — единственное, что правило реально
            // затрагивает (source-clear найденной клетки + сама позиция
            // магнита), консервативно ограничено радиусом с любой стороны.
            let shift_reach = shift_deltas.iter().map(|&(dx, dy)| dx.abs().max(dy.abs())).max().unwrap_or(0);
            let change_reach = rule.changes.iter().map(|&(dx, dy, _)| dx.abs().max(dy.abs())).max().unwrap_or(0);
            let cam_reach = rule.cam.map_or(0, |c| c.radius as i32);
            // Каскад `recursion`: самый дальний уровень `max_depth` сам
            // отстоит на `max_depth` клеток от исходного матча, а его
            // собственные `changes` расширяют охват ещё на `change_reach` —
            // та же логика, что у `shift_reach + change_reach` для обычных
            // сдвигов, только смещение даёт каскад, а не сам сдвиг.
            let recursion_reach = rule.recursion.map_or(0, |spec| spec.max_depth as i32) + change_reach;
            let rule_margin = if shift_deltas.is_empty() { change_reach } else { shift_reach + change_reach }.max(cam_reach).max(recursion_reach);
            margin = margin.max(rule_margin);

            if !shift_deltas.is_empty()
                && matches!(rule.overflow, OverflowAction::Write(_) | OverflowAction::WriteLiteral(_))
            {
                return Err(GpuUnsupportedReason::OverflowNotDiscard { head: head.0, rule_idx });
            }
            if !shift_deltas.is_empty() || rule.changes.iter().any(|&(dx, dy, _)| (dx, dy) != (0, 0)) {
                needs_arbitration = true;
            }
            // `recursion`: ДАЖЕ если объявленные `changes` целиком
            // self-referential ((0,0) относительно исходного матча —
            // единственный случай, не пойманный проверкой выше), каскад
            // `k = 1..=max_depth` пишет их же СМЕЩЁННЫМИ на `k × direction`
            // от исходной позиции — то есть в НЕ-self клетки, независимо от
            // того, что было объявлено. Без этой отдельной проверки
            // rule-с-только-self-changes-но-с-recursion молча получил бы
            // Simple (не арбитражный) пайплайн, хотя реально пишет вне
            // своей клетки, стоит max_depth > 0.
            if rule.recursion.is_some_and(|spec| spec.max_depth > 0) {
                needs_arbitration = true;
            }
            // `starvation_after`: осмысленно только под реальной
            // конкуренцией за клетку — форсируем Arbitrated-пайплайн
            // безусловно, даже если это единственная причина у ИНАЧЕ
            // self-write-only конфига (та же логика, что и у recursion
            // выше: свойство самого правила, не то, что можно вывести из
            // формы `changes`/`shifts`).
            if rule.starvation_after.is_some() {
                needs_arbitration = true;
                needs_starvation = true;
            }
            // `feedback` уже гарантированно требует ровно один сдвиг —
            // needs_arbitration уже станет true через обычную проверку
            // "есть сдвиг" ниже, форсировать здесь незачем; но
            // needs_feedback нужно отметить явно.
            if rule.feedback.is_some() {
                needs_feedback = true;
            }
            // `memory`: в отличие от `starvation_after`/`recursion` выше,
            // форсируем `needs_arbitration` БЕЗУСЛОВНО, даже для правила,
            // которое иначе (без `memory`) было бы чистым self-write —
            // персистентные буферы гейта существуют только в инфраструктуре
            // Arbitrated-пайплайна (см. `GpuRuleTable::needs_memory`'s
            // doc-комментарий), Simple-пайплайн их не заводит вовсе.
            if rule.memory.is_some() {
                needs_arbitration = true;
                needs_memory = true;
            }

            let mut change_fields = [(0i32, 0i32, 0u32); MAX_CHANGES];
            for (i, &(dx, dy, value)) in rule.changes.iter().enumerate() {
                let literal = match value {
                    ChangeValue::Literal(v) => v as u32,
                    ChangeValue::Ref(_) => return Err(GpuUnsupportedReason::ChangeIsRef { head: head.0, rule_idx }),
                };
                change_fields[i] = (dx, dy, literal);
            }

            let pattern = effective_pattern(rule);
            if pattern.len() > MAX_PATTERN_OFFSETS {
                return Err(GpuUnsupportedReason::PatternTooLarge {
                    head: head.0,
                    rule_idx,
                    len: pattern.len(),
                });
            }
            if rule.id.len() > MAX_ID_BYTES {
                return Err(GpuUnsupportedReason::RuleIdTooLong {
                    head: head.0,
                    rule_idx,
                    len: rule.id.len(),
                });
            }

            for &(dx, dy, _) in &pattern {
                pattern_reach = pattern_reach.max((dx as i32).abs().max((dy as i32).abs()));
            }

            let pattern_start = pattern_offsets.len() as u32;
            for &(dx, dy, expected) in &pattern {
                pattern_offsets.push(GpuPatternOffset {
                    dx: dx as i32,
                    dy: dy as i32,
                    expected: expected.0 as u32,
                    _pad: 0,
                });
                if union_seen.insert((dx, dy)) {
                    union_offsets.push((dx, dy));
                }
            }

            let mut id_bytes = [0u32; MAX_ID_BYTES];
            for (i, ct) in rule.id.iter().enumerate() {
                id_bytes[i] = ct.0 as u32;
            }

            let mut shift_fields = [(0i32, 0i32); MAX_SHIFTS];
            for (i, &(dx, dy)) in shift_deltas.iter().enumerate() {
                shift_fields[i] = (dx, dy);
            }
            let mut shift_broadcast_fields = [false; MAX_SHIFTS];
            for (i, &b) in shift_broadcasts.iter().enumerate() {
                shift_broadcast_fields[i] = b;
            }
            let mut shift_keep_source_fields = [false; MAX_SHIFTS];
            for (i, &b) in shift_keep_sources.iter().enumerate() {
                shift_keep_source_fields[i] = b;
            }

            rules.push(GpuRule {
                pattern_start,
                pattern_len: pattern.len() as u32,
                priority: rule.priority,
                min_age: rule.min_age.min(u32::MAX as u64) as u32,
                active_only: rule.active_only as u32,
                id_len: rule.id.len() as u32,
                id_b0: id_bytes[0],
                id_b1: id_bytes[1],
                id_b2: id_bytes[2],
                id_b3: id_bytes[3],
                id_b4: id_bytes[4],
                id_b5: id_bytes[5],
                id_b6: id_bytes[6],
                id_b7: id_bytes[7],
                rule_idx: rule_idx as u32,
                shift_count: shift_deltas.len() as u32,
                shift_dx0: shift_fields[0].0,
                shift_dy0: shift_fields[0].1,
                shift_dx1: shift_fields[1].0,
                shift_dy1: shift_fields[1].1,
                shift_broadcast0: shift_broadcast_fields[0] as u32,
                shift_broadcast1: shift_broadcast_fields[1] as u32,
                shift_keep_source0: shift_keep_source_fields[0] as u32,
                shift_keep_source1: shift_keep_source_fields[1] as u32,
                change_count: rule.changes.len() as u32,
                change_dx0: change_fields[0].0,
                change_dy0: change_fields[0].1,
                change_val0: change_fields[0].2,
                change_dx1: change_fields[1].0,
                change_dy1: change_fields[1].1,
                change_val1: change_fields[1].2,
                change_dx2: change_fields[2].0,
                change_dy2: change_fields[2].1,
                change_val2: change_fields[2].2,
                change_dx3: change_fields[3].0,
                change_dy3: change_fields[3].1,
                change_val3: change_fields[3].2,
                cam_radius: rule.cam.map_or(0, |c| c.radius as u32),
                cam_target_type: rule.cam.map_or(0, |c| c.target_type.0 as u32),
                tie_break: rule.tie_break,
                recursion_max_depth: rule.recursion.map_or(0, |spec| spec.max_depth as u32),
                recursion_dx: rule.recursion.map_or(0, |spec| recursion_direction_delta(spec.direction).0),
                recursion_dy: rule.recursion.map_or(0, |spec| recursion_direction_delta(spec.direction).1),
                has_starvation: rule.starvation_after.is_some() as u32,
                starvation_threshold: rule.starvation_after.unwrap_or(0),
                has_feedback: rule.feedback.is_some() as u32,
                feedback_timeout: rule.feedback.map_or(0, |spec| spec.timeout.min(u32::MAX as u64) as u32),
                feedback_alt_dx: rule.feedback.map_or(0, |spec| recursion_direction_delta(spec.new_direction).0),
                feedback_alt_dy: rule.feedback.map_or(0, |spec| recursion_direction_delta(spec.new_direction).1),
                has_memory: rule.memory.is_some() as u32,
                memory_window: rule.memory.as_ref().map_or(0, |spec| spec.window as u32),
                memory_trigger: rule.memory.as_ref().map_or(0, |spec| match spec.record_trigger {
                    RecordTrigger::NeighborType(_) => 0,
                    RecordTrigger::RuleOutcome => 1,
                }),
                memory_dx: rule.memory.as_ref().map_or(0, |spec| match spec.record_trigger {
                    RecordTrigger::NeighborType(dir) => recursion_direction_delta(dir).0,
                    RecordTrigger::RuleOutcome => 0,
                }),
                memory_dy: rule.memory.as_ref().map_or(0, |spec| match spec.record_trigger {
                    RecordTrigger::NeighborType(dir) => recursion_direction_delta(dir).1,
                    RecordTrigger::RuleOutcome => 0,
                }),
                memory_has_shift: rule.memory.is_some() as u32 * (shift_deltas.len() == 1) as u32,
                memory_pattern0: rule.memory.as_ref().and_then(|spec| spec.match_pattern.first()).map_or(0, |&v| encode_recorded_value(v)),
                memory_pattern1: rule.memory.as_ref().and_then(|spec| spec.match_pattern.get(1)).map_or(0, |&v| encode_recorded_value(v)),
                memory_pattern2: rule.memory.as_ref().and_then(|spec| spec.match_pattern.get(2)).map_or(0, |&v| encode_recorded_value(v)),
                memory_pattern3: rule.memory.as_ref().and_then(|spec| spec.match_pattern.get(3)).map_or(0, |&v| encode_recorded_value(v)),
            });
        }

        let rules_count = rules.len() as u32 - rules_start;

        let offsets_start = head_offsets.len() as u32;
        for (dx, dy) in union_offsets {
            head_offsets.push(GpuOffset { dx: dx as i32, dy: dy as i32 });
        }
        let offsets_count = head_offsets.len() as u32 - offsets_start;

        head_slots[head.0 as usize] = GpuHeadSlot { rules_start, rules_count, offsets_start, offsets_count };
    }

    let mut max_matches_per_cell: u32 = 0;
    if needs_arbitration {
        for (head, slot) in head_slots.iter().enumerate() {
            if slot.rules_count as usize > MAX_MATCHES_PER_CELL {
                return Err(GpuUnsupportedReason::TooManyRulesForArbitration {
                    head: head as u8,
                    len: slot.rules_count as usize,
                });
            }
            max_matches_per_cell = max_matches_per_cell.max(slot.rules_count);
        }
    }

    // `margin` не используется вовсе, если арбитраж не нужен (Simple-пайплайн
    // не трогает дополненную сетку) — обнуляем явно, а не оставляем
    // "правдоподобное, но неиспользуемое" значение.
    let margin = if needs_arbitration { margin } else { 0 };

    Ok(GpuRuleTable {
        head_slots,
        rules,
        pattern_offsets,
        head_offsets,
        needs_arbitration,
        margin,
        max_matches_per_cell,
        pattern_reach,
        needs_starvation,
        needs_feedback,
        needs_memory,
    })
}

#[cfg(test)]
#[path = "rule_table_tests.rs"]
mod tests;
