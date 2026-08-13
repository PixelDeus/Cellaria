use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

// ============================================================================
// Базовые типы данных
// ============================================================================

/// Значение пустой ячейки по умолчанию.
pub const DEFAULT_CELL_VALUE: u8 = 0;

/// Вещественное значение ячейки (обёртка для будущих расширений).
///
/// `Ord`/`PartialOrd` (лексикографически по внутреннему `u8`) нужны, чтобы
/// `RuleId` (`Vec<CellType>`) можно было сравнивать — используется как
/// один из уровней детерминированного тай-брейка в арбитраже
/// (`priority → age → rule_id → coords → rule_idx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CellType(pub u8);

impl CellType {
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    /// Сырое `u8`-значение — `.0` уже публичное поле (тип tuple-struct),
    /// этот метод не про доступ как таковой, а про читаемость сквозных
    /// цепочек (`cell.type_id()` vs `cell.value.0 .0` — см.
    /// `CellValue::raw`/`Cell::type_id`, найдено при поиске эргономических
    /// улучшений: `.value.0 .0` встречается 40+ раз по всей кодовой базе).
    pub const fn raw(&self) -> u8 {
        self.0
    }
}

/// Значение ячейки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CellValue(pub CellType);

impl CellValue {
    pub const fn new(value: u8) -> Self {
        Self(CellType::new(value))
    }

    /// Сырое `u8`-значение, минуя промежуточный `CellType` — см.
    /// `CellType::raw`/`Cell::type_id`.
    pub const fn raw(&self) -> u8 {
        self.0 .0
    }
}

impl Default for CellValue {
    fn default() -> Self {
        Self(CellType::new(DEFAULT_CELL_VALUE))
    }
}

/// Полное представление ячейки.
/// Возраст ячейки вычисляется как `generation - born_at`, где `generation` —
/// глобальный счётчик поколений в `Grid`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub value: CellValue,
    /// Поколение (tick), в котором ячейка была создана/изменена.
    pub born_at: u64,
}

impl Cell {
    /// Создаёт ячейку с born_at = 0.
    pub const fn new(value: u8) -> Self {
        Self {
            value: CellValue::new(value),
            born_at: 0,
        }
    }

    /// Пустая ячейка (значение DEFAULT_CELL_VALUE, born_at = 0).
    pub const fn empty() -> Self {
        Self::new(DEFAULT_CELL_VALUE)
    }

    /// Проверяет, является ли ячейка "дефолтной": значение 0, born_at = 0.
    /// Граничная ячейка (с привязанным BoundaryBuffer) не считается дефолтной,
    /// но эта проверка не учитывает границу — она на уровне Grid::set_cell.
    pub fn is_default(&self) -> bool {
        self.value == CellValue::default() && self.born_at == 0
    }

    /// Сырое `u8`-значение типа клетки — сокращение самой частой сквозной
    /// цепочки в кодовой базе (`cell.value.0 .0`, 40+ раз). Эквивалентно
    /// `self.value.raw()`.
    ///
    /// ```
    /// use cellaria::types::Cell;
    ///
    /// let cell = Cell::new(5);
    /// assert_eq!(cell.type_id(), 5);
    /// assert_eq!(cell.type_id(), cell.value.0 .0, "то же самое значение, что и старая сквозная цепочка");
    /// ```
    pub const fn type_id(&self) -> u8 {
        self.value.raw()
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::empty()
    }
}

// ============================================================================
// ChangeValue — значение изменения: литерал или ссылка на паттерн
// ============================================================================

/// Значение для записи в changes: литерал (u8), ссылка на позицию в
/// паттерне ($0, $1, ...), или арифметика над двумя такими значениями
/// (`Add`/`Sub`, рекурсивно — операнды сами `ChangeValue`, так что можно
/// строить `Add(Ref(0), Literal(1))` = "прочитанное значение + 1").
///
/// **НЕ `#[serde(untagged)]`** — `Literal`/`Ref` сериализовались бы
/// неразличимо как голое число, снимки тихо теряли `Ref` при round-trip
/// (реальный найденный баг, см. `architecture.md`). YAML-
/// конфиги не затронуты: `config.rs`'s `parse_change_value` парсит вручную
/// из `serde_yaml::Value`, не через этот derive.
///
/// ## Examples
///
/// ```
/// use cellaria::types::ChangeValue;
///
/// let lit = ChangeValue::Literal(42);
/// let ref0 = ChangeValue::Ref(0);
/// let incremented = ChangeValue::Add(Box::new(ChangeValue::Ref(0)), Box::new(ChangeValue::Literal(1)));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeValue {
    /// Числовой литерал, например 5.
    Literal(u8),
    /// Ссылка на позицию в паттерне (0-based), например "$0" = Ref(0).
    Ref(usize),
    /// Сумма двух значений, `u8::wrapping_add` (перенос через 255→0, та же
    /// дисциплина, что и у остального u8-арифметики в кодовой базе,
    /// например `TIE_BREAK_MODULUS`'s вращение в арбитраже) — `CellType`
    /// сам по себе просто `u8`-метка, отдельного "числового" типа с
    /// насыщением/ошибкой на переполнение модель не вводит.
    Add(Box<ChangeValue>, Box<ChangeValue>),
    /// Разность двух значений, `u8::wrapping_sub` — та же дисциплина, что и `Add`.
    Sub(Box<ChangeValue>, Box<ChangeValue>),
}

// ============================================================================
// Направления сдвига
// ============================================================================

/// Направления цепочечного сдвига.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// Спецификация сдвига: направление + количество шагов. Полная семантика
/// (включая предупреждение про неограниченный рост `keep_source` без
/// `max_activations`, взаимодействие с `conflict_analyzer`, GPU-статус) —
/// `specs/specification.md` §4.4.
///
/// - `broadcast: false` (умолч.) — телепорт в конечную точку.
///   `true` — копия в КАЖДУЮ клетку пути (source→конец).
/// - `keep_source: false` (умолч.) — источник очищается. `true` — источник
///   не трогается вообще (не входит в `written_cells`, возраст не
///   сбрасывается). Ортогонален `broadcast`.
/// - GPU не поддерживает `keep_source`/`broadcast` — явный отказ
///   (`GpuUnsupportedReason::KeepSourceUnsupported`), не тихая потеря эффекта.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShiftSpec {
    pub direction: Direction,
    pub steps: u16,
    #[serde(default)]
    pub broadcast: bool,
    #[serde(default)]
    pub keep_source: bool,
}

impl ShiftSpec {
    pub const fn new(direction: Direction, steps: u16) -> Self {
        Self {
            direction,
            steps,
            broadcast: false,
            keep_source: false,
        }
    }

    pub const fn broadcast(direction: Direction, steps: u16) -> Self {
        Self {
            direction,
            steps,
            broadcast: true,
            keep_source: false,
        }
    }

    /// "Излучение": как [`ShiftSpec::broadcast`] (копия во ВСЕ клетки пути),
    /// но источник НЕ очищается — значение копируется, а не перемещается.
    pub const fn emit(direction: Direction, steps: u16) -> Self {
        Self {
            direction,
            steps,
            broadcast: true,
            keep_source: true,
        }
    }
}

// ============================================================================
// Идентификаторы правил
// ============================================================================

/// Идентификатор правила — кортеж из значений внутренней области правила.
pub type RuleId = Vec<CellType>;

/// Хранит позицию совпадения и голову (тип центральной ячейки) сработавшего правила.
///
/// ## Examples
///
/// ```
/// use cellaria::types::{RuleMatch, CellType};
///
/// let m = RuleMatch {
///     x: 0,
///     y: 0,
///     head: CellType::new(1),
///     rule_idx: 0,
/// };
/// assert_eq!(m.x, 0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleMatch {
    pub x: u32,
    pub y: u32,
    /// Тип центральной ячейки совпадения — ключ в `rule_index`. Раньше здесь
    /// хранился полный `rule_id: Vec<CellType>` (клон `rule.id`), но он
    /// клонировался на КАЖДОЕ совпадение (сотни тысяч аллокаций за тик на
    /// плотных решётках), хотя ничего, кроме первого элемента, никогда не
    /// требовалось напрямую — полный `id`, если он вообще нужен (например,
    /// тай-брейку в арбитраже), всегда восстановим через
    /// `rule_index[head][rule_idx].id`.
    pub head: CellType,
    /// Позиция сработавшего правила в `rule_index[head]` (после сортировки
    /// по приоритету). `head` сам по себе не уникален — несколько правил могут
    /// иметь одинаковую голову (паттерн недетерминированного выбора), поэтому именно
    /// `rule_idx` однозначно определяет, какое именно правило сработало.
    pub rule_idx: usize,
}

/// Действие при выходе ячейки за границу решётки (overflow).
///
/// ## Examples
///
/// ```
/// use cellaria::types::OverflowAction;
///
/// let discard = OverflowAction::Discard;
/// let write = OverflowAction::Write(1);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OverflowAction {
    /// Отбросить ячейку (не писать в граничный буфер).
    #[serde(rename = "discard")]
    #[default]
    Discard,
    /// Записать ячейку в граничный буфер с указанным типом-заменителем.
    /// `0` означает "пронести собственное значение головки как есть"
    /// (см. doc-комментарий у обработчика в `applicator.rs`) — из-за этого
    /// через `Write` невозможно вывести буквальный литерал `0`.
    #[serde(rename = "write")]
    Write(u8),
    /// Записать БУКВАЛЬНО указанное значение, включая `0` — в отличие от
    /// `Write`, здесь нет специального смысла у нуля: параметр всегда
    /// используется как есть, а не как признак "пронести своё значение".
    #[serde(rename = "write_literal")]
    WriteLiteral(u8),
}

// ============================================================================
// Правила
// ============================================================================

/// Content-addressable поиск с ограниченным радиусом ("магнит"): ищет
/// БЛИЖАЙШУЮ клетку типа `target_type` в диске Chebyshev-радиуса `radius`
/// вокруг головы и притягивает её значение (найденная клетка — дефолт,
/// голова — `target_type`). Цель известна только в рантайме, не на этапе
/// определения правила. Правило с `cam` не должно иметь `shifts`. Почему
/// именно radius-ограничение (не поиск по всей решётке) и как это
/// сочетается со статическим анализом конфликтов — `specs/architecture.md`
/// §4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CamSearch {
    /// Chebyshev-радиус поиска (|dx|.max(|dy|) ≤ radius).
    pub radius: u8,
    /// Тип искомой клетки.
    pub target_type: CellType,
}

/// Полное определение правила.
///
/// `Rule` реализует `Default` (все поля — `Vec`/`Option`/примитивы с
/// осмысленным нулевым значением: пустой паттерн, без сдвигов/изменений,
/// без расширений) — на практике почти каждое правило использует лишь
/// несколько полей из 17, `..Default::default()` избавляет от
/// необходимости перечислять остальные явно на каждом литерале.
///
/// ## Examples
///
/// ```
/// use cellaria::types::{Rule, CellType, ShiftSpec, Direction, ChangeValue};
///
/// let rule = Rule {
///     id: vec![CellType::new(1), CellType::new(2)],
///     pattern: vec![(0, 0, CellType::new(1)), (1, 0, CellType::new(2))],
///     shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
///     changes: vec![(1, 0, ChangeValue::Literal(3))],
///     priority: 10,
///     ..Default::default()
/// };
/// assert_eq!(rule.priority, 10);
/// assert_eq!(rule.active_only, false, "поля, не указанные явно, берутся из Default");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Rule {
    /// Внутренняя область (n-кортеж CellType).
    pub id: RuleId,
    /// Двумерный паттерн для сопоставления: (dx, dy, тип).
    /// Если пуст — строится из `id` как `(dx=0..n, dy=0, id[dx])`.
    pub pattern: Vec<(i8, i8, CellType)>,
    /// Сдвиги, выполняемые правилом при срабатывании.
    ///
    /// Каждый `ShiftSpec` — независимая операция: читает исходную позицию
    /// головки (до сдвигов) и пишет её значение в свою собственную цель.
    /// Вложенность в группы (`Vec<Vec<_>>`) не влияет на применение — все
    /// сдвиги любых групп выполняются одинаково и независимо друг от друга.
    /// Поэтому правило с 2+ сдвигами — это репликация значения в несколько
    /// направлений одновременно, а не цепочка последовательных сдвигов.
    pub shifts: Vec<Vec<ShiftSpec>>,
    /// Изменения ячеек: (смещение_x, смещение_y, новое_значение).
    /// Применяются относительно КАЖДОЙ цели сдвига независимо (если сдвигов
    /// несколько — один раз на каждую); если сдвигов нет — относительно
    /// исходной позиции головки (0,0).
    pub changes: Vec<(i32, i32, ChangeValue)>,
    /// Если true — правило проверяется только в активных ячейках.
    /// Если false — проверяется везде.
    pub active_only: bool,
    /// Приоритет правила (больше = выше).
    pub priority: u32,
    /// Минимальный возраст ячейки-центра для активации правила.
    /// Правило срабатывает только если возраст ячейки ≥ min_age.
    pub min_age: u64,
    /// Действие при overflow (выходе за границу решётки).
    #[serde(default)]
    pub overflow: OverflowAction,
    /// Content-addressable поиск с ограниченным радиусом — см. [`CamSearch`].
    /// `None` (по умолчанию) — обычное правило, поведение не меняется.
    #[serde(default)]
    pub cam: Option<CamSearch>,
    /// Тай-брейк при равном `priority` и возрасте. `0` (по умолчанию) —
    /// без эффекта. Формула чередования и как выбрать значения для
    /// нужной частоты 50/50 или K-стороннего round-robin — см.
    /// `specs/specification.md` §3.4/§7, `arbitrator::TIE_BREAK_MODULUS`.
    #[serde(default)]
    pub tie_break: u32,
    /// Защита от голодания при РАЗНОМ приоритете (`tie_break` решает
    /// только равный). `Some(n)` — после `n` проигрышей подряд эффективный
    /// priority на один тик становится максимальным. Только через
    /// `Engine::run_tick` (нужна память между тиками) — см.
    /// `specs/specification.md` §3.1/§7.
    #[serde(default)]
    pub starvation_after: Option<u32>,
    /// Обратная связь по результату применения — см. [`FeedbackSpec`].
    /// `None` (по умолчанию) — обычное поведение, направление сдвига
    /// неизменно.
    #[serde(default)]
    pub feedback: Option<FeedbackSpec>,
    /// Ограниченная рекурсия в пределах одного тика — см. [`RecursionSpec`].
    /// `None` (по умолчанию) — обычное поведение, правило срабатывает не
    /// более одного раза на матч.
    #[serde(default)]
    pub recursion: Option<RecursionSpec>,
    /// Память по последовательности прошлых наблюдений — см. [`MemorySpec`].
    /// `None` (по умолчанию) — обычное поведение, без дополнительного
    /// условия на историю.
    #[serde(default)]
    pub memory: Option<MemorySpec>,
    /// Глобальный (не по позиции) бюджет побед в арбитраже для правила
    /// целиком — задуман как страховка от неограниченного роста у
    /// `ShiftSpec::keep_source`, но применим к любому правилу. Не кодируется
    /// wire-протоколом, не поддерживается на GPU. Полная семантика —
    /// `specs/specification.md` §4.10.
    #[serde(default)]
    pub max_activations: Option<u32>,
    /// Межслойное ЧТЕНИЕ для `LayeredEngine`: `(dx, dy, dz, тип)`, `dz` —
    /// индекс слоя-соседа (не координата). Только чтение против ПРЕДТИКОВОГО
    /// состояния чужого слоя — запись через слои не поддерживается. Пусто
    /// (по умолчанию) — нулевые накладные расходы, `LayeredEngine` не
    /// требуется. Полная модель и почему `pattern`/`cross_layer_reads` —
    /// разные поля — `layered.rs`'s doc-комментарий модуля,
    /// `specs/specification.md` §15.
    #[serde(default)]
    pub cross_layer_reads: Vec<(i8, i8, i8, CellType)>,
}

/// Собрать `Vec<Rule>` в `rule_index` (`HashMap<CellType, Vec<Rule>>`),
/// готовый для `Engine::new`/`LayeredEngine::new` — группировка по
/// центральному типу (`id.first()`, иначе `pattern.first()`'s тип; ни
/// того, ни другого — правило молча пропускается) + сортировка по
/// приоритету убывающе. Та же логика, что `config::load_config_str`
/// строит из YAML — публичная версия для правил, построенных программно
/// в Rust. Раньше не было единой функции — 20+ файлов независимо
/// переизобретали её, с расхождениями.
///
/// ## Examples
///
/// ```
/// use cellaria::types::{build_rule_index, CellType, Rule};
///
/// let rules = vec![
///     Rule { id: vec![CellType::new(1)], priority: 1, ..Default::default() },
///     Rule { id: vec![CellType::new(1)], priority: 5, ..Default::default() },
/// ];
/// let index = build_rule_index(rules);
/// let group = &index[&CellType::new(1)];
/// assert_eq!(group[0].priority, 5, "выше приоритет должен оказаться первым после сортировки");
/// ```
pub fn build_rule_index(rules: Vec<Rule>) -> HashMap<CellType, Vec<Rule>> {
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        let center_type = rule
            .id
            .first()
            .copied()
            .or_else(|| rule.pattern.first().map(|&(_, _, ct)| ct));
        if let Some(ct) = center_type {
            rule_index.entry(ct).or_default().push(rule);
        }
    }
    for rules in rule_index.values_mut() {
        rules.sort_by_key(|r| std::cmp::Reverse(r.priority));
    }
    rule_index
}

/// Ограниченная внутритиковая рекурсия — правило повторно проверяется и
/// применяется вдоль `direction`, до `max_depth` ДОПОЛНИТЕЛЬНЫХ раз, В
/// ПРЕДЕЛАХ ОДНОГО тика, читая уже ЗАПИСАННОЕ этим же каскадом состояние
/// (единственное осознанное исключение из "detect всегда читает pre-tick" —
/// ограничено ОДНИМ уже-арбитрированным матчем). Требует пустых `shifts`.
/// Полная семантика и статическая граница для графа конфликтов —
/// `specs/specification.md` §4.7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecursionSpec {
    /// Максимальное число ДОПОЛНИТЕЛЬНЫХ уровней каскада (0 — эквивалентно
    /// отсутствию рекурсии, но тогда лучше просто не устанавливать поле).
    pub max_depth: u8,
    /// Направление, вдоль которого повторяется правило.
    pub direction: Direction,
}

/// Защёлка смены направления: после `timeout` тиков подряд, где правило
/// всё ещё детектируется (ожидаемый результат не появился), направление
/// ЕДИНСТВЕННОГО сдвига навсегда переключается на `new_direction` —
/// необратимо, не цикл, счётчик не сбрасывается. Требует ровно один
/// `ShiftSpec`. Счётчик релоцируется вместе с головкой при каждом сдвиге
/// (иначе клетка всегда "новая" и `timeout` никогда бы не достигался).
/// Полная семантика, включая недокументированное взаимодействие с
/// `starvation_after`/`tie_break` — `specs/specification.md` §4.8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackSpec {
    /// Сколько тиков подряд матч должен детектироваться (независимо от
    /// исхода арбитража — считаются попытки, не поражения), прежде чем
    /// сработает переключение направления.
    pub timeout: u64,
    /// Направление сдвига после срабатывания.
    pub new_direction: Direction,
}

/// Точка и содержание записи в FIFO-буфер памяти (см. [`MemorySpec`]).
/// `NeighborType(dir)` — ДО арбитража, тип соседа на детекте. `RuleOutcome`
/// — ПОСЛЕ арбитража, выиграл этот матч или нет. Подробности —
/// `specs/specification.md` §4.9.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RecordTrigger {
    /// Наблюдать тип соседа в данном направлении, на каждом тике, когда
    /// этот матч детектируется (независимо от исхода арбитража).
    NeighborType(Direction),
    /// Наблюдать исход арбитража этого матча (`Applied`/`Missed`).
    RuleOutcome,
}

/// Одно значение, хранимое в FIFO-буфере памяти — либо тип клетки
/// (для `RecordTrigger::NeighborType`), либо исход арбитража
/// (для `RecordTrigger::RuleOutcome`). Один буфер хранит значения только
/// ОДНОГО из этих вариантов — оба триггера одного правила не смешиваются,
/// у правила ровно один `record_trigger`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RecordedValue {
    /// Тип клетки, увиденный через `NeighborType`.
    Type(CellType),
    /// Матч выиграл арбитраж в этом тике.
    Applied,
    /// Матч детектировался, но проиграл арбитраж в этом тике.
    Missed,
}

/// Память по ПОСЛЕДОВАТЕЛЬНОСТИ прошлых наблюдений — в отличие от
/// `starvation_after`/`feedback` (скалярный счётчик), может отличить ЛЮБОЙ
/// конкретный паттерн (например `[Missed, Applied, Missed]`) от другого той
/// же длины. Гейт: FIFO-буфер (`Engine::memory_buffers`) должен быть
/// ПОЛНОСТЬЮ заполнен И совпадать с `match_pattern` поэлементно — частичное
/// совпадение не поддерживается. 0 или 1 сдвиг, несовместимо с `recursion`
/// (`RuleOutcome`). Полная семантика — `specs/specification.md` §4.9.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySpec {
    /// Размер FIFO-буфера (K элементов).
    pub window: usize,
    /// Где и что записывается в буфер — см. [`RecordTrigger`].
    pub record_trigger: RecordTrigger,
    /// Целевая последовательность: гейт открыт, когда буфер полон и
    /// совпадает с этим значением поэлементно. Длина должна равняться
    /// `window` (проверяется в `config::load_config`).
    pub match_pattern: Vec<RecordedValue>,
}

// ============================================================================
// Граничные буферы ввода-вывода
// ============================================================================

/// Буфер для накопления данных на границе решётки.
/// Использует очереди по номерам каналов.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundaryBuffer {
    /// Очереди данных: `channel_number` → значения
    pub queues: HashMap<u32, VecDeque<Cell>>,
    /// Направление: "input" (ввод из stdin) или "output" (вывод в stdout).
    pub direction: String,
    /// Максимальный размер очереди (None = без ограничения).
    pub max_queue: Option<u8>,
}

impl BoundaryBuffer {
    pub fn new() -> Self {
        Self {
            queues: HashMap::new(),
            direction: String::new(),
            max_queue: None,
        }
    }

    /// Если очередь превышает `max_queue` — удаляется самый старый элемент.
    pub fn enqueue(&mut self, channel: u32, cell: Cell) {
        let queue = self.queues.entry(channel).or_default();
        if let Some(max) = self.max_queue {
            if queue.len() >= max as usize {
                queue.pop_front();
            }
        }
        queue.push_back(cell);
    }

    pub fn dequeue(&mut self, channel: u32) -> Vec<Cell> {
        self.queues
            .remove(&channel)
            .map_or_else(Vec::new, |qd| qd.into_iter().collect())
    }

    pub fn clear(&mut self) {
        self.queues.clear();
    }
}

impl Default for BoundaryBuffer {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Управление решёткой
// ============================================================================

/// Описывает область решётки, затронутую применением правила.
///
/// `x_start`/`x_end`/`y_start`/`y_end` — грубый bounding box (прямоугольник),
/// пригодный для приблизительных целей вроде "какую часть экрана
/// перерисовать" — включает промежуточные клетки, даже если реально
/// записаны были только некоторые из них (например, сдвиг на несколько
/// клеток разом пишет только исходную и целевую позиции, но bbox между ними
/// — прямоугольник). Для точного набора РЕАЛЬНО записанных клеток (важно
/// для сброса возраста — см. `written_cells`) используйте его, а не bbox.
#[derive(Debug, Clone, PartialEq)]
pub struct AffectedRegion {
    pub x_start: u32,
    pub x_end: u32,
    pub y_start: u32,
    pub y_end: u32,
    /// Флаг: применялись ли изменения (changes) вдобавок к сдвигам.
    pub has_changes: bool,
    /// Точный список РЕАЛЬНО записанных клеток (не bbox!) — то, что было
    /// вставлено в write-буфер при применении правила: очищенная исходная
    /// позиция сдвига, его цель, цели `changes`. Используется для сброса
    /// возраста (`reset_age`/`reset_age_for_regions`) — иначе клетка,
    /// оказавшаяся ВНУТРИ прямоугольника bbox, но не тронутая ни одной
    /// записью (например, между исходной и целевой позицией сдвига на N>1
    /// клеток), получала бы обнулённый возраст, хотя сама не менялась.
    pub written_cells: Vec<(u32, u32)>,
}

#[cfg(test)]
mod build_rule_index_tests {
    use super::*;

    #[test]
    fn test_build_rule_index_groups_and_sorts_by_priority_desc() {
        let rules = vec![
            Rule {
                id: vec![CellType::new(1)],
                priority: 3,
                ..Default::default()
            },
            Rule {
                id: vec![CellType::new(1)],
                priority: 9,
                ..Default::default()
            },
            Rule {
                id: vec![CellType::new(2)],
                priority: 1,
                ..Default::default()
            },
        ];
        let index = build_rule_index(rules);
        let group1 = &index[&CellType::new(1)];
        assert_eq!(group1.len(), 2);
        assert_eq!(
            group1[0].priority, 9,
            "must be sorted descending, not concatenation order"
        );
        assert_eq!(group1[1].priority, 3);
        assert_eq!(index[&CellType::new(2)].len(), 1);
    }

    /// Тот самый пробел, из-за которого `examples/flagship_gol.rs`'s
    /// собственная копия этой логики (`rule.id[0]`) паникует, а не
    /// корректно падает обратно на `pattern` -- правило без `id`, но с
    /// явным 2D `pattern`, обязано индексироваться по первому элементу
    /// `pattern`, не отбрасываться и не паниковать.
    #[test]
    fn test_build_rule_index_falls_back_to_pattern_when_id_is_empty() {
        let rules = vec![Rule {
            id: vec![],
            pattern: vec![(0, 0, CellType::new(7))],
            priority: 1,
            ..Default::default()
        }];
        let index = build_rule_index(rules);
        assert!(
            index.contains_key(&CellType::new(7)),
            "must fall back to pattern's first cell type when id is empty"
        );
    }

    #[test]
    fn test_build_rule_index_drops_rule_with_neither_id_nor_pattern() {
        let rules = vec![Rule {
            id: vec![],
            pattern: vec![],
            priority: 1,
            ..Default::default()
        }];
        let index = build_rule_index(rules);
        assert!(
            index.is_empty(),
            "a rule with nothing to match on has no way to be indexed -- must be silently dropped, not panic"
        );
    }
}
