use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// Базовые типы данных
// ============================================================================

/// Вещественное значение ячейки (обёртка для будущих расширений).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

/// Полное представление ячейки.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub value: CellValue,
    pub age: u64,
}

impl Cell {
    /// Создаёт ячейку с возрастом 0.
    pub const fn new(value: u8) -> Self {
        Self {
            value: CellValue::new(value),
            age: 0,
        }
    }

    /// Пустая ячейка (значение 0, возраст 0).
    pub const fn empty() -> Self {
        Self::new(0)
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::empty()
    }
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
#[derive(Debug, Clone, PartialEq)]
pub struct RuleMatch {
    pub x: u32,
    pub y: u32,
    pub pattern: Vec<Vec<u8>>,
    pub rule_id: RuleId,
}

// ============================================================================
// Правила
// ============================================================================

/// Действие при выходе за границы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OverflowAction {
    /// Обрезать (не сдвигать).
    Clip,
    /// Циклический перенос (тор).
    Wrap,
    /// Записать значение в канал.
    Channel,
    /// Заменить выходящее значение.
    Write,
    /// Отбросить выходящее значение.
    Discard,
}

/// Полное определение правила.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// Внутренняя область (n-кортеж CellType).
    pub id: RuleId,
    /// Паттерн для сопоставления (как правило выглядит на решётке).
    pub pattern: Vec<Vec<u8>>,
    /// Сдвиги: каждая группа срабатывает в порядке приоритета.
    pub shifts: Vec<Vec<ShiftSpec>>,
    /// Изменения ячеек: (смещение_x, смещение_y, новое_значение).
    /// ВАЖНО: эти изменения применяются только если хотя бы один сдвиг был выполнен.
    pub changes: Vec<(i32, i32, u8)>,
    /// Если true — правило проверяется только в активных ячейках.
    /// Если false — проверяется везде.
    pub active_only: bool,
    /// Приоритет правила (больше = выше).
    pub priority: u32,
    /// Минимальный возраст ячейки-центра для активации правила.
    /// Правило срабатывает только если возраст ячейки ≥ min_age.
    pub min_age: u64,
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
    pub queues: HashMap<u32, Vec<Cell>>,
    /// Направление: "input" (ввод из stdin) или "output" (вывод в stdout).
    pub direction: String,
}

impl BoundaryBuffer {
    pub fn new() -> Self {
        Self {
            queues: HashMap::new(),
            direction: String::new(),
        }
    }

    /// Записать ячейку в очередь указанного канала.
    pub fn enqueue(&mut self, channel: u32, cell: Cell) {
        self.queues.entry(channel).or_default().push(cell);
    }

    /// Извлечь все ячейки из очереди указанного канала.
    pub fn dequeue(&mut self, channel: u32) -> Vec<Cell> {
        self.queues.remove(&channel).unwrap_or_default()
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