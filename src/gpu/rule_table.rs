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
use crate::types::{CellType, ChangeValue, Direction, OverflowAction, Rule};

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

/// Потолок радиуса `CamSearch` на GPU — в отличие от CPU (`CamSearch::radius:
/// u8`, до 255, без искусственного потолка), GPU-версия сканирует диск
/// (2R+1)² на клетку-кандидата КАЖДЫЙ тик (см. `shader.wgsl::cam_search`) и
/// раздувает дополненную сетку арбитража на `2×radius` по каждой оси (тот же
/// механизм, что и `margin` для сдвигов) — оба растут квадратично/линейно с
/// R, так что здесь нужен реальный потолок, а не только желание. 16 — вдвое
/// больше `MAX_SHIFT_REACH`, разумный запас для "найди ближайшую цель
/// поблизости" сценариев без превращения диска в скан половины решётки.
pub const MAX_CAM_RADIUS: u8 = 16;

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

/// Потолок ячеек записи ОДНОГО матча: source-clear (1, только если есть
/// сдвиг) + на каждый сдвиг — сама цель + её `changes`. Зеркалит
/// `shader.wgsl`'s `MAX_WRITE_CELLS` — держать их синхронно ОБЯЗАТЕЛЬНО
/// (см. doc-комментарий [`GpuMatchLayout`]).
pub const MAX_WRITE_CELLS: usize = 1 + MAX_SHIFTS * (1 + MAX_CHANGES);

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
    pub cells: [u32; MAX_WRITE_CELLS],
    pub values: [u32; MAX_WRITE_CELLS],
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
    /// `ShiftSpec::broadcast` — вне подмножества: шейдер пишет только
    /// финальную точку пути сдвига, не весь путь (см. её doc-комментарий
    /// в `types.rs` и валидацию в `build_gpu_rule_table`).
    BroadcastShiftUnsupported { head: u8, rule_idx: usize },
    /// `Rule::starvation_after` — вне подмножества: требует персистентных
    /// МЕЖДУ тиками счётчиков (`Engine::starvation_counters`), а GPU-путь
    /// (`GpuEngine`) не хранит никакого CPU-состояния между вызовами
    /// `run_tick` вообще (см. её doc-комментарий в `types.rs`) — портировать
    /// значило бы держать ещё один storage-буфер и синхронизировать его с
    /// CPU-side `HashMap` на каждый тик, что сводит на нет саму цель GPU-пути
    /// (минимизировать CPU↔GPU трафик). Явный отказ вместо молчаливого
    /// игнорирования поля — та же философия, что и `BroadcastShiftUnsupported`.
    StarvationGuardUnsupported { head: u8, rule_idx: usize },
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
            // `broadcast` — вне подмножества: шейдер сейчас пишет только
            // финальную точку пути (см. `push_write_cell`/`detect_pass`),
            // молча посчитать broadcast-сдвиг как обычный означало бы
            // потерять все промежуточные клетки — неверный результат
            // вместо явного отказа (см. doc-комментарий модуля про то же
            // обязательство для `ChangeValue::Ref`/`cam`).
            if rule.shifts.iter().flatten().any(|s| s.broadcast) {
                return Err(GpuUnsupportedReason::BroadcastShiftUnsupported { head: head.0, rule_idx });
            }
            if rule.starvation_after.is_some() {
                return Err(GpuUnsupportedReason::StarvationGuardUnsupported { head: head.0, rule_idx });
            }
            let shift_deltas: Vec<(i32, i32)> = rule.shifts.iter().flatten().map(shift_delta).collect();
            if shift_deltas.len() > MAX_SHIFTS {
                return Err(GpuUnsupportedReason::TooManyShifts { head: head.0, rule_idx, len: shift_deltas.len() });
            }
            for &(dx, dy) in &shift_deltas {
                if dx.abs() > MAX_SHIFT_REACH || dy.abs() > MAX_SHIFT_REACH {
                    return Err(GpuUnsupportedReason::ShiftTooFar { head: head.0, rule_idx, dx, dy });
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
            let rule_margin = if shift_deltas.is_empty() { change_reach } else { shift_reach + change_reach }.max(cam_reach);
            margin = margin.max(rule_margin);

            if !shift_deltas.is_empty()
                && matches!(rule.overflow, OverflowAction::Write(_) | OverflowAction::WriteLiteral(_))
            {
                return Err(GpuUnsupportedReason::OverflowNotDiscard { head: head.0, rule_idx });
            }
            if !shift_deltas.is_empty() || rule.changes.iter().any(|&(dx, dy, _)| (dx, dy) != (0, 0)) {
                needs_arbitration = true;
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
    })
}

#[cfg(test)]
#[path = "rule_table_tests.rs"]
mod tests;
