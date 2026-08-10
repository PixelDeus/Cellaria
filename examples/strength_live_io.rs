//! Пятая сила: работающая симуляция — это не "запустил и жду один ответ",
//! а живой конвейер с портами ввода/вывода. Пока Engine тикает, в него можно
//! непрерывно подавать внешние данные (`push_input`/`apply_input`) и снимать
//! результаты (`pop_output`) — как с работающим устройством, а не с разовым
//! батч-вычислением. Большинство простых CA-движков — замкнутая система:
//! задал начальное состояние, прогнал N тиков, прочитал финал. Здесь ввод и
//! вывод — часть самого процесса, а не то, что было только "до" и "после".

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{BoundaryBuffer, CellType, Direction, OverflowAction, Rule, ShiftSpec};
use cellaria::{Grid, VecStorage};

const PACKET: u8 = 5;

fn main() {
    // Провод шириной 7: x=0 — вход, x=6 — последняя клетка (она же выходная
    // граница). Пакет едет вправо по одной клетке за тик; когда он пытается
    // сдвинуться ЗА пределы решётки (с x=6 на x=7), это и есть overflow —
    // `OverflowAction::Write` кладёт его в очередь выходного буфера вместо
    // того, чтобы просто потерять.
    let storage = VecStorage::new(7, 1);
    let mut grid = Grid::new(storage, Default::default());

    let mut input_buf = BoundaryBuffer::new();
    input_buf.direction = "input".to_string();
    grid.set_boundary(0, 0, input_buf);

    let mut output_buf = BoundaryBuffer::new();
    output_buf.direction = "output".to_string();
    grid.set_boundary(6, 0, output_buf);

    let rule = Rule {
        id: vec![CellType(PACKET)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
        changes: vec![],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: OverflowAction::Write(0), // 0 = пронести исходное значение как есть
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
    };
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(PACKET), vec![rule]);

    let mut engine = Engine::new(grid, rule_index);

    // Пакеты приходят НЕРЕГУЛЯРНО, по мере готовности — как реальный внешний
    // поток данных, а не заранее известный список начальных ячеек.
    let arrivals: [u32; 3] = [0, 3, 7]; // тики отправки

    println!("Живой конвейер: вход (x=0) -> провод (7 клеток) -> выход.\n");
    for tick in 0..14u32 {
        if arrivals.contains(&tick) {
            engine.push_input(0, PACKET);
            println!("тик {:>2}: внешний пакет пришёл и лёг во входную очередь", tick);
        }
        engine.apply_input();
        engine.run_tick();
        for (_ch, cell) in engine.pop_output() {
            println!("тик {:>2}: на выходе появился пакет: {}", tick, cell.value.0 .0);
        }
    }

    println!(
        "\nСимуляция всё это время продолжала тикать и обрабатывать состояние решётки —\n\
         ввод/вывод шли параллельно с вычислением, а не до и после него."
    );
}
