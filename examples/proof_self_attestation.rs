//! Самоаттестация: решётка сама вычисляет и передаёт наружу "отпечаток"
//! (контрольную сумму по модулю 7) СОБСТВЕННОГО состояния — не одного числа
//! (как в части 2 саморефлексии — там считали, сколько пришло импульсов), а
//! ПОСЛЕДОВАТЕЛЬНОСТИ клеток, то есть настоящего фрагмента своего состояния.
//! Использует ровно тот же канал (`RuleStore`/`OverflowAction::Write`), что
//! и вся предыдущая саморефлексия — только вместо нового ПРАВИЛА решётка
//! передаёт число, которое можно сверить с реальным содержимым.
//!
//! Устройство (два "трека" на решётке, чтобы сканер не портил то, что
//! читает): строка y=0 — данные, которые никогда не трогаются; строка y=1 —
//! маркер-сканер. Маркер несёт текущее накопленное значение как СВОЙ ТИП
//! (типы 200..206 = сумма 0..6), на каждом шаге читает клетку данных прямо
//! под собой (смещение (0,-1) в паттерне — паттерны читаются ДО любых
//! записей этого тика, так что это честное чтение "как было"), складывает
//! её значение со своим накопленным по модулю 7 и сдвигается вправо. Когда
//! данные заканчиваются — ни одно из правил сканирования больше не
//! подходит, маркер просто перестаёт двигаться (естественная остановка, та
//! же идея, что и "нет совпадений — конец", см. paper3.md §2). После паузы
//! (`min_age`, тот же приём, что и в вычисляемой саморефлексии части 2)
//! маркер превращается в готовое к передаче число и уезжает к выходу.
//!
//! Проверка "ловит ли подмену": прогоняем дважды — с честными данными и с
//! ОДНОЙ подменённой клеткой — и показываем, что переданная контрольная
//! сумма в обоих случаях РАЗНАЯ, то есть решётка реально что-то говорит о
//! своём состоянии, а не выдаёт константу.
//!
//! Честная оговорка: это НЕ криптографический хэш (сумма по модулю 7 — тривиально
//! подделываемая, коллизий море) — это демонстрация возможности "решётка
//! вычисляет и передаёт функцию от протяжённого куска своего состояния",
//! а не готовый криптографический примитив.

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{BoundaryBuffer, Cell, CellType, CellValue, ChangeValue, Direction, OverflowAction, Rule, ShiftSpec};
use cellaria::{Grid, VecStorage};

const WIDTH: usize = 60;
const ACC_MOD: u8 = 7;
const ACC_BASE: u8 = 200; // накопитель во время сканирования: типы 200..206
const FINAL_BASE: u8 = 220; // готовое к передаче число: типы 220..226
const QUIET_THRESHOLD: u64 = 15;
const MARKER_ROW: usize = 1;

fn build_rule_index() -> HashMap<CellType, Vec<Rule>> {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for acc in 0..ACC_MOD {
        let mut rules = Vec::new();
        // Сканирование: под маркером — клетка данных со значением d (1..=6);
        // сложить с накопленным по модулю, сдвинуться к следующей клетке.
        for d in 1u8..=6 {
            let next = (acc + d) % ACC_MOD;
            rules.push(Rule {
                id: vec![CellType(ACC_BASE + acc)],
                pattern: vec![(0, 0, CellType(ACC_BASE + acc)), (0, -1, CellType(d))],
                shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
                changes: vec![(0, 0, ChangeValue::Literal(ACC_BASE + next))],
                active_only: false, priority: 20, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
            });
        }
        // Данные закончились (под маркером — пусто, ни одно из правил выше
        // не подходит) — после паузы превратиться в готовое число.
        rules.push(Rule {
            id: vec![CellType(ACC_BASE + acc)],
            pattern: vec![(0, 0, CellType(ACC_BASE + acc))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(FINAL_BASE + acc))],
            active_only: false, priority: 10, min_age: QUIET_THRESHOLD, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
        });
        idx.insert(CellType(ACC_BASE + acc), rules);

        // Готовое число едет к выходу и несёт своё же значение.
        idx.insert(CellType(FINAL_BASE + acc), vec![Rule {
            id: vec![CellType(FINAL_BASE + acc)], pattern: vec![],
            shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
            changes: vec![], active_only: false, priority: 10, min_age: 0,
            overflow: OverflowAction::Write(0),
            cam: None,
            tie_break: 0,
            starvation_after: None, feedback: None, recursion: None, memory: None,
        }]);
    }
    idx
}

/// Прогоняет сканирование данных `data` (значения 1..=6) и возвращает
/// переданную контрольную сумму.
fn attest(data: &[u8]) -> u8 {
    let mut grid = Grid::new(VecStorage::new(WIDTH, 2), Default::default());
    let mut out = BoundaryBuffer::new();
    out.direction = "output".to_string();
    grid.set_boundary(WIDTH - 1, MARKER_ROW, out);

    for (x, &d) in data.iter().enumerate() {
        grid.set_cell(x, 0, Cell { value: CellValue(CellType(d)), born_at: 0 });
    }
    grid.set_cell(0, MARKER_ROW, Cell { value: CellValue(CellType(ACC_BASE)), born_at: 0 });

    let mut engine = Engine::new(grid, build_rule_index());
    for _ in 1..=(WIDTH as u32 * 2) {
        engine.run_tick();
        // Контрольная сумма — просто число на выходе, не операция протокола
        // AddRule, так что читаем очередь буфера напрямую (без RuleStore).
        if let Some(buf) = engine.grid().get_boundary(WIDTH - 1, MARKER_ROW) {
            if let Some(q) = buf.queues.get(&0) {
                if let Some(cell) = q.front() {
                    return cell.value.0 .0 - FINAL_BASE;
                }
            }
        }
    }
    panic!("контрольная сумма не пришла — см. диагностику");
}

fn main() {
    let honest: [u8; 6] = [3, 1, 4, 1, 5, 2];
    let mut tampered = honest;
    tampered[3] = 6; // одна клетка подменена: 1 -> 6

    let checksum_honest = attest(&honest);
    let checksum_tampered = attest(&tampered);

    println!("Честные данные:   {:?} -> контрольная сумма (mod 7): {}", honest, checksum_honest);
    println!("Подменённые данные: {:?} -> контрольная сумма (mod 7): {}", tampered, checksum_tampered);
    println!(
        "\nПодмена ОДНОЙ клетки изменила переданную сумму: {}",
        if checksum_honest != checksum_tampered { "ДА — обнаружено" } else { "НЕТ (!)" }
    );
}
