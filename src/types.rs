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
}

/// Значение ячейки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CellValue(pub CellType);

impl CellValue {
    pub const fn new(value: u8) -> Self {
        Self(CellType::new(value))
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
}

impl Default for Cell {
    fn default() -> Self {
        Self::empty()
    }
}

// ============================================================================
// ChangeValue — значение изменения: литерал или ссылка на паттерн
// ============================================================================

/// Значение для записи в changes: либо литерал (u8), либо ссылка на позицию
/// в паттерне ($0, $1, ...).
///
/// ## Examples
///
/// ```
/// use cellaria::types::ChangeValue;
///
/// let lit = ChangeValue::Literal(42);
/// let ref0 = ChangeValue::Ref(0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChangeValue {
    /// Числовой литерал, например 5.
    Literal(u8),
    /// Ссылка на позицию в паттерне (0-based), например "$0" = Ref(0).
    Ref(usize),
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

/// Спецификация сдвига: направление + количество шагов.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShiftSpec {
    pub direction: Direction,
    pub steps: u16,
}

impl ShiftSpec {
    pub const fn new(direction: Direction, steps: u16) -> Self {
        Self { direction, steps }
    }
}

// ============================================================================
// Идентификаторы правил
// ============================================================================

/// Идентификатор правила — кортеж из значений внутренней области правила.
pub type RuleId = Vec<CellType>;

/// Хранит позицию совпадения и идентификатор (список CellType) сработавшего правила.
///
/// ## Examples
///
/// ```
/// use cellaria::types::{RuleMatch, CellType};
///
/// let m = RuleMatch {
///     x: 0,
///     y: 0,
///     pattern: vec![vec![1, 2]],
///     rule_id: vec![CellType::new(1), CellType::new(2)],
///     rule_idx: 0,
/// };
/// assert_eq!(m.x, 0);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct RuleMatch {
    pub x: u32,
    pub y: u32,
    pub pattern: Vec<Vec<u8>>,
    pub rule_id: RuleId,
    /// Позиция сработавшего правила в `rule_index[rule_id[0]]` (после сортировки
    /// по приоритету). `rule_id` сам по себе не уникален — несколько правил могут
    /// иметь одинаковый `id` (паттерн недетерминированного выбора), поэтому именно
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
    #[serde(rename = "write")]
    Write(u8),
}

// ============================================================================
// Правила
// ============================================================================

/// Полное определение правила.
///
/// ## Examples
///
/// ```
/// use cellaria::types::{Rule, CellType, ShiftSpec, Direction, ChangeValue, OverflowAction};
///
/// let rule = Rule {
///     id: vec![CellType::new(1), CellType::new(2)],
///     pattern: vec![(0, 0, CellType::new(1)), (1, 0, CellType::new(2))],
///     shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
///     changes: vec![(1, 0, ChangeValue::Literal(3))],
///     active_only: false,
///     priority: 10,
///     min_age: 0,
///     overflow: OverflowAction::Discard,
/// };
/// assert_eq!(rule.priority, 10);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl Rule {
    /// Идентификатор (RuleId) — внутренняя область.
    pub fn id(&self) -> &[CellType] {
        &self.id
    }

    /// Длина внутренней области (n).
    pub fn id_len(&self) -> usize {
        self.id.len()
    }
}

// ============================================================================
// Граничные буферы ввода-вывода
// ============================================================================

/// Буфер для накопления данных на границе решётки.
/// Использует очереди по номерам каналов.
#[derive(Debug, Clone)]
pub struct BoundaryBuffer {
    /// Очереди данных: channel_number → [значения]
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

    /// Записать ячейку в очередь указанного канала.
    /// Если очередь превышает max_queue — удаляется самый старый элемент (pop_front).
    pub fn enqueue(&mut self, channel: u32, cell: Cell) {
        let queue = self.queues.entry(channel).or_default();
        if let Some(max) = self.max_queue {
            if queue.len() >= max as usize {
                queue.pop_front();
            }
        }
        queue.push_back(cell);
    }

    /// Извлечь все ячейки из очереди указанного канала.
    pub fn dequeue(&mut self, channel: u32) -> Vec<Cell> {
        self.queues.remove(&channel).map_or_else(Vec::new, |qd| qd.into_iter().collect())
    }

    /// Очистить все очереди.
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
/// Используется для оптимизации: при арбитраже пересекающиеся AffectedRegion
/// позволяют выявить конфликты, не просматривая всю решётку заново.
#[derive(Debug, Clone, PartialEq)]
pub struct AffectedRegion {
    pub x_start: u32,
    pub x_end: u32,
    pub y_start: u32,
    pub y_end: u32,
    /// Флаг: применялись ли изменения (changes) вдобавок к сдвигам.
    pub has_changes: bool,
}