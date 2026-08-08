//! Совместная самомодификация: если на одной решётке живут два независимых
//! модуля (как провод+распад в `big_world.rs`), и ОДИН из них умеет
//! переписывать себя — гарантирует ли это, что он никогда не сломает
//! ДРУГОЙ? Нет — сама по себе саморефлексия ничего не знает о соседях. Но
//! есть уже доказанная теорема композиции (`ConflictGraph::check_composition`,
//! paper2.md §4), и её можно использовать как ОХРАНУ прямо в момент
//! установки самопереданного правила: `Engine::enable_guarded_self_modification`
//! (добавлено в этой сессии) проверяет новое правило против ОСТАЛЬНОГО
//! текущего набора и отклоняет его, если оно доказуемо конфликтует.
//! Расширение своего же, ранее самопереданного id не проверяется (иначе
//! даже increment/finalize из части 2 саморефлексии стало бы невозможным —
//! статический анализ, будучи консервативным, видит их как "конфликт" из-за
//! `min_age`, хотя в реальности они не пересекаются); проверяются только id,
//! существовавшие ДО начала самомодификации — "чужая территория".
//!
//! Байты пакета внедряются НАПРЯМУЮ в очередь выходного буфера, а не через
//! физический "поезд" клеток-носителей (см. `strength_self_modification.rs`
//! и далее — механизм передачи уже трижды доказан, повторять его здесь не
//! нужно). Причина, а не лень: клетки-носители сами двигаются (`shifts`),
//! а анализатор конфликтов КОНСЕРВАТИВЕН именно к движущимся правилам
//! (цель сдвига не ограничена по типу — та же причина, по которой "провод +
//! челнок" помечается Unsafe в `big_world.rs`, хотя в реальности они не
//! пересекаются). Если бы носители сами были частью `rule_index`, ОХРАНА
//! видела бы конфликт с самой инфраструктурой доставки, а не с содержанием —
//! эта путаница ортогональна тому, что здесь проверяется.
//!
//! Сценарий: модуль A — распад (id=1 -> id=3, статичный, никогда не
//! меняется). Дважды напрямую подаём байты пакета `AddRule`:
//!   1. Безопасное правило (id=50, не пересекается ни с чем) — должно
//!      установиться.
//!   2. Опасное правило (id=1 — та же голова, что у модуля A) — должно быть
//!      отклонено, а модуль A остаться нетронутым.

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{BoundaryBuffer, Cell, CellType, CellValue, ChangeValue, Rule};
use cellaria::{Grid, VecStorage};

const RAD: u8 = 1; // модуль A: id, с которым столкнётся "опасная" посылка
const MID: u8 = 3;
const BOUNDARY_X: usize = 0;

fn build_engine() -> Engine<VecStorage> {
    let mut grid = Grid::new(VecStorage::new(1, 1), Default::default());
    let mut out = BoundaryBuffer::new();
    out.direction = "output".to_string();
    grid.set_boundary(BOUNDARY_X, 0, out);

    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    idx.insert(CellType(RAD), vec![Rule {
        id: vec![CellType(RAD)], pattern: vec![], shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(MID))],
        active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
    }]);

    let mut engine = Engine::new(grid, idx);
    engine.enable_guarded_self_modification();
    engine
}

/// Кладёт пакет `AddRule` с изменением ТОЙ ЖЕ клетки (dx=0, dy=0 — это и
/// есть то самое место, за которое в принципе может случиться конфликт с
/// уже существующим правилом на тот же id) сразу в очередь выходного
/// буфера, как будто его только что доставил какой-то (уже доказанный
/// отдельно) физический носитель. Пустое правило (без changes) никогда не
/// было бы отклонено — ему просто нечем конфликтовать.
fn inject_packet(engine: &mut Engine<VecStorage>, target_id: u8, new_value: u8) {
    let packet: [u8; 7] = [10, 1, target_id, 0, 0, new_value, 0xFF]; // priority, id_len, id_byte, dx=0, dy=0, value, terminator
    if let Some(buf) = engine.grid_mut().get_boundary_mut(BOUNDARY_X, 0) {
        for &b in &packet {
            buf.enqueue(0, Cell { value: CellValue(CellType(b)), born_at: 0 });
        }
    }
    engine.run_tick();
}

fn main() {
    let mut engine = build_engine();
    println!("Модуль A (распад, id={}) на месте. Проверка охраны совместной самомодификации.\n", RAD);

    inject_packet(&mut engine, 50, 77);
    println!(
        "После безопасной посылки (id=50): установлено? {} | отклонено охраной: {}",
        engine.rule_index.contains_key(&CellType(50)),
        engine.rejected_self_modifications
    );

    inject_packet(&mut engine, RAD, 99);
    let a_rule = &engine.rule_index[&CellType(RAD)];
    let a_intact = a_rule.len() == 1 && a_rule[0].changes == vec![(0, 0, ChangeValue::Literal(MID))];
    println!(
        "После опасной посылки (id={} — как у модуля A): отклонено охраной: {} | модуль A не тронут: {}",
        RAD, engine.rejected_self_modifications, a_intact
    );

    // Убеждаемся, что модуль A на самом деле продолжает работать, а не
    // просто "выглядит нетронутым" в rule_index.
    let mut check_grid = Grid::new(VecStorage::new(1, 1), Default::default());
    check_grid.set_cell(0, 0, Cell { value: CellValue(CellType(RAD)), born_at: 0 });
    let mut check_engine = Engine::new(check_grid, {
        let mut idx = HashMap::new();
        idx.insert(CellType(RAD), a_rule.clone());
        idx
    });
    check_engine.run_tick();
    println!(
        "\nМодуль A всё ещё превращает {} в {}: {}",
        RAD, MID,
        check_engine.grid().get_cell(0, 0).map(|c| c.value.0 .0) == Some(MID)
    );
}
