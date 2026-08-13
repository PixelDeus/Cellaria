use crate::grid::Grid;
use crate::storage::GridStorage;
use crate::types::{CellType, Rule, RuleId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[path = "rule_store_deserialize.rs"]
mod rule_store_deserialize;
#[path = "rule_store_serialize.rs"]
mod rule_store_serialize;
pub use rule_store_serialize::{serialize_add_rule, serialize_clear_all, serialize_remove_rule};
use rule_store_deserialize::{deserialize_packet, find_terminator};

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
/// безопаснее держать priority вне диапазона 240..=242 (см. OP_CLEAR,
/// OP_ADD_EXT ниже), чем переписывать формат протокола.
///
/// Приоритет 255 (0xFF) НЕДОСТИЖИМ ВООБЩЕ, для ЛЮБОГО пакета (AddRule,
/// RemoveRule, ClearAll, AddRuleExtended) — по совсем другой причине, чем
/// 240..=242: `RuleStore::drain_rule_channel` ищет ПЕРВЫЙ байт 0xFF в сыром
/// накопленном потоке (`find_terminator`) ДО того, как пакет вообще
/// передаётся в `deserialize_packet`, и вырезает всё до него как "пакет".
/// Если priority (первый байт) сам равен 0xFF, поток-уровневый сканер
/// принимает ЕГО за терминатор — пакет обрубается до нулевой длины ДО того,
/// как у деструктора появляется шанс отличить "priority=255" от "пустой
/// пакет". Тот же эффект (обрезание раньше времени) грозит ЛЮБОМУ байту
/// полезной нагрузки, равному 0xFF — не только priority, но и байтам id,
/// шага сдвига, dx/dy/value изменения и т.д. (см. развёрнутую сводку у
/// `deserialize_packet`). Это НЕ новое ограничение — оно уже было раньше,
/// просто нигде не проговаривалось целиком, потому что в кодовой базе не
/// было сериализатора, который мог бы на него напороться; `serialize_add_rule`
/// теперь явно проверяет это и отказывает, а не молча собирает битый пакет.
const OP_REMOVE: u8 = 0xF0;

/// Маркер операции ClearAll. Та же оговорка, что у `OP_REMOVE` — приоритет
/// 241 (0xF1) для AddRule тоже недостижим.
const OP_CLEAR: u8 = 0xF1;

/// Маркер операции AddRuleExtended — см. `deserialize_packet`'s ветку
/// `OP_ADD_EXT`. Та же оговорка, что у `OP_REMOVE`/`OP_CLEAR`: приоритет 242
/// (0xF2) для ОБЫЧНОГО (не-extended) AddRule-пакета тоже становится
/// недостижим — первый байт 0xF2 ВСЕГДА разбирается как заголовок
/// AddRuleExtended.
const OP_ADD_EXT: u8 = 0xF2;

/// Маркер флага shift в пакете AddRule (обычный, БЕЗ `broadcast`).
const SHIFT_FLAG: u8 = 0xFE;

/// Маркер флага РАСШИРЕННОГО shift (quad вместо triplet: добавляет байт
/// флагов). Единственный используемый бит сейчас — бит0 = `broadcast`
/// (см. `types::ShiftSpec::broadcast`). Используется в ТОЙ ЖЕ позиции
/// разбора, что и `SHIFT_FLAG` (см. `parse_shift_section`) — работает
/// одинаково что в обычном AddRule-пакете, что в AddRuleExtended, никакого
/// отдельного op-кода не требуется: `broadcast` — это расширение формата
/// ОДНОГО сдвига, а не расширение всего правила (в отличие от `cam`, у
/// которого нет "своего" места в существующей раскладке байт вообще — см.
/// `OP_ADD_EXT`).
///
/// ИЗВЕСТНОЕ ОГРАНИЧЕНИЕ (то же семейство, что у `SHIFT_FLAG`): значение
/// 0xFD как ПЕРВЫЙ байт секции `changes` (сразу после id или после
/// предыдущего сдвига) теперь тоже всегда разбирается как начало
/// расширенного сдвига, а не как dx=-3 буквального изменения — то есть
/// `changes[0] == (-3, dy, Literal(_))` при пустом `shifts` (или сразу
/// после последнего сдвига) физически невозможно закодировать. См. полную
/// сводку недостижимых байтовых значений у `deserialize_packet`.
const SHIFT_EXT_FLAG: u8 = 0xFD;

/// Маркер флага РАСШИРЕННОГО change (quad вместо triplet: [`CHANGE_REF_FLAG`,
/// dx, dy, ref_index] вместо [dx, dy, value]) — кодирует
/// `types::ChangeValue::Ref(ref_index)` вместо `Literal`. В отличие от
/// `SHIFT_EXT_FLAG`, эта проверка происходит на КАЖДОЙ итерации цикла
/// разбора `changes` (не только на первой) — см. `parse_change_section`.
///
/// ИЗВЕСТНОЕ ОГРАНИЧЕНИЕ: значение 0xFC как dx ЛЮБОГО (не только первого)
/// буквального изменения теперь недостижимо — `(dx=-4, dy, Literal(_))`
/// нельзя закодировать НИ В ОДНОЙ позиции списка `changes`. Более широкий
/// охват, чем у `SHIFT_EXT_FLAG`, ровно потому, что `changes`-цикл (в
/// отличие от `shifts`-цикла) не завершается после первого несовпадения —
/// он смотрит на флаг на каждой итерации.
const CHANGE_REF_FLAG: u8 = 0xFC;

/// Максимальный размер буфера накопления для одного канала (в байтах).
const MAX_BUFFER_SIZE: usize = 1024;

// === Types ===

/// Операция, декодированная из пакета протокола RuleStore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleOp {
    AddRule(Rule),
    RemoveRule(RuleId),
    ClearAll,
}

/// Завершённая операция, готовая к применению.
#[derive(Debug, Clone)]
pub struct CompletedOp {
    pub op: RuleOp,
}

/// Хранилище правил с поддержкой самомодификации через канальный протокол.
#[derive(Clone, Serialize, Deserialize)]
pub struct RuleStore {
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
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            dirty: false,
            accum_buffers: HashMap::new(),
            index: None,
            decode_errors: 0,
        }
    }

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
                    Err(_) => {
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

#[cfg(test)]
#[path = "rule_store_tests.rs"]
mod tests;
