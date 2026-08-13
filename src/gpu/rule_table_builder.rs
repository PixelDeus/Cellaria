//! Единственная функция: `build_gpu_rule_table` — CPU `Rule` → `GpuRuleTable`.

use super::*;

/// Построить GPU-таблицу правил из того же `rule_index`, что использует
/// `Engine`/`detect_matches`. `Err`, если хотя бы одно правило выходит за
/// поддерживаемое подмножество — см. doc-комментарий модуля про то, почему
/// это ошибка для ВСЕГО конфига, а не молчаливый пропуск одного правила.
pub fn build_gpu_rule_table(rule_index: &HashMap<CellType, Vec<Rule>>) -> Result<GpuRuleTable, GpuUnsupportedReason> {
    let mut head_slots = [GpuHeadSlot {
        rules_start: 0,
        rules_count: 0,
        offsets_start: 0,
        offsets_count: 0,
    }; 256];
    let mut rules: Vec<GpuRule> = Vec::new();
    let mut pattern_offsets: Vec<GpuPatternOffset> = Vec::new();
    let mut head_offsets: Vec<GpuOffset> = Vec::new();
    let mut needs_arbitration = false;
    let mut needs_starvation = false;
    let mut needs_feedback = false;
    let mut needs_memory = false;
    let mut margin: i32 = 0;
    let mut pattern_reach: i32 = 0;

    // Порядок обхода `HashMap` не детерминирован, но это не важно: каждый
    // head пишет в СВОЙ слот `head_slots[head]`, слоты друг другу не мешают
    // независимо от порядка, а `rule_idx` внутри головы считается по позиции
    // в `rules` конкретного head (`enumerate()` ниже), не по общему счётчику.
    for (&head, group) in rule_index.iter() {
        let rules_start = rules.len() as u32;
        // Union офсетов ВСЕХ правил группы, в порядке первого появления —
        // как `matcher::build_group_data`'s `all_offsets`/`offset_set` (см.
        // doc-комментарий `GpuHeadSlot::offsets_start`). Дедуп нужен только
        // для компактности буфера — порядок и повторы на корректность
        // bounds-check не влияют, шейдер просто проверяет каждый офсет.
        let mut union_offsets: Vec<(i8, i8)> = Vec::new();
        let mut union_seen: FxHashSet<(i8, i8)> = FxHashSet::default();

        for (rule_idx, rule) in group.iter().enumerate() {
            if rule.max_activations.is_some() {
                return Err(GpuUnsupportedReason::MaxActivationsUnsupported { head: head.0, rule_idx });
            }
            if let Some(spec) = rule.recursion {
                if rule.cam.is_some() {
                    return Err(GpuUnsupportedReason::CamRecursionUnsupported { head: head.0, rule_idx });
                }
                if spec.max_depth > MAX_RECURSION_DEPTH {
                    return Err(GpuUnsupportedReason::RecursionDepthTooLarge {
                        head: head.0,
                        rule_idx,
                        max_depth: spec.max_depth,
                    });
                }
            }
            if let Some(spec) = &rule.memory {
                if rule.recursion.is_some() {
                    return Err(GpuUnsupportedReason::MemoryRecursionUnsupported { head: head.0, rule_idx });
                }
                if rule.cam.is_some() {
                    return Err(GpuUnsupportedReason::MemoryCamUnsupported { head: head.0, rule_idx });
                }
                if spec.window > MAX_MEMORY_WINDOW {
                    return Err(GpuUnsupportedReason::MemoryWindowTooLarge {
                        head: head.0,
                        rule_idx,
                        window: spec.window,
                    });
                }
            }
            // Флаг `broadcast`/`keep_source` КАЖДОГО сдвига, в том же
            // порядке/индексации, что `shift_deltas` — используется ниже и
            // при заполнении `GpuRule::shift_broadcast0/1`/`shift_keep_source0/1`.
            let shift_broadcasts: Vec<bool> = rule.shifts.iter().flatten().map(|s| s.broadcast).collect();
            let shift_keep_sources: Vec<bool> = rule.shifts.iter().flatten().map(|s| s.keep_source).collect();
            let shift_deltas: Vec<(i32, i32)> = rule.shifts.iter().flatten().map(shift_delta).collect();
            if shift_deltas.len() > MAX_SHIFTS {
                return Err(GpuUnsupportedReason::TooManyShifts {
                    head: head.0,
                    rule_idx,
                    len: shift_deltas.len(),
                });
            }
            if rule.feedback.is_some() {
                // `config::load_config` уже требует РОВНО один сдвиг для
                // `feedback` — здесь та же проверка защитно (см. её же
                // паттерн у `cam`'s `id_len == 1` выше): правило, пришедшее
                // мимо YAML-пути (напрямую через Rust API), не должно
                // тихо ломать `GpuRule::feedback_alt_dx/dy`'s единственное
                // предположение "ровно один сдвиг".
                if shift_deltas.len() != 1 {
                    return Err(GpuUnsupportedReason::TooManyShifts {
                        head: head.0,
                        rule_idx,
                        len: shift_deltas.len(),
                    });
                }
                if shift_broadcasts[0] {
                    return Err(GpuUnsupportedReason::FeedbackBroadcastUnsupported { head: head.0, rule_idx });
                }
                if shift_keep_sources[0] {
                    // Перенос счётчика (`update_feedback_relocate_pass`) не
                    // портирован для случая, когда источник НЕ освобождается
                    // — на CPU перенос вообще пропускается при `keep_source`
                    // (`applicator.rs`: "перенос — ТОЛЬКО когда источник
                    // реально освобождается"), GPU-версия такого условия не
                    // знает и релоцировала бы счётчик даже когда оригинал
                    // физически остался на месте. Отдельная, более узкая
                    // задача от `keep_age_mask` (тот отвечает только за
                    // возраст клетки, не за feedback-счётчик).
                    return Err(GpuUnsupportedReason::FeedbackKeepSourceUnsupported { head: head.0, rule_idx });
                }
                // Перенос счётчика (см. `FeedbackChangeCollidesWithShiftTarget`'s
                // doc-комментарий) предполагает, что тип клетки на новой
                // позиции останется `me.value` — ложно, если `changes`
                // ЗАПИСЫВАЕТ туда что-то другое (CPU-семантика "changes
                // побеждают shifts при конфликте клетки"). Проверяем ОБА
                // возможных эффективных смещения — декларированное и
                // альтернативное (`new_direction`), поскольку `feedback`
                // переключается между ними в рантайме одного и того же
                // правила, а эта проверка статическая (на этапе сборки
                // таблицы).
                if let Some(spec) = rule.feedback {
                    let alt = recursion_direction_delta(spec.new_direction);
                    if rule
                        .changes
                        .iter()
                        .any(|&(dx, dy, _)| (dx, dy) == shift_deltas[0] || (dx, dy) == alt)
                    {
                        return Err(GpuUnsupportedReason::FeedbackChangeCollidesWithShiftTarget {
                            head: head.0,
                            rule_idx,
                        });
                    }
                }
            }
            if rule.memory.is_some() {
                // `config::load_config` уже требует 0 или 1 сдвиг для
                // `memory` — та же защитная re-проверка, что и у `feedback`
                // выше, на случай конфига, собранного мимо YAML-пути.
                if shift_deltas.len() > 1 {
                    return Err(GpuUnsupportedReason::TooManyShifts {
                        head: head.0,
                        rule_idx,
                        len: shift_deltas.len(),
                    });
                }
                if shift_deltas.len() == 1 && shift_broadcasts[0] {
                    return Err(GpuUnsupportedReason::MemoryBroadcastUnsupported { head: head.0, rule_idx });
                }
                if shift_deltas.len() == 1 && shift_keep_sources[0] {
                    // Та же причина, что и `FeedbackKeepSourceUnsupported`
                    // выше — перенос буфера памяти на GPU не портирован для
                    // случая, когда источник не освобождается.
                    return Err(GpuUnsupportedReason::MemoryKeepSourceUnsupported { head: head.0, rule_idx });
                }
                // Та же причина, что и `FeedbackChangeCollidesWithShiftTarget`
                // выше — `memory` без "альтернативного" направления, только
                // одно возможное эффективное смещение.
                if shift_deltas.len() == 1 && rule.changes.iter().any(|&(dx, dy, _)| (dx, dy) == shift_deltas[0]) {
                    return Err(GpuUnsupportedReason::MemoryChangeCollidesWithShiftTarget { head: head.0, rule_idx });
                }
            }
            for &(dx, dy) in &shift_deltas {
                if dx.abs() > MAX_SHIFT_REACH || dy.abs() > MAX_SHIFT_REACH {
                    return Err(GpuUnsupportedReason::ShiftTooFar {
                        head: head.0,
                        rule_idx,
                        dx,
                        dy,
                    });
                }
            }
            // Broadcast — ПОДДЕРЖИВАЕТСЯ (см. `push_write_cell`-цикл в
            // `detect_pass`, который теперь пишет ВЕСЬ путь, не только
            // финальную точку), но `steps` ограничен отдельным, более узким
            // потолком [`MAX_BROADCAST_REACH`] — см. её doc-комментарий про
            // то, почему это НЕ то же самое, что [`MAX_SHIFT_REACH`] обычных
            // сдвигов (число ячеек записи растёт с `steps`, не константно).
            for (&(dx, dy), &is_broadcast) in shift_deltas.iter().zip(shift_broadcasts.iter()) {
                if is_broadcast {
                    let steps = dx.abs().max(dy.abs());
                    if steps > MAX_BROADCAST_REACH {
                        return Err(GpuUnsupportedReason::BroadcastPathTooLong {
                            head: head.0,
                            rule_idx,
                            steps,
                        });
                    }
                }
            }
            if rule.changes.len() > MAX_CHANGES {
                return Err(GpuUnsupportedReason::TooManyChanges {
                    head: head.0,
                    rule_idx,
                    len: rule.changes.len(),
                });
            }
            for &(dx, dy, _) in &rule.changes {
                if dx.abs() > MAX_CHANGE_REACH || dy.abs() > MAX_CHANGE_REACH {
                    return Err(GpuUnsupportedReason::ChangeTooFar {
                        head: head.0,
                        rule_idx,
                        dx,
                        dy,
                    });
                }
            }
            if shift_deltas.is_empty() && rule.changes.is_empty() && rule.cam.is_none() {
                return Err(GpuUnsupportedReason::NoEffect { head: head.0, rule_idx });
            }
            if let Some(cam) = rule.cam {
                if cam.radius > MAX_CAM_RADIUS {
                    return Err(GpuUnsupportedReason::CamRadiusTooFar {
                        head: head.0,
                        rule_idx,
                        radius: cam.radius,
                    });
                }
                // CAM всегда нуждается в арбитраже (две клетки-магнита могут
                // притянуть одну и ту же цель) — см. doc-комментарий
                // `needs_arbitration` ниже.
                needs_arbitration = true;
            }

            // Реальный охват ЭТОГО правила — та же формула, что и у
            // `MAX_MARGIN` (shift_reach + change_reach), но по фактическим
            // (а не предельно допустимым) значениям, см. doc-комментарий
            // `GpuRuleTable::margin`. `cam.radius` — тот же смысл: диск
            // радиуса R вокруг магнита — единственное, что правило реально
            // затрагивает (source-clear найденной клетки + сама позиция
            // магнита), консервативно ограничено радиусом с любой стороны.
            let shift_reach = shift_deltas
                .iter()
                .map(|&(dx, dy)| dx.abs().max(dy.abs()))
                .max()
                .unwrap_or(0);
            let change_reach = rule
                .changes
                .iter()
                .map(|&(dx, dy, _)| dx.abs().max(dy.abs()))
                .max()
                .unwrap_or(0);
            let cam_reach = rule.cam.map_or(0, |c| c.radius as i32);
            // Каскад `recursion`: самый дальний уровень `max_depth` сам
            // отстоит на `max_depth` клеток от исходного матча, а его
            // собственные `changes` расширяют охват ещё на `change_reach` —
            // та же логика, что у `shift_reach + change_reach` для обычных
            // сдвигов, только смещение даёт каскад, а не сам сдвиг.
            let recursion_reach = rule.recursion.map_or(0, |spec| spec.max_depth as i32) + change_reach;
            let rule_margin = if shift_deltas.is_empty() {
                change_reach
            } else {
                shift_reach + change_reach
            }
            .max(cam_reach)
            .max(recursion_reach);
            margin = margin.max(rule_margin);

            if !shift_deltas.is_empty()
                && matches!(
                    rule.overflow,
                    OverflowAction::Write(_) | OverflowAction::WriteLiteral(_)
                )
            {
                return Err(GpuUnsupportedReason::OverflowNotDiscard { head: head.0, rule_idx });
            }
            if !shift_deltas.is_empty() || rule.changes.iter().any(|&(dx, dy, _)| (dx, dy) != (0, 0)) {
                needs_arbitration = true;
            }
            // `recursion`: ДАЖЕ если объявленные `changes` целиком
            // self-referential ((0,0) относительно исходного матча —
            // единственный случай, не пойманный проверкой выше), каскад
            // `k = 1..=max_depth` пишет их же СМЕЩЁННЫМИ на `k × direction`
            // от исходной позиции — то есть в НЕ-self клетки, независимо от
            // того, что было объявлено. Без этой отдельной проверки
            // rule-с-только-self-changes-но-с-recursion молча получил бы
            // Simple (не арбитражный) пайплайн, хотя реально пишет вне
            // своей клетки, стоит max_depth > 0.
            if rule.recursion.is_some_and(|spec| spec.max_depth > 0) {
                needs_arbitration = true;
            }
            // `starvation_after`: осмысленно только под реальной
            // конкуренцией за клетку — форсируем Arbitrated-пайплайн
            // безусловно, даже если это единственная причина у ИНАЧЕ
            // self-write-only конфига (та же логика, что и у recursion
            // выше: свойство самого правила, не то, что можно вывести из
            // формы `changes`/`shifts`).
            if rule.starvation_after.is_some() {
                needs_arbitration = true;
                needs_starvation = true;
            }
            // `feedback` уже гарантированно требует ровно один сдвиг —
            // needs_arbitration уже станет true через обычную проверку
            // "есть сдвиг" ниже, форсировать здесь незачем; но
            // needs_feedback нужно отметить явно.
            if rule.feedback.is_some() {
                needs_feedback = true;
            }
            // `memory`: в отличие от `starvation_after`/`recursion` выше,
            // форсируем `needs_arbitration` БЕЗУСЛОВНО, даже для правила,
            // которое иначе (без `memory`) было бы чистым self-write —
            // персистентные буферы гейта существуют только в инфраструктуре
            // Arbitrated-пайплайна (см. `GpuRuleTable::needs_memory`'s
            // doc-комментарий), Simple-пайплайн их не заводит вовсе.
            if rule.memory.is_some() {
                needs_arbitration = true;
                needs_memory = true;
            }

            let mut change_fields = [(0i32, 0i32, 0u32); MAX_CHANGES];
            for (i, &(dx, dy, ref value)) in rule.changes.iter().enumerate() {
                let literal = match value {
                    ChangeValue::Literal(v) => *v as u32,
                    ChangeValue::Ref(_) => return Err(GpuUnsupportedReason::ChangeIsRef { head: head.0, rule_idx }),
                    ChangeValue::Add(..) | ChangeValue::Sub(..) => {
                        return Err(GpuUnsupportedReason::ChangeIsArithmetic { head: head.0, rule_idx })
                    }
                };
                change_fields[i] = (dx, dy, literal);
            }

            let pattern = effective_pattern(rule);
            if pattern.len() > MAX_PATTERN_OFFSETS {
                return Err(GpuUnsupportedReason::PatternTooLarge {
                    head: head.0,
                    rule_idx,
                    len: pattern.len(),
                });
            }
            if rule.id.len() > MAX_ID_BYTES {
                return Err(GpuUnsupportedReason::RuleIdTooLong {
                    head: head.0,
                    rule_idx,
                    len: rule.id.len(),
                });
            }

            for &(dx, dy, _) in &pattern {
                pattern_reach = pattern_reach.max((dx as i32).abs().max((dy as i32).abs()));
            }

            let pattern_start = pattern_offsets.len() as u32;
            for &(dx, dy, expected) in &pattern {
                pattern_offsets.push(GpuPatternOffset {
                    dx: dx as i32,
                    dy: dy as i32,
                    expected: expected.0 as u32,
                    _pad: 0,
                });
                if union_seen.insert((dx, dy)) {
                    union_offsets.push((dx, dy));
                }
            }

            let mut id_bytes = [0u32; MAX_ID_BYTES];
            for (i, ct) in rule.id.iter().enumerate() {
                id_bytes[i] = ct.0 as u32;
            }

            let mut shift_fields = [(0i32, 0i32); MAX_SHIFTS];
            for (i, &(dx, dy)) in shift_deltas.iter().enumerate() {
                shift_fields[i] = (dx, dy);
            }
            let mut shift_broadcast_fields = [false; MAX_SHIFTS];
            for (i, &b) in shift_broadcasts.iter().enumerate() {
                shift_broadcast_fields[i] = b;
            }
            let mut shift_keep_source_fields = [false; MAX_SHIFTS];
            for (i, &b) in shift_keep_sources.iter().enumerate() {
                shift_keep_source_fields[i] = b;
            }

            rules.push(GpuRule {
                pattern_start,
                pattern_len: pattern.len() as u32,
                priority: rule.priority,
                min_age: rule.min_age.min(u32::MAX as u64) as u32,
                active_only: rule.active_only as u32,
                id_len: rule.id.len() as u32,
                id_b0: id_bytes[0],
                id_b1: id_bytes[1],
                id_b2: id_bytes[2],
                id_b3: id_bytes[3],
                id_b4: id_bytes[4],
                id_b5: id_bytes[5],
                id_b6: id_bytes[6],
                id_b7: id_bytes[7],
                rule_idx: rule_idx as u32,
                shift_count: shift_deltas.len() as u32,
                shift_dx0: shift_fields[0].0,
                shift_dy0: shift_fields[0].1,
                shift_dx1: shift_fields[1].0,
                shift_dy1: shift_fields[1].1,
                shift_broadcast0: shift_broadcast_fields[0] as u32,
                shift_broadcast1: shift_broadcast_fields[1] as u32,
                shift_keep_source0: shift_keep_source_fields[0] as u32,
                shift_keep_source1: shift_keep_source_fields[1] as u32,
                change_count: rule.changes.len() as u32,
                change_dx0: change_fields[0].0,
                change_dy0: change_fields[0].1,
                change_val0: change_fields[0].2,
                change_dx1: change_fields[1].0,
                change_dy1: change_fields[1].1,
                change_val1: change_fields[1].2,
                change_dx2: change_fields[2].0,
                change_dy2: change_fields[2].1,
                change_val2: change_fields[2].2,
                change_dx3: change_fields[3].0,
                change_dy3: change_fields[3].1,
                change_val3: change_fields[3].2,
                cam_radius: rule.cam.map_or(0, |c| c.radius as u32),
                cam_target_type: rule.cam.map_or(0, |c| c.target_type.0 as u32),
                tie_break: rule.tie_break,
                recursion_max_depth: rule.recursion.map_or(0, |spec| spec.max_depth as u32),
                recursion_dx: rule
                    .recursion
                    .map_or(0, |spec| recursion_direction_delta(spec.direction).0),
                recursion_dy: rule
                    .recursion
                    .map_or(0, |spec| recursion_direction_delta(spec.direction).1),
                has_starvation: rule.starvation_after.is_some() as u32,
                starvation_threshold: rule.starvation_after.unwrap_or(0),
                has_feedback: rule.feedback.is_some() as u32,
                feedback_timeout: rule.feedback.map_or(0, |spec| spec.timeout.min(u32::MAX as u64) as u32),
                feedback_alt_dx: rule
                    .feedback
                    .map_or(0, |spec| recursion_direction_delta(spec.new_direction).0),
                feedback_alt_dy: rule
                    .feedback
                    .map_or(0, |spec| recursion_direction_delta(spec.new_direction).1),
                has_memory: rule.memory.is_some() as u32,
                memory_window: rule.memory.as_ref().map_or(0, |spec| spec.window as u32),
                memory_trigger: rule.memory.as_ref().map_or(0, |spec| match spec.record_trigger {
                    RecordTrigger::NeighborType(_) => 0,
                    RecordTrigger::RuleOutcome => 1,
                }),
                memory_dx: rule.memory.as_ref().map_or(0, |spec| match spec.record_trigger {
                    RecordTrigger::NeighborType(dir) => recursion_direction_delta(dir).0,
                    RecordTrigger::RuleOutcome => 0,
                }),
                memory_dy: rule.memory.as_ref().map_or(0, |spec| match spec.record_trigger {
                    RecordTrigger::NeighborType(dir) => recursion_direction_delta(dir).1,
                    RecordTrigger::RuleOutcome => 0,
                }),
                memory_has_shift: rule.memory.is_some() as u32 * (shift_deltas.len() == 1) as u32,
                memory_pattern0: rule
                    .memory
                    .as_ref()
                    .and_then(|spec| spec.match_pattern.first())
                    .map_or(0, |&v| encode_recorded_value(v)),
                memory_pattern1: rule
                    .memory
                    .as_ref()
                    .and_then(|spec| spec.match_pattern.get(1))
                    .map_or(0, |&v| encode_recorded_value(v)),
                memory_pattern2: rule
                    .memory
                    .as_ref()
                    .and_then(|spec| spec.match_pattern.get(2))
                    .map_or(0, |&v| encode_recorded_value(v)),
                memory_pattern3: rule
                    .memory
                    .as_ref()
                    .and_then(|spec| spec.match_pattern.get(3))
                    .map_or(0, |&v| encode_recorded_value(v)),
            });
        }

        let rules_count = rules.len() as u32 - rules_start;

        let offsets_start = head_offsets.len() as u32;
        for (dx, dy) in union_offsets {
            head_offsets.push(GpuOffset {
                dx: dx as i32,
                dy: dy as i32,
            });
        }
        let offsets_count = head_offsets.len() as u32 - offsets_start;

        head_slots[head.0 as usize] = GpuHeadSlot {
            rules_start,
            rules_count,
            offsets_start,
            offsets_count,
        };
    }

    let mut max_matches_per_cell: u32 = 0;
    if needs_arbitration {
        for (head, slot) in head_slots.iter().enumerate() {
            if slot.rules_count as usize > MAX_MATCHES_PER_CELL {
                return Err(GpuUnsupportedReason::TooManyRulesForArbitration {
                    head: head as u8,
                    len: slot.rules_count as usize,
                });
            }
            max_matches_per_cell = max_matches_per_cell.max(slot.rules_count);
        }
    }

    // `margin` не используется вовсе, если арбитраж не нужен (Simple-пайплайн
    // не трогает дополненную сетку) — обнуляем явно, а не оставляем
    // "правдоподобное, но неиспользуемое" значение.
    let margin = if needs_arbitration { margin } else { 0 };

    Ok(GpuRuleTable {
        head_slots,
        rules,
        pattern_offsets,
        head_offsets,
        needs_arbitration,
        margin,
        max_matches_per_cell,
        pattern_reach,
        needs_starvation,
        needs_feedback,
        needs_memory,
    })
}
