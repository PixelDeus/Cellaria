//! Энкодер протокола `RuleStore`: `serialize_add_rule`/`serialize_remove_rule`/
//! `serialize_clear_all`. Декодер — `rule_store_deserialize.rs`.

use super::*;

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
            return Err("serialize_add_rule: id byte 0xFF is reserved for RuleStore protocol".to_string());
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
            return Err("serialize_add_rule: rule with `cam` must not also have shifts".to_string());
        }
        if rule.id.len() != 1 {
            return Err("serialize_add_rule: rule with `cam` must have id.len() == 1".to_string());
        }
    }
    if let Some(spec) = rule.recursion {
        if !rule.shifts.is_empty() {
            return Err("serialize_add_rule: rule with `recursion` must not also have shifts".to_string());
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

    fn push_change(out: &mut Vec<u8>, is_first: bool, dx: i32, dy: i32, value: &ChangeValue) -> Result<(), String> {
        if !(i8::MIN as i32..=i8::MAX as i32).contains(&dx) || !(i8::MIN as i32..=i8::MAX as i32).contains(&dy) {
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
                let v = *v;
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
                let idx = *idx;
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
            // `Add`/`Sub` (арифметика над `ChangeValue`) -- вне
            // подмножества протокола, КАК И `feedback`/`memory`/
            // `keep_source` (см. doc-комментарий сериализатора), но
            // ДРУГАЯ категория отказа: те просто нигде не кодируются (весь
            // пакет собирается БЕЗ них, тихо, потому что для них в
            // принципе нет байта/бита нигде в формате). `changes`, в
            // отличие от них, УЖЕ активно кодируется поэлементно
            // (Literal/Ref) -- если бы `Add`/`Sub` просто МОЛЧА
            // пропускались здесь (как feedback/memory целиком), это
            // означало бы, что конкретная запись из `rule.changes`
            // тихо ИСЧЕЗЛА бы из пакета, а не "правило целиком не может
            // нести это расширение" -- получатель применял бы правило,
            // которое должно было писать вычисленное значение, вообще
            // ничего не записывая в эту клетку. Ошибка здесь, а не тихий
            // пропуск -- та же дисциплина, что и у остальных
            // `Err`-веток этой функции.
            ChangeValue::Add(..) | ChangeValue::Sub(..) => {
                return Err(
                    "serialize_add_rule: ChangeValue::Add/Sub are outside the wire protocol's supported subset -- \
                     there is no byte encoding for arithmetic changes (unlike Literal/Ref); a rule using them cannot \
                     be sent through the self-modification channel"
                        .to_string(),
                );
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
    for &(dx, dy, ref value) in &rule.changes {
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
