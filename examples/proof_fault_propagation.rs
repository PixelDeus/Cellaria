//! Эмпирическая проверка Теоремы 7 из paper2.md ("Bounded propagation"):
//! после порчи ОДНОЙ клетки на решётке множество клеток, чьё значение может
//! отличаться от версии без порчи, не может выйти за пределы расстояния
//! 2*K*t от места порчи через t тиков, где K — дальность правил (здесь K=1,
//! у Wireworld-провода паттерн читает соседа на расстоянии 1).
//!
//! Схема: два идентичных движка на одной и той же неподвижной цепочке
//! "провода" (WIRE). В "эталонном" движке ничего не трогаем — он остаётся
//! неизменным навсегда (в проводе без головки нечему меняться). В "порченом"
//! движке на тике 1 вручную (это и есть внешняя порча, а не вычисление
//! движка) превращаем одну клетку провода в "головку" — оттуда сигнал
//! побежит по проводу. Каждый тик сравниваем оба движка клетка за клеткой и
//! замеряем максимальное расстояние от места порчи до любой отличающейся
//! клетки — и проверяем, что оно никогда не превышает 2*K*t.

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Rule};
use cellaria::{Grid, VecStorage};

const WIRE: u8 = 20;
const HEAD: u8 = 21;
const TAIL: u8 = 22;
const WIDTH: usize = 100;
const CORRUPT_X: usize = 50;
const K: i64 = 1; // паттерн провода читает соседа на расстоянии 1 — см. paper2.md §6.2, Определение 6

fn build_rule_index() -> HashMap<CellType, Vec<Rule>> {
    let rules = vec![
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
    ];
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for r in rules {
        idx.entry(r.id[0]).or_default().push(r);
    }
    idx
}

fn build_wire_grid() -> Grid<VecStorage> {
    let storage = VecStorage::new(WIDTH, 1);
    let mut grid = Grid::new(storage, Default::default());
    for x in 0..WIDTH {
        grid.set_cell(
            x,
            0,
            Cell {
                value: CellValue(CellType(WIRE)),
                born_at: 0,
            },
        );
    }
    grid
}

fn main() {
    let mut reference = Engine::new(build_wire_grid(), build_rule_index());
    let mut corrupted = Engine::new(build_wire_grid(), build_rule_index());

    println!(
        "Теорема 7: радиус расхождения после t тиков не должен превышать 2*K*t = {}*t\n",
        2 * K
    );
    println!(
        "{:>4} | {:>16} | {:>10} | {:>12}",
        "тик", "макс. расстояние", "бюджет 2Kt", "в пределах?"
    );
    println!("{}", "-".repeat(52));

    let mut bound_ever_violated = false;
    for t in 1..=30i64 {
        reference.run_tick();
        if t == 1 {
            // Порча: внешнее вмешательство, не решение движка.
            corrupted.grid_mut().set_cell(
                CORRUPT_X,
                0,
                Cell {
                    value: CellValue(CellType(HEAD)),
                    born_at: 0,
                },
            );
        }
        corrupted.run_tick();

        let max_dist = (0..WIDTH)
            .filter(|&x| {
                reference.grid().get_cell(x, 0).map(|c| c.value.0 .0)
                    != corrupted.grid().get_cell(x, 0).map(|c| c.value.0 .0)
            })
            .map(|x| (x as i64 - CORRUPT_X as i64).abs())
            .max();

        let budget = 2 * K * t;
        let within = max_dist.map_or(true, |d| d <= budget);
        if !within {
            bound_ever_violated = true;
        }
        println!(
            "{:>4} | {:>16} | {:>10} | {:>12}",
            t,
            max_dist.map_or("—".to_string(), |d| d.to_string()),
            budget,
            if within { "да" } else { "НАРУШЕНО" }
        );
    }

    println!(
        "\n{}",
        if bound_ever_violated {
            "Граница нарушена хотя бы раз — теорема или её доказательство требуют пересмотра."
        } else {
            "Граница 2*K*t выдержана на всех 30 тиках. Фактическое распространение (фронт \
             сигнала движется на 1 клетку/тик в ОДНУ сторону) заметно у́же доказанной границы — \
             граница консервативна, как и заявлено в тексте доказательства."
        }
    );
}
