//! Часть C стохастической самомодификации + внешней фитнес-селекции
//! (см. `proof_prng_from_rules.rs` — часть A, PRNG; `proof_prng_driven_self_modification.rs`
//! — часть B, PRNG реально выбирает направление в правиле, которое решётка
//! устанавливает себе через `RuleStore`).
//!
//! Здесь — тот же приём, что и в `proof_quorum_fault_tolerance.rs` (TMR):
//! НИ ОДНОЙ правки движка, вся логика — снаружи, обычным Rust-кодом,
//! читающим состояние реплик через уже существующий `grid()`/`get_cell()`.
//! Там внешний код СРАВНИВАЛ реплики и чинил сбой; здесь внешний код
//! ИЗМЕРЯЕТ реплики и ВЫБИРАЕТ лучшую — ровно то же самое отношение
//! "движок ничего не знает про голосование/отбор", применённое к другой
//! задаче (эволюционный отбор, а не отказоустойчивость).
//!
//! Сценарий: N независимых реплик, одни и те же правила, но разный посев
//! PRNG (часть A) => разное направление в самомодифицированном правиле
//! (часть B, Left или Right). После установки правила на каждую реплику
//! ставится одинаковый тестовый токен, и правило само его двигает. Внешняя
//! фитнес-функция (обычный Rust, не правило) меряет итоговую позицию токена
//! и выбирает реплику с максимальной приспособленностью — как шаг отбора в
//! генетическом алгоритме, где "особь" — это решётка с её самостоятельно
//! установленным правилом, а не структура данных, которую сравнивает внешний
//! код по заранее известному критерию.

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{
    BoundaryBuffer, Cell, CellType, CellValue, ChangeValue, Direction, OverflowAction, Rule, ShiftSpec,
};
use cellaria::{Grid, VecStorage};

const WIDTH: usize = 300;
const CENTER: usize = 150;
const OFF: u8 = 1;
const ON: u8 = 2;
const WARMUP_TICKS: u32 = 30;
const TRANSMIT_TICKS: u32 = 400;
const EXTRA_TICKS: u32 = 10; // сколько тиков даём установленному правилу подвигать тестовый токен

const GEN_ID: u8 = 77;
const SELECT_X: usize = CENTER;
const TOKEN_X: usize = 250; // далеко от зоны перевозчиков пакета (~142..158), безопасный запас с обеих сторон

const PACKET_FIXED: [(usize, u8); 6] = [(0, 10), (1, 1), (2, GEN_ID), (3, 0xFE), (5, 4), (6, 0xFF)];
const CARRIER_TYPES: [u8; 6] = [141, 142, 143, 144, 145, 146];

const SELECTOR_TYPE: u8 = 200;
const CARRIER_LOW: u8 = 151; // бит=0 -> Left
const CARRIER_HIGH: u8 = 152; // бит=1 -> Right

const RULE30: [(u8, u8, u8, u8); 8] = [
    (1, 1, 1, 0),
    (1, 1, 0, 0),
    (1, 0, 1, 0),
    (1, 0, 0, 1),
    (0, 1, 1, 1),
    (0, 1, 0, 1),
    (0, 0, 1, 1),
    (0, 0, 0, 0),
];

fn ct(bit: u8) -> CellType {
    CellType(if bit == 1 { ON } else { OFF })
}

fn build_rule_index() -> HashMap<CellType, Vec<Rule>> {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();

    for &(l, c, r, new) in &RULE30 {
        idx.entry(ct(c)).or_default().push(Rule {
            id: vec![ct(c)],
            pattern: vec![(-1, 0, ct(l)), (0, 0, ct(c)), (1, 0, ct(r))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(if new == 1 { ON } else { OFF }))],
            active_only: false,
            priority: 0,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None, feedback: None, recursion: None, memory: None,
        });
    }

    idx.insert(
        CellType(SELECTOR_TYPE),
        vec![
            Rule {
                id: vec![CellType(SELECTOR_TYPE)],
                pattern: vec![(0, 0, CellType(SELECTOR_TYPE)), (0, -1, ct(0))],
                shifts: vec![],
                changes: vec![(0, 0, ChangeValue::Literal(CARRIER_LOW))],
                active_only: false,
                priority: 10,
                min_age: 0,
                overflow: Default::default(),
                cam: None,
                tie_break: 0,
                starvation_after: None, feedback: None, recursion: None, memory: None,
            },
            Rule {
                id: vec![CellType(SELECTOR_TYPE)],
                pattern: vec![(0, 0, CellType(SELECTOR_TYPE)), (0, -1, ct(1))],
                shifts: vec![],
                changes: vec![(0, 0, ChangeValue::Literal(CARRIER_HIGH))],
                active_only: false,
                priority: 10,
                min_age: 0,
                overflow: Default::default(),
                cam: None,
                tie_break: 0,
                starvation_after: None, feedback: None, recursion: None, memory: None,
            },
        ],
    );

    idx.insert(
        CellType(CARRIER_LOW),
        vec![Rule {
            id: vec![CellType(CARRIER_LOW)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
            changes: vec![],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: OverflowAction::Write(2),
            cam: None,
            tie_break: 0,
            starvation_after: None, feedback: None, recursion: None, memory: None,
        }],
    );
    idx.insert(
        CellType(CARRIER_HIGH),
        vec![Rule {
            id: vec![CellType(CARRIER_HIGH)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
            changes: vec![],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: OverflowAction::Write(3),
            cam: None,
            tie_break: 0,
            starvation_after: None, feedback: None, recursion: None, memory: None,
        }],
    );

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
                starvation_after: None, feedback: None, recursion: None, memory: None,
            }],
        );
    }

    idx
}

struct Replica {
    seed: usize,
    direction: Direction,
    fitness: i64, // итоговая позиция токена (внешняя, произвольная функция приспособленности)
}

/// Строит и прогоняет одну реплику от посева PRNG до измеренной
/// приспособленности. Вся "особь" — это решётка + правила; ничего из этого
/// не знает, что её сравнивают с другими репликами.
fn run_replica(seed_x: usize) -> Option<Replica> {
    let storage = VecStorage::new(WIDTH, 2);
    let mut grid = Grid::new(storage, Default::default());

    for x in 0..WIDTH {
        grid.set_cell(x, 0, Cell { value: CellValue::new(OFF), born_at: 0 });
    }
    grid.set_cell(seed_x, 0, Cell { value: CellValue::new(ON), born_at: 0 });

    let mut output_buf = BoundaryBuffer::new();
    output_buf.direction = "output".to_string();
    grid.set_boundary(WIDTH - 1, 1, output_buf);

    let mut engine = Engine::new(grid, build_rule_index());
    engine.enable_self_modification();

    for _ in 0..WARMUP_TICKS {
        engine.run_tick();
    }

    let gen = engine.grid().generation();
    engine.grid_mut().set_cell(SELECT_X, 1, Cell { value: CellValue::new(SELECTOR_TYPE), born_at: gen });
    for (i, &(byte_index, _)) in PACKET_FIXED.iter().enumerate() {
        let offset = if byte_index < 4 { 2 * (4 - byte_index) } else { 2 * (byte_index - 4) };
        let x = if byte_index < 4 { SELECT_X + offset } else { SELECT_X - offset };
        engine.grid_mut().set_cell(x, 1, Cell { value: CellValue::new(CARRIER_TYPES[i]), born_at: gen });
    }

    let mut direction = None;
    for _ in 0..TRANSMIT_TICKS {
        engine.run_tick();
        if let Some(rule) = engine.rule_index.get(&CellType(GEN_ID)).and_then(|v| v.first()) {
            direction = Some(rule.shifts[0][0].direction);
            break;
        }
    }
    let direction = direction?;

    // Ставим тестовый токен и даём самомодифицированному правилу его
    // подвигать — это единственное, что определяет итоговую приспособленность.
    let gen = engine.grid().generation();
    engine.grid_mut().set_cell(TOKEN_X, 1, Cell { value: CellValue::new(GEN_ID), born_at: gen });
    for _ in 0..EXTRA_TICKS {
        engine.run_tick();
    }

    let final_x = (0..WIDTH).find(|&x| {
        engine.grid().get_cell(x, 1).map(|c| c.value.0 .0 == GEN_ID).unwrap_or(false)
    });

    // Фитнес: насколько далеко токен ушёл ВПРАВО от старта — критерий
    // выбран мной как автором демо, но САМО значение для каждой реплики
    // вычисляет исключительно решётка (правило, которое она сама себе
    // установила), не Rust-код.
    let fitness = final_x.map(|x| x as i64 - TOKEN_X as i64).unwrap_or(i64::MIN);

    Some(Replica { seed: seed_x, direction, fitness })
}

fn main() {
    let seeds = [CENTER - 9, CENTER - 4, CENTER, CENTER + 3, CENTER + 8, CENTER + 15];

    let replicas: Vec<Replica> = seeds
        .iter()
        .map(|&s| run_replica(s).unwrap_or_else(|| panic!("реплика с посевом {s} не собрала пакет самомодификации")))
        .collect();

    for r in &replicas {
        println!("реплика seed={:>4}: направление={:?}, фитнес (смещение токена)={:+}", r.seed, r.direction, r.fitness);
    }

    // Внешний отбор — обычное сравнение Vec, как vote() во внешнем TMR:
    // движок ничего не знает про "отбор", это чтение уже посчитанного
    // состояния снаружи.
    let winner = replicas.iter().max_by_key(|r| r.fitness).expect("список реплик не пуст");
    let loser = replicas.iter().min_by_key(|r| r.fitness).expect("список реплик не пуст");

    println!(
        "\nВнешний отбор: лучшая реплика seed={} (фитнес {:+}, направление {:?}), худшая seed={} (фитнес {:+}, направление {:?})",
        winner.seed, winner.fitness, winner.direction, loser.seed, loser.fitness, loser.direction
    );

    assert_eq!(winner.direction, Direction::Right, "реплика с максимальным смещением вправо обязана иметь направление Right — иначе фитнес не отражает реального поведения решётки");
    assert_eq!(loser.direction, Direction::Left, "реплика с минимальным смещением обязана иметь направление Left — иначе фитнес не отражает реального поведения решётки");
    assert!(replicas.iter().any(|r| r.direction == Direction::Left) && replicas.iter().any(|r| r.direction == Direction::Right), "популяция должна содержать обе ветви, иначе отбор ничего не отбирает");

    println!(
        "\nВывод: N реплик с одними и теми же правилами, но разным посевом PRNG (часть A), сами устанавливают \
себе РАЗНЫЕ правила через настоящий RuleStore-канал (часть B) — направление сдвига физически считывается из \
решётки, не из Rust-кода. Внешний Rust-код (часть C) не трогает движок ни строчкой: он только читает итоговое \
состояние каждой реплики (как vote() в proof_quorum_fault_tolerance.rs читает снапшоты), меряет фитнес и \
выбирает победителя обычным max_by_key. 'Стохастическая самомодификация + внешняя фитнес-селекция' — полностью \
реализовано и проверено автоматическими assert, три независимые части (A/B/C) стыкуются без единой правки движка."
    );
}
