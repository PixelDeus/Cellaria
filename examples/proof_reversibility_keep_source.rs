//! Обратимость + `ShiftSpec.keep_source`: держится ли Теорема 9
//! (paper2.md §7.3) для правила-"излучения" (`keep_source: true` — копия в
//! конечную точку БЕЗ очистки source, см. `ShiftSpec`'s doc-комментарий)?
//!
//! В отличие от `feedback`/`memory` (проверено в
//! `proof_reversibility_feedback.rs`), `keep_source` НЕ заводит никакого
//! нового состояния в `Engine` — это чисто поситочная трансформация
//! решётки, тик остаётся честной функцией `config_t → config_{t+1}`. Вопрос
//! здесь другой: тот же самый РЕЦЕПТ построения `R⁻¹`, что работает для
//! обычного сдвига (Теорема 9: "развернуть направление каждого сдвига"),
//! ВООБЩЕ ЛИ применим к копированию? Копирование и перемещение — это не
//! взаимно обратные операции: обратное к "переместить" — это "переместить
//! назад" (именно это доказывает Теорема 9), а обратное к "скопировать" —
//! это "стереть копию", а НЕ "скопировать в обратную сторону". У операции
//! "скопировать в обратную сторону" нет никаких оснований быть обратной к
//! "скопировать вперёд" — это просто ДРУГАЯ операция того же рода.
//!
//! Дополнительная, более серьёзная проблема: обычный сдвиг ОЧИЩАЕТ source
//! после хода — токен физически СУЩЕСТВУЕТ ровно в одном месте всегда,
//! поэтому у результирующей клетки ровно ОДИН кандидат-предок. `keep_source`
//! этого не делает — после "излучения" ДВЕ клетки (source и dest) имеют
//! ОДИНАКОВЫЙ тип, то есть ОБЕ независимо совпадают с ЭТИМ ЖЕ правилом на
//! следующем тике. Наивный `R⁻¹` (направление развёрнуто, `keep_source`
//! сохранён) не "отматывает одно излучение назад" — он излучает СНОВА, из
//! ОБЕИХ копий одновременно, порождая цепную реакцию вместо инверсии.
//!
//! Ниже — конструктивная проверка (не рассуждение): один "излучатель",
//! один тик вперёд, наивный `R⁻¹`, один тик назад — и посимвольное сравнение
//! с исходной решёткой.

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Direction, Rule, ShiftSpec};
use cellaria::{ConflictGraph, Grid, VecStorage};

const WIDTH: usize = 20;
const SRC: u8 = 50;
const SOURCE_X: usize = 5;
const STEPS: u16 = 3; // цель излучения: SOURCE_X + STEPS = 8

fn reverse_direction(d: Direction) -> Direction {
    match d {
        Direction::Up => Direction::Down,
        Direction::Down => Direction::Up,
        Direction::Left => Direction::Right,
        Direction::Right => Direction::Left,
    }
}

fn emit_rule(direction: Direction) -> HashMap<CellType, Vec<Rule>> {
    let mut idx = HashMap::new();
    idx.insert(
        CellType(SRC),
        vec![Rule {
            id: vec![CellType(SRC)],
            pattern: vec![],
            // Точечное излучение: keep_source=true, broadcast=false — копия
            // ТОЛЬКО в конечную точку (source+direction*steps), а не по
            // всему пути. `ShiftSpec::emit` — это broadcast=true вариант
            // ("излучение по пути"), здесь он не нужен и запутал бы вывод.
            shifts: vec![vec![ShiftSpec { direction, steps: STEPS, broadcast: false, keep_source: true }]],
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
        }],
    );
    idx
}

fn build_grid(src_x: usize) -> Grid<VecStorage> {
    let mut grid = Grid::new(VecStorage::new(WIDTH, 1), Default::default());
    grid.set_cell(src_x, 0, Cell { value: CellValue(CellType(SRC)), born_at: 0 });
    grid
}

fn grid_from_snapshot(snap: &[u8]) -> Grid<VecStorage> {
    let mut g = Grid::new(VecStorage::new(WIDTH, 1), Default::default());
    for (x, &v) in snap.iter().enumerate() {
        if v != 0 {
            g.set_cell(x, 0, Cell { value: CellValue(CellType(v)), born_at: 0 });
        }
    }
    g
}

fn snapshot(engine: &Engine<VecStorage>) -> Vec<u8> {
    (0..WIDTH).map(|x| engine.grid().get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(0)).collect()
}

fn main() {
    let initial_snapshot: Vec<u8> = {
        let g = build_grid(SOURCE_X);
        (0..WIDTH).map(|x| g.get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(0)).collect()
    };
    println!("Исходная решётка:  {:?}", initial_snapshot);
    println!("Излучатель на x={SOURCE_X}, `ShiftSpec::emit(Right, {STEPS})` -> копия на x={}.", SOURCE_X + STEPS as usize);

    // ── Прямой прогон: ОДИН тик ─────────────────────────────────────────
    let mut forward = Engine::new(build_grid(SOURCE_X), emit_rule(Direction::Right));
    forward.run_tick();
    let final_snapshot = snapshot(&forward);
    println!("\nПосле 1 тика вперёд: {:?}", final_snapshot);
    assert_eq!(final_snapshot[SOURCE_X], SRC, "source обязан остаться на месте — keep_source не трогает исходную клетку");
    assert_eq!(final_snapshot[SOURCE_X + STEPS as usize], SRC, "копия обязана появиться в конечной точке");
    println!(
        "Обе клетки (x={SOURCE_X} и x={}) теперь одного типа {SRC} — обе НЕЗАВИСИМО совпадают с тем же правилом на следующем тике.",
        SOURCE_X + STEPS as usize
    );

    // Побочная, но важная проверка ДО реверса, сделанная СРАВНЕНИЕМ, а не
    // предположением: у ПРАВИЛА (pattern=[], значит формально совпадает с
    // ЛЮБОЙ клеткой типа SRC, включая гипотетическую вторую) граф
    // конфликтов "с самим собой" (два МАТЧА одного правила на разных
    // смещениях) НЕ пуст — но, как показывает сравнение ниже, это ОДИНАКОВО
    // верно и для ОБЫЧНОГО сдвига (`keep_source=false`) с теми же
    // параметрами: статический граф консервативен и не знает, что в
    // реальности на решётке когда-либо будет больше одной клетки этого типа
    // — сам по себе самоконфликт в графе НЕ является тем, что отличает
    // `keep_source` от обычного сдвига (оба "самоконфликтны" на бумаге).
    let emit_self_conflict = ConflictGraph::build(&emit_rule(Direction::Right)[&CellType(SRC)]);
    let mut move_rule = emit_rule(Direction::Right);
    move_rule.get_mut(&CellType(SRC)).expect("emit_rule всегда содержит запись для CellType(SRC)")[0].shifts[0][0].keep_source = false;
    let move_self_conflict = ConflictGraph::build(&move_rule[&CellType(SRC)]);
    println!(
        "\nСтатический самоконфликт правила: keep_source=true -> {} рёбер; keep_source=false (обычный сдвиг, те же STEPS) -> {} рёбер \
— ОДИНАКОВО не пусто в обоих случаях, значит дело не в самом факте самоконфликта на бумаге.",
        emit_self_conflict.edges.len(),
        move_self_conflict.edges.len()
    );
    println!(
        "Настоящая разница: обычный сдвиг СОХРАНЯЕТ ровно одну клетку типа SRC на решётке всегда (source \
очищается), поэтому теоретическая возможность самоконфликта НИКОГДА не реализуется на практике (некому \
конкурировать — ровно один токен, см. `proof_reversibility.rs`, где та же форма правила работает 20 тиков без сбоев). \
`keep_source` — ЕДИНСТВЕННЫЙ механизм, который увеличивает популяцию клеток этого типа (1 -> 2 после первого \
излучения), и именно это превращает статическую возможность в реальное столкновение при реверсе, проверено ниже."
    );

    // ── Наивный R⁻¹: направление развёрнуто (тот же приём, что и в
    // proof_reversibility.rs), keep_source сохранён как есть ──────────────
    let mut reverse = Engine::new(grid_from_snapshot(&final_snapshot), emit_rule(reverse_direction(Direction::Right)));
    reverse.run_tick();
    let recovered_snapshot = snapshot(&reverse);
    println!("\nПосле 1 тика наивного реверса: {:?}", recovered_snapshot);

    let occupied: Vec<usize> = recovered_snapshot.iter().enumerate().filter(|&(_, &v)| v != 0).map(|(x, _)| x).collect();
    println!("Занятые клетки после реверса: {:?} (ожидалось ровно [{SOURCE_X}] — исходная решётка)", occupied);
    println!(
        "x={} (куда должна была прийти обратная копия от x={SOURCE_X}) остался пуст: {} — обе копии (x={SOURCE_X} и x={}) \
теперь ОДНОГО типа и НЕЗАВИСИМО претендуют на клетку x={SOURCE_X} с перекрывающимися консервативными зонами \
(bbox правила = {{0,{STEPS}}}, самоперекрытие ровно на смещении {STEPS} — правило самоконфликтно именно ДЛЯ ЭТОЙ \
решётки с двумя копиями), арбитраж принимает только ОДНУ из двух заявок и отбрасывает вторую — по факту \
ходом x={SOURCE_X}->x={} вообще никто не сходил.",
        SOURCE_X as i32 - STEPS as i32,
        recovered_snapshot[(SOURCE_X as i32 - STEPS as i32) as usize] == 0,
        SOURCE_X + STEPS as usize,
        SOURCE_X as i32 - STEPS as i32
    );
    println!(
        "\nВосстановленная решётка совпадает с исходной клетка в клетку: {}",
        if recovered_snapshot == initial_snapshot { "ДА" } else { "НЕТ" }
    );
    assert_ne!(
        recovered_snapshot, initial_snapshot,
        "если побитовое совпадение ВСЁ-ТАКИ произошло, вывод примера неверен — нужно пересмотреть конструкцию"
    );
    assert_eq!(
        occupied, vec![SOURCE_X, SOURCE_X + STEPS as usize],
        "ожидалось: обе копии остались НА МЕСТЕ (x={SOURCE_X} и x={}), обратная копия на x={} НЕ появилась — \
арбитраж отбросил один из двух конкурирующих матчей",
        SOURCE_X + STEPS as usize,
        SOURCE_X as i32 - STEPS as i32
    );

    println!(
        "\nВывод (наивный R⁻¹): рецепт Теоремы 9 (\"развернуть направление каждого сдвига\") НЕ подходит \
для `keep_source` — \"скопировать\" и \"переместить\" не взаимно обратные операции. Обратное к перемещению — \
переместить назад. Обратное к копированию — СТЕРЕТЬ копию, а НЕ скопировать в другую сторону. Это не значит \
\"обратимость невозможна\" — значит только, что рецепт должен быть ДРУГИМ."
    );

    // ── А если не наивно? "Правило-ластик" вместо симметричного сдвига ─────
    //
    // Обратное к копированию — стереть копию. Это НЕ новый примитив: обычное
    // правило БЕЗ сдвигов, чей pattern читает ОБЕ клетки пары (источник на
    // (0,0) и копию на смещении (STEPS,0) относительно источника) и chьи
    // changes очищают ТОЛЬКО копию — уже полностью выражимо существующими
    // примитивами. Ключевое отличие от наивного R⁻¹: этот rule матчится
    // ТОЛЬКО там, где пара "источник+копия" реально существует (копия САМА
    // не имеет своего собственного соседа на +STEPS дальше — грид дозаполнен
    // нулями), так что никакого самоконфликта между двумя копиями тут нет:
    // ластик — не сдвиговое правило, у него нет второго независимого матча.
    fn eraser_rule() -> HashMap<CellType, Vec<Rule>> {
        let mut idx = HashMap::new();
        idx.insert(
            CellType(SRC),
            vec![Rule {
                id: vec![CellType(SRC)],
                pattern: vec![(0, 0, CellType(SRC)), (STEPS as i8, 0, CellType(SRC))],
                shifts: vec![],
                changes: vec![(STEPS as i32, 0, ChangeValue::Literal(0))],
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
            }],
        );
        idx
    }

    let mut smart_reverse = Engine::new(grid_from_snapshot(&final_snapshot), eraser_rule());
    smart_reverse.run_tick();
    let smart_recovered = snapshot(&smart_reverse);
    println!("\nПосле 1 тика УМНОГО реверса (ластик, без сдвигов): {:?}", smart_recovered);
    println!(
        "Восстановленная решётка совпадает с исходной клетка в клетку: {}",
        if smart_recovered == initial_snapshot { "ДА" } else { "НЕТ" }
    );
    assert_eq!(
        smart_recovered, initial_snapshot,
        "ластик ОБЯЗАН точно восстановить исходную решётку -- если нет, конструкция ластика неверна"
    );

    println!(
        "\nИтоговый вывод: `keep_source` НЕСОВМЕСТИМ конкретно с рецептом Теоремы 9 \
(\"R⁻¹ = развернуть направление каждого сдвига\"), но это ограничение РЕЦЕПТА, не модели. \
Для этого сценария (одна эмиссия, копия сама не каскадирует дальше) обратимость ДЕРЖИТСЯ при другом \
рецепте: R⁻¹ для keep_source-правила — не симметричный сдвиг, а `pattern`-based \"ластик\", читающий \
пару источник+копия и очищающий только копию. Это уже сегодня выразимо существующими примитивами \
(pattern+changes, без единого нового поля движка) -- ПРОВЕРЕНО выше конструктивно, не предположено. \
Открытый вопрос (не проверен здесь): каскадный случай, где копия сама становится независимым источником \
и эмитит ДАЛЬШЕ (см. `test_broadcast_shift_fills_entire_path`-подобные сценарии, `broadcast`+`keep_source` \
на N>1 шагов) -- там ластик тоже потребовался бы, но на КАЖДОМ уровне каскада, в порядке, обратном тому, \
как каскад рос; правдоподобно, что это тоже строится (ластик просто мог бы сам каскадировать назад по \
тому же принципу), но это отдельная конструкция, не тот же однострочный ластик, что здесь."
    );
}
