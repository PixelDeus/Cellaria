//! Обратимость + `Rule.recursion`: держится ли Теорема 9 (paper2.md §7.3),
//! если в наборе правил участвует правило с ограниченным intra-tick
//! каскадом (`Rule.recursion: Some(RecursionSpec)`)?
//!
//! Отличие от `feedback`/`memory` (см. `proof_reversibility_feedback.rs`):
//! `recursion` НЕ заводит вообще никакого нового `Engine`-состояния — весь
//! каскад целиком живёт внутри ОДНОГО вызова `apply_rule_buffered`, тик
//! остаётся честной функцией ОДНОЙ решётки (см. её doc-комментарий в
//! `types.rs`). Значит формальная причина, ломающая обратимость `feedback`
//! (скрытое состояние вне решётки), тут отсутствует ИЗНАЧАЛЬНО.
//!
//! Но у `recursion` нет и "наивного рецепта" Теоремы 9 в принципе:
//! правило с `recursion` ОБЯЗАНО иметь ПУСТОЙ `shifts` (см. валидацию в
//! `config::load_config`) — каскад расширяет `changes` вдоль направления, а
//! не двигает голову, так что "развернуть направление сдвига" здесь просто
//! неприменимо (сдвига нет). Каскад — это операция ЗАЛИВКИ (несколько
//! клеток одним тиком становятся FILLED), а не перемещения — вопрос в том,
//! есть ли у заливки СВОЙ собственный, отдельно построенный обратный
//! рецепт (по аналогии с "ластиком" для `keep_source`), а не в том,
//! ломает ли она уже готовый.
//!
//! Ответ здесь — конструктивный и прямой: заливка вдоль линии обратима
//! ОБЫЧНЫМ (без recursion) правилом, которое стирает КАЖДУЮ клетку, чей
//! ЛЕВЫЙ сосед тоже залит — все клетки каскада стирают друг друга
//! ПАРАЛЛЕЛЬНО за ОДИН тик (детект читает pre-tick срез, так что вся цепочка
//! видна целиком сразу), потому что каскад `recursion` сам по себе уже
//! ОДНОТИКОВАЯ операция — обратная ей операция тоже укладывается в один тик,
//! без каскада вообще.

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Direction, RecursionSpec, Rule};
use cellaria::{Grid, VecStorage};

const WIDTH: usize = 10;
const FILLED: u8 = 80;
const UNFILLED: u8 = 81;
const MAX_DEPTH: u8 = 3;

fn cascade_rules() -> HashMap<CellType, Vec<Rule>> {
    let mut idx = HashMap::new();
    idx.insert(
        CellType(UNFILLED),
        vec![Rule {
            id: vec![CellType(UNFILLED)],
            pattern: vec![(0, 0, CellType(UNFILLED)), (-1, 0, CellType(FILLED))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(FILLED))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: Some(RecursionSpec {
                max_depth: MAX_DEPTH,
                direction: Direction::Right,
            }),
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        }],
    );
    idx
}

/// Обратное правило: НЕ recursion, обычное правило без каскада. Стирает
/// клетку, если она FILLED и её левый сосед ТОЖЕ FILLED — это условие верно
/// ОДНОВРЕМЕННО для ВСЕХ клеток каскада (позиции 1,2,3 относительно затравки
/// на 0), потому что детект читает pre-tick состояние целиком, ДО того как
/// хоть одна из них стёрлась — все они стирают себя параллельно за один
/// тик, а не по цепочке. Затравка (позиция 0) НЕ матчится: её левый сосед
/// вне решётки (default-тип, не FILLED).
fn eraser_rules() -> HashMap<CellType, Vec<Rule>> {
    let mut idx = HashMap::new();
    idx.insert(
        CellType(FILLED),
        vec![Rule {
            id: vec![CellType(FILLED)],
            pattern: vec![(0, 0, CellType(FILLED)), (-1, 0, CellType(FILLED))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(UNFILLED))],
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
    idx
}

fn build_grid() -> Grid<VecStorage> {
    let mut grid = Grid::new(VecStorage::new(WIDTH, 1), Default::default());
    grid.set_cell(
        0,
        0,
        Cell {
            value: CellValue(CellType(FILLED)),
            born_at: 0,
        },
    );
    for x in 1..WIDTH {
        grid.set_cell(
            x,
            0,
            Cell {
                value: CellValue(CellType(UNFILLED)),
                born_at: 0,
            },
        );
    }
    grid
}

fn grid_from_snapshot(snap: &[u8]) -> Grid<VecStorage> {
    let mut g = Grid::new(VecStorage::new(WIDTH, 1), Default::default());
    for (x, &v) in snap.iter().enumerate() {
        g.set_cell(
            x,
            0,
            Cell {
                value: CellValue(CellType(v)),
                born_at: 0,
            },
        );
    }
    g
}

fn snapshot(engine: &Engine<VecStorage>) -> Vec<u8> {
    (0..WIDTH)
        .map(|x| engine.grid().get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(UNFILLED))
        .collect()
}

fn main() {
    let initial_snapshot: Vec<u8> = {
        let g = build_grid();
        (0..WIDTH)
            .map(|x| g.get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(UNFILLED))
            .collect()
    };
    println!("Исходная решётка:  {:?}", initial_snapshot);
    println!("Затравка на x=0, `recursion` (max_depth={MAX_DEPTH}, Right) -- базовый матч + каскад заливают x=1..={} за ОДИН тик.", MAX_DEPTH as usize + 1);

    // ── Прямой прогон: ОДИН тик, каскад заливает 3 клетки сразу ─────────
    let mut forward = Engine::new(build_grid(), cascade_rules());
    forward.run_tick();
    let final_snapshot = snapshot(&forward);
    println!("\nПосле 1 тика (каскад): {:?}", final_snapshot);
    // Базовый матч правила -- это x=1 (первая UNFILLED-клетка с FILLED
    // слева), НЕ затравка x=0 (та уже FILLED, паттерн требует UNFILLED в
    // (0,0)). Базовый матч заливает СЕБЯ (x=1), каскад k=1..=MAX_DEPTH
    // добавляет x=2..=MAX_DEPTH+1 -- вместе с нетронутой затравкой x=0
    // итого залито x=0..=MAX_DEPTH+1.
    let last_filled = MAX_DEPTH as usize + 1;
    for x in 0..=last_filled {
        assert_eq!(
            final_snapshot[x], FILLED,
            "клетка x={x} обязана быть залита (затравка или каскад) за один тик"
        );
    }
    for x in (last_filled + 1)..WIDTH {
        assert_eq!(
            final_snapshot[x], UNFILLED,
            "клетка x={x} НЕ должна быть затронута -- каскад ограничен max_depth"
        );
    }

    // ── Обратный прогон: ОБЫЧНОЕ (без recursion) правило-ластик, ОДИН тик ──
    let mut reverse = Engine::new(grid_from_snapshot(&final_snapshot), eraser_rules());
    reverse.run_tick();
    let recovered_snapshot = snapshot(&reverse);
    println!(
        "\nПосле 1 тика реверса (ластик, без recursion): {:?}",
        recovered_snapshot
    );
    println!(
        "Восстановленная решётка совпадает с исходной клетка в клетку: {}",
        if recovered_snapshot == initial_snapshot {
            "ДА"
        } else {
            "НЕТ"
        }
    );
    assert_eq!(
        recovered_snapshot, initial_snapshot,
        "ластик ОБЯЗАН точно восстановить исходную решётку за ОДИН тик -- если нет, конструкция неверна"
    );

    println!(
        "\nВывод: `Rule.recursion` СОВМЕСТИМ с обратимостью -- не автоматически по Теореме 9 (у recursion \
нет сдвига, значит нет и наивного рецепта \"развернуть направление\" в принципе), а через ОТДЕЛЬНО \
построенный обратный рецепт, специфичный для формы каскада: обычное (без recursion) правило, стирающее \
клетку, если она залита И её сосед со стороны затравки ТОЖЕ залит. Ключевое свойство, которое это позволяет: \
recursion каскадирует ВНУТРИ одного тика, и его natural inverse ТОЖЕ укладывается в один тик -- все клетки \
каскада стирают себя ПАРАЛЛЕЛЬНО (детект читает pre-tick срез целиком), а не по цепочке в обратном порядке, \
как потребовалось бы для feedback/keep_source. Открытый вопрос: этот рецепт специфичен для линейной заливки \
(каждая клетка проверяет РОВНО одного соседа со стороны затравки) -- не проверено, обобщается ли он на более \
сложные `changes` внутри каскада (например, если бы recursion писал не только (0,0), но и куда-то ещё \
относительно каждого уровня)."
    );
}
