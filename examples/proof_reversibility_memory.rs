//! Обратимость + `Rule.memory` (`MemorySpec`, FIFO-гейт по последовательности
//! прошлых наблюдений, см. её doc-комментарий в `types.rs` и гейт-фильтр в
//! `src/engine/mod.rs`'s `run_tick_with_cache`): держится ли Теорема 9
//! (paper2.md §7.3), если в наборе правил участвует правило с
//! `memory: Some(MemorySpec)`?
//!
//! Формальная причина для сомнения — ТА ЖЕ, что уже проверена для
//! `Rule.feedback` (`proof_reversibility_feedback.rs`): у правила с `memory`
//! эффект НА ЭТОМ ТИКЕ зависит не только от `config_t`, а ЕЩЁ и от
//! `Engine::memory_buffers` — приватного, накопленного за ВСЮ историю
//! FIFO-буфера, который НЕ является частью решётки. `feedback` был ПОЧИНЁН
//! (не "признан необратимым") другим рецептом: не пересчитывать скрытый
//! счётчик, а ЗАПИСАТЬ фактически применённую последовательность направлений
//! (уже наблюдаемую через публичный API — позицию токена) и проиграть её
//! назад ОБЫЧНЫМИ (без feedback) правилами. Вопрос этого файла: работает ли
//! ТОТ ЖЕ рецепт для `memory` — и работает ли он ОДИНАКОВО для обоих
//! `RecordTrigger`?
//!
//! Ответ — РАЗНЫЙ для двух триггеров, и разница конструктивно
//! продемонстрирована ниже, а не просто заявлена:
//!
//! - `RecordTrigger::NeighborType(dir)` пишет в буфер значение, которое ПО
//!   ОПРЕДЕЛЕНИЮ является прямым чтением ТЕКУЩЕГО pre-tick содержимого
//!   решётки (тип соседа в направлении `dir` — см. `engine/mod.rs` вокруг
//!   `RecordTrigger::NeighborType(dir) => { ... grid.get_cell(nx, ny) ... }`).
//!   Это значит, что КАЖДЫЙ элемент буфера — это то же самое, что наблюдатель
//!   снаружи увидел бы, просто посмотрев на решётку в нужный момент. Раздел 1
//!   ниже строит рабочий `R⁻¹` тем же приёмом, что и `feedback`: записываем
//!   позицию токена после каждого тика (публичный API), выводим из дельт,
//!   сработал ли гейт в этот тик, и проигрываем запись назад обычными
//!   правилами — включая декой, проверено побитово.
//!
//! - `RecordTrigger::RuleOutcome` пишет `Applied`/`Missed` — ИСХОД
//!   АРБИТРАЖА, а не чтение решётки. Раздел 2 строит два КОНСТРУКТИВНЫХ
//!   контрпримера: (2a) самобутстрапящийся дедлок — `match_pattern =
//!   [Applied]` при пустом стартовом буфере никогда не открывается В ПРИНЦИПЕ
//!   (совпадает с находкой этой сессии про "self-referential RuleOutcome
//!   bootstrap deadlock"), проверено вживую: правило с РЕАЛЬНЫМ (не
//!   idempotent) сдвигом ни разу не срабатывает за 10 тиков. (2b) —
//!   решающий пример: ДВА разных набора правил (A — гейтованное правило
//!   само по себе; B — то же гейтованное правило ПЛЮС всегда-побеждающий
//!   конкурент более высокого приоритета на ту же клетку), при
//!   idempotent-эффекте (запись того же значения, что уже есть), дают
//!   ПОБИТОВО ОДИНАКОВУЮ решётку на КАЖДОМ из N тиков — но по построению
//!   арбитража (`arbitrator::arbitrate_with_cam`: `Reverse(priority)` первым
//!   полем тай-брейка, высокий priority побеждает) их буферы ОБЯЗАНЫ разойтись
//!   (A рано или поздно побеждает в одиночку и пишет `Applied`; B-шный gated
//!   кандидат приоритетом 10 против конкурента 20 никогда не побеждает и
//!   пишет только `Missed`) — то есть публично неотличимые прогоны с РАЗНОЙ
//!   внутренней историей. Раз внешне неотличимые прогоны могут иметь разную
//!   `RuleOutcome`-историю, "прочитать историю по дельтам решётки" для этого
//!   триггера в общем случае НЕВОЗМОЖНО — не потому что рецепт `feedback` не
//!   подошёл технически, а потому что нужной информации структурно нет во
//!   внешне наблюдаемых данных.

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{
    Cell, CellType, CellValue, ChangeValue, Direction, MemorySpec, RecordTrigger, RecordedValue, Rule, ShiftSpec,
};
use cellaria::{Grid, VecStorage};

fn reverse_direction(d: Direction) -> Direction {
    match d {
        Direction::Up => Direction::Down,
        Direction::Down => Direction::Up,
        Direction::Left => Direction::Right,
        Direction::Right => Direction::Left,
    }
}

fn base_rule(id: u8, priority: u32) -> Rule {
    Rule {
        id: vec![CellType(id)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![],
        active_only: false,
        priority,
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
    }
}

// ============================================================================
// Раздел 1: `RecordTrigger::NeighborType` — рабочий рецепт
// ============================================================================
//
// Мир: 2 строки. Строка y=0 — "часы" (каждая клетка КАЖДЫЙ тик безусловно
// переключается CLOCK_A<->CLOCK_B обычным, без memory, правилом — полностью
// публичное, ничем не скрытое состояние). Строка y=1 — токен, который
// сдвигается Right ТОЛЬКО если его gate открыт: `memory` смотрит на соседа
// сверху (Direction::Up, НЕ на направление своего же сдвига — иначе сдвиг
// токена перезаписывал бы саму клетку-часы, которую он наблюдает) и требует
// буфер `[CLOCK_A, CLOCK_B]` (окно 2). Часы декуплированы от пути токена
// (разные строки), так что расписание "открыт/закрыт" — не тривиальное
// "всегда"/"никогда", а зависит от фазы часов на момент, когда буфер
// заполнился.

const WIDTH1: usize = 30;
const CLOCK_ROW: usize = 0;
const TOKEN_ROW: usize = 1;
const CLOCK_A: u8 = 70;
const CLOCK_B: u8 = 71;
const TOKEN1: u8 = 60;
const DECOY1: u8 = 61;
const START_X1: usize = 5;
const DECOY_X1: usize = 25; // заведомо вне пути токена (максимум ~TICKS1/2 шагов вправо)
                            // НЕЧЁТНОЕ специально: при чётном TICKS1 (проверено эмпирически на 12/14/16/18)
                            // периодичность часов (период 2) делает наивный реверс СЛУЧАЙНО побитово
                            // совпадающим с исходной решёткой — не потому что наивный рецепт верен, а
                            // из-за симметрии открыт/закрыт-расписания при чётном числе тиков туда и
                            // обратно. Нечётное TICKS1 (проверено на 11/13/15/17) ломает эту
                            // случайную симметрию и делает ассерт "наивный реверс НЕ должен был
                            // случайно восстановить решётку" содержательным, а не тавтологией.
const TICKS1: u32 = 15;

fn clock_rules() -> HashMap<CellType, Vec<Rule>> {
    let mut idx = HashMap::new();
    idx.insert(
        CellType(CLOCK_A),
        vec![Rule {
            changes: vec![(0, 0, ChangeValue::Literal(CLOCK_B))],
            ..base_rule(CLOCK_A, 5)
        }],
    );
    idx.insert(
        CellType(CLOCK_B),
        vec![Rule {
            changes: vec![(0, 0, ChangeValue::Literal(CLOCK_A))],
            ..base_rule(CLOCK_B, 5)
        }],
    );
    idx
}

fn forward_rules_section1() -> HashMap<CellType, Vec<Rule>> {
    let mut idx = clock_rules();
    idx.insert(
        CellType(TOKEN1),
        vec![Rule {
            shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
            memory: Some(MemorySpec {
                window: 2,
                record_trigger: RecordTrigger::NeighborType(Direction::Up),
                match_pattern: vec![
                    RecordedValue::Type(CellType(CLOCK_A)),
                    RecordedValue::Type(CellType(CLOCK_B)),
                ],
            }),
            ..base_rule(TOKEN1, 10)
        }],
    );
    idx
}

/// Наивный рецепт (по аналогии с `proof_reversibility_feedback.rs`):
/// тот же memory-гейт (СВЕЖИЙ буфер — другого пути нет, поле приватное),
/// направление сдвига развёрнуто.
fn naive_reverse_rules_section1() -> HashMap<CellType, Vec<Rule>> {
    let mut idx = clock_rules();
    idx.insert(
        CellType(TOKEN1),
        vec![Rule {
            shifts: vec![vec![ShiftSpec::new(Direction::Left, 1)]],
            memory: Some(MemorySpec {
                window: 2,
                record_trigger: RecordTrigger::NeighborType(Direction::Up),
                match_pattern: vec![
                    RecordedValue::Type(CellType(CLOCK_A)),
                    RecordedValue::Type(CellType(CLOCK_B)),
                ],
            }),
            ..base_rule(TOKEN1, 10)
        }],
    );
    idx
}

/// Умный рецепт: ОБЫЧНОЕ (без memory) правило на один тик — либо сдвиг в
/// указанном направлении, либо вообще без правила для TOKEN1 (если этот тик
/// по записи не двигался).
fn reverse_step_rules_section1(direction: Option<Direction>) -> HashMap<CellType, Vec<Rule>> {
    let mut idx = clock_rules();
    if let Some(d) = direction {
        idx.insert(
            CellType(TOKEN1),
            vec![Rule {
                shifts: vec![vec![ShiftSpec::new(d, 1)]],
                ..base_rule(TOKEN1, 10)
            }],
        );
    }
    idx
}

fn build_grid1(token_x: usize) -> Grid<VecStorage> {
    let mut grid = Grid::new(VecStorage::new(WIDTH1, 2), Default::default());
    for x in 0..WIDTH1 {
        grid.set_cell(
            x,
            CLOCK_ROW,
            Cell {
                value: CellValue(CellType(CLOCK_A)),
                born_at: 0,
            },
        );
    }
    grid.set_cell(
        token_x,
        TOKEN_ROW,
        Cell {
            value: CellValue(CellType(TOKEN1)),
            born_at: 0,
        },
    );
    grid.set_cell(
        DECOY_X1,
        TOKEN_ROW,
        Cell {
            value: CellValue(CellType(DECOY1)),
            born_at: 0,
        },
    );
    grid
}

fn grid_from_snapshot1(snap: &[u8]) -> Grid<VecStorage> {
    let mut g = Grid::new(VecStorage::new(WIDTH1, 2), Default::default());
    for y in 0..2 {
        for x in 0..WIDTH1 {
            let v = snap[y * WIDTH1 + x];
            if v != 0 {
                g.set_cell(
                    x,
                    y,
                    Cell {
                        value: CellValue(CellType(v)),
                        born_at: 0,
                    },
                );
            }
        }
    }
    g
}

fn snapshot1(engine: &Engine<VecStorage>) -> Vec<u8> {
    (0..2)
        .flat_map(|y| (0..WIDTH1).map(move |x| (x, y)))
        .map(|(x, y)| engine.grid().get_cell(x, y).map(|c| c.value.0 .0).unwrap_or(0))
        .collect()
}

fn token1_x_of(snap: &[u8]) -> Option<usize> {
    (0..WIDTH1).find(|&x| snap[TOKEN_ROW * WIDTH1 + x] == TOKEN1)
}

fn section1() {
    println!("========== Раздел 1: RecordTrigger::NeighborType ==========\n");
    let initial_snapshot: Vec<u8> = {
        let g = build_grid1(START_X1);
        (0..2)
            .flat_map(|y| (0..WIDTH1).map(move |x| (x, y)))
            .map(|(x, y)| g.get_cell(x, y).map(|c| c.value.0 .0).unwrap_or(0))
            .collect()
    };
    println!(
        "Токен на x={START_X1} (строка {TOKEN_ROW}), декой на x={DECOY_X1}. Часы (строка {CLOCK_ROW}) — обычное \
правило, переключается CLOCK_A<->CLOCK_B КАЖДЫЙ тик безусловно, публично видно. Гейт токена: окно=2, \
NeighborType(Up), match_pattern=[CLOCK_A, CLOCK_B]."
    );

    // ── Прямой прогон — записываем позицию токена ПОСЛЕ каждого тика ────────
    let mut forward = Engine::new(build_grid1(START_X1), forward_rules_section1());
    let mut positions: Vec<usize> = vec![START_X1];
    for _ in 1..=TICKS1 {
        forward.run_tick();
        positions.push(token1_x_of(&snapshot1(&forward)).expect("токен обязан остаться на решётке"));
    }
    let final_snapshot = snapshot1(&forward);
    println!("\nПозиции токена по тикам: {:?}", positions);
    assert_eq!(
        final_snapshot[TOKEN_ROW * WIDTH1 + DECOY_X1],
        DECOY1,
        "декой обязан пережить прямой прогон нетронутым"
    );

    let fired: Vec<bool> = (1..=TICKS1 as usize)
        .map(|i| positions[i] != positions[i - 1])
        .collect();
    let open_ticks = fired.iter().filter(|&&f| f).count();
    println!(
        "Расписание гейта (по дельтам позиции, публичный API, БЕЗ доступа к `memory_buffers`): {} тиков открыт \
из {TICKS1} — {:?}",
        open_ticks,
        fired
            .iter()
            .map(|&f| if f { "открыт" } else { "закрыт" })
            .collect::<Vec<_>>()
    );
    assert!(
        open_ticks > 0 && open_ticks < TICKS1 as usize,
        "расписание должно быть НЕтривиальным (смесь открыт/закрыт), иначе тест не проверяет ничего интересного"
    );

    // ── Наивный R⁻¹: тот же memory-гейт, свежий буфер, направление сдвига
    // развёрнуто (по аналогии с proof_reversibility_feedback.rs) ────────────
    let mut naive = Engine::new(grid_from_snapshot1(&final_snapshot), naive_reverse_rules_section1());
    for _ in 1..=TICKS1 {
        naive.run_tick();
    }
    let naive_snapshot = snapshot1(&naive);
    println!(
        "\nНаивный реверс (тот же memory-гейт, свежий буфер, Left вместо Right) совпал с исходной решёткой: {}",
        if naive_snapshot == initial_snapshot {
            "ДА"
        } else {
            "НЕТ"
        }
    );
    assert_ne!(
        naive_snapshot, initial_snapshot,
        "если наивный рецепт СЛУЧАЙНО восстановил решётку побитово, тест ничего не доказывает — нужно другое TICKS1/окно"
    );
    println!(
        "Как и ожидалось: свежий буфер на реверсе НЕ воспроизводит фазу часов, при которой реально открывался \
гейт на прямом прогоне — расписание получается другим."
    );

    // ── Умный R⁻¹: записанное расписание (fired), проиграно в обратном
    // порядке ОБЫЧНЫМИ (без memory) правилами ───────────────────────────────
    let mut smart_snapshot = final_snapshot.clone();
    for i in (0..TICKS1 as usize).rev() {
        let direction = if fired[i] {
            Some(reverse_direction(Direction::Right))
        } else {
            None
        };
        let mut step = Engine::new(
            grid_from_snapshot1(&smart_snapshot),
            reverse_step_rules_section1(direction),
        );
        step.run_tick();
        smart_snapshot = snapshot1(&step);
    }
    println!(
        "\nУмный реверс (записанное по дельтам расписание, без memory на реверсе) совпал с исходной решёткой: {}",
        if smart_snapshot == initial_snapshot {
            "ДА"
        } else {
            "НЕТ"
        }
    );
    assert_eq!(
        smart_snapshot, initial_snapshot,
        "умный рецепт обязан точно восстановить решётку, включая часы и декой — если нет, конструкция неверна"
    );
    println!(
        "\nВывод раздела 1: `memory` с `NeighborType` — ЧИНИТСЯ ТЕМ ЖЕ рецептом, что и `feedback`. Причина \
структурная, не случайная: `NeighborType` по определению читает ТЕКУЩУЮ решётку, так что \"сработал ли гейт \
этот тик\" всегда выводимо из дельты видимого эффекта, точно как направление feedback."
    );
}

// ============================================================================
// Раздел 2: `RecordTrigger::RuleOutcome` — рецепт ломается СТРУКТУРНО
// ============================================================================

const WIDTH2: usize = 12;

/// 2a: самобутстрапящийся дедлок. `match_pattern = [Applied]` при старте с
/// пустым буфером никогда не может открыться: буфер пуст => гейт закрыт по
/// определению (см. `run_tick_with_cache`'s гейт-фильтр: `buf.len() ==
/// spec.window` — при закрытом гейте кандидат исключается ДО арбитража =>
/// не может выиграть => не может ЗАПИСАТЬ `Applied` => буфер никогда не
/// станет `[Applied]` => гейт остаётся закрытым НАВСЕГДА. Проверяется здесь
/// не рассуждением, а вживую: правило с РЕАЛЬНЫМ (видимым) сдвигом ни разу
/// не срабатывает.
fn section2a() {
    println!("\n========== Раздел 2a: RuleOutcome, match_pattern=[Applied] — дедлок ==========\n");
    const TOKEN4: u8 = 62;
    const START_X4: usize = 5;
    const TICKS4: u32 = 10;

    let mut idx = HashMap::new();
    idx.insert(
        CellType(TOKEN4),
        vec![Rule {
            shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
            memory: Some(MemorySpec {
                window: 1,
                record_trigger: RecordTrigger::RuleOutcome,
                match_pattern: vec![RecordedValue::Applied],
            }),
            ..base_rule(TOKEN4, 10)
        }],
    );

    let mut grid = Grid::new(VecStorage::new(WIDTH2, 1), Default::default());
    grid.set_cell(
        START_X4,
        0,
        Cell {
            value: CellValue(CellType(TOKEN4)),
            born_at: 0,
        },
    );
    let mut engine = Engine::new(grid, idx);
    for _ in 1..=TICKS4 {
        engine.run_tick();
    }
    let final_x = (0..WIDTH2).find(|&x| engine.grid().get_cell(x, 0).map(|c| c.value.0 .0) == Some(TOKEN4));
    println!("Токен после {TICKS4} тиков: x={:?} (старт x={START_X4})", final_x);
    assert_eq!(final_x, Some(START_X4), "match_pattern=[Applied] с пустым стартовым буфером обязан ДЕДЛОКНУТЬСЯ — токен не должен был сдвинуться НИ РАЗУ");
    println!("Подтверждено: гейт ни разу не открылся за {TICKS4} тиков — self-referential bootstrap deadlock реален, не гипотеза.");
}

fn outcome_rule(priority: u32) -> Rule {
    Rule {
        pattern: vec![(0, 0, CellType(63))],
        changes: vec![(0, 0, ChangeValue::Literal(63))], // idempotent: пишет то же значение, что уже есть
        memory: Some(MemorySpec {
            window: 1,
            record_trigger: RecordTrigger::RuleOutcome,
            match_pattern: vec![RecordedValue::Missed],
        }),
        ..base_rule(63, priority)
    }
}

fn competitor_rule(priority: u32) -> Rule {
    Rule {
        pattern: vec![(0, 0, CellType(63))],
        changes: vec![(0, 0, ChangeValue::Literal(63))], // тоже idempotent — тот же видимый результат
        ..base_rule(63, priority)
    }
}

const DECOY2: u8 = 90;
const DECOY_X2: usize = 10;
const TOKEN3_X: usize = 3;

fn build_grid2() -> Grid<VecStorage> {
    let mut grid = Grid::new(VecStorage::new(WIDTH2, 1), Default::default());
    grid.set_cell(
        TOKEN3_X,
        0,
        Cell {
            value: CellValue(CellType(63)),
            born_at: 0,
        },
    );
    grid.set_cell(
        DECOY_X2,
        0,
        Cell {
            value: CellValue(CellType(DECOY2)),
            born_at: 0,
        },
    );
    grid
}

fn snapshot2(engine: &Engine<VecStorage>) -> Vec<u8> {
    (0..WIDTH2)
        .map(|x| engine.grid().get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(0))
        .collect()
}

/// 2b: решающий пример. Сценарий A — гейтованное правило (приоритет 10)
/// В ОДИНОЧКУ на своей клетке. Сценарий B — ТО ЖЕ гейтованное правило ПЛЮС
/// всегда активный конкурент (приоритет 20, БЕЗ memory) на ТУ ЖЕ клетку.
/// Оба эффекта idempotent (пишут то значение, что уже есть) — так что
/// ПОБЕДИТЕЛЬ арбитража каждый тик НЕ виден в решётке никак, только в
/// приватном `memory_buffers`.
fn section2b() {
    println!("\n========== Раздел 2b: RuleOutcome — внешне неотличимые, внутренне разные прогоны ==========\n");
    const TICKS_B: u32 = 8;

    let mut rules_a = HashMap::new();
    rules_a.insert(CellType(63), vec![outcome_rule(10)]);

    let mut rules_b = HashMap::new();
    rules_b.insert(CellType(63), vec![outcome_rule(10), competitor_rule(20)]);

    let initial_snapshot: Vec<u8> = {
        let g = build_grid2();
        (0..WIDTH2)
            .map(|x| g.get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(0))
            .collect()
    };

    let mut engine_a = Engine::new(build_grid2(), rules_a);
    let mut engine_b = Engine::new(build_grid2(), rules_b);
    let mut all_identical = true;
    for t in 1..=TICKS_B {
        engine_a.run_tick();
        engine_b.run_tick();
        let snap_a = snapshot2(&engine_a);
        let snap_b = snapshot2(&engine_b);
        let a_matches_initial = snap_a == initial_snapshot;
        let b_matches_initial = snap_b == initial_snapshot;
        println!(
            "тик {t}: A{}={:?}  B{}={:?}",
            if a_matches_initial { "==init" } else { "!=init" },
            snap_a,
            if b_matches_initial { "==init" } else { "!=init" },
            snap_b
        );
        if snap_a != snap_b || !a_matches_initial || !b_matches_initial {
            all_identical = false;
        }
    }

    println!(
        "\nСценарий A (соло, приоритет 10) и сценарий B (то же правило + конкурент приоритета 20) дали ПОБИТОВО \
одинаковую решётку на КАЖДОМ из {TICKS_B} тиков, и она РАВНА исходной: {}",
        if all_identical { "ДА" } else { "НЕТ" }
    );
    assert!(
        all_identical,
        "если A и B хоть где-то разошлись видимо (или решётка вообще изменилась), демонстрация не удалась — \
эффекты должны быть idempotent и потому невидимы, а разница должна быть ТОЛЬКО внутренней"
    );

    println!(
        "\nНо по построению арбитража (`arbitrator::arbitrate_with_cam`: сортировка по `Reverse(priority)` первым \
полем — высокий priority побеждает БЕЗУСЛОВНО при конфликте на одну клетку):\n\
- Сценарий A: gated-правило (приоритет 10) — ЕДИНСТВЕННЫЙ кандидат на клетку x={TOKEN3_X}. Тик 1: буфер пуст \
=> гейт закрыт => Missed (по исключению). Тик 2: буфер=[Missed] == match_pattern => гейт ОТКРЫТ => правило — \
единственный кандидат => побеждает БЕЗУСЛОВНО => буфер становится [Applied]. Тик 3: [Applied] != [Missed] => \
закрыт => Missed. Тик 4: [Missed] => открыт => Applied. Буфер A НАВСЕГДА чередует Missed/Applied.\n\
- Сценарий B: та же логика гейта у gated-правила (её собственный буфер зависит только от ЕЁ собственной \
прошлой истории, а не от наличия конкурента) — тик 2 гейт ТОЖЕ открывается, правило ТОЖЕ становится \
кандидатом... но теперь конкурент (приоритет 20, всегда кандидат) присутствует на ТОЙ ЖЕ клетке => \
конфликт => побеждает конкурент (20 > 10) => gated-правило ПРОИГРЫВАЕТ => буфер B получает Missed (не \
Applied!). Тик 3: буфер=[Missed] => гейт снова открыт => снова проигрывает конкуренту => снова Missed. Буфер \
B НАВСЕГДА остаётся [Missed] — открыт, но обречён проигрывать.\n\
Оба идемпотентных эффекта (Missed из-за исключения ДО арбитража в A и Missed из-за проигрыша конкуренту в B) \
выглядят СНАРУЖИ идентично — решётка не меняется ни там, ни там. Значит: `[Missed, Applied, Missed, Applied, \
...]` (A) и `[Missed, Missed, Missed, Missed, ...]` (B) — два РАЗНЫХ, детерминированных, бесконечно расходящихся \
внутренних расписания, дающих ПОБИТОВО ОДИНАКОВУЮ последовательность решёток. Никакой алгоритм, читающий \
ТОЛЬКО дельты решётки (ровно то, чем был построен рецепт для `feedback` и для `NeighborType` в разделе 1), не \
может определить, какое из двух расписаний реально произошло — потому что снаружи это не два разных \
наблюдения одного факта, а буквально ОДНО наблюдение двух разных фактов."
    );

    println!(
        "\nВывод раздела 2: `RuleOutcome` — это НЕ вариант того же рецепта \"записать и проиграть назад\", который \
работает для `feedback` и для `NeighborType`. Разница не в реализации, а в типе наблюдаемого значения: \
`NeighborType` ПО ОПРЕДЕЛЕНИЮ равен чтению видимой решётки (см. код в `engine/mod.rs`: `grid.get_cell(nx, \
ny)`), так что его значение НИКОГДА не может разойтись с внешней историей. `RuleOutcome` — это исход \
арбитража, отдельная бухгалтерия, которая обязана иметь ВИДИМЫЙ эффект, ТОЛЬКО ЕСЛИ `changes`/`shifts` самого \
правила не idempotent — а идемпотентность (или, шире, любое совпадение эффекта победителя с эффектом \
гипотетического альтернативного победителя) — это свойство КОНКРЕТНОГО набора правил, не гарантированное в \
общем случае. Открытый вопрос (не проверено здесь): если `changes` заведомо НЕ idempotent и НЕТ конкурента, \
воспроизводящего тот же видимый эффект, `RuleOutcome` вырождается в тот же случай, что `NeighborType` (Applied \
== видимый эффект появился) — раздел 2 показывает не \"RuleOutcome всегда непочинимо\", а что рецепт \"читать \
дельты решётки\" НЕ ЯВЛЯЕТСЯ ОБЩИМ решением для этого триггера, в отличие от `NeighborType`, для которого он \
общее решение по построению."
    );
}

fn main() {
    section1();
    section2a();
    section2b();
}
