//! Блок F, п.4: "автономная самопроверка без внешнего наблюдателя".
//!
//! `proof_quorum_fault_tolerance.rs` (TMR) уже показал, что детерминизм
//! (Theorem 1) даёт бесплатное обнаружение сбоя — но СРАВНЕНИЕ там делал
//! внешний Rust-код (`vote()`), читающий снапшоты ТРЁХ отдельных `Engine`.
//! `proof_self_attestation.rs` показал, что решётка может ВЫЧИСЛИТЬ и
//! передать наружу отпечаток своего состояния — но СРАВНЕНИЕ двух
//! отпечатков (честный/подменённый) опять же делал внешний `main()`.
//!
//! Здесь сравнение — тоже ОБЫЧНОЕ ПРАВИЛО, не внешний код: решётка держит
//! ДВА независимых счётчика (A и B), тактируемых одним и тем же тиком
//! (значит, в отсутствие сбоя они ОБЯЗАНЫ совпадать на каждом тике — то же
//! обоснование детерминизмом, что и в TMR, но теперь ВНУТРИ одной решётки,
//! а не между тремя `Engine`), и клетку-компаратор МЕЖДУ ними: её паттерн
//! читает ОБА счётчика одновременно (offset -1 и +1) и, если они равны,
//! ничего не меняет; если хоть один из N вариантов "A==B==k" не подошёл —
//! срабатывает запасное правило с более низким приоритетом, и компаратор
//! необратимо становится ALARM. Обнаружение целиком внутри `pattern`/
//! `changes` — внешний код только ОДИН РАЗ читает финальный тип клетки, не
//! сравнивает никаких сырых значений сам.
//!
//! Проверка: без вмешательства компаратор ни разу не срабатывает за
//! несколько полных циклов счётчика; при точечной порче ОДНОГО счётчика
//! (прямая запись в обход `run_tick` — та же имитация аппаратного сбоя, что
//! и в TMR) компаратор фиксирует ALARM ровно на следующем тике и остаётся в
//! нём (защёлка), не "отходит" сам по себе.

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Rule};
use cellaria::{Grid, VecStorage};

const COUNTER_MOD: u8 = 6;
const CNT_A_BASE: u8 = 100; // счётчик A: типы 100..105
const CNT_B_BASE: u8 = 110; // счётчик B: типы 110..115 (та же фаза, другой диапазон типов)
const COMPARATOR: u8 = 50;
const ALARM: u8 = 51;

const POS_A: usize = 0;
const POS_CMP: usize = 1;
const POS_B: usize = 2;
const WIDTH: usize = 3;

fn build_rule_index() -> HashMap<CellType, Vec<Rule>> {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();

    // Оба счётчика тактируются одним и тем же тиком независимо друг от
    // друга — под нагрузкой без сбоя они ОБЯЗАНЫ идти в фазе (детерминизм,
    // как и в TMR).
    for k in 0..COUNTER_MOD {
        idx.insert(
            CellType(CNT_A_BASE + k),
            vec![Rule {
                id: vec![CellType(CNT_A_BASE + k)],
                pattern: vec![],
                shifts: vec![],
                changes: vec![(0, 0, ChangeValue::Literal(CNT_A_BASE + (k + 1) % COUNTER_MOD))],
                active_only: false,
                priority: 5,
                min_age: 0,
                overflow: Default::default(),
                cam: None,
                tie_break: 0,
                starvation_after: None,
            }],
        );
        idx.insert(
            CellType(CNT_B_BASE + k),
            vec![Rule {
                id: vec![CellType(CNT_B_BASE + k)],
                pattern: vec![],
                shifts: vec![],
                changes: vec![(0, 0, ChangeValue::Literal(CNT_B_BASE + (k + 1) % COUNTER_MOD))],
                active_only: false,
                priority: 5,
                min_age: 0,
                overflow: Default::default(),
                cam: None,
                tie_break: 0,
                starvation_after: None,
            }],
        );
    }

    // Компаратор: N правил "A==B==k" (приоритет выше — побеждают, если
    // подошли), одно запасное "иначе -> ALARM" (приоритет ниже — срабатывает,
    // только если НИ ОДНО из N выше не подошло). Всё сравнение — в pattern,
    // никакого внешнего кода.
    let mut comparator_rules = Vec::new();
    for k in 0..COUNTER_MOD {
        comparator_rules.push(Rule {
            id: vec![CellType(COMPARATOR)],
            pattern: vec![(0, 0, CellType(COMPARATOR)), (-1, 0, CellType(CNT_A_BASE + k)), (1, 0, CellType(CNT_B_BASE + k))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(COMPARATOR))],
            active_only: false,
            priority: 20,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
        });
    }
    comparator_rules.push(Rule {
        id: vec![CellType(COMPARATOR)],
        pattern: vec![(0, 0, CellType(COMPARATOR))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(ALARM))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
    });
    idx.insert(CellType(COMPARATOR), comparator_rules);
    // Для ALARM правил нет — защёлка: обнаруженный сбой навсегда виден,
    // не "рассасывается" сам собой на следующем тике.

    idx
}

fn build_grid() -> Grid<VecStorage> {
    let mut grid = Grid::new(VecStorage::new(WIDTH, 1), Default::default());
    grid.set_cell(POS_A, 0, Cell { value: CellValue::new(CNT_A_BASE), born_at: 0 });
    grid.set_cell(POS_CMP, 0, Cell { value: CellValue::new(COMPARATOR), born_at: 0 });
    grid.set_cell(POS_B, 0, Cell { value: CellValue::new(CNT_B_BASE), born_at: 0 });
    grid
}

fn comparator_state(engine: &Engine<VecStorage>) -> u8 {
    engine.grid().get_cell(POS_CMP, 0).map(|c| c.value.0 .0).expect("comparator cell always present")
}

fn main() {
    // ── Сценарий 1: без вмешательства — компаратор НИКОГДА не срабатывает ──
    let mut engine = Engine::new(build_grid(), build_rule_index());
    const CLEAN_TICKS: u32 = 20; // > 3 полных цикла COUNTER_MOD=6
    let mut ever_alarmed = false;
    for _ in 0..CLEAN_TICKS {
        engine.run_tick();
        if comparator_state(&engine) == ALARM {
            ever_alarmed = true;
        }
    }
    println!(
        "[1] {CLEAN_TICKS} тиков без вмешательства: компаратор {}",
        if ever_alarmed { "ЛОЖНО СРАБОТАЛ (!)" } else { "ни разу не сработал ✓" }
    );
    assert!(!ever_alarmed, "без сбоя компаратор не должен переходить в ALARM ни разу — иначе это ложные срабатывания, а не самопроверка");

    // ── Сценарий 2: точечная порча счётчика B в обход правил (имитация
    // аппаратного сбоя, как в proof_quorum_fault_tolerance.rs) ─────────────
    let mut engine = Engine::new(build_grid(), build_rule_index());
    const FAULT_TICK: u32 = 10;
    const TOTAL_TICKS: u32 = 20;
    let mut alarm_tick: Option<u32> = None;
    for tick in 1..=TOTAL_TICKS {
        engine.run_tick();
        if tick == FAULT_TICK {
            // Текущее значение A на этот момент (детерминированно известно:
            // A начал с 0 и увеличивается на 1 каждый тик по модулю 6).
            let current_k = (tick % COUNTER_MOD as u32) as u8;
            let wrong_k = (current_k + 1) % COUNTER_MOD; // заведомо другое значение
            let gen = engine.grid().generation();
            engine.grid_mut().set_cell(POS_B, 0, Cell { value: CellValue::new(CNT_B_BASE + wrong_k), born_at: gen });
            println!("[тик {tick}] прямая порча счётчика B в обход run_tick (сбой аппаратуры)");
        }
        let state = comparator_state(&engine);
        if state == ALARM && alarm_tick.is_none() {
            alarm_tick = Some(tick);
        }
        if tick < FAULT_TICK {
            assert_ne!(state, ALARM, "до сбоя компаратор не должен быть в ALARM (тик {tick})");
        }
    }

    println!(
        "[2] Сбой внесён на тике {FAULT_TICK}, компаратор перешёл в ALARM на тике {:?}",
        alarm_tick
    );
    assert_eq!(alarm_tick, Some(FAULT_TICK + 1), "компаратор должен зафиксировать расхождение РОВНО на следующем тике после сбоя (детект читает состояние ДО тика)");
    assert_eq!(comparator_state(&engine), ALARM, "ALARM — защёлка: должен остаться сработавшим до конца прогона");

    println!(
        "\nВывод: обнаружение расхождения между двумя тактируемыми одним тиком счётчиками — ОБЫЧНОЕ правило \
(pattern, читающий обе клетки сразу, плюс запасное правило с более низким приоритетом), а не внешний Rust-код. \
Внешний код здесь лишь ОДИН РАЗ читает финальный тип клетки-компаратора для проверки — не сравнивает никаких \
сырых значений сам и не участвует в самом обнаружении. Автономная самопроверка без внешнего наблюдателя — \
доказано, а не 'должно быть возможно в принципе'."
    );
}
