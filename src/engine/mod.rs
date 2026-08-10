pub mod applicator;
pub mod arbitrator;
pub mod matcher;
mod rule_state;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::conflict_analyzer::build_rule_data_cache;
use crate::fast_hash::{FxHashMap, FxHashSet};
use crate::grid::Grid;
use crate::storage::GridStorage;
use crate::types::{
    AffectedRegion, Cell, CellType, CellValue, RecordTrigger, RecordedValue, Rule, RuleMatch,
    DEFAULT_CELL_VALUE,
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

impl<S: GridStorage> Engine<S> {
    /// Создать новый двигатель с решёткой и индексом правил.
    pub fn new(
        grid: Grid<S>,
        rule_index: HashMap<CellType, Vec<Rule>>,
    ) -> Self {
        let rule_cache = build_rule_data_cache(&rule_index);
        let group_cache = build_group_data(&rule_index);
        let search_radius_cache = compute_search_radius_cache(&rule_index);
        let extension_flags = compute_extension_flags(&rule_index);
        let (conflict_partners, max_affected_radius) = compute_conflict_partners(&rule_index, &rule_cache);
        // Заполняет `RuleStateStore`'s внутренний снимок `rule_index` СРАЗУ,
        // а не оставляет его пустым до первого `rebuild_rule_cache()` — иначе
        // первый же вызов `rebuild_rule_cache()` после нескольких реальных
        // тиков увидел бы ВСЕ текущие правила как "новые" (диф против
        // пустоты) и ошибочно счёл бы уже накопленные счётчики устаревшими.
        // Счётчики здесь ещё пусты (только что созданы), так что сама
        // чистка — no-op, важен только побочный эффект — правильный снимок.
        let mut state = RuleStateStore::default();
        state.invalidate_stale(&rule_index, &grid);
        Self {
            grid,
            rule_index,
            rule_cache,
            group_cache,
            search_radius_cache,
            extension_flags,
            conflict_partners,
            max_affected_radius,
            self_mod: None,
            guard_self_modification: false,
            // Заполняется в `enable_self_modification` — снимок берётся ТАМ,
            // а не здесь, поскольку `rule_index` может законно поменяться
            // напрямую между `Engine::new` и включением самомодификации.
            original_rule_index: HashMap::new(),
            self_mod_managed_heads: FxHashSet::default(),
            rejected_self_modifications: 0,
            state,
            input_log: None,
            tick_log: None,
            write_buffer: WriteBuffer::default(),
            pattern_buffer: Vec::new(),
        }
    }

    /// Снимок текущего состояния для сохранения (см. [`EngineSnapshot`]) —
    /// НЕ потребляет `self`, движок продолжает работать после вызова как ни
    /// в чём не бывало (нужен `S: Clone`, только для этого метода, не для
    /// всего `Engine<S>` — большинству кода клонировать `storage` незачем).
    pub fn snapshot(&self) -> EngineSnapshot<S>
    where
        S: Clone,
    {
        EngineSnapshot {
            grid: self.grid.clone(),
            rule_index: self.rule_index.clone(),
            state: self.state.clone(),
            self_mod: self.self_mod.clone(),
            guard_self_modification: self.guard_self_modification,
            original_rule_index: self.original_rule_index.clone(),
            self_mod_managed_heads: self.self_mod_managed_heads.clone(),
            rejected_self_modifications: self.rejected_self_modifications,
        }
    }

    /// Восстановить движок из снимка ([`Engine::snapshot`]/сериализованного
    /// [`EngineSnapshot`]). Пересобирает ВСЕ кэши заново из `rule_index`
    /// (та же логика, что `Engine::new` — кэши никогда не входят в снимок,
    /// см. её doc-комментарий), затем восстанавливает персистентное
    /// состояние (`state`, самомодификацию) ТОЧНО как было на момент
    /// снимка, а не заново с нуля.
    pub fn from_snapshot(snapshot: EngineSnapshot<S>) -> Self {
        let mut engine = Self::new(snapshot.grid, snapshot.rule_index);
        engine.state = snapshot.state;
        engine.self_mod = snapshot.self_mod;
        engine.guard_self_modification = snapshot.guard_self_modification;
        engine.original_rule_index = snapshot.original_rule_index;
        engine.self_mod_managed_heads = snapshot.self_mod_managed_heads;
        engine.rejected_self_modifications = snapshot.rejected_self_modifications;
        engine
    }

    /// Включить самомодификацию: с этого момента `run_tick` сам, каждый тик,
    /// дренирует канал 0 всех выходных граничных буферов через протокол
    /// `RuleStore`, применяет готовые операции (`AddRule`/`RemoveRule`/
    /// `ClearAll`) к `rule_index` и перестраивает кэши — решётка может сама
    /// писать себе новые правила во время работы, без внешнего кода,
    /// вызывающего `rebuild_rule_cache` вручную после каждой посылки.
    ///
    /// Идемпотентна: повторный вызов просто ничего не делает, если уже
    /// включено (не сбрасывает уже накопленный внутри `RuleStore` буфер).
    pub fn enable_self_modification(&mut self) {
        if self.self_mod.is_none() {
            self.self_mod = Some(crate::rule_store::RuleStore::new());
            self.original_rule_index = self.rule_index.clone();
        }
    }

    /// Как [`Engine::enable_self_modification`], но с проверкой композиции:
    /// самопереданное правило с НОВЫМ id (ещё не встречавшимся в
    /// `rule_index`) устанавливается только если
    /// `ConflictGraph::check_composition` подтверждает, что оно не
    /// конфликтует с остальным текущим набором правил. Небезопасное правило
    /// молча отбрасывается — уже установленные модули (например, независимо
    /// написанный домен, с которым текущий делит решётку) не могут быть
    /// сломаны чужой самомодификацией, даже недобросовестной или ошибочной.
    pub fn enable_guarded_self_modification(&mut self) {
        self.enable_self_modification();
        self.guard_self_modification = true;
    }

    /// Получить ссылку на решётку.
    pub fn grid(&self) -> &Grid<S> {
        &self.grid
    }

    /// Получить мутабельную ссылку на решётку.
    pub fn grid_mut(&mut self) -> &mut Grid<S> {
        &mut self.grid
    }

    // ─── Обнаружение совпадений ───

    /// Обнаружить все совпадения на текущей решётке.
    ///
    /// НЕ включает `cam`-совпадения (`types::CamSearch`) — их детекция
    /// (`matcher::detect_cam_matches`) требует найденные позиции для
    /// последующего арбитража, а этот путь (ручная связка
    /// `detect_matches` → `arbitrate` → `apply_matches`) их никуда не
    /// передаёт. Полностью работают только через `run_tick`/`Engine::run_tick`.
    pub fn detect_matches(&self) -> Vec<RuleMatch> {
        let search_coords = resolve_search_coords_peek(&self.grid, &self.search_radius_cache);
        detect_matches_with_group_data(&self.grid, &self.group_cache, &search_coords)
    }

    // ─── Арбитраж ───

    /// Выбрать непротиворечивый набор совпадений.
    ///
    /// См. doc-комментарий `detect_matches` — `cam`-матчи сюда не попадают
    /// (детектируются отдельно, только внутри `run_tick`); `arbitrate`
    /// (свободная функция) сама подставляет пустую карту позиций внутри.
    pub fn arbitrate(&self, all_matches: Vec<RuleMatch>) -> Vec<RuleMatch> {
        arbitrate(
            all_matches,
            &self.rule_index,
            &self.rule_cache,
            (self.grid.width(), self.grid.height()),
            |x, y| self.grid.get_age(x, y) as u32,
        )
    }

    // ─── Применение ───

    /// Применить набор совпадений к решётке.
    pub fn apply_matches(
        &mut self,
        matches: Vec<RuleMatch>,
    ) -> (Vec<AffectedRegion>, Vec<(u32, Cell)>) {
        apply_matches(&mut self.grid, matches, &self.rule_index, &self.rule_cache)
    }

    // ─── IO ───

    /// Отправить значение на входной канал граничной ячейки.
    /// Ищет первый граничный буфер с direction == "input" и
    /// помещает значение в очередь указанного канала.
    ///
    /// Если включена запись ([`Engine::enable_input_recording`]) — вызов
    /// ЗАПИСЫВАЕТСЯ в `input_log` ДО поиска буфера, безусловно (даже если
    /// подходящего input-буфера почему-то не нашлось) — [`Engine::replay`]
    /// должен воспроизвести ТУ ЖЕ последовательность вызовов, что была на
    /// самом деле, а не только те, что успешно на что-то подействовали.
    pub fn push_input(&mut self, ch: u32, value: u8) {
        if let Some(log) = self.input_log.as_mut() {
            log.push(InputEvent { tick: self.grid.generation(), channel: ch, value });
        }
        for (_, buf) in self.grid.iter_boundaries_mut() {
            if buf.direction == "input" {
                buf.enqueue(ch, Cell::new(value));
                return;
            }
        }
    }

    /// Включить запись вызовов `push_input` в `input_log` (для
    /// [`Engine::replay`]) — с этого момента, не с начала работы движка;
    /// уже прошедшие вызовы `push_input` не восстанавливаются задним числом.
    pub fn enable_input_recording(&mut self) {
        self.input_log.get_or_insert_with(Vec::new);
    }

    /// Текущий журнал записанных вызовов `push_input` — `None`, если запись
    /// не включена.
    pub fn input_log(&self) -> Option<&[InputEvent]> {
        self.input_log.as_deref()
    }

    /// Включить запись структурированного лога тиков ([`TickLogEntry`]) —
    /// с этого момента, не с начала работы движка; уже прошедшие тики не
    /// восстанавливаются задним числом. Каждый последующий вызов
    /// [`Engine::run_tick`] добавляет одну запись.
    pub fn enable_tick_logging(&mut self) {
        self.tick_log.get_or_insert_with(Vec::new);
    }

    /// Текущий структурированный лог тиков — `None`, если запись не
    /// включена. Сериализуется через `serde_json` (не `serde_yaml`, в
    /// отличие от [`Engine::snapshot`] — здесь нет нестроковых ключей
    /// `HashMap`, только плоский `Vec` из полей-примитивов, так что
    /// ограничение `EngineSnapshot`'s doc-комментария сюда не относится).
    pub fn tick_log(&self) -> Option<&[TickLogEntry]> {
        self.tick_log.as_deref()
    }

    /// Восстановить движок из снимка и повторно применить журнал ввода до
    /// (не включая) `target_tick` — воспроизводит РЕАЛЬНУЮ
    /// последовательность `push_input`/`run_tick`, а не просто "дошли до
    /// нужного тика как-то". Пример использования (отладка): нашли
    /// расхождение на тике 1000 → взять снимок и `input_log`, снятые на
    /// тике 900 (или раньше) → `Engine::replay(snapshot, &log, 1000)` →
    /// получить движок ровно в том состоянии, в котором он был бы на тике
    /// 1000 в оригинальном прогоне, и продолжить исследовать оттуда, не
    /// пересчитывая весь прогон с нуля вручную.
    ///
    /// `target_tick` сравнивается с `grid.generation()` — то же число,
    /// которое `InputEvent::tick` записывает при `push_input`, так что
    /// каждое событие подаётся РОВНО перед тем `run_tick()`, который
    /// изначально его и забрал (см. doc-комментарий `InputEvent`).
    ///
    /// `apply_input()` вызывается КАЖДУЮ итерацию, БЕЗУСЛОВНО (даже если
    /// на этот тик нет ни одного события в `log`) — `push_input` только
    /// кладёт значение в очередь граничного буфера, реальный перенос на
    /// решётку делает `apply_input()`, отдельный шаг, не часть `run_tick()`
    /// (см. её doc-комментарий) — канонический паттерн использования (см.
    /// `examples/strength_live_io.rs`) вызывает его каждый тик безусловно,
    /// не только когда только что был `push_input`; `replay` обязан
    /// воспроизвести ТУ ЖЕ последовательность вызовов.
    pub fn replay(snapshot: EngineSnapshot<S>, log: &[InputEvent], target_tick: u64) -> Self {
        let mut engine = Self::from_snapshot(snapshot);
        while engine.grid.generation() < target_tick {
            let current = engine.grid.generation();
            for event in log.iter().filter(|e| e.tick == current) {
                engine.push_input(event.channel, event.value);
            }
            engine.apply_input();
            engine.run_tick();
        }
        engine
    }

    /// Извлечь выходные данные из граничных буферов.
    pub fn pop_output(&mut self) -> Vec<(u32, Cell)> {
        let mut outputs = Vec::new();
        let coords: Vec<(usize, usize)> = self.grid.boundary_coords().collect();
        for (x, y) in coords {
            if let Some(buf) = self.grid.get_boundary_mut(x, y) {
                let channels: Vec<u32> = buf.queues.keys().copied().collect();
                for ch in channels {
                    for cell in buf.dequeue(ch) {
                        outputs.push((x as u32, cell));
                    }
                }
            }
        }
        outputs
    }

    /// Применить данные из входных буферов к решётке.
    ///
    /// Каждый вызов потребляет ровно одно значение с фронта очереди каждого
    /// input-буфера (по первому непустому каналу) и продвигает очередь —
    /// иначе следующий тик увидел бы то же самое значение снова, а
    /// остальные когда-либо запушенные значения никогда бы не дошли до
    /// решётки.
    pub fn apply_input(&mut self) {
        let inputs: Vec<(usize, usize, u32, u8)> = {
            let mut v = Vec::new();
            for (&(x, y), buf) in self.grid.iter_boundaries() {
                if buf.direction == "input" {
                    for (&ch, queue) in &buf.queues {
                        if let Some(cell) = queue.front() {
                            v.push((x, y, ch, cell.value.0 .0));
                            break;
                        }
                    }
                }
            }
            v
        };
        let gen = self.grid.generation();
        for (x, y, ch, val) in inputs {
            self.grid.set_cell(
                x,
                y,
                Cell {
                    value: CellValue::new(val),
                    born_at: gen,
                },
            );
            // Потребляем значение — иначе оно будет применяться повторно
            // на каждом следующем тике, а очередь никогда не продвинется.
            if let Some(buf) = self.grid.get_boundary_mut(x, y) {
                if let Some(queue) = buf.queues.get_mut(&ch) {
                    queue.pop_front();
                }
            }
        }
    }

    /// Извлечь и очистить выходные буферы.
    pub fn drain_output(&mut self) -> Vec<(u32, Cell)> {
        let mut outputs = Vec::new();
        let coords: Vec<(usize, usize)> = self.grid.boundary_coords().collect();
        for (x, y) in coords {
            if let Some(buf) = self.grid.get_boundary_mut(x, y) {
                if buf.direction == "output" {
                    let channels: Vec<u32> = buf.queues.keys().copied().collect();
                    for ch in channels {
                        for cell in buf.dequeue(ch) {
                            outputs.push((x as u32, cell));
                        }
                    }
                }
            }
        }
        outputs
    }

    // ─── Age ───

    /// Увеличить возраст всех активных ячеек на 1.
    pub fn advance_age(&mut self) {
        self.grid.advance_age();
    }

    /// Сбросить возраст в affected regions.
    /// Устанавливает born_at = текущее поколение.
    pub fn reset_age(&mut self, regions: &[AffectedRegion]) {
        let gen = self.grid.generation();
        for region in regions {
            for y in region.y_start..region.y_end {
                for x in region.x_start..region.x_end {
                    if let Some(cell) = self.grid.get_cell(x as usize, y as usize) {
                        self.grid.storage.set(
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
    }

    // ─── Терминация ───

    /// Проверить, достигла ли система устойчивого состояния.
    pub fn detect_termination(&self, tick: u32) -> TerminationVerdict {
        let search_coords = resolve_search_coords_peek(&self.grid, &self.search_radius_cache);
        let matches = detect_matches_with_group_data(&self.grid, &self.group_cache, &search_coords);
        if matches.is_empty() {
            TerminationVerdict::Stable
        } else if tick > 0 {
            let accepted = self.arbitrate(matches);
            if accepted.is_empty() {
                TerminationVerdict::Stable
            } else {
                TerminationVerdict::Active
            }
        } else {
            TerminationVerdict::Active
        }
    }

    /// Проверить возможность композиции.
    pub fn compose_with(&mut self) -> CompositionVerdict {
        let initial_cells: Vec<(usize, usize, Cell)> = {
            let mut v = Vec::new();
            for (x, y) in self.grid.iter_active() {
                if let Some(cell) = self.grid.get_cell(x, y) {
                    v.push((x, y, *cell));
                }
            }
            v
        };
        let initial_count = initial_cells.len();

        let mut tick = 0u32;
        loop {
            if tick > 1000 {
                return CompositionVerdict::NonTerminating;
            }
            let search_coords = resolve_search_coords_advance(&mut self.grid, &self.search_radius_cache);
            let matches = detect_matches_with_group_data(&self.grid, &self.group_cache, &search_coords);
            if matches.is_empty() {
                break;
            }
            // См. комментарий в свободной функции run_tick: помечаем
            // ВСЕ найденные совпадения, не только принятые — проигравшее
            // арбитраж совпадение остаётся актуальным условием и должно
            // переоцениваться на следующем тике.
            for m in &matches {
                self.grid.mark_dirty(m.x as usize, m.y as usize);
            }
            let accepted = self.arbitrate(matches);
            if accepted.is_empty() {
                break;
            }
            let (regions, _) = self.apply_matches(accepted);

            // Старение
            self.grid.advance_age();
            self.reset_age(&regions);
            tick += 1;
        }

        let final_cells: Vec<(usize, usize, Cell)> = {
            let mut v = Vec::new();
            for (x, y) in self.grid.iter_active() {
                if let Some(cell) = self.grid.get_cell(x, y) {
                    v.push((x, y, *cell));
                }
            }
            v
        };
        let final_count = final_cells.len();

        if final_count > initial_count * 2 {
            CompositionVerdict::Divergent
        } else if final_count < initial_count / 2 && initial_count > 0 {
            CompositionVerdict::Shrinking
        } else {
            CompositionVerdict::Bounded(final_count)
        }
    }

    // ─── Вспомогательные ───

    /// Получить приоритет правила.
    pub fn get_priority(&self, rule_id: &[CellType]) -> u32 {
        if let Some(first) = rule_id.first() {
            if let Some(rules) = self.rule_index.get(first) {
                for rule in rules {
                    if rule.id == rule_id {
                        return rule.priority;
                    }
                }
            }
        }
        0
    }

    /// Найти правило по ID.
    pub fn find_rule(&self, rule_id: &[CellType]) -> Option<&Rule> {
        if let Some(first) = rule_id.first() {
            if let Some(rules) = self.rule_index.get(first) {
                for rule in rules {
                    if rule.id == rule_id {
                        return Some(rule);
                    }
                }
            }
        }
        None
    }

    /// Выполнить один тик симуляции.
    ///
    /// В отличие от свободной функции `run_tick`, использует уже
    /// закэшированные `self.rule_cache`/`self.group_cache` вместо пересборки
    /// на каждый вызов — `Engine` хранит их именно для переиспользования
    /// между тиками (как уже делают `Engine::arbitrate`/`Engine::apply_matches`/
    /// `Engine::detect_matches`). Валиден, пока `rule_index` не менялся
    /// снаружи после `Engine::new` — если правила были изменены напрямую
    /// (в обход RuleStore), нужно вызвать [`Engine::rebuild_rule_cache`]
    /// перед следующим тиком.
    pub fn run_tick(&mut self) -> (Vec<RuleMatch>, Vec<(u32, Cell)>) {
        let conflict_ctx = ConflictContext { partners: &self.conflict_partners, max_radius: self.max_affected_radius };
        let tick = self.grid.generation();
        let mut counts = self.tick_log.is_some().then(TickEventCounts::default);
        let result = run_tick_with_cache(
            &mut self.grid,
            &self.rule_index,
            &self.rule_cache,
            &self.group_cache,
            &self.search_radius_cache,
            &self.extension_flags,
            Some(&conflict_ctx),
            &mut self.state,
            None,
            counts.as_mut(),
            &mut self.write_buffer,
            &mut self.pattern_buffer,
        );
        if let (Some(log), Some(c)) = (self.tick_log.as_mut(), counts) {
            log.push(TickLogEntry {
                tick,
                accepted: c.accepted,
                rejected: c.rejected,
                starvation_events: c.starvation_events,
                feedback_events: c.feedback_events,
            });
        }
        self.absorb_self_modifications();
        result
    }

    /// Как [`Engine::run_tick`], но дополнительно возвращает разбивку этого
    /// тика по фазам ([`TickPhaseTimings`]) — для профилирования "где узкое
    /// место у конкретного конфига", без общего времени всего тика как
    /// единственного числа. `Instant::now()` вызывается ТОЛЬКО этим путём
    /// (`run_tick` использует ветку `None`, см. `mark_phase!` в
    /// `run_tick_with_cache`) — нулевая цена для кода, который об этом не
    /// просил.
    pub fn run_tick_profiled(&mut self) -> (Vec<RuleMatch>, Vec<(u32, Cell)>, TickPhaseTimings) {
        let conflict_ctx = ConflictContext { partners: &self.conflict_partners, max_radius: self.max_affected_radius };
        let mut timings = TickPhaseTimings::default();
        let result = run_tick_with_cache(
            &mut self.grid,
            &self.rule_index,
            &self.rule_cache,
            &self.group_cache,
            &self.search_radius_cache,
            &self.extension_flags,
            Some(&conflict_ctx),
            &mut self.state,
            Some(&mut timings),
            None,
            &mut self.write_buffer,
            &mut self.pattern_buffer,
        );
        self.absorb_self_modifications();
        (result.0, result.1, timings)
    }

    /// Если самомодификация включена — дренировать канал 0 выходных
    /// граничных буферов, применить готовые операции к `rule_index` и,
    /// только если что-то реально изменилось, перестроить кэши. Отдельный
    /// метод, а не код прямо в `run_tick`, только чтобы не загромождать его —
    /// логически это часть одного тика.
    fn absorb_self_modifications(&mut self) {
        let Some(mut rule_store) = self.self_mod.take() else { return };
        let ops = rule_store.drain_rule_channel(&mut self.grid);
        let mut changed = false;
        for op in ops {
            if self.guard_self_modification && !self.composition_allows(&op, &mut rule_store) {
                self.rejected_self_modifications += 1;
                continue;
            }
            if rule_store.apply(op) {
                changed = true;
            }
        }
        if changed {
            let self_mod_index = rule_store.get_index().clone();
            let current: FxHashSet<CellType> = self_mod_index.keys().copied().collect();
            // Каждая голова, задействованная либо изначально (rule_index на
            // момент Engine::new), либо самомодификацией (сейчас или раньше),
            // пересобирается заново как "оригинал (если был) ++ то, что
            // сейчас реально числится за RuleStore" — а не просто
            // ЗАМЕНЯЕТСЯ содержимым get_index(). Без этого самопереданное
            // ДОПОЛНИТЕЛЬНОЕ правило на уже существующую голову молча стирало
            // оригинал (get_index() ничего не знает о правилах, заведённых в
            // обход RuleStore), а удаление последнего самопереданного
            // правила у головы оставляло висеть устаревшую копию вместо
            // возврата к оригиналу.
            for head in self.self_mod_managed_heads.union(&current).copied().collect::<Vec<_>>() {
                let mut merged = self.original_rule_index.get(&head).cloned().unwrap_or_default();
                if let Some(rules) = self_mod_index.get(&head) {
                    merged.extend(rules.iter().cloned());
                }
                if merged.is_empty() {
                    self.rule_index.remove(&head);
                } else {
                    self.rule_index.insert(head, merged);
                }
            }
            self.self_mod_managed_heads = current;
            self.rebuild_rule_cache();
        }
        self.self_mod = Some(rule_store);
    }

    /// Часть [`Engine::enable_guarded_self_modification`]: пропускает всё,
    /// кроме `AddRule` с id, которого ЕЩЁ НЕТ среди того, чем реально
    /// владеет самомодификация (расширение своего же модуля не
    /// проверяется — см. doc-комментарий поля `guard_self_modification`).
    /// Для действительно нового или ЧУЖОГО (protected) id — честная
    /// проверка композиции против ОСТАЛЬНОГО текущего набора правил.
    ///
    /// Принимает `rule_store` живым (`&mut`, не `&self.self_mod`), а не
    /// читает `self.rule_index` — потому что `rule_index` обновляется
    /// ОДИН РАЗ в конце всего пакета операций (`absorb_self_modifications`),
    /// а `rule_store` отражает КАЖДУЮ уже принятую операцию немедленно.
    /// Если в одном тике декодируются ДВЕ посылки, конфликтующие ДРУГ С
    /// ДРУГОМ (а не с чем-то предсуществующим), проверка по `rule_index`
    /// не увидела бы первую при проверке второй (обе прошли бы, каждая
    /// как будто в одиночестве) — проверка по `rule_store` видит.
    fn composition_allows(&self, op: &crate::rule_store::CompletedOp, rule_store: &mut crate::rule_store::RuleStore) -> bool {
        let crate::rule_store::RuleOp::AddRule(rule) = &op.op else { return true };
        let Some(&head) = rule.id.first() else { return true };
        let self_mod_index = rule_store.get_index().clone();
        // Расширение id, которым самомодификация уже владеет (сейчас, в том
        // числе принятое РАНЕЕ В ЭТОМ ЖЕ пакете операций) — не проверяем,
        // ЕСЛИ это не чужая (protected) территория: однажды доказанная
        // совместимость одного самопереданного правила с оригиналом головы
        // не означает, что и ВТОРОЕ самопереданное правило туда же тоже
        // совместимо с оригиналом — каждая заявка на protected-голову
        // проверяется свежо.
        if self_mod_index.contains_key(&head) && !self.original_rule_index.contains_key(&head) {
            return true;
        }
        let mut existing: Vec<Rule> = self.original_rule_index.values().flatten().cloned().collect();
        existing.extend(self_mod_index.values().flatten().cloned());
        // `check_composition` returns `Unsafe` for ANY conflict in the
        // combined graph, including a SELF-loop of the new rule alone (e.g.
        // an ordinary shift rule that could collide with another instance
        // of itself at an adjacent position — the same "moving object"
        // limitation as everywhere else in this project, nothing to do
        // with colliding with existing modules). `Unsafe`'s pair list is
        // empty in exactly that case (self-loops never satisfy the
        // cross-set `i < n_a && j >= n_a` filter) — reject only when there
        // is an ACTUAL rule_a×existing pair, not merely a non-empty verdict.
        let grid_ctx = crate::conflict_analyzer::GridContext {
            width: self.grid.width(),
            height: self.grid.height(),
            boundaries: &self.grid.boundaries,
        };
        match crate::ConflictGraph::check_composition_with_grid(std::slice::from_ref(rule), &existing, &grid_ctx) {
            crate::conflict_analyzer::CompositionVerdict::Safe => true,
            crate::conflict_analyzer::CompositionVerdict::Unsafe(pairs) => pairs.is_empty(),
        }
    }

    /// Пересобрать `rule_cache`/`group_cache` из текущего `rule_index`.
    ///
    /// Нужен только если `rule_index` был изменён напрямую после
    /// `Engine::new` (RuleStore-путь сам работает с отдельным индексом и
    /// передаётся в `Engine` целиком через пересоздание). Без вызова этого
    /// метода после такого изменения `run_tick`/`arbitrate`/`apply_matches`/
    /// `detect_matches` будут использовать устаревшие данные — в лучшем
    /// случае для новых правил, в худшем встретят `rule_idx`, которого ещё
    /// нет в кэше.
    ///
    /// Заодно помечает "грязными" ВСЕ активные клетки. Dirty-tracking следит
    /// за записями в решётку, а не за сменой самого набора правил: клетка,
    /// которая давно не менялась, может теперь подпадать под только что
    /// добавленное правило, но без этого никогда бы не попала в кандидатов
    /// на пересканирование на следующем тике (нашли этот случай на практике —
    /// живое добавление правила иначе просто не срабатывало на "осевших"
    /// клетках). Это может стоить одного лишнего полного прохода по
    /// активному набору сразу после вызова — приемлемая цена за то, что
    /// смена правил на лету всегда честно работает, без необходимости
    /// вызывающему коду знать про это взаимодействие и вручную "трогать"
    /// конкретные клетки.
    pub fn rebuild_rule_cache(&mut self) {
        // Инвалидация переиспользованных `rule_idx` в персистентном
        // состоянии правил — см. `rule_state::RuleStateStore::invalidate_stale`'s
        // doc-комментарий (логика и её обоснование перенесены туда целиком,
        // п.5 сессии 2026-08-09: `RuleStateStore` — единая точка для всего,
        // что касается персистентного состояния правил, не только карт, но
        // и их инвалидации).
        self.state.invalidate_stale(&self.rule_index, &self.grid);
        self.rule_cache = build_rule_data_cache(&self.rule_index);
        self.group_cache = build_group_data(&self.rule_index);
        self.search_radius_cache = compute_search_radius_cache(&self.rule_index);
        self.extension_flags = compute_extension_flags(&self.rule_index);
        let (partners, radius) = compute_conflict_partners(&self.rule_index, &self.rule_cache);
        self.conflict_partners = partners;
        self.max_affected_radius = radius;
        self.resync_original_rule_index();
        let active: Vec<(usize, usize)> = self.grid.active_coords().clone();
        for (x, y) in active {
            self.grid.mark_dirty(x, y);
        }
    }

    /// Только-чтение доступ к текущему набору правил — см. doc-комментарий
    /// поля `rule_index` про то, почему нет прямого мутабельного доступа
    /// извне: единственный способ ИЗМЕНИТЬ состав правил — [`Engine::set_rule_index`]/
    /// [`Engine::set_rules_for_head`], которые сами вызывают
    /// [`Engine::rebuild_rule_cache`], так что забыть — физически нельзя.
    pub fn rule_index(&self) -> &HashMap<CellType, Vec<Rule>> {
        &self.rule_index
    }

    /// Заменить ВЕСЬ набор правил и перестроить кэши — единственный
    /// поддерживаемый способ извне заменить `rule_index` целиком (п.4 сессии
    /// 2026-08-09). Раньше поле было `pub`, и прямая правка `engine.rule_index = ...`
    /// в обход `rebuild_rule_cache()` молча портила кэши/счётчики — этот
    /// метод физически не даёт забыть.
    pub fn set_rule_index(&mut self, new_rule_index: HashMap<CellType, Vec<Rule>>) {
        self.rule_index = new_rule_index;
        self.rebuild_rule_cache();
    }

    /// Заменить список правил ОДНОЙ головы (вставить новую голову, если её
    /// раньше не было) и перестроить кэши — тот же принцип, что и
    /// [`Engine::set_rule_index`], только для точечной правки одной головы
    /// (самый частый случай на практике — см. `strength_live_rules.rs`).
    /// Чтобы заменить/добавить ОДНО конкретное правило внутри уже
    /// существующего списка головы, прочитайте текущий список через
    /// [`Engine::rule_index`], постройте изменённый `Vec<Rule>` и передайте
    /// его сюда целиком — отдельного метода "заменить правило по индексу"
    /// нет специально, чтобы не плодить API на каждый частный случай.
    pub fn set_rules_for_head(&mut self, head: CellType, rules: Vec<Rule>) {
        self.rule_index.insert(head, rules);
        self.rebuild_rule_cache();
    }

    /// Пересчитать `original_rule_index` из ТЕКУЩЕГО `rule_index` — как "то,
    /// что сейчас там стоит, за вычетом того, чем сейчас реально владеет
    /// `RuleStore`". Нужно вызывать при каждой перестройке кэша (не только
    /// один раз при включении самомодификации), иначе прямая правка
    /// `rule_index` ПОСЛЕ того, как самомодификация уже что-то добавила на
    /// лету, либо потеряется при следующем слиянии, либо ошибочно будет
    /// считаться "чужой территорией" навсегда неверно. Ничего не делает,
    /// если самомодификация не включена — `original_rule_index` тогда
    /// просто не используется.
    fn resync_original_rule_index(&mut self) {
        let Some(mut rule_store) = self.self_mod.take() else { return };
        let self_mod_index = rule_store.get_index().clone();
        self.self_mod = Some(rule_store);

        let mut resynced: HashMap<CellType, Vec<Rule>> = HashMap::new();
        for (head, rules) in &self.rule_index {
            let self_owned = self_mod_index.get(head);
            let foreign: Vec<Rule> = rules
                .iter()
                .filter(|r| !self_owned.is_some_and(|so| so.contains(r)))
                .cloned()
                .collect();
            if !foreign.is_empty() {
                resynced.insert(*head, foreign);
            }
        }
        self.original_rule_index = resynced;
    }
}

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
fn compute_conflict_partners(
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
            min_x.unsigned_abs().max(max_x.unsigned_abs()).max(min_y.unsigned_abs()).max(max_y.unsigned_abs()) as i32
        })
        .max()
        .unwrap_or(0);

    (partners, max_radius)
}

/// Структурный вход для `spatial_bypass_split` — см. doc-комментарии
/// `Engine::conflict_partners`/`Engine::max_affected_radius`.
struct ConflictContext<'a> {
    partners: &'a FxHashMap<CellType, FxHashSet<CellType>>,
    max_radius: i32,
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
    run_tick_with_cache(grid, rule_index, &rule_cache, &group_cache, &search_radius_cache, &extension_flags, None, &mut state, None, None, &mut write_buffer, &mut pattern_buffer)
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

fn spatial_bypass_split(matches: Vec<RuleMatch>, ctx: &ConflictContext) -> (Vec<RuleMatch>, Vec<RuleMatch>) {
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
        let Some(my_partners) = ctx.partners.get(&m.head) else { continue };
        let (bx, by) = ((m.x as i32).div_euclid(bucket), (m.y as i32).div_euclid(bucket));
        'neighbors: for dbx in -1..=1 {
            for dby in -1..=1 {
                let Some(members) = buckets.get(&(bx + dbx, by + dby)) else { continue };
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
fn run_tick_with_cache<S: GridStorage>(
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
            snap.memory_buffers().get(&(m.x, m.y, m.rule_idx)).is_some_and(|buf| {
                buf.len() == spec.window && buf.iter().eq(spec.match_pattern.iter())
            })
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
            snap.activation_counters().get(&(m.head, m.rule_idx)).copied().unwrap_or(0) < limit
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
            .filter(|m| {
                lookup_rule(&head_index, m.head, m.rule_idx).is_some_and(|r| r.starvation_after.is_some())
            })
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
            None => arbitrate_with_cam(matches, rule_index, rule_cache, (grid.width(), grid.height()), &cam_positions, generation, snap.starvation_counters(), snap.feedback_counters(), |x, y| {
                grid.get_age(x, y) as u32
            }),
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
    let accepted_keys: FxHashSet<(u32, u32, usize)> = if starving_keys.is_empty() && memory_targets.is_empty() && feedback_keys.is_empty() {
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
        let (regions, outputs) = apply_matches_with_cam(grid, &accepted, rule_index, rule_cache, &cam_positions, feedback_counters, memory_buffers, write_buffer, pattern_buffer);

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

/// Максимальный |смещение| паттерна среди ВСЕХ правил (любой головы).
///
/// Если клетка D изменилась, то любая клетка-центр C, чей паттерн ссылается
/// на D на смещении (dx,dy) (то есть C+(dx,dy) = D, C = D-(dx,dy)), могла
/// изменить статус совпадения — и C лежит в пределах этого радиуса от D
/// (диапазон -radius..=radius симметричен, так что направление роли не
/// играет). Используется для расширения "грязного" множества (см.
/// `Grid::dirty_coords`) до множества кандидатов на пересканирование.
fn max_pattern_radius(rule_index: &HashMap<CellType, Vec<Rule>>) -> i32 {
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
fn zero_head_radius(rule_index: &HashMap<CellType, Vec<Rule>>) -> i32 {
    match rule_index.get(&CellType(DEFAULT_CELL_VALUE)) {
        Some(rules) => pattern_radius(rules.iter()),
        None => 0,
    }
}

fn pattern_radius<'a>(rules: impl Iterator<Item = &'a Rule>) -> i32 {
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
fn min_age_gated_types(rule_index: &HashMap<CellType, Vec<Rule>>) -> FxHashSet<CellType> {
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
struct SearchRadiusCache {
    min_age_gated_types: FxHashSet<CellType>,
    max_pattern_radius: i32,
    zero_head_radius: i32,
}

fn compute_search_radius_cache(rule_index: &HashMap<CellType, Vec<Rule>>) -> SearchRadiusCache {
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
struct TickEventCounts {
    accepted: usize,
    rejected: usize,
    starvation_events: usize,
    feedback_events: usize,
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
struct ExtensionFlags {
    starvation_after: bool,
    feedback: bool,
    memory: bool,
    /// Хоть одно правило набора использует `Rule::max_activations`. НЕ
    /// участвует в `extension_rule_indices` ниже — тот список только для
    /// per-tick чистки ПОЗИЦИОННЫХ (`x, y, rule_idx`) осиротевших записей;
    /// `activation_counters` ключуется `(head, rule_idx)` без позиции и не
    /// устаревает от того, что клетка перестала совпадать (устаревание
    /// покрыто отдельно — `Engine::invalidate_stale_extension_state`, при
    /// смене состава правил, а не каждый тик).
    max_activations: bool,
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
    extension_rule_indices: Vec<usize>,
}

fn compute_extension_flags(rule_index: &HashMap<CellType, Vec<Rule>>) -> ExtensionFlags {
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
fn build_candidates<S: GridStorage>(
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
        let mut seen: FxHashSet<(usize, usize)> =
            candidates.iter().copied().collect();
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
fn dirty_base_and_radius<S: GridStorage>(
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
fn resolve_search_coords_peek<S: GridStorage>(
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
fn resolve_search_coords_advance<S: GridStorage>(
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
fn expand_neighborhood<S: GridStorage>(
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
    if let Some((w, h)) = grid.storage.bounds() {
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

fn reset_age_for_regions<S: GridStorage>(grid: &mut Grid<S>, regions: &[AffectedRegion]) {
    let gen = grid.generation();
    // По точному списку `written_cells`, а не по прямоугольнику bbox —
    // прямоугольник, охватывающий исходную и целевую позицию сдвига на N>1
    // клеток, включает и клетки МЕЖДУ ними, которые сдвиг не трогает вовсе
    // (найдено экспериментально: клетка между позициями получала обнулённый
    // возраст, хотя сама не менялась). `written_cells` — ровно то, что было
    // вставлено в write-буфер при применении, без лишнего.
    for region in regions {
        for &(x, y) in &region.written_cells {
            if let Some(cell) = grid.get_cell(x as usize, y as usize) {
                grid.storage.set(
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

/// Вердикт терминации.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminationVerdict {
    /// Система активна — есть совпадения.
    Active,
    /// Система стабильна — нет совпадений или арбитраж отклонил все.
    Stable,
}

/// Вердикт композиции.
#[derive(Debug, Clone, PartialEq)]
pub enum CompositionVerdict {
    /// Композиция ограничена.
    Bounded(usize),
    /// Композиция расходится.
    Divergent,
    /// Композиция сжимается.
    Shrinking,
    /// Композиция не завершается за лимит тиков.
    NonTerminating,
    /// Композиция conflict-free (статический анализ).
    Safe,
    /// Композиция с потенциальными конфликтами (статический анализ).
    Unsafe(Vec<(usize, usize)>),
}

// ──────────────────────────────────────────────────────────────
// Тесты
// ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
