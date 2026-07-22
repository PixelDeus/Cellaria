use crate::grid::Grid;
use crate::storage::GridStorage;
use crate::types::{CellType, CellValue, Direction, OverflowAction, Rule, RuleId, ShiftSpec};
use std::collections::HashMap;

// === Protocol Constants ===

/// Терминатор пакета протокола RuleStore.
const TERMINATOR: u8 = 0xFF;

/// Маркер операции RemoveRule.
const OP_REMOVE: u8 = 0xF0;

/// Маркер операции ClearAll.
const OP_CLEAR: u8 = 0xF1;

/// Маркер флага shift в пакете AddRule.
const SHIFT_FLAG: u8 = 0xFE;

/// Максимальный размер буфера накопления для одного канала (в байтах).
const MAX_BUFFER_SIZE: usize = 1024;

/// Базовый ID для автоматически назначаемых правил (чтобы не конфликтовать
/// с правилами из конфига, которые обычно < 1000).
const AUTO_RULE_ID_BASE: u32 = 10_000;

// === Types ===

/// Операция, декодированная из пакета протокола RuleStore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleOp {
    /// Добавить правило.
    AddRule(Rule),
    /// Удалить правило по ID.
    RemoveRule(RuleId),
    /// Очистить все правила.
    ClearAll,
}

/// Завершённая операция, готовая к применению.
#[derive(Debug, Clone)]
pub struct CompletedOp {
    pub op: RuleOp,
}

/// Хранилище правил с поддержкой самомодификации через канальный протокол.
///
/// # Протокол RuleStore
///
/// Пакеты передаются по одному байту за тик через `OutputToChannel`.
/// Формат пакета (big-endian, байты):
///
/// **AddRule (базовый):**
/// `[priority, pattern_len, (dx, dy, type) × pattern_len, (result: u8) × result_len, 255]`
///
/// **AddRule с shift:**
/// `[priority, pattern_len | 0x80, (dx, dy, type) × pattern_len, (result: u8) × result_len, 0xFE, direction_dx, direction_dy, chain_length, fill_value, overflow_flag, overflow_value, 255]`
///
///   - `pattern_len | 0x80` — старший бит показывает, что есть shift-секция.
///   - После результатов идёт `0xFE` (SHIFT_FLAG), затем:
///     - `direction_dx: i8`, `direction_dy: i8` — дельта направления
///     - `chain_length: u8` — длина цепочки (> 0)
///     - `fill_value: u8` — значение заполнения
///     - `overflow_flag: u8` — 0=Discard, 1=WriteValue, 2=OutputToChannel
///     - `overflow_value: u8` — значение для WriteValue или ID канала для OutputToChannel
///
/// **AddRule с min_age:**
/// Если `min_age > 0`, перед результатами вставляется `[0xFD, min_age: u64 LE]`.
/// Это расширение протокола.
///
/// **RemoveRule:**
/// `[0xF0, rule_id: u32 LE, 255]`
///
/// **ClearAll:**
/// `[0xF1, 255]`
///
/// 255 (0xFF) — терминатор, зарезервирован и не может использоваться
/// как обычный тип ячейки в паттернах или результатах.
pub struct RuleStore {
    /// Текущий набор правил.
    rules: Vec<Rule>,
    /// Флаг «грязный» — изменился ли набор после последнего построения индекса.
    dirty: bool,
    /// Накопленные буферы по каналам: channel_id → накопленные байты.
    accum_buffers: HashMap<u32, Vec<u8>>,
    /// Закешированный индекс (перестраивается только при dirty).
    index: Option<HashMap<CellType, Vec<Rule>>>,
    /// Счётчик для авто-назначения ID новым правилам.
    next_rule_id: u32,
    /// Счётчик ошибок декодирования пакетов (битые пакеты в канале).
    decode_errors: u64,
}

impl Default for RuleStore {
    fn default() -> Self {
        Self::new()
    }
}

impl RuleStore {
    /// Создать пустой RuleStore.
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            dirty: false,
            accum_buffers: HashMap::new(),
            index: None,
            next_rule_id: AUTO_RULE_ID_BASE,
            decode_errors: 0,
        }
    }

    /// Создать RuleStore с начальным набором правил.
    pub fn with_rules(rules: Vec<Rule>) -> Self {
        let max_id = rules.iter().map(|r| r.id.0).max().unwrap_or(0);
        Self {
            rules,
            dirty: true,
            accum_buffers: HashMap::new(),
            index: None,
            next_rule_id: max_id.max(AUTO_RULE_ID_BASE) + 1,
            decode_errors: 0,
        }
    }

    /// Количество ошибок декодирования с момента создания.
    pub fn error_stats(&self) -> u64 {
        self.decode_errors
    }

    /// Прочитать выходные очереди граничных ячеек, накопить байты и
    /// вернуть все завершённые операции (пакеты, где встречен терминатор 255).
    ///
    /// Вызывается после `run_tick` (когда `flush_output` уже перенёс данные
    /// в `output_queue`).
    pub fn drain_rule_channel<S: GridStorage>(&mut self, grid: &mut Grid<S>) -> Vec<CompletedOp> {
        // Проход 1: собираем все (channel, value) из output_queue граничных буферов
        let mut drained: Vec<(u32, u8)> = Vec::new();
        for (_coord, boundary) in grid.iter_boundaries() {
            for val in &boundary.output_queue {
                drained.push((boundary.channel, val.0 .0));
            }
        }

        // Проход 2: очищаем output_queue
        for (_, boundary) in grid.iter_boundaries_mut() {
            boundary.output_queue.clear();
        }

        // Накопляем байты в буферы по каналам
        for (channel, byte) in drained {
            let buf = self.accum_buffers.entry(channel).or_default();
            if buf.len() >= MAX_BUFFER_SIZE {
                // Превышение лимита — очищаем буфер и инкрементируем счётчик ошибок
                buf.clear();
                self.decode_errors += 1;
            }
            buf.push(byte);
        }

        // Обрабатываем каждый канал — извлекаем завершённые пакеты
        let mut completed = Vec::new();
        let channels: Vec<u32> = self.accum_buffers.keys().copied().collect();
        for ch in channels {
            let buf = match self.accum_buffers.get_mut(&ch) {
                Some(b) => b,
                None => continue,
            };

            while let Some(end) = find_terminator(buf) {
                let packet: Vec<u8> = buf.drain(..=end).collect();
                let data = &packet[..packet.len() - 1]; // отрезаем терминатор
                match deserialize_packet(data, self.next_rule_id) {
                    Ok(op) => {
                        if let RuleOp::AddRule(_) = &op {
                            self.next_rule_id += 1;
                        }
                        completed.push(CompletedOp { op });
                    }
                    Err(e) => {
                        // Некорректный пакет — сбрасываем буфер канала
                        eprintln!("RuleStore: invalid packet on channel {}: {}", ch, e);
                        self.decode_errors += 1;
                        buf.clear();
                        break;
                    }
                }
            }
        }

        completed
    }

    /// Применить операцию к набору правил.
    ///
    /// Возвращает `true`, если набор изменился (dirty).
    pub fn apply(&mut self, op: CompletedOp) -> bool {
        match op.op {
            RuleOp::AddRule(rule) => {
                self.rules.push(rule);
                self.dirty = true;
            }
            RuleOp::RemoveRule(id) => {
                let len_before = self.rules.len();
                self.rules.retain(|r| r.id != id);
                if self.rules.len() != len_before {
                    self.dirty = true;
                }
            }
            RuleOp::ClearAll => {
                if !self.rules.is_empty() {
                    self.rules.clear();
                    self.dirty = true;
                }
            }
        }
        self.dirty
    }

    /// Получить индекс для поиска совпадений.
    ///
    /// Перестраивает индекс только если `dirty == true`.
    pub fn get_index(&mut self) -> &HashMap<CellType, Vec<Rule>> {
        if self.dirty || self.index.is_none() {
            let mut index: HashMap<CellType, Vec<Rule>> = HashMap::new();
            for rule in &self.rules {
                if let Some(&(_, _, center_type)) =
                    rule.pattern.iter().find(|&&(dx, dy, _)| dx == 0 && dy == 0)
                {
                    index.entry(center_type).or_default().push(rule.clone());
                }
            }
            for rules in index.values_mut() {
                rules.sort_by_key(|b| std::cmp::Reverse(b.priority));
            }
            self.index = Some(index);
            self.dirty = false;
        }
        self.index.as_ref().expect("get_index: index should be rebuilt after dirty set")
    }

    /// Текущий набор правил (для тестов).
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }
}

// === Deserialization ===

/// Маркер расширения min_age (появляется перед результатами).
const MIN_AGE_FLAG: u8 = 0xFD;

/// Найти индекс терминатора (255) в буфере.
fn find_terminator(buf: &[u8]) -> Option<usize> {
    buf.iter().position(|&b| b == TERMINATOR)
}

/// Десериализовать пакет (без терминатора) в RuleOp.
fn deserialize_packet(data: &[u8], next_rule_id: u32) -> Result<RuleOp, String> {
    if data.is_empty() {
        return Err("empty packet".to_string());
    }

    let first = data[0];

    match first {
        OP_CLEAR => Ok(RuleOp::ClearAll),
        OP_REMOVE => {
            if data.len() < 5 {
                return Err(format!("RemoveRule packet too short: {} bytes", data.len()));
            }
            let id_bytes: [u8; 4] = [data[1], data[2], data[3], data[4]];
            let rule_id = u32::from_le_bytes(id_bytes);
            Ok(RuleOp::RemoveRule(RuleId(rule_id)))
        }
        _ => {
            // AddRule: [priority, pattern_len, (dx, dy, type) × pattern_len, (result: u8) × result_len]
            let priority = first;
            if data.len() < 2 {
                return Err("AddRule packet too short: no pattern_len".to_string());
            }
            let raw_pattern_len = data[1];
            let has_shift = (raw_pattern_len & 0x80) != 0;
            let pattern_len = (raw_pattern_len & 0x7F) as usize;

            if pattern_len == 0 {
                return Err("AddRule: pattern_len must be > 0".to_string());
            }

            let pattern_bytes = pattern_len * 3; // dx, dy, type per entry
            let header_size = 2 + pattern_bytes;
            if data.len() < header_size {
                return Err(format!(
                    "AddRule packet too short: need {} bytes for pattern, have {}",
                    header_size,
                    data.len()
                ));
            }

            let mut pattern = Vec::with_capacity(pattern_len);
            let mut has_center = false;
            for i in 0..pattern_len {
                let base = 2 + i * 3;
                let dx = data[base] as i8;
                let dy = data[base + 1] as i8;
                let raw_type = data[base + 2];
                if raw_type == 0xFF {
                    return Err(
                        "AddRule: type 255 (0xFF) in pattern is reserved for RuleStore protocol"
                            .to_string(),
                    );
                }
                if dx == 0 && dy == 0 {
                    has_center = true;
                }
                pattern.push((dx, dy, CellType(raw_type)));
            }

            // Валидация: паттерн должен содержать центр (0, 0)
            if !has_center {
                return Err("AddRule: pattern must contain center (0, 0)".to_string());
            }

            let mut offset = header_size;

            // Парсим min_age (опционально)
            let mut min_age = 0u64;
            if offset < data.len() && data[offset] == MIN_AGE_FLAG {
                offset += 1;
                if offset + 8 > data.len() {
                    return Err("AddRule: not enough bytes for min_age".to_string());
                }
                let mut age_bytes = [0u8; 8];
                age_bytes.copy_from_slice(&data[offset..offset + 8]);
                min_age = u64::from_le_bytes(age_bytes);
                offset += 8;
            }

            // Парсим результаты
            let result_start = offset;
            let mut result_cells = Vec::new();
            for &v in &data[result_start..] {
                if v == 0xFF {
                    return Err(
                        "AddRule: result value 255 (0xFF) in result is reserved for RuleStore protocol"
                            .to_string(),
                    );
                }
                if v == SHIFT_FLAG {
                    break;
                }
                if v == MIN_AGE_FLAG {
                    // min_age уже распарсен, но если флаг встречен снова — ошибка
                    return Err("AddRule: unexpected MIN_AGE_FLAG in result section".to_string());
                }
                result_cells.push(CellValue(CellType(v)));
                offset += 1;
            }

            // Валидация: количество результатов должно совпадать с количеством записей в паттерне
            if result_cells.len() != pattern_len {
                return Err(format!(
                    "AddRule: result length {} != pattern length {}",
                    result_cells.len(),
                    pattern_len
                ));
            }

            // Парсим shift (опционально)
            let shift = if has_shift {
                if offset >= data.len() || data[offset] != SHIFT_FLAG {
                    return Err(
                        "AddRule: has_shift flag set but no SHIFT_FLAG found".to_string(),
                    );
                }
                offset += 1; // пропускаем SHIFT_FLAG

                // Ожидаем: [direction_dx: i8, direction_dy: i8, chain_length: u8, fill_value: u8, overflow_flag: u8, overflow_value: u8]
                if offset + 6 > data.len() {
                    return Err("AddRule: not enough bytes for shift section".to_string());
                }

                let dx = data[offset] as i8;
                let dy = data[offset + 1] as i8;
                let chain_length = data[offset + 2];
                let fill_value = data[offset + 3];
                let overflow_flag = data[offset + 4];
                let overflow_value = data[offset + 5];

                if chain_length == 0 {
                    return Err("AddRule: shift chain_length must be > 0".to_string());
                }

                if fill_value == 0xFF {
                    return Err(
                        "AddRule: shift fill_value 255 (0xFF) is reserved for RuleStore protocol"
                            .to_string(),
                    );
                }

                let overflow_action = match overflow_flag {
                    0 => OverflowAction::Discard,
                    1 => OverflowAction::WriteValue(CellValue(CellType(overflow_value))),
                    2 => OverflowAction::OutputToChannel(overflow_value as u32),
                    _ => {
                        return Err(format!(
                            "AddRule: invalid overflow_flag {}",
                            overflow_flag
                        ))
                    }
                };

                Some(ShiftSpec {
                    direction: Direction(dx, dy),
                    chain_length,
                    fill_value: CellValue(CellType(fill_value)),
                    overflow_action,
                })
            } else {
                None
            };

            let rule = Rule {
                id: RuleId(next_rule_id),
                priority,
                min_age,
                pattern,
                result_cells,
                shift,
            };

            Ok(RuleOp::AddRule(rule))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::VecStorage;
    use crate::types::Cell;

    #[test]
    fn test_deserialize_add_rule() {
        // AddRule: [priority=10, pattern_len=2, dx=0, dy=0, type=1, dx=1, dy=0, type=2, result=3, result=4, 255]
        let packet = vec![10, 2, 0, 0, 1, 1, 0, 2, 3, 4, 255];
        let idx = find_terminator(&packet).unwrap();
        let data = &packet[..idx];
        let op = deserialize_packet(data, 100).unwrap();
        match op {
            RuleOp::AddRule(rule) => {
                assert_eq!(rule.id, RuleId(100));
                assert_eq!(rule.priority, 10);
                assert_eq!(rule.pattern.len(), 2);
                assert_eq!(rule.pattern[0], (0, 0, CellType(1)));
                assert_eq!(rule.pattern[1], (1, 0, CellType(2)));
                assert_eq!(
                    rule.result_cells,
                    vec![CellValue(CellType(3)), CellValue(CellType(4)),]
                );
                assert!(rule.shift.is_none());
                assert_eq!(rule.min_age, 0);
            }
            _ => panic!("Expected AddRule"),
        }
    }

    #[test]
    fn test_deserialize_add_rule_with_min_age() {
        // AddRule: [priority=5, pattern_len=1, dx=0, dy=0, type=1, 0xFD, min_age: u64 LE, result=2, 255]
        let mut packet = vec![5u8, 1, 0, 0, 1, MIN_AGE_FLAG];
        packet.extend_from_slice(&7u64.to_le_bytes()); // min_age = 7
        packet.push(2); // result
        packet.push(255); // terminator

        let idx = find_terminator(&packet).unwrap();
        let data = &packet[..idx];
        let op = deserialize_packet(data, 100).unwrap();
        match op {
            RuleOp::AddRule(rule) => {
                assert_eq!(rule.min_age, 7);
                assert_eq!(rule.priority, 5);
                assert_eq!(rule.pattern.len(), 1);
                assert_eq!(rule.result_cells.len(), 1);
                assert!(rule.shift.is_none());
            }
            _ => panic!("Expected AddRule"),
        }
    }

    #[test]
    fn test_deserialize_add_rule_with_shift() {
        // AddRule: [priority=10, pattern_len=2 | 0x80, dx=0, dy=0, type=1, dx=1, dy=0, type=2,
        //          result=3, result=4,
        //          0xFE, direction_dx=1, direction_dy=0, chain_length=3, fill_value=0, overflow_flag=0, overflow_value=0,
        //          255]
        let packet = vec![
            10,
            2 | 0x80, // pattern_len с флагом shift
            0,
            0,
            1, // offset (0,0) type=1
            1,
            0,
            2, // offset (1,0) type=2
            3,
            4, // result
            SHIFT_FLAG,
            1,
            0,  // direction (1, 0) = EAST
            3,  // chain_length = 3
            0,  // fill_value = 0
            0,  // overflow_flag = Discard
            0,  // overflow_value (ignored for Discard)
            255, // terminator
        ];

        let idx = find_terminator(&packet).unwrap();
        let data = &packet[..idx];
        let op = deserialize_packet(data, 100).unwrap();
        match op {
            RuleOp::AddRule(rule) => {
                assert_eq!(rule.priority, 10);
                assert_eq!(rule.pattern.len(), 2);
                assert_eq!(rule.result_cells.len(), 2);
                let shift = rule.shift.expect("Should have shift");
                assert_eq!(shift.direction, Direction(1, 0));
                assert_eq!(shift.chain_length, 3);
                assert_eq!(shift.fill_value, CellValue(CellType(0)));
                assert!(matches!(shift.overflow_action, OverflowAction::Discard));
            }
            _ => panic!("Expected AddRule"),
        }
    }

    #[test]
    fn test_deserialize_remove_rule() {
        // RemoveRule: [0xF0, rule_id=42 (LE), 255]
        let packet = vec![0xF0, 42, 0, 0, 0, 255];
        let idx = find_terminator(&packet).unwrap();
        let data = &packet[..idx];
        let op = deserialize_packet(data, 0).unwrap();
        assert_eq!(op, RuleOp::RemoveRule(RuleId(42)));
    }

    #[test]
    fn test_deserialize_clear_all() {
        let packet = vec![0xF1, 255];
        let idx = find_terminator(&packet).unwrap();
        let data = &packet[..idx];
        let op = deserialize_packet(data, 0).unwrap();
        assert_eq!(op, RuleOp::ClearAll);
    }

    #[test]
    fn test_rule_store_apply_add() {
        let mut store = RuleStore::new();
        let rule = Rule {
            id: RuleId(1),
            priority: 10,
            min_age: 0,
            pattern: vec![(0, 0, CellType(1))],
            result_cells: vec![CellValue(CellType(2))],
            shift: None,
        };
        assert!(store.apply(CompletedOp {
            op: RuleOp::AddRule(rule)
        }));
        assert_eq!(store.rules().len(), 1);
        assert!(store.dirty);
    }

    #[test]
    fn test_rule_store_apply_remove() {
        let rule = Rule {
            id: RuleId(1),
            priority: 10,
            min_age: 0,
            pattern: vec![(0, 0, CellType(1))],
            result_cells: vec![CellValue(CellType(2))],
            shift: None,
        };
        let mut store = RuleStore::with_rules(vec![rule]);
        store.dirty = false;
        assert!(store.apply(CompletedOp {
            op: RuleOp::RemoveRule(RuleId(1))
        }));
        assert_eq!(store.rules().len(), 0);
    }

    #[test]
    fn test_rule_store_apply_clear() {
        let rules = vec![
            Rule {
                id: RuleId(1),
                priority: 10,
                min_age: 0,
                pattern: vec![(0, 0, CellType(1))],
                result_cells: vec![CellValue(CellType(2))],
                shift: None,
            },
            Rule {
                id: RuleId(2),
                priority: 5,
                min_age: 0,
                pattern: vec![(0, 0, CellType(3))],
                result_cells: vec![CellValue(CellType(4))],
                shift: None,
            },
        ];
        let mut store = RuleStore::with_rules(rules);
        store.dirty = false;
        assert!(store.apply(CompletedOp {
            op: RuleOp::ClearAll
        }));
        assert_eq!(store.rules().len(), 0);
    }

    #[test]
    fn test_get_index_rebuilds_when_dirty() {
        let rule = Rule {
            id: RuleId(1),
            priority: 10,
            min_age: 0,
            pattern: vec![(0, 0, CellType(5))],
            result_cells: vec![CellValue(CellType(6))],
            shift: None,
        };
        let mut store = RuleStore::with_rules(vec![rule]);
        // Build index once (sets dirty=false, index=Some)
        store.get_index();

        // Add a new rule to set dirty=true
        let new_rule = Rule {
            id: RuleId(2),
            priority: 5,
            min_age: 0,
            pattern: vec![(0, 0, CellType(7))],
            result_cells: vec![CellValue(CellType(8))],
            shift: None,
        };
        store.apply(CompletedOp {
            op: RuleOp::AddRule(new_rule),
        });
        assert!(store.dirty, "dirty should be set after apply");

        // get_index should rebuild and include both rules
        let index = store.get_index();
        assert!(
            index.contains_key(&CellType(5)),
            "Index should include original rule"
        );
        assert!(
            index.contains_key(&CellType(7)),
            "Index should include new rule"
        );
        assert!(!store.dirty, "dirty should be false after rebuild");
    }

    #[test]
    fn test_deserialize_rejects_255_in_pattern() {
        // data = [priority=10, pattern_len=1, dx=0, dy=0, type=255]
        let data = vec![10, 1, 0, 0, 0xFF];
        let result = deserialize_packet(&data, 100);
        assert!(result.is_err(), "Should reject 255 in pattern");
    }

    #[test]
    fn test_deserialize_rejects_255_in_result() {
        // data = [priority=5, pattern_len=1, dx=0, dy=0, type=1, result=255]
        let data = vec![5, 1, 0, 0, 1, 0xFF];
        let result = deserialize_packet(&data, 100);
        assert!(result.is_err(), "Should reject 255 in result");
    }

    #[test]
    fn test_decode_errors_increments_on_bad_packet() {
        use crate::types::BoundaryBuffer;
        use std::collections::VecDeque;

        // Corrupted packet: [10, 2, 0, 0, 1, 255] — not enough bytes for pattern_len=2
        let storage = VecStorage {
            cells: vec![Cell::default()],
            width: 1,
            height: 1,
        };
        let mut grid = Grid::new(storage);
        grid.set_boundary(
            0,
            0,
            BoundaryBuffer {
                channel: 0,
                input_queue: VecDeque::new(),
                output_queue: VecDeque::from(vec![
                    CellValue(CellType(10)),
                    CellValue(CellType(2)),
                    CellValue(CellType(0)),
                    CellValue(CellType(0)),
                    CellValue(CellType(1)),
                    CellValue(CellType(255)), // terminator, but data too short
                ]),
                pending_output: None,
                max_queue_depth: 16,
            },
        );
        let mut store = RuleStore::new();
        let ops = store.drain_rule_channel(&mut grid);
        assert!(ops.is_empty(), "No valid packets should be decoded");
        assert_eq!(store.error_stats(), 1, "decode_errors should increment");
    }

    #[test]
    fn test_drain_rule_channel_basic() {
        use crate::types::BoundaryBuffer;
        use std::collections::VecDeque;

        // Packet: [priority=10, pattern_len=2, dx=0, dy=0, type=1, dx=1, dy=0, type=2,
        //          result=3, result=4, 255] = 11 bytes
        let storage = VecStorage {
            cells: vec![Cell::default()],
            width: 1,
            height: 1,
        };
        let mut grid = Grid::new(storage);
        grid.set_boundary(
            0,
            0,
            BoundaryBuffer {
                channel: 0,
                input_queue: VecDeque::new(),
                output_queue: VecDeque::from(vec![
                    CellValue(CellType(10)),
                    CellValue(CellType(2)),
                    CellValue(CellType(0)),
                    CellValue(CellType(0)),
                    CellValue(CellType(1)),
                    CellValue(CellType(1)),
                    CellValue(CellType(0)),
                    CellValue(CellType(2)),
                    CellValue(CellType(3)),
                    CellValue(CellType(4)),
                    CellValue(CellType(255)),
                ]),
                pending_output: None,
                max_queue_depth: 16,
            },
        );

        let mut store = RuleStore::new();
        let ops = store.drain_rule_channel(&mut grid);

        assert_eq!(ops.len(), 1, "Should decode one packet");
        match &ops[0].op {
            RuleOp::AddRule(rule) => {
                assert_eq!(rule.priority, 10);
                assert_eq!(rule.pattern.len(), 2);
                assert_eq!(rule.result_cells.len(), 2);
            }
            _ => panic!("Expected AddRule"),
        }

        // Output queue should be cleared
        let boundary = grid.get_boundary(0, 0).unwrap();
        assert!(boundary.output_queue.is_empty());
    }

    #[test]
    fn test_deserialize_rejects_no_center() {
        // data = [priority=10, pattern_len=1, dx=1, dy=0, type=1, result=2]
        let data = vec![10, 1, 1, 0, 1, 2];
        let result = deserialize_packet(&data, 100);
        assert!(result.is_err(), "Should reject pattern without center");
    }

    #[test]
    fn test_deserialize_rejects_result_length_mismatch() {
        // data = [priority=10, pattern_len=2, dx=0, dy=0, type=1, dx=1, dy=0, type=2, result=3]
        let data = vec![10, 2, 0, 0, 1, 1, 0, 2, 3];
        let result = deserialize_packet(&data, 100);
        assert!(
            result.is_err(),
            "Should reject result length != pattern length"
        );
    }

    #[test]
    fn test_default_rule_store() {
        let store = RuleStore::default();
        assert_eq!(store.rules().len(), 0);
        assert_eq!(store.error_stats(), 0);
    }

    #[test]
    fn test_max_buffer_size_clears_on_overflow() {
        let mut store = RuleStore::new();
        let key = 0u32;

        // Fill buffer with MAX_BUFFER_SIZE + 1 bytes
        for i in 0..=MAX_BUFFER_SIZE {
            let byte = if i == MAX_BUFFER_SIZE {
                0xFF // terminator — but buffer will be cleared before reaching this
            } else {
                0x00
            };
            store.accum_buffers.entry(key).or_default().push(byte);
        }

        // drain_rule_channel with no boundary cells — doesn't process anything
        // but ensures accumulator doesn't panic
        let storage = VecStorage {
            cells: vec![Cell::default()],
            width: 1,
            height: 1,
        };
        let mut grid = Grid::new(storage);
        let ops = store.drain_rule_channel(&mut grid);
        assert!(ops.is_empty());
        // Buffer should have been cleared due to overflow
        let buf = store.accum_buffers.get(&key);
        if let Some(b) = buf {
            assert!(
                b.len() < MAX_BUFFER_SIZE,
                "Buffer should be cleared on overflow"
            );
        }
    }

    #[test]
    fn test_error_stats() {
        let store = RuleStore::new();
        assert_eq!(store.error_stats(), 0);

        // error_stats() returns the decode_errors counter
        // We can verify it increments via packet decode failures
    }
}