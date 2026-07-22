use std::collections::VecDeque;

// === Core Types ===

/// Условный тип глобальной координаты решётки.
pub(crate) type GridCoord = (usize, usize);

/// Тип ячейки (0–255). Ячейки с одинаковым типом считаются совпадающими.
///
/// **Важно:** значение 255 (0xFF) навсегда зарезервировано под протокол
/// RuleStore (терминатор пакета). Запрещено использовать 255 в `pattern`
/// или `result_cells` обычных правил — это приведёт к ошибке загрузки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CellType(pub u8);

/// Значение ячейки. Оборачивает [`CellType`] для семантического разделения.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellValue(pub CellType);

/// Уникальный идентификатор правила.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RuleId(pub u32);

/// Направление как дельта (dx, dy).
///
/// Обобщённая замена [`ShiftDirection`] — позволяет задавать произвольные
/// направления сдвига, а не только основные четыре стороны.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Direction(pub i8, pub i8);

impl Direction {
    /// Север (0, -1).
    pub const NORTH: Direction = Direction(0, -1);
    /// Юг (0, 1).
    pub const SOUTH: Direction = Direction(0, 1);
    /// Восток (1, 0).
    pub const EAST: Direction = Direction(1, 0);
    /// Запад (-1, 0).
    pub const WEST: Direction = Direction(-1, 0);
}

/// Направление сдвига цепочки (устаревший enum, сохранён для обратной
/// совместимости).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftDirection {
    North,
    South,
    East,
    West,
}

impl From<ShiftDirection> for Direction {
    fn from(d: ShiftDirection) -> Self {
        match d {
            ShiftDirection::North => Direction::NORTH,
            ShiftDirection::South => Direction::SOUTH,
            ShiftDirection::East => Direction::EAST,
            ShiftDirection::West => Direction::WEST,
        }
    }
}

/// Действие при выталкивании значения за край цепочки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverflowAction {
    /// Вытолкнутое значение отбрасывается.
    Discard,
    /// Вытолкнутое значение заменяется указанным.
    WriteValue(CellValue),
    /// Вытолкнутое значение отправляется в канал с указанным ID.
    OutputToChannel(u32),
}

/// Спецификация сдвига цепочки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShiftSpec {
    /// Направление сдвига.
    pub direction: Direction,
    /// Длина цепочки (количество ячеек, участвующих в сдвиге).
    pub chain_length: u8,
    /// Значение, которым заполняется освободившаяся первая ячейка.
    pub fill_value: CellValue,
    /// Действие с вытолкнутым значением.
    pub overflow_action: OverflowAction,
}

/// Область, затронутая применением правила.
///
/// - [`LocalGroup`](AffectedRegion::LocalGroup) — группа ячеек, которые заменяются на результат.
/// - [`Chain`](AffectedRegion::Chain) — группа ячеек + цепочка сдвига вдоль направления.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AffectedRegion {
    /// Простая замена группы ячеек без сдвига.
    LocalGroup {
        /// Координаты ячеек, совпавших с паттерном.
        group_cells: Vec<GridCoord>,
        /// Новые значения для этих ячеек.
        result_cells: Vec<CellValue>,
    },
    /// Замена группы ячеек со сдвигом цепочки.
    Chain {
        /// Координаты ячеек, совпавших с паттерном.
        group_cells: Vec<GridCoord>,
        /// Новые значения для этих ячеек.
        result_cells: Vec<CellValue>,
        /// Координаты цепочки для сдвига (включая group_cells).
        chain_cells: Vec<GridCoord>,
        /// Направление сдвига.
        direction: Direction,
        /// Значение, вставляемое в начало цепочки.
        fill_value: CellValue,
        /// Действие с вытолкнутым значением.
        overflow_action: OverflowAction,
    },
}

/// Правило редукции.
///
/// Содержит паттерн (набор смещений и типов для сравнения),
/// результат (новые значения для ячеек паттерна) и опциональный сдвиг.
///
/// ## min_age
///
/// Минимальный возраст ячейки-центра для активации правила.
/// Правило сработает, только если `cell.age >= min_age`.
/// По умолчанию 0 — правило срабатывает в любой момент.
///
/// Назначение: «очистка через правило» (аксиома 5). Вместо TTL
/// в ячейке, неактивные ячейки очищаются правилом с `min_age > 0`.
/// Возраст — пассивная история для арбитража; условие очистки —
/// `min_age` в правиле.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// Уникальный идентификатор.
    pub id: RuleId,
    /// Приоритет (больше = выше).
    pub priority: u8,
    /// Минимальный возраст ячейки-центра для активации (по умолчанию 0).
    pub min_age: u64,
    /// Паттерн: (dx, dy, CellType) — смещения относительно центра.
    pub pattern: Vec<(i8, i8, CellType)>,
    /// Результат: новые значения ячеек в порядке паттерна.
    pub result_cells: Vec<CellValue>,
    /// Опциональный сдвиг цепочки.
    pub shift: Option<ShiftSpec>,
}

/// Буфер граничного канала ввода-вывода.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundaryBuffer {
    /// ID канала.
    pub channel: u32,
    /// Входящая очередь (данные для записи в ячейку на входе).
    pub input_queue: VecDeque<CellValue>,
    /// Исходящая очередь (данные, выведенные из ячейки на выходе).
    pub output_queue: VecDeque<CellValue>,
    /// Ожидающий вывода данных (устанавливается при OverflowAction::OutputToChannel).
    pub pending_output: Option<CellValue>,
    /// Максимальная глубина очереди.
    pub max_queue_depth: u8,
}

/// Ячейка решётки.
///
/// Больше не содержит `boundary` — граничные буферы вынесены
/// в отдельную HashMap в Grid.
#[derive(Debug, Clone, Default)]
pub struct Cell {
    /// Значение (тип) ячейки.
    pub value: CellValue,
    /// Возраст (количество тиков без изменений).
    pub age: u64,
}

/// Совпадение правила на конкретной ячейке.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleMatch {
    /// ID сработавшего правила.
    pub rule_id: RuleId,
    /// Координата центра совпадения.
    pub center: GridCoord,
    /// Приоритет правила (для арбитража).
    pub priority: u8,
    /// Возраст ячейки (для арбитража).
    pub age: u64,
    /// Область, затронутая применением.
    pub affected_region: AffectedRegion,
}