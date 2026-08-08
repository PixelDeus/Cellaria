//! Сводная демонстрация: не изолированные примеры, а связная история о
//! мире, который безопасно управляет сам собой — используя ТОЛЬКО то, что
//! доказано и проверено в этой сессии (ничего нового здесь не строится,
//! только собирается вместе). Три действия:
//!
//!   Действие 1. Существующий модуль (распад) + "рабочий" регион, который
//!   САМ ВЫЧИСЛЯЕТ значение и передаёт себе новое правило (часть 2
//!   саморефлексии) — под ОХРАНОЙ композиции (Теорема 5, paper2.md §4.6):
//!   безопасная посылка устанавливается, опасная (претендующая на
//!   территорию существующего модуля) отклоняется, и существующий модуль
//!   доказанно продолжает работать как ни в чём не бывало.
//!
//!   Действие 2. Тот же принцип передачи используется не для НОВОГО
//!   ПРАВИЛА, а для ОТЧЁТА о собственном состоянии — самоаттестация
//!   (Теорема 8, paper3.md §4.6): решётка передаёт контрольную сумму куска
//!   своих данных, и подмена одной клетки заметно меняет то, что она
//!   сообщает о себе.
//!
//!   Действие 3. Независимая проверка: часть модели, которая устроена
//!   обратимо (Теорема 9, paper2.md §7) — прогнанная вперёд, а затем в
//!   буквально развёрнутых правилах назад, восстанавливает исходное
//!   состояние клетка в клетку, не приблизительно.
//!
//! Каждое действие использует свой Engine — не потому что они не могли бы
//! жить на одной решётке (первые два действия ровно это и показывают), а
//! потому что обратимость (действие 3) требует ПОЛНОЙ ЗАМЕНЫ набора
//! правил на обратный, что для честности не должно молчаливо задевать
//! действия 1–2.

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{
    BoundaryBuffer, Cell, CellType, CellValue, ChangeValue, Direction, Rule, ShiftSpec,
};
use cellaria::{Grid, VecStorage};

// ============================================================================
// Действие 1: существующий модуль + вычисляющий рабочий регион под охраной.
// ============================================================================

// Намеренно вне диапазонов, занятых инфраструктурой рабочего региона
// (1..MAX_COUNT-1 — носители посчитанного числа, 60 — импульс, 121..126 —
// носители фиксированных байт, 150..158 — состояния счётчика) — иначе
// регистрация инфраструктуры молча перезаписала бы правило существующего
// модуля ещё на этапе построения rule_index, до всякой симуляции.
const RAD: u8 = 200;
const MID: u8 = 201;
const COUNTER_BASE: u8 = 150; // рабочий регион: счётчик импульсов (часть 2 саморефлексии)
const MAX_COUNT: u8 = 9;
const QUIET_THRESHOLD: u64 = 20;
const SAFE_NEW_ID: u8 = 66; // id правила, которое рабочий регион передаст себе
// Фиксированные байты пакета (кроме "steps" — посчитанного числа) и их
// перевозчики — точно как в strength_self_modification_computed.rs.
const PACKET_FIXED: [(usize, u8); 6] = [
    (0, 10),           // priority
    (1, 1),            // id_len
    (2, SAFE_NEW_ID),  // id_byte
    (3, 0xFE),         // SHIFT_FLAG
    (4, 3),            // dir_byte = Right
    (6, 0xFF),         // terminator
];
const CARRIER_TYPES: [u8; 6] = [121, 122, 123, 124, 125, 126];
const WIDTH: usize = 200;
const COUNTER_X: usize = 50;
const PULSE: u8 = 60;

fn act1_rule_index() -> HashMap<CellType, Vec<Rule>> {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();

    // Существующий модуль — простой, статичный, никогда не меняется.
    idx.insert(CellType(RAD), vec![Rule {
        id: vec![CellType(RAD)], pattern: vec![], shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(MID))],
        active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
    }]);

    // Импульс, считающий счётчик (та же конструкция, что в
    // strength_self_modification_computed.rs).
    idx.insert(CellType(PULSE), vec![Rule {
        id: vec![CellType(PULSE)], pattern: vec![], shifts: vec![vec![ShiftSpec::new(Direction::Left, 1)]],
        changes: vec![], active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
    }]);
    for k in 0..MAX_COUNT {
        let counter_id = CellType(COUNTER_BASE + k);
        let mut rules = Vec::new();
        rules.push(Rule {
            id: vec![counter_id],
            pattern: vec![(0, 0, counter_id), (1, 0, CellType(PULSE))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(COUNTER_BASE + k + 1)), (1, 0, ChangeValue::Literal(0))],
            active_only: false, priority: 20, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
        });
        if k > 0 {
            rules.push(Rule {
                id: vec![counter_id], pattern: vec![(0, 0, counter_id)], shifts: vec![],
                changes: vec![(0, 0, ChangeValue::Literal(k))],
                active_only: false, priority: 10, min_age: QUIET_THRESHOLD, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
            });
        }
        idx.insert(counter_id, rules);
    }
    for k in 1..MAX_COUNT {
        idx.insert(CellType(k), vec![Rule {
            id: vec![CellType(k)], pattern: vec![], shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
            changes: vec![], active_only: false, priority: 10, min_age: 0,
            overflow: cellaria::types::OverflowAction::Write(0),
            cam: None,
            tie_break: 0,
            starvation_after: None, feedback: None, recursion: None, memory: None,
        }]);
    }
    for (i, &(_, byte)) in PACKET_FIXED.iter().enumerate() {
        idx.insert(CellType(CARRIER_TYPES[i]), vec![Rule {
            id: vec![CellType(CARRIER_TYPES[i])], pattern: vec![], shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
            changes: vec![], active_only: false, priority: 10, min_age: 0,
            overflow: cellaria::types::OverflowAction::Write(byte),
            cam: None,
            tie_break: 0,
            starvation_after: None, feedback: None, recursion: None, memory: None,
        }]);
    }
    idx
}

fn act1() {
    println!("========== ДЕЙСТВИЕ 1: существующий модуль + охраняемая самомодификация ==========\n");

    let mut grid = Grid::new(VecStorage::new(WIDTH, 1), Default::default());
    let mut out = BoundaryBuffer::new();
    out.direction = "output".to_string();
    grid.set_boundary(WIDTH - 1, 0, out);
    grid.set_cell(5, 0, Cell { value: CellValue(CellType(RAD)), born_at: 0 });
    grid.set_cell(COUNTER_X, 0, Cell { value: CellValue(CellType(COUNTER_BASE)), born_at: 0 });
    let num_pulses = 3usize;
    for j in 0..num_pulses {
        grid.set_cell(COUNTER_X + 2 * (j + 1), 0, Cell { value: CellValue(CellType(PULSE)), born_at: 0 });
    }

    let mut engine = Engine::new(grid, act1_rule_index());
    // Пока рабочий регион считает и передаёт вычисленное правило, охрана ещё
    // не включена — это ДВИЖУЩЕЕСЯ (shift) правило, а перевозчики сами тоже
    // движутся, и консервативный анализатор конфликтов (справедливо) не
    // может исключить столкновение движущихся правил друг с другом — та же
    // причина, по которой в proof_guarded_self_modification.rs для
    // guarded-демо используется прямая инъекция байт, а не физические
    // носители. Охрану включаем ПОСЛЕ безопасной установки, специально для
    // атаки на существующий модуль.
    engine.enable_self_modification();

    println!("Существующий модуль (id={}) уже работает: {} -> {}.", RAD, RAD, MID);
    println!("Рабочий регион считает импульсы ({} штук) и передаёт себе новое правило id={}.\n", num_pulses, SAFE_NEW_ID);

    // Фаза A: считаем, пока не появится "чистое" число k.
    let mut dynamic: Option<(usize, u8)> = None;
    for _ in 1..=500u32 {
        engine.run_tick();
        // Ищем ТОЛЬКО рядом со счётчиком — иначе клетка существующего
        // модуля (RAD=1 распадается в MID=3) тоже попадает в диапазон
        // 1..MAX_COUNT и ложно принимается за "досчитанный" счётчик.
        if let Some(x) = (COUNTER_X.saturating_sub(5)..COUNTER_X + 5).find(|&x| {
            engine.grid().get_cell(x, 0).map(|c| c.value.0 .0 >= 1 && c.value.0 .0 < MAX_COUNT).unwrap_or(false)
        }) {
            let k = engine.grid().get_cell(x, 0).unwrap().value.0 .0;
            dynamic = Some((x, k));
            break;
        }
    }
    let (dynamic_x, k) = dynamic.expect("счёт должен был завершиться");
    println!("Рабочий регион сам насчитал: {}.", k);

    // Фаза B: безопасный пакет [priority,id_len,id_byte,SHIFT_FLAG,dir=Right,steps(вычислено),terminator]
    // — расставляем перевозчики фиксированных байт вокруг уже посчитанного
    // числа, точно как в strength_self_modification_computed.rs.
    for (i, &(byte_index, _)) in PACKET_FIXED.iter().enumerate() {
        let offset = if byte_index < 5 { 2 * (5 - byte_index) } else { 2 };
        let x = if byte_index < 5 { dynamic_x + offset } else { dynamic_x.saturating_sub(offset) };
        let gen = engine.grid().generation();
        engine.grid_mut().set_cell(x, 0, Cell { value: CellValue(CellType(CARRIER_TYPES[i])), born_at: gen });
    }

    for _ in 1..=(WIDTH as u32) {
        engine.run_tick();
        if engine.rule_index.contains_key(&CellType(SAFE_NEW_ID)) {
            break;
        }
    }
    let installed = engine.rule_index.get(&CellType(SAFE_NEW_ID)).and_then(|v| v.first());
    println!(
        "Безопасная посылка установлена: {} (шагов сдвига = {}, совпадает с посчитанным числом: {})",
        installed.is_some(),
        installed.map(|r| r.shifts[0][0].steps).unwrap_or(0),
        installed.map(|r| r.shifts[0][0].steps as u8 == k).unwrap_or(false)
    );
    // Фаза C: атака — пакет, претендующий на территорию существующего
    // модуля (id=RAD), подаётся НАПРЯМУЮ (перенос уже трижды доказан
    // отдельно — здесь показывается решение охраны, а не сам перенос).
    // Охрану включаем ТОЛЬКО теперь.
    engine.enable_guarded_self_modification();
    {
        let buf = engine.grid_mut().get_boundary_mut(WIDTH - 1, 0).unwrap();
        for &b in &[10u8, 1, RAD, 0, 0, 99, 0xFF] {
            buf.enqueue(0, Cell { value: CellValue(CellType(b)), born_at: 0 });
        }
    }
    let rejected_before = engine.rejected_self_modifications;
    engine.run_tick();
    let rad_rule = &engine.rule_index[&CellType(RAD)];
    let rad_intact = rad_rule.len() == 1 && rad_rule[0].changes == vec![(0, 0, ChangeValue::Literal(MID))];

    println!(
        "Опасная посылка (id={} — территория существующего модуля): отклонена: {} | модуль не тронут: {}",
        RAD,
        engine.rejected_self_modifications > rejected_before,
        rad_intact
    );

    // Убеждаемся, что существующий модуль ДЕЙСТВИТЕЛЬНО продолжает работать.
    let mut check = Grid::new(VecStorage::new(1, 1), Default::default());
    check.set_cell(0, 0, Cell { value: CellValue(CellType(RAD)), born_at: 0 });
    let mut check_engine = Engine::new(check, {
        let mut i = HashMap::new();
        i.insert(CellType(RAD), rad_rule.clone());
        i
    });
    check_engine.run_tick();
    println!(
        "Существующий модуль всё ещё превращает {} в {}: {}\n",
        RAD, MID,
        check_engine.grid().get_cell(0, 0).map(|c| c.value.0 .0) == Some(MID)
    );
}

// ============================================================================
// Действие 2: самоаттестация — тот же канал передаёт не правило, а отчёт.
// ============================================================================

const ACC_MOD: u8 = 7;
const ACC_BASE2: u8 = 200;
const FINAL_BASE2: u8 = 220;
const QUIET2: u64 = 15;
const WIDTH2: usize = 60;
const MARKER_ROW: usize = 1;

fn act2_rule_index() -> HashMap<CellType, Vec<Rule>> {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for acc in 0..ACC_MOD {
        let mut rules = Vec::new();
        for d in 1u8..=6 {
            let next = (acc + d) % ACC_MOD;
            rules.push(Rule {
                id: vec![CellType(ACC_BASE2 + acc)],
                pattern: vec![(0, 0, CellType(ACC_BASE2 + acc)), (0, -1, CellType(d))],
                shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
                changes: vec![(0, 0, ChangeValue::Literal(ACC_BASE2 + next))],
                active_only: false, priority: 20, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
            });
        }
        rules.push(Rule {
            id: vec![CellType(ACC_BASE2 + acc)], pattern: vec![(0, 0, CellType(ACC_BASE2 + acc))], shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(FINAL_BASE2 + acc))],
            active_only: false, priority: 10, min_age: QUIET2, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
        });
        idx.insert(CellType(ACC_BASE2 + acc), rules);
        idx.insert(CellType(FINAL_BASE2 + acc), vec![Rule {
            id: vec![CellType(FINAL_BASE2 + acc)], pattern: vec![], shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
            changes: vec![], active_only: false, priority: 10, min_age: 0,
            overflow: cellaria::types::OverflowAction::Write(0),
            cam: None,
            tie_break: 0,
            starvation_after: None, feedback: None, recursion: None, memory: None,
        }]);
    }
    idx
}

fn attest(data: &[u8]) -> u8 {
    let mut grid = Grid::new(VecStorage::new(WIDTH2, 2), Default::default());
    let mut out = BoundaryBuffer::new();
    out.direction = "output".to_string();
    grid.set_boundary(WIDTH2 - 1, MARKER_ROW, out);
    for (x, &d) in data.iter().enumerate() {
        grid.set_cell(x, 0, Cell { value: CellValue(CellType(d)), born_at: 0 });
    }
    grid.set_cell(0, MARKER_ROW, Cell { value: CellValue(CellType(ACC_BASE2)), born_at: 0 });

    let mut engine = Engine::new(grid, act2_rule_index());
    for _ in 1..=(WIDTH2 as u32 * 2) {
        engine.run_tick();
        if let Some(buf) = engine.grid().get_boundary(WIDTH2 - 1, MARKER_ROW) {
            if let Some(q) = buf.queues.get(&0) {
                if let Some(cell) = q.front() {
                    return cell.value.0 .0 - FINAL_BASE2;
                }
            }
        }
    }
    panic!("контрольная сумма не пришла");
}

fn act2() {
    println!("========== ДЕЙСТВИЕ 2: тот же канал передаёт отчёт о себе, не правило ==========\n");
    let honest: [u8; 6] = [3, 1, 4, 1, 5, 2];
    let mut tampered = honest;
    tampered[3] = 6;
    let c1 = attest(&honest);
    let c2 = attest(&tampered);
    println!("Честные данные {:?} -> контрольная сумма: {}", honest, c1);
    println!("Подменённые данные {:?} -> контрольная сумма: {}", tampered, c2);
    println!("Подмена одной клетки замечена (суммы разные): {}\n", c1 != c2);
}

// ============================================================================
// Действие 3: обратимость — независимая проверка "мир можно прокрутить назад".
// ============================================================================

const CYCLE_LEN: u8 = 6;
const CYCLE_BASE: u8 = 10;
const TOKEN: u8 = 100;
const WIDTH3: usize = 40;
const TICKS3: u32 = 20;

fn act3_rules(forward: bool) -> HashMap<CellType, Vec<Rule>> {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for s in 0..CYCLE_LEN {
        let next = if forward { (s + 1) % CYCLE_LEN } else { (s + CYCLE_LEN - 1) % CYCLE_LEN };
        idx.insert(CellType(CYCLE_BASE + s), vec![Rule {
            id: vec![CellType(CYCLE_BASE + s)], pattern: vec![], shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(CYCLE_BASE + next))],
            active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
        }]);
    }
    let dir = if forward { Direction::Right } else { Direction::Left };
    idx.insert(CellType(TOKEN), vec![Rule {
        id: vec![CellType(TOKEN)], pattern: vec![], shifts: vec![vec![ShiftSpec::new(dir, 1)]],
        changes: vec![], active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
    }]);
    idx
}

fn act3() {
    println!("========== ДЕЙСТВИЕ 3: часть мира можно прокрутить назад буквально ==========\n");
    let mut grid = Grid::new(VecStorage::new(WIDTH3, 1), Default::default());
    grid.set_cell(0, 0, Cell { value: CellValue(CellType(CYCLE_BASE)), born_at: 0 });
    grid.set_cell(4, 0, Cell { value: CellValue(CellType(CYCLE_BASE + 2)), born_at: 0 });
    grid.set_cell(15, 0, Cell { value: CellValue(CellType(TOKEN)), born_at: 0 });
    let initial: Vec<u8> = (0..WIDTH3).map(|x| grid.get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(0)).collect();

    let mut fwd = Engine::new(grid, act3_rules(true));
    for _ in 1..=TICKS3 {
        fwd.run_tick();
    }
    let after: Vec<u8> = (0..WIDTH3).map(|x| fwd.grid().get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(0)).collect();

    let mut back_grid = Grid::new(VecStorage::new(WIDTH3, 1), Default::default());
    for (x, &v) in after.iter().enumerate() {
        if v != 0 {
            back_grid.set_cell(x, 0, Cell { value: CellValue(CellType(v)), born_at: 0 });
        }
    }
    let mut bwd = Engine::new(back_grid, act3_rules(false));
    for _ in 1..=TICKS3 {
        bwd.run_tick();
    }
    let recovered: Vec<u8> = (0..WIDTH3).map(|x| bwd.grid().get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(0)).collect();

    println!("{} тиков вперёд, затем {} тиков по развёрнутым правилам назад.", TICKS3, TICKS3);
    println!("Восстановленное состояние совпадает с исходным клетка в клетку: {}\n", recovered == initial);
}

fn main() {
    act1();
    act2();
    act3();
    println!("========== Итог ==========");
    println!("Все три действия используют ИСКЛЮЧИТЕЛЬНО то, что доказано и проверено в этой сессии.");
}
