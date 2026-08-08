//! Пункт 6 обсуждения ("приоритет по расстоянию"): сигнал сам находит
//! кратчайший путь к цели, потому что чем ближе к цели сосед, тем выше
//! приоритет сдвига в его сторону. НЕ требует CAM и НЕ требует новых полей
//! движка — расстояние кодируется как ТИП клетки (тот же приём, что и
//! везде в проекте), а "маршрутизация" — это просто N конкурирующих
//! правил сдвига с приоритетом = f(расстояние соседа), где обычный
//! арбитраж уже решает, какое направление ближе к цели.
//!
//! Устройство в два этапа:
//! 1. Волновая разметка (multi-source BFS одним источником — целью):
//!    клетка-цель стартует с расстоянием 0; каждая размеченная клетка
//!    расстояния d пишет d+1 в СВОЙ ещё неразмеченный сосед. Если два
//!    соседних источника претендуют на одну клетку в один тик — приоритет
//!    выше у МЕНЬШЕГО d (побеждает более короткий путь). Стены (WALL) —
//!    отдельный тип, никогда не размечаются и никогда не размечают дальше
//!    себя — разметка их огибает.
//! 2. Токен: 4 конкурирующих правила сдвига (по одному на сторону),
//!    приоритет = f(расстояние клетки, в которую сдвигается) — выше для
//!    меньшего расстояния. Токен каждый тик выбирает сторону с наименьшим
//!    расстоянием без единой строчки pathfinding-логики — это чистое
//!    следствие арбитража по приоритету.
//!
//! Проверка не на открытом поле (там "просто идти к цели по прямой" и без
//! разметки дал бы тот же результат, ничего не доказывая) — а со стеной
//! между стартом и целью, вынуждающей реальный обход. Число тиков токена
//! до цели сверяется с НЕЗАВИСИМО посчитанным (обычным BFS на Rust, не
//! через движок) кратчайшим путём с учётом стены.

use std::collections::{HashMap, VecDeque};

use cellaria::engine::Engine;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Direction, Rule, ShiftSpec};
use cellaria::{Grid, VecStorage};

const WIDTH: usize = 11;
const HEIGHT: usize = 11;
// Обход стены (вниз-вправо-вверх, по ~8 клеток на плечо) даёт путь длиннее
// простой манхэттенской суммы WIDTH+HEIGHT — 40 взято с явным запасом для
// этой конкретной сцены (реально нужно 24), не общая формула для любой сетки.
const MAX_DIST: u8 = 40;

const UNLABELED: u8 = 0;
const WALL: u8 = 50;
const TOKEN: u8 = 60;
const ARRIVED: u8 = 61;
const DIST_BASE: u8 = 100; // расстояние d кодируется как тип DIST_BASE+d, d = 0..MAX_DIST

const TARGET: (usize, usize) = (9, 1);
const START: (usize, usize) = (1, 1);
// Стена x=5, y=0..=8 — проход только через y=9 или y=10.
fn is_wall(x: usize, y: usize) -> bool {
    x == 5 && y <= 8
}

const DIRS: [(Direction, i8, i8); 4] =
    [(Direction::Up, 0, -1), (Direction::Down, 0, 1), (Direction::Left, -1, 0), (Direction::Right, 1, 0)];

fn dist_ct(d: u8) -> CellType {
    CellType(DIST_BASE + d)
}

fn build_rule_index() -> HashMap<CellType, Vec<Rule>> {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();

    // --- Волновая разметка: для каждого d=0..MAX_DIST-1 и каждой стороны,
    // клетка с расстоянием d пишет d+1 в соседа, если тот ещё UNLABELED.
    // Приоритет выше для МЕНЬШЕГО d — короткий путь побеждает при
    // одновременной конкуренции за одну клетку.
    for d in 0..MAX_DIST {
        let mut rules = Vec::new();
        for &(_, dx, dy) in &DIRS {
            rules.push(Rule {
                id: vec![dist_ct(d)],
                pattern: vec![(0, 0, dist_ct(d)), (dx, dy, CellType(UNLABELED))],
                shifts: vec![],
                changes: vec![(dx as i32, dy as i32, ChangeValue::Literal(DIST_BASE + d + 1))],
                active_only: false,
                priority: (MAX_DIST - d) as u32,
                min_age: 0,
                overflow: Default::default(),
                cam: None,
                tie_break: 0,
                starvation_after: None, feedback: None, recursion: None, memory: None,
            });
        }
        idx.insert(dist_ct(d), rules);
    }

    // --- Токен: 4 конкурирующих правила сдвига на каждое d, приоритет =
    // f(расстояние соседа, в которого сдвигаемся) — арбитраж сам выбирает
    // сторону с наименьшим расстоянием, без единой строчки pathfinding-кода.
    // Особый случай d=0 (сосед — сама цель): после сдвига `changes`
    // относительно ЦЕЛИ сдвига (см. doc-комментарий `Rule::changes`)
    // немедленно переписывает перенесённое значение TOKEN на ARRIVED — а
    // не отдельное правило, конфликтующее по (0,0) с TOKEN (клетка не
    // может одновременно быть TOKEN и DIST_BASE+0, это одно значение).
    let mut token_rules = Vec::new();
    for &(direction, dx, dy) in &DIRS {
        for d in 0..MAX_DIST {
            let changes = if d == 0 { vec![(0, 0, ChangeValue::Literal(ARRIVED))] } else { vec![] };
            token_rules.push(Rule {
                id: vec![CellType(TOKEN)],
                pattern: vec![(0, 0, CellType(TOKEN)), (dx, dy, dist_ct(d))],
                shifts: vec![vec![ShiftSpec::new(direction, 1)]],
                changes,
                active_only: false,
                priority: (MAX_DIST - d) as u32,
                min_age: 0,
                overflow: Default::default(),
                cam: None,
                tie_break: 0,
                starvation_after: None, feedback: None, recursion: None, memory: None,
            });
        }
    }
    idx.insert(CellType(TOKEN), token_rules);

    idx
}

/// Решётка БЕЗ токена — только стены и клетка-цель (d=0). Токен впрыскивается
/// отдельно, ПОСЛЕ того как волна полностью разметит поле (см. `main`) — иначе
/// он тронется, как только у него появится хотя бы один размеченный сосед,
/// не дожидаясь готового поля, и общее время станет "ожидание волны + путь",
/// а не чистый путь (тоже честный результат, но не тот, что здесь проверяется).
fn build_grid() -> Grid<VecStorage> {
    let mut grid = Grid::new(VecStorage::new(WIDTH, HEIGHT), Default::default());
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            if is_wall(x, y) {
                grid.set_cell(x, y, Cell { value: CellValue::new(WALL), born_at: 0 });
            }
        }
    }
    grid.set_cell(TARGET.0, TARGET.1, Cell { value: CellValue::new(DIST_BASE), born_at: 0 }); // d=0
    grid
}

/// Независимый (не через движок) BFS с учётом стены — эталон для сверки.
fn true_shortest_path_len() -> u32 {
    let mut dist = vec![vec![u32::MAX; WIDTH]; HEIGHT];
    let mut q = VecDeque::new();
    dist[TARGET.1][TARGET.0] = 0;
    q.push_back(TARGET);
    while let Some((x, y)) = q.pop_front() {
        let d = dist[y][x];
        for &(_, dx, dy) in &DIRS {
            let (nx, ny) = (x as i32 + dx as i32, y as i32 + dy as i32);
            if nx < 0 || ny < 0 || nx as usize >= WIDTH || ny as usize >= HEIGHT {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            if is_wall(nx, ny) || dist[ny][nx] != u32::MAX {
                continue;
            }
            dist[ny][nx] = d + 1;
            q.push_back((nx, ny));
        }
    }
    dist[START.1][START.0]
}

fn find_token(engine: &Engine<VecStorage>) -> Option<(usize, usize, u8)> {
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let v = engine.grid().get_cell(x, y).map(|c| c.value.0 .0).unwrap_or(0);
            if v == TOKEN || v == ARRIVED {
                return Some((x, y, v));
            }
        }
    }
    None
}

fn main() {
    let expected = true_shortest_path_len();
    println!("Независимо посчитанный кратчайший путь (BFS в обход стены): {expected} шагов");
    assert!(
        expected as usize > (TARGET.0 as i32 - START.0 as i32).unsigned_abs() as usize + (TARGET.1 as i32 - START.1 as i32).unsigned_abs() as usize,
        "стена должна реально удлинять путь относительно прямой манхэттенской дистанции — иначе сцена не проверяет обход"
    );

    let mut engine = Engine::new(build_grid(), build_rule_index());

    // Фаза 1: разметка. Токена на решётке ЕЩЁ НЕТ — измеряем только волну,
    // до полного покрытия достижимой области.
    for _ in 0..(MAX_DIST as u32) {
        engine.run_tick();
    }
    let start_label = engine.grid().get_cell(START.0, START.1).map(|c| c.value.0 .0).unwrap_or(0);
    assert_eq!(
        start_label as u32,
        DIST_BASE as u32 + expected,
        "клетка старта должна получить РОВНО ту дистанцию, что посчитал независимый BFS"
    );

    // Фаза 2: поле готово — впрыскиваем токен и даём ему идти, ведомым
    // ИСКЛЮЧИТЕЛЬНО приоритетом соседей (никакого pathfinding-кода).
    let gen = engine.grid().generation();
    engine.grid_mut().set_cell(START.0, START.1, Cell { value: CellValue::new(TOKEN), born_at: gen });

    let mut ticks = 0u32;
    let mut arrived_at = None;
    for _ in 1..=(expected + 5) {
        engine.run_tick();
        ticks += 1;
        if let Some((x, y, v)) = find_token(&engine) {
            if v == ARRIVED {
                arrived_at = Some((ticks, (x, y)));
                break;
            }
        }
    }

    let (ticks_taken, final_pos) = arrived_at.expect("токен обязан был достичь цели за отведённые тики");
    println!("Токен достиг цели за {ticks_taken} тиков (позиция {final_pos:?}), обходя стену чисто по приоритету соседей");
    assert_eq!(final_pos, TARGET, "финальная позиция обязана совпасть с целью");
    assert_eq!(
        ticks_taken as u64, expected as u64,
        "на готовом поле токен обязан пройти РОВНО кратчайший путь без единого лишнего тика"
    );

    println!(
        "\nВывод: сигнал нашёл кратчайший путь В ОБХОД стены, не имея ни единой строчки pathfinding-кода — \
только N конкурирующих правил сдвига с priority = f(расстояние соседа). Расстояние закодировано как ТИП \
клетки (обычный приём проекта), CAM не понадобился вообще. Маршрутизация — чистое следствие уже существующего \
арбитража по приоритету, примененного к волновой разметке."
    );
}
