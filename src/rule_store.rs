use crate::grid::Grid;
use crate::storage::GridStorage;
use crate::types::{CellType, Rule, RuleId};
use std::collections::HashMap;

// === Protocol Constants ===

/// Терминатор пакета протокола RuleStore.
const TERMINATOR: u8 = 0xFF;

/// Маркер операции RemoveRule.
///
/// ИЗВЕСТНОЕ ОГРАНИЧЕНИЕ ПРОТОКОЛА: это же значение — первый байт пакета
/// AddRule (`priority`, см. `deserialize_packet`). Значит приоритет 240
/// (0xF0) физически невозможно закодировать в AddRule-пакете — первый байт
/// 0xF0 ВСЕГДА разбирается как RemoveRule, независимо от намерения
/// отправителя. Не исправлено: любой фикс требует смены формата пакета
/// (например, отдельного байта "тип операции" перед priority), что ломает
/// уже существующий, много где захардкоженный байт-в-байт формат
/// (`strength_self_modification*.rs` и другие примеры/тесты). Дешевле и
/// безопаснее держать priority вне диапазона 240..=241 (см. OP_CLEAR ниже),
/// чем переписывать формат протокола.
const OP_REMOVE: u8 = 0xF0;

/// Маркер операции ClearAll. Та же оговорка, что у `OP_REMOVE` — приоритет
/// 241 (0xF1) для AddRule тоже недостижим.
const OP_CLEAR: u8 = 0xF1;

/// Маркер флага shift в пакете AddRule.
const SHIFT_FLAG: u8 = 0xFE;

/// Максимальный размер буфера накопления для одного канала (в байтах).
const MAX_BUFFER_SIZE: usize = 1024;

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
    /// Накопленные буферы канала 0, ПО ОТДЕЛЬНОСТИ на каждый физический
    /// выходной буфер (координата → накопленные байты) — а не один общий
    /// буфер на всех сразу. Если бы буфер был один общий, а два независимых
    /// самомодифицирующихся региона слали бы каждый свою передачу через
    /// СВОЙ выходной порт одновременно, их байты перемешивались бы в одном
    /// потоке в порядке итерации `HashMap` (недетерминированном) — оба
    /// пакета ломались бы, даже если каждый по отдельности был бы устроен
    /// безупречно. Раздельные буферы на координату делают порты по-настоящему
    /// независимыми: то, что происходит на одном, никак не портит другой.
    accum_buffers: HashMap<(usize, usize), Vec<u8>>,
    /// Закешированный индекс (перестраивается только при dirty).
    index: Option<HashMap<CellType, Vec<Rule>>>,
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
        // Собираем значения из канала 0 КАЖДОГО output-буфера ОТДЕЛЬНО —
        // не в один общий поток (см. doc-комментарий `accum_buffers`).
        let mut per_boundary: Vec<((usize, usize), Vec<u8>)> = Vec::new();
        for (&coord, boundary) in grid.iter_boundaries() {
            if boundary.direction == "output" {
                if let Some(queue) = boundary.queues.get(&0) {
                    per_boundary.push((coord, queue.iter().map(|c| c.value.0 .0).collect()));
                }
            }
        }

        // Очищаем только очередь канала 0 в output-буферах
        for (_, boundary) in grid.iter_boundaries_mut() {
            if boundary.direction == "output" {
                boundary.queues.remove(&0);
            }
        }

        let mut completed = Vec::new();
        for (coord, drained) in per_boundary {
            let buf = self.accum_buffers.entry(coord).or_default();
            if buf.len() >= MAX_BUFFER_SIZE {
                buf.clear();
                self.decode_errors += 1;
            }
            buf.extend(drained);

            // Извлекаем завершённые пакеты ИЗ ЭТОГО буфера — не трогая
            // накопления других портов.
            while let Some(end) = find_terminator(buf) {
                let packet: Vec<u8> = buf.drain(..=end).collect();
                let data = &packet[..packet.len() - 1];
                match deserialize_packet(data) {
                    Ok(op) => {
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
///
/// Формат пакета RemoveRule: `[0xF0, id_len, (type_byte × id_len), 255]` —
/// тот же `id_len`-префикс, что и у AddRule. Раньше поддерживался только
/// однобайтовый id (`[0xF0, rule_id, 255]`), хотя `RuleStore::apply`
/// сравнивает на удаление ПОЛНЫЙ `rule.id` (может быть многоэлементным) —
/// правило с составным id в принципе нельзя было убрать через протокол.
fn deserialize_packet(data: &[u8]) -> Result<RuleOp, String> {
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
            let id_len = data[1] as usize;
            if id_len == 0 {
                return Err("RemoveRule: id_len must be > 0".to_string());
            }
            let id_start = 2;
            let id_end = id_start + id_len;
            if data.len() < id_end {
                return Err(format!(
                    "RemoveRule packet too short: need {} bytes for id, have {}",
                    id_end,
                    data.len()
                ));
            }
            let id: RuleId = data[id_start..id_end].iter().map(|&b| CellType(b)).collect();
            Ok(RuleOp::RemoveRule(id))
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
            // Позиции паттерна — i8 (см. `Rule::pattern` и весь матчер,
            // работающий со смещениями i8): `i as i8` ниже для i >= 128
            // молча заворачивается в отрицательное значение, давая
            // паттерну из >127 клеток мусорные (отрицательные) координаты
            // вместо ошибки. id_len — байт, теоретически до 255 — граница
            // протокола шире, чем реально может представить паттерн.
            if id_len > i8::MAX as usize {
                return Err(format!(
                    "AddRule: id_len {} exceeds i8 pattern offset range (max {})",
                    id_len,
                    i8::MAX
                ));
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

                // Группировка не влияет на применение (см. doc-комментарий
                // Rule::shifts) — каждый разобранный SHIFT_FLAG-триплет
                // просто становится своей независимой записью.
                // Протокол не кодирует `broadcast` (та же категория
                // ограничения, что и `cam`/`ChangeValue::Ref` — см. их
                // doc-комментарии выше в этом файле): переданные по каналу
                // правила никогда не используют broadcast-сдвиг.
                shifts.push(vec![crate::types::ShiftSpec { direction, steps, broadcast: false }]);
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
                // Протокол RuleStore не кодирует `cam` (та же категория
                // ограничения, что и `ChangeValue::Ref` — см. её
                // doc-комментарий выше в этом файле): переданные по каналу
                // правила никогда не используют CAM-поиск.
                cam: None,
                tie_break: 0,
                starvation_after: None,
            };

            Ok(RuleOp::AddRule(rule))
        }
    }
}

#[cfg(test)]
#[path = "rule_store_tests.rs"]
mod tests;
