//! Декодер протокола `RuleStore`: `find_terminator`/`parse_shift_section`/
//! `parse_change_section`/`deserialize_packet`. Энкодер — `rule_store_serialize.rs`.

use super::*;

// === Deserialization ===

/// Найти индекс терминатора (255) в буфере.
pub(crate) fn find_terminator(buf: &[u8]) -> Option<usize> {
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
fn parse_shift_section(data: &[u8], mut offset: usize) -> Result<(Vec<Vec<crate::types::ShiftSpec>>, usize), String> {
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
        shifts.push(vec![ShiftSpec {
            direction,
            steps,
            broadcast,
            keep_source: false,
        }]);
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

fn parse_change_section(data: &[u8], mut offset: usize) -> Result<(ParsedChanges, usize), String> {
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
pub(crate) fn deserialize_packet(data: &[u8]) -> Result<RuleOp, String> {
    if data.is_empty() {
        return Err("empty packet".to_string());
    }

    let first = data[0];

    match first {
        OP_CLEAR => Ok(RuleOp::ClearAll),
        OP_REMOVE => {
            if data.len() < 2 {
                return Err(format!("RemoveRule packet too short: {} bytes", data.len()));
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
                        "AddRuleExtended: type 255 (0xFF) in id is reserved for RuleStore protocol".to_string(),
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
                    other => {
                        return Err(format!(
                            "AddRuleExtended: invalid recursion direction byte {other} (expected 0..=3)"
                        ))
                    }
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
                return Err("AddRuleExtended: rule with `cam` must not also have shifts".to_string());
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
                return Err("AddRuleExtended: rule with `recursion` must not also have shifts".to_string());
            }

            let pattern: Vec<(i8, i8, CellType)> = id.iter().enumerate().map(|(i, &ct)| (i as i8, 0i8, ct)).collect();

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
                max_activations: None,
                cross_layer_reads: Vec::new(),
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
                    return Err("AddRule: type 255 (0xFF) in id is reserved for RuleStore protocol".to_string());
                }
                id.push(CellType(b));
            }

            let offset = type_end;
            let (shifts, offset) = parse_shift_section(data, offset)?;
            let (changes, _offset) = parse_change_section(data, offset)?;

            // Строим pattern из id (обратная совместимость)
            let pattern: Vec<(i8, i8, CellType)> = id.iter().enumerate().map(|(i, &ct)| (i as i8, 0i8, ct)).collect();

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
                max_activations: None,
                cross_layer_reads: Vec::new(),
            };

            Ok(RuleOp::AddRule(rule))
        }
    }
}
