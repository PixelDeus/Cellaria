use crate::grid::Grid;
use crate::storage::GridStorage;
use crate::types::{CellType, Rule, RuleId};
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
const AUTO_RULE_ID_BASE: u8 = 200;

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
    next_id: u8,
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
            next_id: AUTO_RULE_ID_BASE,
            decode_errors: 0,
        }
    }

    /// Создать RuleStore с начальным набором правил.
    pub fn with_rules(rules: Vec<Rule>) -> Self {
        Self {
            rules,
            dirty: true,
            accum_buffers: HashMap::new(),
            index: None,
            next_id: AUTO_RULE_ID_BASE,
            decode_errors: 0,
        }
    }

    /// Количество ошибок декодирования с момента создания.
    pub fn error_stats(&self) -> u64 {
        self.decode_errors
    }

    /// Прочитать выходные граничных ячеек и вернуть все завершённые операции
    /// (пакеты, где встречен терминатор 255).
    ///
    /// Вызывается после `run_tick` (когда `flush_output` уже перенёс данные).
    /// Дренирует только канал 0 (rule-канал), не затрагивая другие каналы.
    pub fn drain_rule_channel<S: GridStorage>(&mut self, grid: &mut Grid<S>) -> Vec<CompletedOp> {
        // Собираем все значения из канала 0 граничных буферов c direction == "output"
        let mut drained: Vec<u8> = Vec::new();
        for (_coord, boundary) in grid.iter_boundaries() {
            if boundary.direction == "output" {
                if let Some(queue) = boundary.queues.get(&0) {
                    for cell in queue {
                        drained.push(cell.value.0 .0);
                    }
                }
            }
        }

        // Очищаем только очередь канала 0 в output-буферах
        for (_, boundary) in grid.iter_boundaries_mut() {
            if boundary.direction == "output" {
                boundary.queues.remove(&0);
            }
        }

        // Накопляем байты в буфер
        let buf = self.accum_buffers.entry(0).or_default();
        if buf.len() >= MAX_BUFFER_SIZE {
            buf.clear();
            self.decode_errors += 1;
        }
        buf.extend(drained);

        // Извлекаем завершённые пакеты
        let mut completed = Vec::new();
        while let Some(end) = find_terminator(buf) {
            let packet: Vec<u8> = buf.drain(..=end).collect();
            let data = &packet[..packet.len() - 1];
            match deserialize_packet(data, self.next_id) {
                Ok(op) => {
                    if let RuleOp::AddRule(_) = &op {
                        self.next_id = self.next_id.wrapping_add(1);
                    }
                    completed.push(CompletedOp { op });
                }
                Err(e) => {
                    eprintln!("RuleStore: invalid packet: {}", e);
                    self.decode_errors += 1;
                    buf.clear();
                    break;
                }
            }
        }

        completed
    }

    /// Применить операцию к набору правил.
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
    pub fn get_index(&mut self) -> &HashMap<CellType, Vec<Rule>> {
        if self.dirty || self.index.is_none() {
            let mut index: HashMap<CellType, Vec<Rule>> = HashMap::new();
            for rule in &self.rules {
                if let Some(center) = rule.id.first() {
                    index.entry(*center).or_default().push(rule.clone());
                }
            }
            for rules in index.values_mut() {
                rules.sort_by_key(|b| std::cmp::Reverse(b.priority));
            }
            self.index = Some(index);
            self.dirty = false;
        }
        self.index
            .as_ref()
            .expect("get_index: index should be rebuilt after dirty set")
    }

    /// Текущий набор правил (для тестов).
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }
}

// === Deserialization ===

/// Найти индекс терминатора (255) в буфере.
fn find_terminator(buf: &[u8]) -> Option<usize> {
    buf.iter().position(|&b| b == TERMINATOR)
}

/// Десериализовать пакет (без терминатора) в RuleOp.
///
/// Формат пакета AddRule:
/// `[priority, id_len, (type_byte × id_len), 0xFE, dir_byte, steps, 255]`
fn deserialize_packet(data: &[u8], _next_id: u8) -> Result<RuleOp, String> {
    if data.is_empty() {
        return Err("empty packet".to_string());
    }

    let first = data[0];

    match first {
        OP_CLEAR => Ok(RuleOp::ClearAll),
        OP_REMOVE => {
            if data.len() < 2 {
                return Err(format!(
                    "RemoveRule packet too short: {} bytes",
                    data.len()
                ));
            }
            let rule_id = data[1];
            Ok(RuleOp::RemoveRule(vec![CellType(rule_id)]))
        }
        _ => {
            // AddRule: [priority, id_len, type_byte × id_len, SHIFT_FLAG?, dir_byte, steps, 255]
            let priority = first as u32;
            if data.len() < 2 {
                return Err("AddRule packet too short: no id_len".to_string());
            }
            let id_len = data[1] as usize;
            if id_len == 0 {
                return Err("AddRule: id_len must be > 0".to_string());
            }

            let type_start = 2;
            let type_end = type_start + id_len;
            if data.len() < type_end {
                return Err(format!(
                    "AddRule packet too short: need {} bytes for id, have {}",
                    type_end,
                    data.len()
                ));
            }

            let mut id = Vec::with_capacity(id_len);
            for &b in &data[type_start..type_end] {
                if b == 0xFF {
                    return Err(
                        "AddRule: type 255 (0xFF) in id is reserved for RuleStore protocol"
                            .to_string(),
                    );
                }
                id.push(CellType(b));
            }

            let mut offset = type_end;
            let mut shifts: Vec<Vec<crate::types::ShiftSpec>> = Vec::new();
            let mut changes: Vec<(i32, i32, crate::types::ChangeValue)> = Vec::new();

            // Парсим сдвиги (опционально)
            while offset < data.len() && data[offset] == SHIFT_FLAG {
                offset += 1;
                if offset + 2 > data.len() {
                    return Err("AddRule: not enough bytes for shift".to_string());
                }
                let dir_byte = data[offset];
                let steps = data[offset + 1] as u16;
                offset += 2;

                let direction = match dir_byte {
                    0 => crate::types::Direction::Up,
                    1 => crate::types::Direction::Down,
                    2 => crate::types::Direction::Left,
                    3 => crate::types::Direction::Right,
                    _ => {
                        return Err(format!("AddRule: invalid direction byte {}", dir_byte))
                    }
                };

                if shifts.is_empty() || !shifts.last().is_some_and(|last| last.is_empty()) {
                    shifts.push(Vec::new());
                }
                if let Some(last) = shifts.last_mut() {
                    last.push(crate::types::ShiftSpec { direction, steps });
                }
            }

            // Парсим изменения (оставшиеся байты до 255)
            while offset < data.len() {
                let b = data[offset];
                if b == 0xFF {
                    break;
                }
                if offset + 2 >= data.len() {
                    return Err("AddRule: not enough bytes for change".to_string());
                }
                // Простой формат: dx, dy, value — все байты
                // Protocol limitation: only ChangeValue::Literal is supported in packets.
                // ChangeValue::Ref is not encoded and will be rejected by the parser.
                let dx = data[offset] as i8 as i32;
                let dy = data[offset + 1] as i8 as i32;
                let value = data[offset + 2];
                changes.push((dx, dy, crate::types::ChangeValue::Literal(value)));
                offset += 3;
            }

            // Строим pattern из id (обратная совместимость)
            let pattern: Vec<(i8, i8, CellType)> = id.iter().enumerate()
                .map(|(i, &ct)| (i as i8, 0i8, ct))
                .collect();

            let rule = Rule {
                id,
                pattern,
                shifts,
                changes,
                active_only: false,
                priority,
                min_age: 0,
                overflow: Default::default(),
            };

            Ok(RuleOp::AddRule(rule))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Grid;
    use crate::storage::VecStorage;
    use crate::types::{BoundaryBuffer, Cell, CellType, CellValue};
    use std::collections::HashSet;

    fn make_grid_from_vec(width: usize, height: usize) -> Grid<VecStorage> {
        let storage = VecStorage {
            cells: vec![Cell::default(); width * height],
            width,
            height,
        };
        Grid::new(storage, HashSet::new())
    }

    #[test]
    fn test_deserialize_add_rule() {
        // AddRule: [priority=10, id_len=1, type=5, 255]
        let packet = vec![10, 1, 5, 255];
        let idx = find_terminator(&packet).unwrap();
        let data = &packet[..idx];
        let op = deserialize_packet(data, 100).unwrap();
        match op {
            RuleOp::AddRule(rule) => {
                assert_eq!(rule.id, vec![CellType(5)]);
                assert_eq!(rule.priority, 10);
            }
            _ => panic!("Expected AddRule"),
        }
    }

    #[test]
    fn test_deserialize_remove_rule() {
        // RemoveRule: [0xF0, rule_id=42, 255]
        let packet = vec![0xF0, 42, 255];
        let idx = find_terminator(&packet).unwrap();
        let data = &packet[..idx];
        let op = deserialize_packet(data, 0).unwrap();
        assert_eq!(op, RuleOp::RemoveRule(vec![CellType(42)]));
    }

    #[test]
    fn test_deserialize_clear_all() {
        let packet = vec![0xF1, 255];
        let idx = find_terminator(&packet).unwrap();
        let data = &packet[..idx];
        let op = deserialize_packet(data, 0).unwrap();
        assert_eq!(op, RuleOp::ClearAll);
    }

    fn make_rule(id: Vec<CellType>, priority: u32, changes: Vec<(i32, i32, crate::types::ChangeValue)>) -> Rule {
        Rule {
            id,
            pattern: vec![],
            shifts: vec![],
            changes,
            active_only: false,
            priority,
            min_age: 0,
            overflow: Default::default(),
        }
    }

    #[test]
    fn test_rule_store_apply_add() {
        let mut store = RuleStore::new();
        let rule = make_rule(vec![CellType(1)], 10, vec![(0, 0, crate::types::ChangeValue::Literal(2))]);
        assert!(store.apply(CompletedOp {
            op: RuleOp::AddRule(rule)
        }));
        assert_eq!(store.rules().len(), 1);
        assert!(store.dirty);
    }

    #[test]
    fn test_rule_store_apply_remove() {
        let rule = make_rule(vec![CellType(1)], 10, vec![(0, 0, crate::types::ChangeValue::Literal(2))]);
        let mut store = RuleStore::with_rules(vec![rule]);
        store.dirty = false;
        assert!(store.apply(CompletedOp {
            op: RuleOp::RemoveRule(vec![CellType(1)])
        }));
        assert_eq!(store.rules().len(), 0);
    }

    #[test]
    fn test_rule_store_apply_clear() {
        let rules = vec![
            make_rule(vec![CellType(1)], 10, vec![(0, 0, crate::types::ChangeValue::Literal(2))]),
            make_rule(vec![CellType(3)], 5, vec![(0, 0, crate::types::ChangeValue::Literal(4))]),
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
        let rule = make_rule(vec![CellType(5)], 10, vec![(0, 0, crate::types::ChangeValue::Literal(6))]);
        let mut store = RuleStore::with_rules(vec![rule]);
        store.get_index();

        let new_rule = make_rule(vec![CellType(7)], 5, vec![(0, 0, crate::types::ChangeValue::Literal(8))]);
        store.apply(CompletedOp {
            op: RuleOp::AddRule(new_rule),
        });
        assert!(store.dirty, "dirty should be set after apply");

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
    fn test_deserialize_rejects_255_in_id() {
        // data = [priority=10, id_len=1, type=255]
        let data = vec![10, 1, 0xFF];
        let result = deserialize_packet(&data, 100);
        assert!(result.is_err(), "Should reject 255 in id");
    }

    #[test]
    fn test_drain_rule_channel_basic() {
        let mut grid = make_grid_from_vec(1, 1);
        let mut bb = BoundaryBuffer::new();
        bb.direction = "output".to_string();
        grid.set_boundary(0, 0, bb);
        // Симулируем вывод в граничный буфер (через канал 0)
        if let Some(buf) = grid.get_boundary_mut(0, 0) {
            buf.enqueue(0, Cell {
                value: CellValue(CellType(10)),
                age: 0,
            });
            buf.enqueue(0, Cell {
                value: CellValue(CellType(1)),
                age: 0,
            });
            buf.enqueue(0, Cell {
                value: CellValue(CellType(5)),
                age: 0,
            });
            buf.enqueue(0, Cell {
                value: CellValue(CellType(255)),
                age: 0,
            });
        }

        let mut store = RuleStore::new();
        let ops = store.drain_rule_channel(&mut grid);

        assert_eq!(ops.len(), 1, "Should decode one packet");
        match &ops[0].op {
            RuleOp::AddRule(rule) => {
                assert_eq!(rule.priority, 10);
                assert_eq!(rule.id, vec![CellType(5)]);
            }
            _ => panic!("Expected AddRule"),
        }
    }

    #[test]
    fn test_decode_errors_increments_on_bad_packet() {
        let mut grid = make_grid_from_vec(1, 1);
        let mut bb = BoundaryBuffer::new();
        bb.direction = "output".to_string();
        grid.set_boundary(0, 0, bb);
        // Corrupted data: just 255 (empty packet = error)
        if let Some(buf) = grid.get_boundary_mut(0, 0) {
            buf.enqueue(0, Cell {
                value: CellValue(CellType(255)),
                age: 0,
            });
        }

        let mut store = RuleStore::new();
        let ops = store.drain_rule_channel(&mut grid);
        assert!(ops.is_empty(), "No valid packets should be decoded");
        assert_eq!(store.error_stats(), 1, "decode_errors should increment");
    }

    #[test]
    fn test_default_rule_store() {
        let store = RuleStore::default();
        assert_eq!(store.rules().len(), 0);
        assert_eq!(store.error_stats(), 0);
    }

    #[test]
    fn test_error_stats() {
        let store = RuleStore::new();
        assert_eq!(store.error_stats(), 0);
    }

    // ====================================================================
    // Интеграционный тест: RuleStore + Engine (самомодификация)
    // ====================================================================

    #[test]
    fn test_integration_self_modification() {
        use crate::types::ChangeValue;

        // 1) Создаём решётку 3×3 с VecStorage
        let mut grid = make_grid_from_vec(3, 3);

        // 2) Правило 1: id=[7], priority=10, changes=[(0,0,0)]
        let rule1 = Rule {
            id: vec![CellType(7)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(0))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
        };

        // 3) Правило 2: id=[9], priority=5,
        let _rule2 = Rule {
            id: vec![CellType(9)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(42))],
            active_only: false,
            priority: 5,
            min_age: 0,
            overflow: Default::default(),
        };

        // 4) RuleStore с правилом 1
        let mut store = RuleStore::with_rules(vec![rule1]);

        // 5) Симулируем внешний пакет: кладём rule2 в граничный буфер
        if grid.get_boundary(0, 0).is_none() {
            let mut bb = BoundaryBuffer::new();
            bb.direction = "output".to_string();
            grid.set_boundary(0, 0, bb);
        }
        if let Some(buf) = grid.get_boundary_mut(0, 0) {
            buf.enqueue(0, Cell { value: CellValue(CellType(5)), age: 0 });
            buf.enqueue(0, Cell { value: CellValue(CellType(1)), age: 0 });
            buf.enqueue(0, Cell { value: CellValue(CellType(9)), age: 0 });
            buf.enqueue(0, Cell { value: CellValue(CellType(255)), age: 0 });
        }

        // 6) Дренируем канал и применяем
        let ops = store.drain_rule_channel(&mut grid);
        assert_eq!(ops.len(), 1, "should decode one AddRule packet");
        for op in ops {
            store.apply(op);
        }

        // 7) Проверяем, что в store теперь два правила
        assert_eq!(
            store.rules().len(),
            2,
            "should have 2 rules after self-modification"
        );

        // 8) Проверяем, что индекс перестроился и включает новое правило
        let idx = store.get_index();
        assert!(
            idx.contains_key(&CellType(7)),
            "index should contain rule1"
        );
        assert!(
            idx.contains_key(&CellType(9)),
            "index should contain rule2 after self-modification"
        );
        assert_eq!(
            idx[&CellType(9)].len(),
            1,
            "index should have exactly 1 rule for CellType(9)"
        );
    }
}
