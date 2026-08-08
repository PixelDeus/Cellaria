//! Расширение Теоремы 7 (paper2.md, "Bounded propagation") на случай, когда
//! набор правил САМ меняется во время работы (самомодификация). Исходная
//! теорема предполагает ОДНО фиксированное K (дальность правил) на всё
//! время t. Если K меняется — правило с БОЛЬШЕЙ дальностью устанавливается
//! на лету — старая граница 2*K_old*t перестаёт быть верной: с какого-то
//! момента фактическое распространение обгонит её.
//!
//! Обобщение простое и следует из ТОГО ЖЕ индукционного шага: граница —
//! это не 2*K*t, а 2*Σ K_i (сумма дальности АКТИВНОГО на каждый конкретный
//! тик набора правил, а не одна константа на всё время). Проверяем именно
//! это: наивная (старая) граница по итогу НАРУШАЕТСЯ, а честная (по сумме
//! за каждый тик) — нет, ни разу.
//!
//! Устройство: "фронт" (тип ACTIVE) шагает вправо на K клеток за тик, K
//! сначала 1, затем (прямая имитация вступившего в силу самомодификации
//! правила — сам механизм передачи уже трижды доказан отдельно) меняется на
//! 3 у ОБОИХ копий сразу — как и должно быть: правило одно и то же
//! (изменение набора правил — это не порча, а факт эволюции программы,
//! которую честная копия тоже обязана отразить).

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{Cell, CellType, CellValue, Direction, Rule, ShiftSpec};
use cellaria::{Grid, VecStorage};

const WIDTH: usize = 400;
const ACTIVE: u8 = 50;
const CORRUPT_X: usize = 100;
const SWITCH_TICK: i64 = 15; // самомодификация вступает в силу здесь
const TOTAL_TICKS: i64 = 60;

fn active_rule(steps: u16) -> HashMap<CellType, Vec<Rule>> {
    let mut idx = HashMap::new();
    idx.insert(CellType(ACTIVE), vec![Rule {
        id: vec![CellType(ACTIVE)], pattern: vec![],
        shifts: vec![vec![ShiftSpec::new(Direction::Right, steps)]],
        changes: vec![], active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
    }]);
    idx
}

fn build_grid() -> Grid<VecStorage> {
    Grid::new(VecStorage::new(WIDTH, 1), Default::default())
}

fn main() {
    let mut reference = Engine::new(build_grid(), active_rule(1));
    let mut corrupted = Engine::new(build_grid(), active_rule(1));

    println!("K=1 до тика {}, затем K=3 (самомодификация вступает в силу у ОБЕИХ копий).\n", SWITCH_TICK);
    println!(
        "{:>4} | {:>10} | {:>12} | {:>14} | {:>10}",
        "тик", "факт.радиус", "наивн. 2*K1*t", "честн. 2*ΣKi", "наивная OK?"
    );
    println!("{}", "-".repeat(62));

    let mut naive_ever_violated = false;
    let mut honest_ever_violated = false;
    let mut sum_k: i64 = 0;
    let k_initial: i64 = 1;

    for t in 1..=TOTAL_TICKS {
        reference.run_tick();
        if t == 1 {
            corrupted.grid_mut().set_cell(CORRUPT_X, 0, Cell { value: CellValue(CellType(ACTIVE)), born_at: 0 });
        }
        corrupted.run_tick();

        let k_t = if t <= SWITCH_TICK { 1 } else { 3 };
        sum_k += k_t;
        if t == SWITCH_TICK + 1 {
            // Самомодификация вступает в силу для СЛЕДУЮЩЕГО тика — меняем
            // правило в ОБЕИХ копиях (это факт эволюции программы, не порча).
            reference.rule_index = active_rule(3);
            reference.rebuild_rule_cache();
            corrupted.rule_index = active_rule(3);
            corrupted.rebuild_rule_cache();
        }

        let max_dist = (0..WIDTH)
            .filter(|&x| {
                reference.grid().get_cell(x, 0).map(|c| c.value.0 .0)
                    != corrupted.grid().get_cell(x, 0).map(|c| c.value.0 .0)
            })
            .map(|x| (x as i64 - CORRUPT_X as i64).abs())
            .max()
            .unwrap_or(0);

        let naive_budget = 2 * k_initial * t;
        let honest_budget = 2 * sum_k;
        let naive_ok = max_dist <= naive_budget;
        let honest_ok = max_dist <= honest_budget;
        if !naive_ok {
            naive_ever_violated = true;
        }
        if !honest_ok {
            honest_ever_violated = true;
        }

        println!(
            "{:>4} | {:>10} | {:>12} | {:>14} | {:>10}",
            t, max_dist, naive_budget, honest_budget, if naive_ok { "да" } else { "НАРУШЕНА" }
        );
    }

    println!(
        "\nНаивная граница (одно K на всё время) нарушена: {}",
        if naive_ever_violated { "ДА — как и предсказано, K выросло, а граница не учла это" } else { "нет" }
    );
    println!(
        "Честная граница (сумма K по каждому тику) нарушена: {}",
        if honest_ever_violated { "ДА (!) — теорема неверна, нужно разбираться" } else { "нет, ни разу — граница верна даже когда K меняется" }
    );
}
