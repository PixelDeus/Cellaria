//! Вторая ось "где Cellaria реально сильнее" (после dirty-tracking) —
//! ПРОВЕРЕННАЯ КОМПОЗИЦИЯ независимо написанных наборов правил.
//!
//! У большинства "наивных" CA-движков объединение двух независимо
//! написанных доменов (например, "радиоактивный распад" + "проводник
//! Wireworld") в одной решётке — это ручная работа: нужно вручную убедиться,
//! что их правила никогда не пишут в одну клетку одновременно, иначе
//! получится состояние гонки, которое молча портит симуляцию. У Cellaria эта
//! проверка есть как встроенная фича: `ConflictGraph::check_composition`
//! статически доказывает (или опровергает) безопасность объединения ДО
//! того, как что-либо запущено — не нужно ни читать код другого модуля, ни
//! гонять симуляцию в надежде заметить баг.
//!
//! Важный нюанс, обнаруженный при подготовке этого примера: анализ работает
//! в ОТНОСИТЕЛЬНЫХ координатах (не знает абсолютного расположения на
//! решётке), поэтому модуль, использующий СДВИГИ (объект, движущийся в
//! пространстве — например, падающий шарик), в принципе не может быть
//! доказан безопасным относительно ЛЮБОГО другого модуля: анализ обязан
//! консервативно предположить, что траектория движения может пройти через
//! позицию другого модуля, где бы он ни был размещён — и это не баг
//! анализатора, а корректный, честный ответ (см. первую, отброшенную версию
//! этого примера, где "шарик" со сдвигом вниз был признан потенциально
//! конфликтующим с "проводом" — совершенно справедливо, потому что путь
//! падения физически проходит через ряд провода). Модуль ниже нарочно БЕЗ
//! сдвигов (радиоактивный распад на месте) — только тогда композиция
//! доказуема безопасной вне зависимости от взаимного расположения.
//!
//! Демонстрация:
//!   1. Модуль A ("распад": 1→3→7 на месте) и модуль B ("провод") используют
//!      непересекающиеся типы клеток и не двигаются в пространстве —
//!      `check_composition` доказывает Safe, и мы реально запускаем оба
//!      домена одновременно на одной решётке одним Engine.
//!   2. Модуль C — намеренно конфликтующий с A (тот же head-тип, другое
//!      безусловное правило) — показываем, что `check_composition` эту
//!      гонку ловит и называет ровно тот конфликт, из-за которого объединять
//!      C с A небезопасно.

use std::collections::HashMap;

use cellaria::conflict_analyzer::CompositionVerdict;
use cellaria::engine::Engine;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Rule};
use cellaria::{ConflictGraph, Grid, VecStorage};

// ============================================================================
// Модуль A: радиоактивный распад НА МЕСТЕ, без единого сдвига: 1 -> 3 -> 7
// (7 — стабильный конечный продукт, для него правил нет).
// ============================================================================
const RAD: u8 = 1;
const MID: u8 = 3;
const STABLE: u8 = 7;

fn module_decay() -> Vec<Rule> {
    vec![
        Rule {
            id: vec![CellType(RAD)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(MID))],
            active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None,
        },
        Rule {
            id: vec![CellType(MID)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(STABLE))],
            active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None,
        },
    ]
}

// ============================================================================
// Модуль B: "провод" — сигнал (Wireworld-lite), типы 20/21/22, тоже без
// сдвигов (голова "идёт" по проводу переключением типов соседних клеток,
// а не физическим перемещением) — не пересекается с A ни по типам, ни по
// траектории движения.
// ============================================================================
const WIRE: u8 = 20;
const HEAD: u8 = 21;
const TAIL: u8 = 22;

fn module_wire() -> Vec<Rule> {
    vec![
        Rule {
            id: vec![CellType(HEAD)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(TAIL))],
            active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None,
        },
        Rule {
            id: vec![CellType(TAIL)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(WIRE))],
            active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None,
        },
        Rule {
            id: vec![CellType(WIRE)],
            pattern: vec![(0, 0, CellType(WIRE)), (-1, 0, CellType(HEAD))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(HEAD))],
            active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None,
        },
    ]
}

// ============================================================================
// Модуль C: тот же head-тип RAD(1), безусловное правило без ограничения по
// паттерну — срабатывает в ТЕХ ЖЕ позициях, что и правило распада A[0], оба
// пишут в (0,0) — настоящий конфликт (два правила одного head-типа,
// расщеплённые по разным "модулям", могут сработать в одной и той же клетке
// и обе хотят там что-то записать).
// ============================================================================
fn module_bad() -> Vec<Rule> {
    vec![Rule {
        id: vec![CellType(RAD)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(99))],
        active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None,
    }]
}

fn make_rule_index(rules: Vec<Rule>) -> HashMap<CellType, Vec<Rule>> {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        idx.entry(rule.id[0]).or_default().push(rule);
    }
    idx
}

fn main() {
    let decay = module_decay();
    let wire = module_wire();
    let bad = module_bad();

    // ── 1. Безопасная композиция: A + B ──
    println!("Проверка композиции A(распад) + B(провод):");
    match ConflictGraph::check_composition(&decay, &wire) {
        CompositionVerdict::Safe => println!("  Safe — доказано статически, без единого запуска симуляции.\n"),
        CompositionVerdict::Unsafe(pairs) => println!("  Unsafe: {:?}\n", pairs),
    }

    // ── 2. Небезопасная композиция: A + C (намеренно конфликтующая) ──
    println!("Проверка композиции A(распад) + C(намеренно конфликтующий модуль):");
    match ConflictGraph::check_composition(&decay, &bad) {
        CompositionVerdict::Safe => println!("  Safe (неожиданно)\n"),
        CompositionVerdict::Unsafe(pairs) => {
            println!("  Unsafe — найдена(ы) конфликтующая(ие) пара(ы) правил: {:?}", pairs);
            println!("  (индексы — позиция правила в своём модуле; конфликт — оба под head=RAD(1),\n   оба безусловно срабатывают в (0,0), оба туда же и пишут)\n");
        }
    }

    // ── 3. Реальный совместный запуск проверенно безопасной композиции A+B ──
    println!("Совместный запуск A+B на одной решётке, один Engine, 4 тика:\n");
    let mut merged = decay.clone();
    merged.extend(wire.clone());
    let rule_index = make_rule_index(merged);

    let storage = VecStorage::new(6, 1);
    let mut grid = Grid::new(storage, Default::default());
    // Модуль A: один радиоактивный атом в (0,0) — отдельно от провода.
    grid.set_cell(0, 0, Cell { value: CellValue(CellType(RAD)), born_at: 0 });
    // Модуль B: непрерывный провод в (1,0)..(5,0), та же строка решётки,
    // головка слева — оба модуля буквально делят одну строку, просто разные
    // клетки в ней, и это доказанно безопасно (см. вывод выше).
    for x in 1..6 {
        grid.set_cell(x, 0, Cell { value: CellValue(CellType(WIRE)), born_at: 0 });
    }
    grid.set_cell(1, 0, Cell { value: CellValue(CellType(HEAD)), born_at: 0 });

    let mut engine = Engine::new(grid, rule_index);
    for tick in 0..4 {
        engine.run_tick();
        let atom = engine.grid().get_cell(0, 0).map(|c| c.value.0 .0);
        let wire_head_x = (1..6usize)
            .find(|&x| engine.grid().get_cell(x, 0).map(|c| c.value.0 .0) == Some(HEAD));
        println!("  тик {}: атом(x=0)={:?}, головка провода на x={:?}", tick + 1, atom, wire_head_x);
    }
    println!(
        "\nОба независимо написанных, разделяющих ОДНУ строку решётки домена\n\
         корректно эволюционируют одновременно в одном Engine — потому что их\n\
         безопасность была ДОКАЗАНА до запуска (без сдвигов у обоих — значит,\n\
         независимо от взаимного расположения), а не проверена постфактум по логам."
    );
}
