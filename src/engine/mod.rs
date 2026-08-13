pub mod applicator;
pub mod arbitrator;
pub mod matcher;
mod pipeline;
mod rule_state;

// `impl<S: GridStorage> Engine<S>` разбит по concern'ам на несколько файлов
// (35 методов, ~800 строк в одном блоке — самые крупные группы см. в
// CHANGELOG.md), каждый со СВОИМ `impl` блоком для того же `Engine<S>` —
// стандартная и полностью безопасная в Rust практика (компилятор просто
// собирает все `impl`-блоки одного типа воедино, независимо от файла).
mod io;
mod lifecycle;
mod raw_phases;
mod rules;
mod state_mgmt;
mod tick;

pub use state_mgmt::{CompositionVerdict, TerminationVerdict};

pub use pipeline::run_tick;
pub use pipeline::{TickLogEntry, TickPhaseTimings};
pub(crate) use pipeline::{build_head_index, lookup_rule, HeadRuleIndex};
// Реэкспорт только для `super::super::*` в `engine::tests::*` (unused-import
// лишь потому, что сам mod.rs не вызывает эту функцию напрямую).
#[allow(unused_imports)]
pub(crate) use pipeline::reset_age_for_regions;
use pipeline::{
    compute_conflict_partners, compute_extension_flags, compute_search_radius_cache, resolve_search_coords_advance,
    resolve_search_coords_peek, run_tick_with_cache, ConflictContext, ExtensionFlags, SearchRadiusCache,
    TickEventCounts,
};

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::conflict_analyzer::build_rule_data_cache;
use crate::fast_hash::{FxHashMap, FxHashSet};
use crate::grid::Grid;
use crate::storage::GridStorage;
use crate::types::{
    AffectedRegion, Cell, CellType, CellValue, RecordTrigger, RecordedValue, Rule, RuleMatch, DEFAULT_CELL_VALUE,
};

pub use applicator::apply_matches;
use applicator::{apply_matches_with_cam, WriteBuffer};
pub use arbitrator::{arbitrate, arbitrate_spatial};
use arbitrator::{arbitrate_spatial_with_cam, arbitrate_with_cam};
pub use matcher::detect_matches;
use matcher::{build_group_data, detect_cam_matches, detect_matches_with_group_data, GroupCache};
use rule_state::RuleStateStore;

/// Двигатель симуляции Cellaria.
pub struct Engine<S: GridStorage> {
    pub grid: Grid<S>,
    /// Приватно с [`Engine::rule_index`] (точка 4, сессия 2026-08-09) — единственный
    /// способ ИЗМЕНИТЬ состав правил извне это [`Engine::set_rule_index`]/
    /// [`Engine::set_rules_for_head`], которые сами вызывают
    /// [`Engine::rebuild_rule_cache`]. Раньше поле было `pub`, и прямая правка
    /// в обход `rebuild_rule_cache` молча ломала кэши/счётчики (см.
    /// doc-комментарий `rebuild_rule_cache`) — только дисциплина
    /// разработчика "не забыть вызвать" не давала это гарантий.
    rule_index: HashMap<CellType, Vec<Rule>>,
    pub rule_cache: crate::conflict_analyzer::RuleDataCache,
    /// Кэш `GroupData` матчера (эффективные паттерны, offset-карты,
    /// упакованные u128-паттерны) — то же самое соображение, что и у
    /// `rule_cache`: `matcher::detect_matches` (свободная функция)
    /// пересобирает такой кэш заново на каждый вызов, что заметно на
    /// разреженных сценариях с частыми тиками (см. doc-комментарий
    /// `matcher::GroupCache`). `Engine` строит его один раз и переиспользует.
    group_cache: GroupCache,
    /// `min_age_gated_types`/`max_pattern_radius`/`zero_head_radius`,
    /// посчитанные один раз — см. doc-комментарий `SearchRadiusCache`.
    search_radius_cache: SearchRadiusCache,
    /// "Использует ли ХОТЬ ОДНО правило набора данное поле" — три флага,
    /// посчитанные один раз из `rule_index`, тот же приём и та же причина,
    /// что и у `search_radius_cache` (см. doc-комментарий
    /// `ExtensionFlags`).
    extension_flags: ExtensionFlags,
    /// Структурная информация из `ConflictGraph`, посчитанная один раз (не на
    /// каждый тик) — используется `run_tick_with_cache`, чтобы не гонять
    /// полный арбитраж для матчей, у которых физически нет никого рядом, кто
    /// мог бы с ними столкнуться.
    ///
    /// Уточнение предыдущей версии ("голова либо безопасна ГЛОБАЛЬНО и
    /// НАВСЕГДА, либо нет"): та версия проверяла ГОЛОВУ целиком — если у
    /// головы есть хоть одно ребро в графе конфликтов (с ЛЮБОЙ другой
    /// головой, на любом расстоянии), она выключала оптимизацию для себя
    /// НАВСЕГДА. Реальный пример (`big_world.rs`): провод и распад (5 голов,
    /// статичные) сами по себе бесконфликтны, но стоит на той же решётке
    /// (за миллион клеток) появиться челноку (сдвигается — см. давно
    /// известный "moving object"-лимит статического анализа), и структурно
    /// он конфликтует уже со ВСЕМИ 7 головами — включая провод и распад,
    /// хотя физически они никогда не соприкасаются. Старая версия гасила
    /// быстрый путь для всей решётки целиком из-за одной детали за миллион
    /// клеток. Здесь вместо одного глобального решения — ДВЕ ступени:
    /// `conflict_partners` только называет, какие ГОЛОВЫ структурно МОГЛИ
    /// БЫ столкнуться (как и раньше), а решение "нужен ли арбитраж
    /// конкретному совпадению" принимается КАЖДЫЙ тик заново — только если
    /// хоть один партнёр этой головы реально совпал где-то в пределах
    /// досягаемости (`max_affected_radius`) от НЕЁ САМОЙ (см.
    /// `spatial_bypass_split`). Провод и распад проходят напрямую каждый
    /// тик, как и должны — челнок им попросту не сосед.
    conflict_partners: FxHashMap<CellType, FxHashSet<CellType>>,
    /// Максимальный "радиус" (наибольшее расстояние по x/y от позиции
    /// совпадения до затронутой клетки — `RuleData::bbox`) среди ВСЕХ
    /// правил набора. Два совпадения дальше `2 × max_affected_radius` друг
    /// от друга по любой оси НИКОГДА не могут иметь пересекающихся
    /// affected-регионов, какие бы конкретно правила ни сработали — граница
    /// консервативна (общий максимум по всем правилам, не по паре), но
    /// звучит, и этого достаточно для корректного пространственного отсева
    /// в `spatial_bypass_split`.
    max_affected_radius: i32,
    /// Если включена самомодификация (см. [`Engine::enable_self_modification`]),
    /// здесь живёт декодер протокола `RuleStore` — `run_tick` сам, без
    /// внешнего кода, дренирует канал 0 выходных граничных буферов после
    /// каждого тика и применяет готовые операции к `rule_index`. `None`
    /// (по умолчанию) означает "как раньше" — нулевые накладные расходы и
    /// нулевая разница в поведении для кода, который об этом не просил.
    self_mod: Option<crate::rule_store::RuleStore>,
    /// Если true (см. [`Engine::enable_guarded_self_modification`]) — новый
    /// самопереданный `AddRule` с id, которого ЕЩЁ НЕТ в `rule_index`,
    /// принимается только если доказуемо не конфликтует с ОСТАЛЬНЫМ текущим
    /// набором правил ([`crate::ConflictGraph::check_composition`]).
    /// Расширение УЖЕ существующего id, добавленного САМОЙ самомодификацией
    /// (например, increment/finalize — `strength_self_modification_computed.rs`),
    /// не проверяется: риск композиции — в столкновении с ЧУЖОЙ территорией,
    /// а не в том, что модуль развивает своё же поведение. "Чужая
    /// территория" — это `protected_heads`: id, существовавшие ДО начала
    /// самомодификации, а не то, что она сама успела вырастить.
    guard_self_modification: bool,
    /// Снимок "чужой территории" — правил, которыми `RuleStore` НЕ
    /// управляет (пришли в обход протокола, либо стояли в `rule_index` с
    /// самого начала, либо были добавлены напрямую позже — оба случая
    /// равноправны). И граница защиты в [`Engine::enable_guarded_self_modification`]
    /// (её ключи — "protected heads"), и то, что обязано пережить любое
    /// слияние в `absorb_self_modifications` — одно и то же множество.
    ///
    /// Инициализируется в [`Engine::enable_self_modification`] (снимок
    /// `rule_index` НА ТОТ МОМЕНТ, а не на момент `Engine::new` — код может
    /// вполне законно менять `rule_index` напрямую между созданием движка и
    /// включением самомодификации, как `strength_live_rules.rs`, и такие
    /// правила ничем не отличаются от заданных при `Engine::new`).
    /// Дальше пересчитывается заново при каждом вызове [`Engine::rebuild_rule_cache`]
    /// (документированная точка входа "я поменял `rule_index` напрямую") —
    /// как "то, что сейчас в `rule_index`, за вычетом того, чем сейчас
    /// реально владеет `RuleStore`" — так что и более ПОЗДНИЕ прямые правки
    /// (после того как самомодификация уже что-то на лету добавила)
    /// корректно учитываются, а не теряются.
    ///
    /// Область действия честно ограничена: защищает только то, чем
    /// `RuleStore` не управляет НА ДАННЫЙ МОМЕНТ. Гонку МЕЖДУ двумя
    /// самомодифицирующимися регионами за один и тот же, ранее НИКЕМ не
    /// занятый id эта версия не разбирает — тот, кто застолбит id первым,
    /// для всех последующих посылок на тот же id считается "уже
    /// существующим" и больше не проверяется. Общая многосторонняя гонка за
    /// территорию — отдельная, более сложная задача.
    original_rule_index: HashMap<CellType, Vec<Rule>>,
    /// Голова-типы, за которыми прямо сейчас числится хоть одно
    /// самопереданное правило (используется вместе с `original_rule_index`
    /// для корректной обработки `RemoveRule`/`ClearAll`: если у головы,
    /// которой самомодификация когда-то управляла, самопереданных правил
    /// больше не осталось, `rule_index[head]` нужно вернуть к ОРИГИНАЛУ
    /// (или убрать вовсе, если оригинала и не было) — а не оставить
    /// висеть устаревшую копию, как было раньше `get_index()` только
    /// добавлял ключи и никогда не убирал исчезнувшие).
    self_mod_managed_heads: FxHashSet<CellType>,
    /// Сколько раз [`Engine::enable_guarded_self_modification`] отклонила
    /// самопереданное правило как небезопасное для композиции.
    pub rejected_self_modifications: u64,
    /// Всё персистентное (живущее между тиками) состояние правил —
    /// `starvation_counters`/`feedback_counters`/`memory_buffers`/
    /// `activation_counters` — в одной точке, за типобезопасным
    /// snapshot/writer-доступом. Раньше это были четыре отдельных поля
    /// прямо здесь; вынесено в `rule_state::RuleStateStore` (пп.3+5,
    /// сессия 2026-08-09) — см. её подробный doc-комментарий модуля про то,
    /// что это не просто перегруппировка полей, а формализация дисциплины
    /// снимка тика (2.2.1) в типах, plus единая точка инвалидации при
    /// изменении состава правил (бывший `last_rebuilt_rule_index`, теперь
    /// тоже внутри неё).
    state: RuleStateStore,
    /// Журнал вызовов `push_input`, только пока запись включена
    /// ([`Engine::enable_input_recording`]) — см. [`InputEvent`] и
    /// [`Engine::replay`]. `None` (по умолчанию) — нулевые накладные
    /// расходы, `push_input` не делает лишней работы для кода, который об
    /// этом не просил.
    input_log: Option<Vec<InputEvent>>,
    /// Структурированный лог событий тика (п.5, сессия 2026-08-09), только
    /// пока запись включена ([`Engine::enable_tick_logging`]) — см.
    /// [`TickLogEntry`]. Тот же принцип, что и `input_log` выше: `None` по
    /// умолчанию, `run_tick` не считает счётчики событий, если их некуда
    /// записывать.
    tick_log: Option<Vec<TickLogEntry>>,
    /// Персистентный буфер для `apply_matches_with_cam` (п.4, сессия
    /// 2026-08-09) — переиспользуется между тиками через `.clear()` вместо
    /// `WriteBuffer::default()` заново на каждый тик. Измерено (generic
    /// `HashMap`, не специфично для `FxHashMap`): ~58.6% экономии на
    /// реалистичном объёме записей на тик. Чистое царапина-состояние
    /// (никогда не читается ДО заполнения в рамках одного и того же
    /// вызова `apply_matches_with_cam`, который сам чистит его в начале) —
    /// не часть `EngineSnapshot`, как и `rule_cache`/`group_cache`.
    write_buffer: WriteBuffer,
    /// Персистентный буфер значений паттерна для динамических ссылок
    /// (`$0`/`$1`/...) внутри `apply_rule_buffered` (третья оптимизация
    /// того же класса, что `write_buffer` выше, сессия 2026-08-09) —
    /// переиспользуется через `.clear()` вместо `Vec::new()` на каждый
    /// match с непустым `pattern`. Та же оговорка про "чистое
    /// царапина-состояние", не часть `EngineSnapshot`.
    pattern_buffer: Vec<CellValue>,
}

/// Один вызов `Engine::push_input` — `tick` это `grid.generation()` НА
/// МОМЕНТ вызова (то есть поколение, которое ЕЩЁ НЕ наступило — станет
/// текущим на следующем `run_tick`, чья фаза Input и заберёт значение из
/// очереди). Для [`Engine::replay`] это единственный способ узнать, ПЕРЕД
/// каким по счёту тиком нужно повторно вызвать `push_input`, а не просто
/// "когда-то до конца записи".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputEvent {
    pub tick: u64,
    pub channel: u32,
    pub value: u8,
}

/// Сохраняемый снимок состояния симуляции — см. [`Engine::snapshot`]/
/// [`Engine::from_snapshot`]. Содержит РОВНО то, что не выводится заново из
/// остального: `grid`, состав правил, персистентные счётчики расширений,
/// самомодификационное состояние. НЕ содержит `rule_cache`/`group_cache`/
/// `search_radius_cache`/`extension_flags`/`conflict_partners`/
/// `max_affected_radius` — все они чистые функции от `rule_index`,
/// `Engine::new` (внутри `from_snapshot`) пересобирает их заново, хранить
/// их в снимке было бы избыточной, никогда не читаемой копией.
///
/// Поля намеренно приватные — работа со снимком идёт через `Serialize`/
/// `Deserialize` (любой формат serde), а не через прямой доступ к полям;
/// `RuleStateStore`/`FxHashSet` (внутренние, `pub(crate)` типы) иначе не
/// смогли бы появиться в сигнатуре публичной структуры.
///
/// **НЕ `serde_json`** — `rule_index` (ключ `CellType`), `Grid::boundaries`
/// (ключ `(usize,usize)`) и все карты внутри `RuleStateStore` (ключи вида
/// `(u32,u32,usize)`) используют НЕ-строковые ключи `HashMap`, а JSON
/// требует, чтобы ключи объекта были строками — `serde_json::to_string`
/// падает с "key must be a string" (найдено эмпирически, см. тест). Любой
/// формат без этого ограничения подходит — `serde_yaml` (уже зависимость
/// проекта, проверено тестом), `bincode`, MessagePack и т.п.
#[derive(Serialize, Deserialize)]
#[serde(bound(serialize = "S: Serialize", deserialize = "S: serde::de::DeserializeOwned"))]
pub struct EngineSnapshot<S: GridStorage> {
    grid: Grid<S>,
    rule_index: HashMap<CellType, Vec<Rule>>,
    state: RuleStateStore,
    self_mod: Option<crate::rule_store::RuleStore>,
    guard_self_modification: bool,
    original_rule_index: HashMap<CellType, Vec<Rule>>,
    self_mod_managed_heads: FxHashSet<CellType>,
    rejected_self_modifications: u64,
}

// ──────────────────────────────────────────────────────────────
// Тесты
// ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
