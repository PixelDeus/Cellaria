//! Большой пример: сводит вместе всё, что нашли за сессию, в один связный
//! сценарий, а не изолированные микро-демо.
//!
//! Один Engine на безграничном (`ChunkStorage`, недавно исправленном —
//! правила на нём раньше вообще не применялись) мире держит ОДНОВРЕМЕННО:
//!
//!   - "Провод" (Wireworld-lite) в одном регионе — питается извне через
//!     живой input-порт (сила: живой ввод/вывод во время работы).
//!   - Цепочку радиоактивного распада в ДРУГОМ регионе, за миллион клеток
//!     от первого (сила: безграничный мир — реальное расстояние, не игрушка).
//!   - Челнок (двигается сдвигами) в ТРЕТЬЕМ регионе, ещё дальше.
//!
//! Перед запуском — проверка безопасности объединения через
//! `ConflictGraph::check_composition` для каждой пары: провод и распад не
//! двигаются в пространстве — доказуемо безопасны друг с другом. Челнок
//! использует сдвиги — с ним компоновка НЕ доказывается статически ни с
//! чем (см. предыдущее обсуждение: анализ консервативен для движущихся
//! объектов), но это не мешает ему корректно работать в реальном времени —
//! арбитраж на рантайме всё равно верно разруливает любые пересечения,
//! просто без предварительной гарантии "точно никогда не столкнутся".
//!
//! По ходу дела — живое добавление ЧЕТВЁРТОГО правила на лету (сила: смена
//! правил без остановки), и замер времени тика, чтобы показать: несмотря
//! на номинально гигантский мир, стоимость тика не зависит от того, что
//! между тремя регионами лежат миллионы пустых клеток.

use std::collections::HashMap;
use std::time::Instant;

use cellaria::conflict_analyzer::CompositionVerdict;
use cellaria::engine::Engine;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Direction, Rule, ShiftSpec};
use cellaria::{ChunkStorage, ConflictGraph, Grid};

// ============================================================================
// Регион A (около x=0): провод — Wireworld-lite.
// ============================================================================
const WIRE: u8 = 20;
const HEAD: u8 = 21;
const TAIL: u8 = 22;
const WIRE_START: usize = 0;
const WIRE_LEN: usize = 40;

fn module_wire() -> Vec<Rule> {
    vec![
        Rule {
            id: vec![CellType(HEAD)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(TAIL))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        },
        Rule {
            id: vec![CellType(TAIL)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(WIRE))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        },
        Rule {
            id: vec![CellType(WIRE)],
            pattern: vec![(0, 0, CellType(WIRE)), (-1, 0, CellType(HEAD))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(HEAD))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        },
    ]
}

// ============================================================================
// Регион B (за миллион клеток от A): распад на месте, без единого сдвига.
// ============================================================================
const RAD: u8 = 1;
const MID: u8 = 3;
const STABLE: u8 = 7;
const DECAY_REGION_X: usize = 1_000_000;

fn module_decay() -> Vec<Rule> {
    vec![
        Rule {
            id: vec![CellType(RAD)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(MID))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        },
        Rule {
            id: vec![CellType(MID)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(STABLE))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        },
    ]
}

// ============================================================================
// Регион C (ещё дальше): челнок, двигается сдвигами — единственный из трёх,
// кто реально путешествует по решётке.
// ============================================================================
const HEAD_R: u8 = 50;
const HEAD_L: u8 = 51;
const WALL: u8 = 52;
const SHUTTLE_REGION_X: usize = 2_000_000;
const SHUTTLE_WIDTH: usize = 20;

fn module_shuttle() -> Vec<Rule> {
    vec![
        Rule {
            id: vec![CellType(HEAD_R)],
            pattern: vec![(0, 0, CellType(HEAD_R)), (1, 0, CellType(WALL))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(HEAD_L))],
            active_only: false,
            priority: 20,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        },
        Rule {
            id: vec![CellType(HEAD_R)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
            changes: vec![],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        },
        Rule {
            id: vec![CellType(HEAD_L)],
            pattern: vec![(0, 0, CellType(HEAD_L)), (-1, 0, CellType(WALL))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(HEAD_R))],
            active_only: false,
            priority: 20,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        },
        Rule {
            id: vec![CellType(HEAD_L)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec::new(Direction::Left, 1)]],
            changes: vec![],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        },
    ]
}

// ============================================================================
// Четвёртое правило — добавляется НА ЛЕТУ в работающий Engine (не заранее).
// STABLE(7) сейчас инертен; добавим ему поведение "светится" — превращается
// в тип 8 один раз (демонстрация: новое поведение для уже существующих на
// решётке клеток срабатывает без перезапуска).
// ============================================================================
const GLOW: u8 = 8;

fn make_rule_index(rules: Vec<Rule>) -> HashMap<CellType, Vec<Rule>> {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        idx.entry(rule.id[0]).or_default().push(rule);
    }
    idx
}

fn main() {
    let wire = module_wire();
    let decay = module_decay();
    let shuttle = module_shuttle();

    println!("=== Проверка безопасности объединения (до единого запуска) ===\n");
    match ConflictGraph::check_composition(&wire, &decay) {
        CompositionVerdict::Safe => println!("провод + распад: Safe (оба не двигаются в пространстве)"),
        CompositionVerdict::Unsafe(p) => println!("провод + распад: Unsafe {:?}", p),
    }
    match ConflictGraph::check_composition(&wire, &shuttle) {
        CompositionVerdict::Safe => println!("провод + челнок: Safe"),
        CompositionVerdict::Unsafe(_) => println!(
            "провод + челнок: Unsafe (ожидаемо — челнок двигается сдвигами, статический \
             анализ не может исключить пересечение траектории ни с чем; реальному арбитражу \
             на рантайме это не мешает работать корректно, см. обсуждение сессии)"
        ),
    }
    match ConflictGraph::check_composition(&decay, &shuttle) {
        CompositionVerdict::Safe => println!("распад + челнок: Safe"),
        CompositionVerdict::Unsafe(_) => println!("распад + челнок: Unsafe (та же причина — сдвиги)"),
    }

    // ── Собираем мир: один Engine, ChunkStorage, три региона за миллионы клеток друг от друга ──
    let mut merged = wire.clone();
    merged.extend(decay.clone());
    merged.extend(shuttle.clone());
    let rule_index = make_rule_index(merged);

    let storage = ChunkStorage::new();
    let mut grid = Grid::new(storage, Default::default());

    // Регион A: провод, головка ждёт первого input-пакета.
    for x in WIRE_START..WIRE_START + WIRE_LEN {
        grid.set_cell(
            x,
            0,
            Cell {
                value: CellValue(CellType(WIRE)),
                born_at: 0,
            },
        );
    }
    let mut input_buf = cellaria::types::BoundaryBuffer::new();
    input_buf.direction = "input".to_string();
    grid.set_boundary(WIRE_START, 0, input_buf);

    // Регион B: пять радиоактивных атомов, за миллион клеток от A.
    for i in 0..5 {
        grid.set_cell(
            DECAY_REGION_X + i * 3,
            0,
            Cell {
                value: CellValue(CellType(RAD)),
                born_at: 0,
            },
        );
    }

    // Регион C: челнок между двух стен, ещё дальше.
    grid.set_cell(
        SHUTTLE_REGION_X,
        0,
        Cell {
            value: CellValue(CellType(WALL)),
            born_at: 0,
        },
    );
    grid.set_cell(
        SHUTTLE_REGION_X + SHUTTLE_WIDTH + 1,
        0,
        Cell {
            value: CellValue(CellType(WALL)),
            born_at: 0,
        },
    );
    grid.set_cell(
        SHUTTLE_REGION_X + 1,
        0,
        Cell {
            value: CellValue(CellType(HEAD_R)),
            born_at: 0,
        },
    );

    let mut engine = Engine::new(grid, rule_index);

    println!("\n=== Запуск: 40 тиков, живой ввод в провод на тиках 2 и 20 ===\n");
    let mut tick_times = Vec::new();
    for tick in 1..=40u32 {
        if tick == 2 || tick == 20 {
            engine.push_input(0, HEAD);
            println!("тик {:>2}: внешний импульс подан в провод", tick);
        }
        engine.apply_input();

        let t0 = Instant::now();
        engine.run_tick();
        tick_times.push(t0.elapsed());

        if tick == 15 {
            // Живое добавление правила прямо посреди работы движка.
            engine.set_rules_for_head(
                CellType(STABLE),
                vec![Rule {
                    id: vec![CellType(STABLE)],
                    pattern: vec![],
                    shifts: vec![],
                    changes: vec![(0, 0, ChangeValue::Literal(GLOW))],
                    active_only: false,
                    priority: 10,
                    min_age: 0,
                    overflow: Default::default(),
                    cam: None,
                    tie_break: 0,
                    starvation_after: None,
                    feedback: None,
                    recursion: None,
                    memory: None,
                    max_activations: None,
                    cross_layer_reads: Vec::new(),
                }],
            );
            println!(
                "тик {:>2}: добавили правило STABLE->GLOW на лету, решётка не трогалась",
                tick
            );
        }

        if tick % 10 == 0 || tick == 1 {
            let wire_head = (WIRE_START..WIRE_START + WIRE_LEN)
                .find(|&x| engine.grid().get_cell(x, 0).map(|c| c.value.0 .0) == Some(HEAD));
            let decayed = (0..5)
                .filter(|&i| {
                    engine.grid().get_cell(DECAY_REGION_X + i * 3, 0).map(|c| c.value.0 .0) == Some(STABLE)
                        || engine.grid().get_cell(DECAY_REGION_X + i * 3, 0).map(|c| c.value.0 .0) == Some(GLOW)
                })
                .count();
            let shuttle_pos = (SHUTTLE_REGION_X..SHUTTLE_REGION_X + SHUTTLE_WIDTH + 2).find(|&x| {
                let v = engine.grid().get_cell(x, 0).map(|c| c.value.0 .0);
                v == Some(HEAD_R) || v == Some(HEAD_L)
            });
            println!(
                "тик {:>2}: провод-головка@{:?} | распад: {}/5 завершено | челнок@{:?}",
                tick, wire_head, decayed, shuttle_pos
            );
        }
    }

    let total: std::time::Duration = tick_times.iter().sum();
    let avg_us = total.as_micros() as f64 / tick_times.len() as f64;
    println!(
        "\nСредняя стоимость тика: {:.1} мкс — несмотря на то, что регионы разнесены\n\
         на МИЛЛИОНЫ клеток номинально пустого пространства между ними.",
        avg_us
    );
}
