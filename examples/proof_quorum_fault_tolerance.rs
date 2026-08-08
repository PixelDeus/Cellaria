//! Кворумный арбитраж (TMR — triple modular redundancy) без единой правки
//! движка: сама идея опирается ИСКЛЮЧИТЕЛЬНО на уже доказанный детерминизм
//! (Theorem 1, paper2.md) — три независимые реплики ОДНОЙ и той же
//! симуляции, запущенные с одного начального состояния и одних правил,
//! ОБЯЗАНЫ давать побитово идентичный результат на каждом тике, если
//! железо исправно. Значит расхождение хотя бы одной реплики с двумя
//! другими — само по себе доказательство сбоя, без вероятностных
//! допущений, которые нужны для голосования в недетерминированных
//! системах.
//!
//! Демонстрация: 3 реплики в лок-степе → на выбранном тике одна реплика
//! искусственно "портится" (имитация cosmic ray / битового сбоя в
//! памяти — прямая правка клетки в обход `run_tick`, не через правило,
//! ровно как выглядел бы реальный аппаратный сбой) → голосование по
//! большинству находит испорченную реплику → её состояние ПОЛНОСТЬЮ
//! перезаписывается состоянием большинства → дальнейшие тики подтверждают,
//! что исцелённая реплика снова синхронна, не просто временно похожа.

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Direction, Rule, ShiftSpec};
use cellaria::{Grid, VecStorage};

const WIDTH: usize = 20;
const TICKS: u32 = 12;
const FAULT_TICK: u32 = 5;
const FAULT_REPLICA: usize = 1; // средняя из трёх — без потери общности

fn build_rules() -> HashMap<CellType, Vec<Rule>> {
    // Простой, но не тривиальный сценарий: движущийся токен + счётчик,
    // меняющийся каждый тик — реальная эволюция состояния, не статика.
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    idx.insert(CellType(1), vec![Rule {
        id: vec![CellType(1)], pattern: vec![], shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
        changes: vec![], active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
    }]);
    for s in 0..6u8 {
        idx.insert(CellType(10 + s), vec![Rule {
            id: vec![CellType(10 + s)], pattern: vec![], shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(10 + (s + 1) % 6))],
            active_only: false, priority: 5, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
        }]);
    }
    idx
}

fn build_initial_grid() -> Grid<VecStorage> {
    let mut grid = Grid::new(VecStorage::new(WIDTH, 1), Default::default());
    grid.set_cell(0, 0, Cell { value: CellValue::new(1), born_at: 0 });
    grid.set_cell(10, 0, Cell { value: CellValue::new(10), born_at: 0 });
    grid
}

fn snapshot(engine: &Engine<VecStorage>) -> Vec<Cell> {
    (0..WIDTH).map(|x| engine.grid().get_cell(x, 0).copied().unwrap_or_default()).collect()
}

/// Голосование по большинству, клетка за клеткой: для каждой позиции —
/// значение, встретившееся у ≥2 реплик из 3-х. Возвращает (majority,
/// индексы реплик, разошедшихся хоть в одной клетке).
fn vote(snapshots: &[Vec<Cell>; 3]) -> (Vec<Cell>, Vec<usize>) {
    let mut majority = Vec::with_capacity(WIDTH);
    let mut faulty: Vec<usize> = Vec::new();
    for (x, ((&a, &b), &c)) in snapshots[0].iter().zip(snapshots[1].iter()).zip(snapshots[2].iter()).enumerate() {
        let winner = if a == b {
            if c != a && !faulty.contains(&2) { faulty.push(2); }
            a
        } else if a == c {
            if !faulty.contains(&1) { faulty.push(1); }
            a
        } else if b == c {
            if !faulty.contains(&0) { faulty.push(0); }
            b
        } else {
            panic!("все три реплики разошлись в клетке x={x} — не сбой одной, а фундаментальная поломка детерминизма");
        };
        majority.push(winner);
    }
    (majority, faulty)
}

fn main() {
    let rules = build_rules();
    let mut replicas: Vec<Engine<VecStorage>> =
        (0..3).map(|_| Engine::new(build_initial_grid(), rules.clone())).collect();

    let mut repaired = false;
    for tick in 1..=TICKS {
        for r in &mut replicas {
            r.run_tick();
        }

        if tick == FAULT_TICK {
            // Имитация аппаратного сбоя: прямая правка клетки В ОБХОД
            // run_tick (не через правило — ровно так выглядел бы реальный
            // bit-flip в памяти, не логическая ошибка вычисления).
            replicas[FAULT_REPLICA].grid_mut().set_cell(3, 0, Cell { value: CellValue::new(99), born_at: 0 });
            println!("[тик {tick}] искусственный сбой внесён в реплику {FAULT_REPLICA} (клетка x=3 -> 99)");
        }

        let snaps: [Vec<Cell>; 3] = [snapshot(&replicas[0]), snapshot(&replicas[1]), snapshot(&replicas[2])];
        let (majority, faulty) = vote(&snaps);

        if faulty.is_empty() {
            println!("[тик {tick:>2}] все 3 реплики согласны ✓");
        } else {
            println!("[тик {tick:>2}] РАСХОЖДЕНИЕ: реплика(и) {faulty:?} не совпадает с большинством — исправляю");
            for &idx in &faulty {
                for (x, &cell) in majority.iter().enumerate() {
                    replicas[idx].grid_mut().set_cell(x, 0, cell);
                }
            }
            assert_eq!(faulty, vec![FAULT_REPLICA], "голосование должно было указать именно на искусственно испорченную реплику");
            repaired = true;
        }
    }

    assert!(repaired, "сценарий должен был хотя бы раз обнаружить и исправить сбой — иначе тест ничего не проверяет");

    // Финальная проверка: после исцеления реплики остаются синхронны
    // (не просто временно совпали в момент ремонта, а продолжают идти
    // в ногу дальше, поскольку тик — чистая функция состояния).
    let final_snaps: [Vec<Cell>; 3] = [snapshot(&replicas[0]), snapshot(&replicas[1]), snapshot(&replicas[2])];
    assert_eq!(final_snaps[0], final_snaps[1], "после исцеления реплики должны совпадать побитово");
    assert_eq!(final_snaps[1], final_snaps[2], "после исцеления реплики должны совпадать побитово");

    println!(
        "\nВывод: детерминизм даёт обнаружение И исправление сбоя без единой строчки нового кода в движке — \
голосование чисто внешнее (сравнение снапшотов), исправление — обычная запись через существующий \
`grid_mut().set_cell()`. Никакой вероятностной логики: 2-из-3 согласны ⇒ они правы, это доказано \
Theorem 1 (детерминизм арбитража), не предположение."
    );
}
