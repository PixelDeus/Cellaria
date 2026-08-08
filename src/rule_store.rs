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

/// Разобрать секцию сдвигов начиная с `offset`: ноль или больше триплетов
/// `[SHIFT_FLAG, dir_byte, steps]` (обычный сдвиг, `broadcast=false`) вперемешку
/// с квадами `[SHIFT_EXT_FLAG, dir_byte, steps, flags]` (`flags` бит0 =
/// `broadcast`) — обе формы читаются ОДНИМ циклом (см. doc-комментарии
/// `SHIFT_FLAG`/`SHIFT_EXT_FLAG`), используется и обычным AddRule-пакетом, и
/// AddRuleExtended (общий код, общее ограничение). Останавливается на первом
/// байте, который не является ни тем, ни другим флагом — дальше начинается
/// секция `changes` (см. `parse_change_section`).
fn parse_shift_section(
    data: &[u8],
    mut offset: usize,
) -> Result<(Vec<Vec<crate::types::ShiftSpec>>, usize), String> {
    use crate::types::{Direction, ShiftSpec};

    let mut shifts: Vec<Vec<ShiftSpec>> = Vec::new();
    while offset < data.len() && (data[offset] == SHIFT_FLAG || data[offset] == SHIFT_EXT_FLAG) {
        let extended = data[offset] == SHIFT_EXT_FLAG;
        offset += 1;
        let needed = if extended { 3 } else { 2 };
        if offset + needed > data.len() {
            return Err("AddRule: not enough bytes for shift".to_string());
        }
        let dir_byte = data[offset];
        let steps = data[offset + 1] as u16;

        let direction = match dir_byte {
            0 => Direction::Up,
            1 => Direction::Down,
            2 => Direction::Left,
            3 => Direction::Right,
            _ => return Err(format!("AddRule: invalid direction byte {}", dir_byte)),
        };

        // Группировка не влияет на применение (см. doc-комментарий
        // Rule::shifts) — каждый разобранный триплет/квад просто становится
        // своей независимой записью.
        let broadcast = extended && (data[offset + 2] & 0b0000_0001 != 0);
        offset += needed;

        // `keep_source` — не кодируется протоколом (та же категория, что
        // `Rule::feedback`/`recursion`/`memory` — см. их doc-комментарий у
        // Rule-конструкторов ниже): SHIFT_EXT_FLAG резервирует только 1 бит
        // флагов сейчас, `broadcast` его занимает, свободных бит для
        // keep_source там технически есть (biты 1-7), но использовать их
        // сейчас не нужно — вне текущего запроса на расширение протокола.
        shifts.push(vec![ShiftSpec { direction, steps, broadcast, keep_source: false }]);
    }
    Ok((shifts, offset))
}

/// Разобрать секцию изменений начиная с `offset`: ноль или больше триплетов
/// `[dx, dy, value]` (`ChangeValue::Literal`) вперемешку с квадами
/// `[CHANGE_REF_FLAG, dx, dy, ref_index]` (`ChangeValue::Ref`) — см.
/// doc-комментарий `CHANGE_REF_FLAG`. В отличие от `parse_shift_section`,
/// флаг проверяется на КАЖДОЙ итерации (не только на первой), потому что тут
/// нет отдельного "конца секции", кроме терминатора 255 самого пакета.
/// Общий код для обычного AddRule и AddRuleExtended.
type ParsedChanges = Vec<(i32, i32, crate::types::ChangeValue)>;

fn parse_change_section(
    data: &[u8],
    mut offset: usize,
) -> Result<(ParsedChanges, usize), String> {
    use crate::types::ChangeValue;

    let mut changes = Vec::new();
    while offset < data.len() {
        let b = data[offset];
        if b == 0xFF {
            break;
        }
        if b == CHANGE_REF_FLAG {
            if offset + 3 >= data.len() {
                return Err("AddRule: not enough bytes for extended (Ref) change".to_string());
            }
            let dx = data[offset + 1] as i8 as i32;
            let dy = data[offset + 2] as i8 as i32;
            let ref_index = data[offset + 3] as usize;
            changes.push((dx, dy, ChangeValue::Ref(ref_index)));
            offset += 4;
        } else {
            if offset + 2 >= data.len() {
                return Err("AddRule: not enough bytes for change".to_string());
            }
            let dx = data[offset] as i8 as i32;
            let dy = data[offset + 1] as i8 as i32;
            let value = data[offset + 2];
            changes.push((dx, dy, ChangeValue::Literal(value)));
            offset += 3;
        }
    }
    Ok((changes, offset))
}

/// Десериализовать пакет (без терминатора) в RuleOp.
///
/// Формат пакета AddRule (обычный):
/// `[priority, id_len, (type_byte × id_len), shift-секция?, change-секция?, 255]`
/// где shift-секция — ноль или больше `[0xFE, dir_byte, steps]` /
/// `[0xFD, dir_byte, steps, flags]` (см. `parse_shift_section`), а
/// change-секция — ноль или больше `[dx, dy, value]` /
/// `[0xFC, dx, dy, ref_index]` (см. `parse_change_section`).
///
/// Формат пакета RemoveRule: `[0xF0, id_len, (type_byte × id_len), 255]` —
/// тот же `id_len`-префикс, что и у AddRule. Раньше поддерживался только
/// однобайтовый id (`[0xF0, rule_id, 255]`), хотя `RuleStore::apply`
/// сравнивает на удаление ПОЛНЫЙ `rule.id` (может быть многоэлементным) —
/// правило с составным id в принципе нельзя было убрать через протокол.
///
/// Формат пакета AddRuleExtended: `[0xF2, priority, id_len, (type_byte ×
/// id_len), ext_flags, cam-байты?, recursion-байты?, shift-секция?,
/// change-секция?, 255]` — тот же shift-/change-формат, что у обычного
/// AddRule (общий код), плюс один дополнительный байт `ext_flags` сразу
/// после id (бит0 = `has_cam`, бит1 = `has_recursion`) и, для каждого
/// установленного бита, в ЭТОМ порядке (cam первым, затем recursion) — 2
/// байта `[radius, target_type]` для `types::CamSearch`, затем 2 байта
/// `[max_depth, direction_byte]` для `types::RecursionSpec` (direction:
/// 0=Up,1=Down,2=Left,3=Right, то же сопоставление, что и у обычных сдвигов
/// — см. `parse_shift_section`). Единственная причина, по которой `cam`/
/// `recursion` нужен ОТДЕЛЬНЫЙ op-код, а не ещё один флаг-байт внутри
/// shift-секции (как `broadcast`/`Ref`): оба — свойство ВСЕГО правила
/// (взаимоисключающее с `shifts`), а не одного сдвига/изменения, так что им
/// просто нет "своего" места в существующей раскладке до самого id.
///
/// ## Сводка байтовых значений, которые СТРУКТУРНО невозможно передать
/// (актуально для serialize_add_rule и любого будущего сериализатора):
///
/// - `priority == 255` (0xFF) — байт-уровневый терминатор
///   (`find_terminator`) обрежет пакет до этого байта РАНЬШЕ, чем
///   `deserialize_packet` его увидит (пустой пакет).
/// - `priority ∈ {240, 241, 242}` (0xF0/0xF1/0xF2) — зарезервированы под
///   `OP_REMOVE`/`OP_CLEAR`/`OP_ADD_EXT`.
/// - Любой байт id, значения shift/change полей (`steps`, `dx`, `dy`,
///   `value`, `ref_index`, `radius`, `target_type`) равный 255 (0xFF) — та
///   же причина, что и `priority == 255`: терминатор общий на весь пакет,
///   не только на первый байт.
/// - Первый байт `changes` СРАЗУ ПОСЛЕ id (при пустых `shifts`) ИЛИ сразу
///   после последнего сдвига, если он равен `dx = -2` (0xFE) или `dx = -3`
///   (0xFD) — совпадает с `SHIFT_FLAG`/`SHIFT_EXT_FLAG`, разбирается как
///   продолжение shift-секции, а не как начало change-секции.
/// - ЛЮБОЙ change с `dx = -4` (0xFC) — совпадает с `CHANGE_REF_FLAG` на
///   любой позиции списка `changes`, не только первой.
///
/// Ничего из этого не тихая порча данных: `deserialize_packet` либо
/// разбирает пакет иначе, чем предполагал отправитель (задокументированные
/// коллизии флагов выше), либо (для 0xFF-коллизий) сам байтовый поток
/// обрывается раньше на уровне `find_terminator`, а не внутри этой функции.
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
        OP_ADD_EXT => {
            // AddRuleExtended: [0xF2, priority, id_len, type_byte × id_len,
            // ext_flags, cam-байты?, shift-секция?, change-секция?, 255]
            if data.len() < 2 {
                return Err("AddRuleExtended packet too short: no priority".to_string());
            }
            let priority = data[1] as u32;
            if data.len() < 3 {
                return Err("AddRuleExtended packet too short: no id_len".to_string());
            }
            let id_len = data[2] as usize;
            if id_len == 0 {
                return Err("AddRuleExtended: id_len must be > 0".to_string());
            }
            if id_len > i8::MAX as usize {
                return Err(format!(
                    "AddRuleExtended: id_len {} exceeds i8 pattern offset range (max {})",
                    id_len,
                    i8::MAX
                ));
            }

            let type_start = 3;
            let type_end = type_start + id_len;
            if data.len() < type_end {
                return Err(format!(
                    "AddRuleExtended packet too short: need {} bytes for id, have {}",
                    type_end,
                    data.len()
                ));
            }

            let mut id = Vec::with_capacity(id_len);
            for &b in &data[type_start..type_end] {
                if b == 0xFF {
                    return Err(
                        "AddRuleExtended: type 255 (0xFF) in id is reserved for RuleStore protocol"
                            .to_string(),
                    );
                }
                id.push(CellType(b));
            }

            if type_end >= data.len() {
                return Err("AddRuleExtended packet too short: no ext_flags byte".to_string());
            }
            let ext_flags = data[type_end];
            let has_cam = ext_flags & 0b0000_0001 != 0;
            let has_recursion = ext_flags & 0b0000_0010 != 0;
            let mut offset = type_end + 1;

            let cam = if has_cam {
                if offset + 2 > data.len() {
                    return Err("AddRuleExtended: not enough bytes for cam".to_string());
                }
                let radius = data[offset];
                let target_type = CellType(data[offset + 1]);
                offset += 2;
                Some(crate::types::CamSearch { radius, target_type })
            } else {
                None
            };

            // `RecursionSpec` — [max_depth, direction_byte], сразу после
            // cam-байт (если есть) и ДО shift/change-секций — тот же
            // порядок, что и cam, просто следующий бит `ext_flags`. Байтовый
            // формат direction зеркалит `serialize_add_rule`'s локальный
            // `dir_byte` (0=Up,1=Down,2=Left,3=Right) — единственное место
            // в протоколе, кодирующее `Direction`, поэтому именно здесь и
            // держится каноническое сопоставление.
            let recursion = if has_recursion {
                if offset + 2 > data.len() {
                    return Err("AddRuleExtended: not enough bytes for recursion".to_string());
                }
                let max_depth = data[offset];
                if max_depth == TERMINATOR {
                    return Err(
                        "AddRuleExtended: recursion max_depth of 255 (0xFF) is reserved for the RuleStore protocol terminator".to_string(),
                    );
                }
                let direction = match data[offset + 1] {
                    0 => crate::types::Direction::Up,
                    1 => crate::types::Direction::Down,
                    2 => crate::types::Direction::Left,
                    3 => crate::types::Direction::Right,
                    other => return Err(format!("AddRuleExtended: invalid recursion direction byte {other} (expected 0..=3)")),
                };
                offset += 2;
                Some(crate::types::RecursionSpec { max_depth, direction })
            } else {
                None
            };

            let (shifts, offset) = parse_shift_section(data, offset)?;
            let (changes, _offset) = parse_change_section(data, offset)?;

            // Валидация: та же пара инвариантов, что `config::load_config`
            // применяет к правилам из YAML (см. её комментарии там) —
            // правило, пришедшее по каналу самомодификации, не должно уметь
            // обойти эти проверки просто потому, что оно не проходит через
            // `load_config`. `cam` — это единственный сдвиг правила, так что
            // явные `shifts` рядом с ним бессмысленны и запрещены; и
            // `id_len != 1` для cam-правила запрещён здесь (строже, чем
            // "нет явного pattern" в YAML-пути), потому что `pattern` в этом
            // протоколе ВСЕГДА строится из полного `id` — единственный
            // способ гарантировать "pattern — только голова" на этом пути
            // это потребовать однобайтовый id.
            if cam.is_some() && !shifts.is_empty() {
                return Err(
                    "AddRuleExtended: rule with `cam` must not also have shifts".to_string(),
                );
            }
            if cam.is_some() && id_len != 1 {
                return Err(
                    "AddRuleExtended: rule with `cam` must have id_len == 1 (cam's identity is just the head cell type — see types::CamSearch's doc-comment)"
                        .to_string(),
                );
            }
            // `recursion` + `shifts` — то же исключение, что `config.rs`
            // применяет к YAML-правилам (`RecursionSpec` расширяет `changes`
            // вдоль направления, а не двигает голову — см. её doc-комментарий
            // в `types.rs`). `cam`+`recursion` ВМЕСТЕ разрешены на этом пути
            // (как и на CPU, см. `applicator::apply_cam_buffered`) — здесь
            // никакой дополнительной проверки не нужно, `cam`'s собственная
            // `id_len == 1` проверка выше уже покрывает cam-специфичную часть.
            if recursion.is_some() && !shifts.is_empty() {
                return Err(
                    "AddRuleExtended: rule with `recursion` must not also have shifts".to_string(),
                );
            }

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
                cam,
                tie_break: 0,
                starvation_after: None,
                // `recursion` кодируется через `ext_flags` бит1 (см. парсинг
                // выше) — единственное из четырёх "структурно невыразимых"
                // расширений (см. следующий комментарий), которое
                // AddRuleExtended ТЕПЕРЬ умеет — см.
                // `test_add_rule_extended_recursion_roundtrip_via_serializer`.
                recursion,
                // `feedback`/`memory`/`keep_source` — та же категория
                // структурного ограничения, что и `ChangeValue::Ref` выше:
                // ни AddRule, ни AddRuleExtended не резервируют ни одного
                // байта под `FeedbackSpec`/`MemorySpec` (последний —
                // переменной длины) или под бит `keep_source` в shift-секции
                // — эти три остаются структурно невыразимыми на ОБОИХ путях
                // (см. `rule_store_tests.rs`'s
                // `test_add_rule_protocol_cannot_express_feedback_recursion_memory_or_keep_source`,
                // который бьёт по plain AddRule и остаётся верным для него
                // как был).
                feedback: None,
                memory: None,
            };

            Ok(RuleOp::AddRule(rule))
        }
        _ => {
            // AddRule: [priority, id_len, type_byte × id_len, shift-секция?, change-секция?, 255]
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

            let offset = type_end;
            let (shifts, offset) = parse_shift_section(data, offset)?;
            let (changes, _offset) = parse_change_section(data, offset)?;

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
                // Обычный (не-Extended) AddRule-пакет не кодирует `cam` — у
                // него просто нет байта под него (см. `OP_ADD_EXT` выше:
                // единственный путь для `cam` — AddRuleExtended). Переданные
                // через этот путь правила никогда не используют CAM-поиск.
                cam: None,
                tie_break: 0,
                starvation_after: None,
                // Обычный (не-Extended) AddRule ТАКЖЕ не кодирует `recursion`
                // (нет `ext_flags`-байта вовсе на этом пути) — только
                // AddRuleExtended умеет (см. её `recursion` выше).
                // `feedback`/`memory`/`keep_source` — та же категория
                // структурного ограничения, что и `ChangeValue::Ref` выше: ни
                // AddRule, ни AddRuleExtended не резервируют ни одного байта
                // под `FeedbackSpec`/`MemorySpec` (последний — переменной
                // длины) или под бит `keep_source` в shift-секции. Правило,
                // переданное через ЭТОТ (не-Extended) путь, СТРУКТУРНО не
                // может запросить ни одну из этих четырёх возможностей — не
                // тихая порча присланных данных, а физическая невозможность
                // закодировать намерение в этом формате (см.
                // `rule_store_tests.rs`'s
                // `test_add_rule_protocol_cannot_express_feedback_recursion_memory_or_keep_source`).
                feedback: None,
                recursion: None,
                memory: None,
            };

            Ok(RuleOp::AddRule(rule))
        }
    }
}

// === Serialization ===
//
// Раньше в кодовой базе НЕ БЫЛО сериализатора вообще — байты пакетов везде
// собирались вручную (`examples/strength_self_modification*.rs`,
// `rule_store_tests.rs`). `serialize_add_rule` — первый настоящий
// энкодер, взаимно-обратный `deserialize_packet`: пригождается и для
// round-trip тестов (encode → decode → сравнить с исходным `Rule`), и как
// готовый к использованию инструмент для любого будущего кода, которому
// нужно СФОРМИРОВАТЬ пакет, а не только его разобрать (например,
// генератор клеток-перевозчиков для новых демо самомодификации).

/// Сериализовать `RuleOp::AddRule(rule)` в байты пакета (терминатор 255
/// включён, готов к постановке в очередь выходного граничного буфера).
///
/// Автоматически выбирает МИНИМАЛЬНЫЙ формат: обычный AddRule, если правило
/// укладывается в него (нет `cam`/`recursion`, priority не коллизирует ни с
/// одним op-кодом), иначе AddRuleExtended (см. `OP_ADD_EXT`). `broadcast` и
/// `ChangeValue::Ref` кодируются ОДИНАКОВО в обоих форматах (общие
/// `SHIFT_EXT_FLAG`/`CHANGE_REF_FLAG` — см. `parse_shift_section`/
/// `parse_change_section`), поэтому сами по себе они НЕ требуют
/// AddRuleExtended — `cam` и `recursion` требуют, потому что для них в
/// принципе нет места в обычной раскладке (см. `deserialize_packet`'s
/// doc-комментарий у `OP_ADD_EXT`).
///
/// Возвращает `Err`, если правило структурно невозможно передать этим
/// протоколом — см. полную сводку недостижимых байтовых значений в
/// doc-комментарии `deserialize_packet`. Это НЕ баг сериализатора: он не
/// молча собирает битый пакет, а честно отказывается его строить.
pub fn serialize_add_rule(rule: &Rule) -> Result<Vec<u8>, String> {
    use crate::types::{ChangeValue, Direction, ShiftSpec};

    if rule.id.is_empty() {
        return Err("serialize_add_rule: id must not be empty".to_string());
    }
    if rule.id.len() > i8::MAX as usize {
        return Err(format!(
            "serialize_add_rule: id length {} exceeds i8 pattern offset range (max {})",
            rule.id.len(),
            i8::MAX
        ));
    }
    for ct in &rule.id {
        if ct.0 == 0xFF {
            return Err(
                "serialize_add_rule: id byte 0xFF is reserved for RuleStore protocol"
                    .to_string(),
            );
        }
    }
    if rule.priority > u8::MAX as u32 {
        return Err(format!(
            "serialize_add_rule: priority {} exceeds the protocol's u8 range (0..=255)",
            rule.priority
        ));
    }
    let priority = rule.priority as u8;
    if priority == TERMINATOR {
        return Err(
            "serialize_add_rule: priority 255 (0xFF) is unreachable -- the stream-level terminator scan (find_terminator) would consume it before deserialize_packet ever runs"
                .to_string(),
        );
    }

    if rule.cam.is_some() {
        if !rule.shifts.is_empty() {
            return Err(
                "serialize_add_rule: rule with `cam` must not also have shifts".to_string(),
            );
        }
        if rule.id.len() != 1 {
            return Err(
                "serialize_add_rule: rule with `cam` must have id.len() == 1".to_string(),
            );
        }
    }
    if let Some(spec) = rule.recursion {
        if !rule.shifts.is_empty() {
            return Err(
                "serialize_add_rule: rule with `recursion` must not also have shifts".to_string(),
            );
        }
        if spec.max_depth == TERMINATOR {
            return Err(
                "serialize_add_rule: recursion max_depth of 255 (0xFF) is unreachable -- would be consumed by the stream-level terminator scan"
                    .to_string(),
            );
        }
    }

    fn dir_byte(d: Direction) -> u8 {
        match d {
            Direction::Up => 0,
            Direction::Down => 1,
            Direction::Left => 2,
            Direction::Right => 3,
        }
    }

    fn push_shift(out: &mut Vec<u8>, s: &ShiftSpec) -> Result<(), String> {
        if s.steps > u8::MAX as u16 {
            return Err(format!(
                "serialize_add_rule: shift steps {} exceeds the protocol's u8 range (0..=255)",
                s.steps
            ));
        }
        let steps = s.steps as u8;
        if steps == TERMINATOR {
            return Err(
                "serialize_add_rule: shift steps 255 (0xFF) is unreachable -- would be consumed by the stream-level terminator scan"
                    .to_string(),
            );
        }
        if s.broadcast {
            out.push(SHIFT_EXT_FLAG);
            out.push(dir_byte(s.direction));
            out.push(steps);
            out.push(0b0000_0001);
        } else {
            out.push(SHIFT_FLAG);
            out.push(dir_byte(s.direction));
            out.push(steps);
        }
        Ok(())
    }

    fn push_change(out: &mut Vec<u8>, is_first: bool, dx: i32, dy: i32, value: ChangeValue) -> Result<(), String> {
        if !(i8::MIN as i32..=i8::MAX as i32).contains(&dx)
            || !(i8::MIN as i32..=i8::MAX as i32).contains(&dy)
        {
            return Err(format!(
                "serialize_add_rule: change offset ({dx}, {dy}) exceeds the protocol's i8 range (-128..=127)"
            ));
        }
        let dxb = dx as i8 as u8;
        let dyb = dy as i8 as u8;
        if dxb == TERMINATOR || dyb == TERMINATOR {
            return Err(
                "serialize_add_rule: change dx/dy of -1 (0xFF byte) is unreachable -- would be consumed by the stream-level terminator scan"
                    .to_string(),
            );
        }
        match value {
            ChangeValue::Literal(v) => {
                if v == TERMINATOR {
                    return Err(
                        "serialize_add_rule: change value 255 (0xFF) is unreachable -- would be consumed by the stream-level terminator scan"
                            .to_string(),
                    );
                }
                if dxb == CHANGE_REF_FLAG {
                    return Err(
                        "serialize_add_rule: change dx of -4 (0xFC byte) collides with CHANGE_REF_FLAG and cannot be encoded as a Literal change".to_string(),
                    );
                }
                if is_first && (dxb == SHIFT_FLAG || dxb == SHIFT_EXT_FLAG) {
                    return Err(
                        "serialize_add_rule: the first change's dx of -2/-3 (0xFE/0xFD byte) collides with SHIFT_FLAG/SHIFT_EXT_FLAG when it immediately follows the id or the last shift".to_string(),
                    );
                }
                out.push(dxb);
                out.push(dyb);
                out.push(v);
            }
            ChangeValue::Ref(idx) => {
                if idx > u8::MAX as usize {
                    return Err(format!(
                        "serialize_add_rule: ChangeValue::Ref({idx}) exceeds the protocol's u8 range (0..=255)"
                    ));
                }
                let idxb = idx as u8;
                if idxb == TERMINATOR {
                    return Err(
                        "serialize_add_rule: ChangeValue::Ref(255) is unreachable -- would be consumed by the stream-level terminator scan"
                            .to_string(),
                    );
                }
                out.push(CHANGE_REF_FLAG);
                out.push(dxb);
                out.push(dyb);
                out.push(idxb);
            }
        }
        Ok(())
    }

    let mut body = Vec::new();
    for group in &rule.shifts {
        for s in group {
            push_shift(&mut body, s)?;
        }
    }
    let mut first_change = true;
    for &(dx, dy, value) in &rule.changes {
        push_change(&mut body, first_change, dx, dy, value)?;
        first_change = false;
    }

    let needs_ext = rule.cam.is_some()
        || rule.recursion.is_some()
        || priority == OP_REMOVE
        || priority == OP_CLEAR
        || priority == OP_ADD_EXT;

    let mut packet = Vec::new();
    if needs_ext {
        packet.push(OP_ADD_EXT);
        packet.push(priority);
        packet.push(rule.id.len() as u8);
        for ct in &rule.id {
            packet.push(ct.0);
        }
        let has_cam = rule.cam.is_some();
        let has_recursion = rule.recursion.is_some();
        packet.push((if has_cam { 0b0000_0001 } else { 0 }) | (if has_recursion { 0b0000_0010 } else { 0 }));
        if let Some(cam) = rule.cam {
            if cam.radius == TERMINATOR || cam.target_type.0 == TERMINATOR {
                return Err(
                    "serialize_add_rule: cam radius/target_type of 255 (0xFF) is unreachable -- would be consumed by the stream-level terminator scan"
                        .to_string(),
                );
            }
            packet.push(cam.radius);
            packet.push(cam.target_type.0);
        }
        if let Some(spec) = rule.recursion {
            // max_depth==TERMINATOR уже отклонён валидацией выше.
            packet.push(spec.max_depth);
            packet.push(dir_byte(spec.direction));
        }
    } else {
        packet.push(priority);
        packet.push(rule.id.len() as u8);
        for ct in &rule.id {
            packet.push(ct.0);
        }
    }
    packet.extend(body);
    packet.push(TERMINATOR);
    Ok(packet)
}

/// Сериализовать `RuleOp::RemoveRule(id)` в байты пакета. Мелкий помощник
/// для симметрии с `serialize_add_rule` и удобства тестов/примеров — формат
/// не менялся в этой сессии, просто раньше не было готовой функции.
pub fn serialize_remove_rule(id: &RuleId) -> Result<Vec<u8>, String> {
    if id.is_empty() {
        return Err("serialize_remove_rule: id must not be empty".to_string());
    }
    if id.len() > u8::MAX as usize {
        return Err(format!(
            "serialize_remove_rule: id length {} exceeds the protocol's u8 range (0..=255)",
            id.len()
        ));
    }
    let mut packet = vec![OP_REMOVE, id.len() as u8];
    for ct in id {
        if ct.0 == TERMINATOR {
            return Err(
                "serialize_remove_rule: id byte 0xFF is unreachable -- would be consumed by the stream-level terminator scan"
                    .to_string(),
            );
        }
        packet.push(ct.0);
    }
    packet.push(TERMINATOR);
    Ok(packet)
}

/// Сериализовать `RuleOp::ClearAll` в байты пакета.
pub fn serialize_clear_all() -> Vec<u8> {
    vec![OP_CLEAR, TERMINATOR]
}

#[cfg(test)]
#[path = "rule_store_tests.rs"]
mod tests;
