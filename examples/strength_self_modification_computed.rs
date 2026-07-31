//! Шестая сила, часть 2: в первой части (`strength_self_modification.rs`)
//! решётка САМА передавала себе правило — но байты этого правила были
//! готовым сообщением, которое просто физически пронесли через решётку;
//! содержание я как автор демо выбрал заранее в Rust-коде.
//!
//! Здесь число, которое попадает в сгенерированное правило (сколько клеток
//! сдвигать), — результат вычисления, которое сделала сама решётка во время
//! работы: она считает, сколько "импульсов" пришло, и именно эта посчитанная
//! величина, а не константа, которую я где-то написал, оказывается в новом
//! правиле. Доказательство: одна и та же логика (одни и те же правила)
//! запускается дважды с разным числом импульсов на входе — и оба раза
//! получившееся правило корректно отражает именно то число, которое решётка
//! сама насчитала, а не что-то, что я подставил в код по результату.
//!
//! Как считает: клетка-счётчик стоит на месте с "кодированным" значением
//! (150+k, k — текущий счёт). Каждый прилетевший импульс превращает 150+k в
//! 150+k+1 и исчезает. После того как новых импульсов какое-то время не было
//! (`min_age` у финального правила — обычный, уже существующий механизм
//! движка, не что-то новое), счётчик переводит себя в "чистое" число k —
//! именно это число становится значением, которое несёт клетка, вылетающая с
//! края решётки (`OverflowAction::Write(0)` — "неси СВОЁ значение", тот же
//! механизм, что читает моё собственное значение а не что-то захардкоженное,
//! см. часть 1).

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{BoundaryBuffer, Cell, CellType, CellValue, ChangeValue, Direction, OverflowAction, Rule, ShiftSpec};
use cellaria::{Grid, VecStorage};

const WIDTH: usize = 200;
const COUNTER_X: usize = 50;
const PULSE: u8 = 60;
const COUNTER_BASE: u8 = 150;
const MAX_COUNT: u8 = 9;
const QUIET_THRESHOLD: u64 = 20;
const GEN_ID: u8 = 66; // id правила, которое решётка сгенерирует сама

// Фиксированные байты пакета (см. rule_store.rs::deserialize_packet), кроме
// "steps" — он и есть посчитанное число.
const PACKET_FIXED: [(usize, u8); 6] = [
    (0, 10),        // priority
    (1, 1),         // id_len
    (2, GEN_ID),    // id_byte
    (3, 0xFE),      // SHIFT_FLAG
    (4, 3),         // dir_byte = Right
    (6, 0xFF),      // terminator
];
// Каждому фиксированному байту — свой зарезервированный тип клетки-перевозчика.
const CARRIER_TYPES: [u8; 6] = [121, 122, 123, 124, 125, 126];

fn build_rule_index() -> HashMap<CellType, Vec<Rule>> {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();

    // Импульс безусловно едет влево, пока не встретит счётчик.
    idx.insert(
        CellType(PULSE),
        vec![Rule {
            id: vec![CellType(PULSE)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec::new(Direction::Left, 1)]],
            changes: vec![],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
        }],
    );

    for k in 0..MAX_COUNT {
        let counter_id = CellType(COUNTER_BASE + k);
        let mut rules = Vec::new();

        // Прилетел импульс справа — увеличить счёт, съесть импульс.
        // Приоритет ВЫШЕ, чем у простого сдвига импульса (10), поэтому
        // именно инкремент побеждает в конфликте за клетку счётчика, а не
        // импульс, наивно сдвигающийся поверх счётчика.
        rules.push(Rule {
            id: vec![counter_id],
            pattern: vec![(0, 0, counter_id), (1, 0, CellType(PULSE))],
            shifts: vec![],
            changes: vec![
                (0, 0, ChangeValue::Literal(COUNTER_BASE + k + 1)),
                (1, 0, ChangeValue::Literal(0)),
            ],
            active_only: false,
            priority: 20,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
        });

        // Тихо (никто не прилетал QUIET_THRESHOLD тиков) — перевести
        // кодированное значение в чистое число k. min_age — обычный
        // механизм движка: правило видит клетку, только если она не
        // менялась минимум это число тиков, а инкремент выше каждый раз
        // сбрасывает возраст клетки заново.
        if k > 0 {
            rules.push(Rule {
                id: vec![counter_id],
                pattern: vec![(0, 0, counter_id)],
                shifts: vec![],
                changes: vec![(0, 0, ChangeValue::Literal(k))],
                active_only: false,
                priority: 10,
                min_age: QUIET_THRESHOLD,
                overflow: Default::default(),
                cam: None,
                tie_break: 0,
                starvation_after: None,
            });
        }
        idx.insert(counter_id, rules);
    }

    // Как только счётчик превратился в чистое число k — эта клетка сама
    // едет к выходу и на переполнении несёт своё же значение (Write(0)).
    for k in 1..MAX_COUNT {
        idx.insert(
            CellType(k),
            vec![Rule {
                id: vec![CellType(k)],
                pattern: vec![],
                shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
                changes: vec![],
                active_only: false,
                priority: 10,
                min_age: 0,
                overflow: OverflowAction::Write(0),
                cam: None,
                tie_break: 0,
                starvation_after: None,
            }],
        );
    }

    // Шесть перевозчиков фиксированных байт пакета — точно как в части 1.
    for (i, &(_, byte)) in PACKET_FIXED.iter().enumerate() {
        idx.insert(
            CellType(CARRIER_TYPES[i]),
            vec![Rule {
                id: vec![CellType(CARRIER_TYPES[i])],
                pattern: vec![],
                shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
                changes: vec![],
                active_only: false,
                priority: 10,
                min_age: 0,
                overflow: OverflowAction::Write(byte),
                cam: None,
                tie_break: 0,
                starvation_after: None,
            }],
        );
    }

    idx
}

fn run_experiment(num_pulses: usize) -> Option<(u8, ShiftSpec)> {
    let storage = VecStorage::new(WIDTH, 1);
    let mut grid = Grid::new(storage, Default::default());

    let mut output_buf = BoundaryBuffer::new();
    output_buf.direction = "output".to_string();
    grid.set_boundary(WIDTH - 1, 0, output_buf);

    grid.set_cell(COUNTER_X, 0, Cell { value: CellValue(CellType(COUNTER_BASE)), born_at: 0 });
    for j in 0..num_pulses {
        let x = COUNTER_X + 2 * (j + 1);
        grid.set_cell(x, 0, Cell { value: CellValue(CellType(PULSE)), born_at: 0 });
    }

    let mut engine = Engine::new(grid, build_rule_index());
    engine.enable_self_modification();

    println!("\n--- Прогон с {} импульсами ---", num_pulses);

    // Фаза 1: считаем, пока на решётке не появится "чистое" число k.
    let mut dynamic: Option<(usize, u8)> = None;
    for _ in 1..=500u32 {
        engine.run_tick();
        if let Some(x) = (0..WIDTH).find(|&x| {
            engine
                .grid()
                .get_cell(x, 0)
                .map(|c| c.value.0 .0 >= 1 && c.value.0 .0 < MAX_COUNT)
                .unwrap_or(false)
        }) {
            let k = engine.grid().get_cell(x, 0).unwrap().value.0 .0;
            dynamic = Some((x, k));
            break;
        }
    }
    let (dynamic_x, k) = dynamic?;
    println!("Решётка сама насчитала: {} (клетка со значением {} стоит на x={})", k, k, dynamic_x);

    // Фаза 2: ставим 6 перевозчиков фиксированных байт вокруг уже
    // посчитанного числа, в порядке, который сложится в правильный пакет.
    for (i, &(byte_index, _)) in PACKET_FIXED.iter().enumerate() {
        // Байты 0..4 идут ДО steps (byte_index 5) — должны прийти раньше,
        // значит стоят БЛИЖЕ к выходу (больше x). Терминатор (byte_index 6)
        // идёт ПОСЛЕ — дальше от выхода.
        let offset = if byte_index < 5 { 2 * (5 - byte_index) } else { 2 };
        let x = if byte_index < 5 { dynamic_x + offset } else { dynamic_x.saturating_sub(offset) };
        let gen = engine.grid().generation();
        engine.grid_mut().set_cell(x, 0, Cell { value: CellValue(CellType(CARRIER_TYPES[i])), born_at: gen });
    }

    for _ in 1..=(WIDTH as u32) {
        engine.run_tick();
        if let Some(rule) = engine.rule_index.get(&CellType(GEN_ID)).and_then(|v| v.first()) {
            return Some((k, rule.shifts[0][0].clone()));
        }
    }
    None
}

fn main() {
    for &num_pulses in &[3usize, 5usize] {
        match run_experiment(num_pulses) {
            Some((k, shift)) => {
                println!(
                    "Сгенерированное решёткой правило: id=[{}], shift={:?} — шагов сдвига = {}, посчитанное число = {} → {}",
                    GEN_ID, shift, shift.steps, k,
                    if shift.steps as u8 == k { "СОВПАДАЕТ" } else { "НЕ СОВПАДАЕТ (!)" }
                );
            }
            None => println!("Пакет не собрался — см. диагностику."),
        }
    }
    println!(
        "\nОдна и та же логика, разное число импульсов на входе — разные правила на выходе.\n\
         Число внутри правила пришло из вычисления на решётке, а не из моего кода."
    );
}
