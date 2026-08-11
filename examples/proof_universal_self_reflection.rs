//! Универсальная саморефлексия: ОДИН И ТОТ ЖЕ, построенный один раз, набор
//! из 255 правил-ретрансляторов способен передать пакет протокола `AddRule`
//! для ЛЮБОГО правила R — под новое R не добавляется ни одного нового
//! ПРАВИЛА, меняются только ДАННЫЕ (расстановка клеток-носителей байт).
//!
//! Раньше (`strength_self_modification.rs`, `strength_self_modification_computed.rs`)
//! под каждое конкретное R собирался свой набор клеток-перевозчиков под его
//! конкретные байты. Здесь — фиксированная "универсальная машина": по одному
//! правилу-ретранслятору на каждое возможное байтовое значение (кроме
//! терминатора протокола, который всегда один и тот же байт 255 независимо
//! от R — это не данные, а рамка протокола, как стоп-бит в модеме).
//!
//! Значение 0 требует отдельного механизма: клетка со значением 0 в этом
//! движке — это "пустая" клетка (см. `Cell::is_default`), её нельзя нести
//! как обычные данные через `OverflowAction::Write(0)` (там 0 зарезервирован
//! под другой смысл — "пронеси своё значение"). Поэтому в этой сессии в
//! движок добавлен `OverflowAction::WriteLiteral` — вариант, где параметр
//! всегда буквален, включая 0 (см. `src/types.rs`, `src/engine/applicator.rs`).
//! Без этого добавления универсальная машина не могла бы передать НИ ОДНОГО
//! правила с сдвигом Up (dir_byte=0) или изменением той же клетки (dx=dy=0) —
//! то есть подавляющее большинство реальных правил.
//!
//! Проверка: R выбрано намеренно НЕ похожим ни на одно из прошлых демо —
//! сдвиг Up (единственное направление, кодируемое нулём) плюс изменение той
//! же клетки (dx=0, dy=0) — то есть специально бьёт по обоим "нулевым" местам
//! протокола. Машина строится ОДИН раз, до того как R вообще выбрано.

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::rule_store::{RuleOp, RuleStore};
use cellaria::types::{BoundaryBuffer, Cell, CellType, CellValue, Direction, OverflowAction, Rule, ShiftSpec};
use cellaria::{Grid, VecStorage};

const WIDTH: usize = 200;

/// Строим универсальную машину: 254 ретранслятора "неси своё значение"
/// (типы 1..=254, покрывают байты 1..=254) плюс один ретранслятор нуля
/// (тип 255, единственный, кому нужен `WriteLiteral`, а не `Write`).
/// Ничего в этом наборе не знает и не может знать о том, какое R когда-либо
/// попытаются передать — набор один и тот же для абсолютно любого R.
fn build_universal_machine() -> HashMap<CellType, Vec<Rule>> {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for v in 1u16..=254 {
        let t = CellType(v as u8);
        idx.insert(t, vec![Rule {
            id: vec![t], pattern: vec![], shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
            changes: vec![], active_only: false, priority: 10, min_age: 0,
            overflow: OverflowAction::Write(0), // неси своё значение как есть
            cam: None,
            tie_break: 0,
            starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None, cross_layer_reads: Vec::new(),
        }]);
    }
    idx.insert(CellType(255), vec![Rule {
        id: vec![CellType(255)], pattern: vec![], shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
        changes: vec![], active_only: false, priority: 10, min_age: 0,
        overflow: OverflowAction::WriteLiteral(0), // единственный, кому нужен буквальный 0
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None, cross_layer_reads: Vec::new(),
    }]);
    idx
}

/// Кодируем DATA-часть пакета (без терминатора — он рамка протокола, не
/// данные R) как последовательность клеток-носителей: byte b (1..=254) —
/// клетка типа b; byte 0 — клетка типа 255 (зарезервирован под ноль).
fn place_carriers(grid: &mut Grid<VecStorage>, data: &[u8]) {
    for (i, &b) in data.iter().enumerate() {
        let carrier = if b == 0 { CellType(255) } else { CellType(b) };
        // packet[0] должен прийти первым — стартует БЛИЖЕ к краю. Зазор 2 —
        // чтобы цель сдвига одного носителя не совпадала с исходной клеткой
        // соседа (см. обсуждение в strength_self_modification.rs).
        let start_x = 2 * (data.len() - 1 - i);
        grid.set_cell(start_x, 0, Cell { value: CellValue(carrier), born_at: 0 });
    }
}

fn main() {
    // R выбрано ПОСЛЕ того, как машина уже определена — машина ничего о нём
    // не знает заранее. Сдвиг Up (dir_byte=0) + изменение той же клетки
    // (dx=0, dy=0) — оба места, где нужен буквальный ноль.
    let target_id: u8 = 77;
    let priority: u8 = 5;
    let steps: u8 = 3;
    let new_value: u8 = 222;
    // [priority, id_len, id_byte, SHIFT_FLAG, dir_byte=Up(0), steps, dx=0, dy=0, value]
    let data: [u8; 9] = [priority, 1, target_id, 0xFE, 0, steps, 0, 0, new_value];

    let mut grid = Grid::new(VecStorage::new(WIDTH, 1), Default::default());
    let mut out = BoundaryBuffer::new();
    out.direction = "output".to_string();
    grid.set_boundary(WIDTH - 1, 0, out);
    place_carriers(&mut grid, &data);

    let mut engine = Engine::new(grid, build_universal_machine());

    println!("Универсальная машина: 255 фиксированных правил, построена ДО выбора R.");
    println!("R для передачи (данные, без терминатора): {:?}", data);
    println!(
        "До передачи: правило для типа {} — это правило-ретранслятор универсальной машины\n\
         (тип {} тоже используется как обычный носитель байта; после установки R оно будет\n\
         замещено настоящим поведением R — это ожидаемо, не ошибка): {:?}\n",
        target_id, target_id,
        engine.rule_index().get(&CellType(target_id)).and_then(|v| v.first())
    );

    let mut rule_store = RuleStore::new();
    let mut installed = false;
    let mut terminator_sent = false;
    let mut bytes_seen = 0usize;
    for tick in 1..=WIDTH as u32 {
        engine.run_tick();

        // Считаем реально прибывшие байты ДО дренажа (drain каждый тик
        // очищает очередь буфера, так что её длина в любой момент — не
        // накопленный счётчик, а только то, что пришло за этот тик).
        if let Some(buf) = engine.grid().get_boundary(WIDTH - 1, 0) {
            if let Some(q) = buf.queues.get(&0) {
                if !q.is_empty() {
                    bytes_seen += q.len();
                    println!("тик {:>3}: пришло {:?} (всего данных получено: {}/{})", tick, q.iter().map(|c| c.value.0.0).collect::<Vec<_>>(), bytes_seen, data.len());
                }
            }
        }

        // Терминатор — рамка протокола, ОДНА и та же для любого R (не часть
        // того, что решётка вычисляет); модем дописывает стоп-бит сам, как
        // только все байты данных получены.
        if !terminator_sent && bytes_seen >= data.len() {
            if let Some(buf) = engine.grid_mut().get_boundary_mut(WIDTH - 1, 0) {
                buf.enqueue(0, Cell { value: CellValue(CellType(255)), born_at: 0 });
                terminator_sent = true;
                println!("тик {:>3}: терминатор дописан", tick);
            }
        }

        for completed in rule_store.drain_rule_channel(engine.grid_mut()) {
            if let RuleOp::AddRule(rule) = &completed.op {
                println!(
                    "тик {:>3}: универсальная машина передала правило: id={:?}, priority={}, shifts={:?}, changes={:?}",
                    tick, rule.id, rule.priority, rule.shifts, rule.changes
                );
            }
            rule_store.apply(completed);
            installed = true;
        }
        if installed {
            for (k, v) in rule_store.get_index() {
                engine.set_rules_for_head(*k, v.clone());
            }
            break;
        }
    }

    println!("Ошибок декодирования протокола: {}\n", rule_store.error_stats());

    match engine.rule_index().get(&CellType(target_id)).and_then(|v| v.first()) {
        Some(rule) => {
            let ok = rule.priority == priority as u32
                && rule.shifts == vec![vec![ShiftSpec::new(Direction::Up, steps as u16)]]
                && rule.changes == vec![(0, 0, cellaria::types::ChangeValue::Literal(new_value))];
            println!(
                "Установленное правило: {:?}\nСовпадает с задуманным R: {}",
                rule, if ok { "ДА" } else { "НЕТ" }
            );
        }
        None => println!("Правило не установилось."),
    }
}
