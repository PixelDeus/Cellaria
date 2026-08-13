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

#[path = "rule_table_builder.rs"]
mod rule_table_builder;
pub use rule_table_builder::build_gpu_rule_table;

/// Тот же потолок, что у `matcher::GroupData::packed_patterns` — сохранён
/// для единообразия, сама упаковка здесь не используется.
pub const MAX_PATTERN_OFFSETS: usize = 16;

/// Потолок длины `Rule::id`, участвующей в тай-брейке арбитража на GPU.
pub const MAX_ID_BYTES: usize = 8;

/// Потолок числа независимых сдвигов на правило.
pub const MAX_SHIFTS: usize = 2;

/// Потолок числа `changes` на правило.
pub const MAX_CHANGES: usize = 4;

/// Потолок |dx|/|dy| одного сдвига.
pub const MAX_SHIFT_REACH: i32 = 12;

/// Потолок |dx|/|dy| одного `changes`-смещения.
pub const MAX_CHANGE_REACH: i32 = 4;

/// Потолок `ShiftSpec::steps` для `broadcast: true` — отдельный, более узкий,
/// чем [`MAX_SHIFT_REACH`]. Почему именно это число и как оно связано с
/// размером `GpuMatch::cells` в шейдере — `specs/architecture.md` §8.
pub const MAX_BROADCAST_REACH: i32 = 4;

/// Потолок радиуса `CamSearch` на GPU (CPU не ограничен искусственно) —
/// почему GPU нужен реальный потолок — `specs/architecture.md` §8.
pub const MAX_CAM_RADIUS: u8 = 16;

/// Потолок `RecursionSpec::max_depth` на GPU. Почему `recursion`
/// GPU-совместим (в отличие от `feedback`/`memory`/`starvation_after`) —
/// `specs/architecture.md` §8.
pub const MAX_RECURSION_DEPTH: u8 = 4;

/// Потолок `MemorySpec::window` на GPU — почему длина ограничена только этой
/// константой, не техническим лимитом WGSL — `specs/architecture.md` §8.
pub const MAX_MEMORY_WINDOW: usize = 4;

/// Максимальная дистанция (по любой оси) от клетки-источника матча до любой
/// потенциально затронутой клетки — `MAX_SHIFT_REACH + MAX_CHANGE_REACH`.
/// Почему именно эта сумма нужна для паритета с CPU-арбитражем у края
/// решётки (реальный найденный баг) — `specs/architecture.md` §8.
pub const MAX_MARGIN: i32 = MAX_SHIFT_REACH + MAX_CHANGE_REACH;

/// Потолок числа правил одной головы при полном арбитраже
/// (`GpuRuleTable::needs_arbitration == true`) — размер статически
/// резервируемых слотов кандидатов на клетку в `shader.wgsl::GpuMatch`.
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
    /// булев флаг, не "0 = выключено" (как у `cam_radius`/`recursion_max_depth`
    /// выше) — намеренно: `starvation_after: Some(0)` РЕАЛЬНЫЙ, отличный от
    /// "выключено" случай (побеждает через голодание с первого же тика),
    /// кодировать нулём означало бы тихо путать его с `None`.
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
    /// 1, если у правила с `memory` есть ровно один сдвиг (буфер переезжает
    /// на новую позицию при выигранном сдвиге, см.
    /// `update_memory_relocate_pass` в `shader.wgsl`), 0 — сдвига нет вовсе
    /// (буфер живёт на фиксированной позиции). `MemorySpec` допускает только
    /// 0 или 1 сдвиг — плоский булев, не "число сдвигов", потому что других
    /// значений тут быть не может.
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
    if shift_path > recursion_path {
        shift_path
    } else {
        recursion_path
    }
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
    /// `ChangeValue::Add`/`Sub` — вне подмножества (та же категория, что
    /// `ChangeIsRef`: нет шейдерного пути для вычисления значения на
    /// GPU-стороне, всё подмножество ниже ожидает готовый `u32`-литерал).
    /// CPU поддерживает безусловно (см. `ChangeValue`'s doc-комментарий) —
    /// правило целиком уходит в CPU-fallback, как и с `Ref`.
    ChangeIsArithmetic { head: u8, rule_idx: usize },
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
    ShiftTooFar {
        head: u8,
        rule_idx: usize,
        dx: i32,
        dy: i32,
    },
    /// `|dx|`/`|dy`| `changes`-смещения больше [`MAX_CHANGE_REACH`].
    ChangeTooFar {
        head: u8,
        rule_idx: usize,
        dx: i32,
        dy: i32,
    },
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
        rule.id.iter().enumerate().map(|(i, &ct)| (i as i8, 0i8, ct)).collect()
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

#[cfg(test)]
#[path = "rule_table_tests.rs"]
mod tests;
