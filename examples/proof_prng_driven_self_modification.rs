//! Часть B стохастической самомодификации (см. `proof_prng_from_rules.rs` —
//! часть A, PRNG из обычных правил; `strength_self_modification_computed.rs`
//! — механика "carrier train", уже доказанная перевозка ВЫЧИСЛЕННОГО байта
//! через реальный `RuleStore`-канал).
//!
//! Здесь эти два кирпича соединяются: направление сдвига (`Left`/`Right`) в
//! правиле, которое решётка сама себе устанавливает через
//! `RuleStore::drain_rule_channel`, определяется не константой из моего
//! Rust-кода и не счётчиком импульсов, а ТЕКУЩИМ битом Rule-30 PRNG —
//! ровно тем генератором, что доказан в части A.
//!
//! Устройство: решётка 2 строки×WIDTH. Строка 0 — PRNG (Rule 30), строка 1 —
//! обычный "carrier train" (как в `strength_self_modification_computed.rs`).
//! Между ними ОДНА клетка-селектор: она читает СВОЙ сосед сверху (offset
//! (0,-1), клетка PRNG прямо над собой) и превращается в один из двух типов
//! перевозчика — каждый несёт СВОЙ байт направления (`Write(2)`=Left или
//! `Write(3)`=Right) при переполнении. Выбор между двумя ветвями — это
//! обычное сопоставление `pattern` (два правила с разным ожидаемым соседом),
//! а не код, написанный мной заранее для конкретного исхода.
//!
//! Проверка: для нескольких разных посевов PRNG среди сгенерированных решёткой
//! правил встречаются ОБА направления — подтверждает, что ветвление реально
//! зависит от значения на решётке, а не всегда уходит в одну и ту же сторону.

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
const WARMUP_TICKS: u32 = 30; // PRNG "мешает" сама себя, прежде чем её читают
const TRANSMIT_TICKS: u32 = 400;

const GEN_ID: u8 = 77;
const SELECT_X: usize = CENTER; // клетка-селектор стоит прямо под клеткой PRNG, которую читает

// Фиксированные байты пакета, КРОМЕ dir_byte (byte_index=4) — тот выбирается
// PRNG-селектором, а не задан здесь заранее.
const PACKET_FIXED: [(usize, u8); 6] = [
    (0, 10),        // priority
    (1, 1),         // id_len
    (2, GEN_ID),    // id_byte
    (3, 0xFE),      // SHIFT_FLAG
    (5, 4),         // steps (фиксировано — переменная часть здесь направление, не число шагов)
    (6, 0xFF),      // terminator
];
const CARRIER_TYPES: [u8; 6] = [141, 142, 143, 144, 145, 146];

const SELECTOR_TYPE: u8 = 200;
const CARRIER_LOW: u8 = 151; // PRNG-бит=0 -> направление Left
const CARRIER_HIGH: u8 = 152; // PRNG-бит=1 -> направление Right

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

    // --- Строка 0: Rule 30 PRNG (тождественно part A) ---------------------
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
            starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
        });
    }

    // --- Селектор: читает клетку PRNG ПРЯМО НАД собой (offset (0,-1)) и --
    // превращается в один из двух перевозчиков направления. Два правила,
    // различающихся только ожидаемым соседом — решётка сама выбирает, какое
    // сработает, на основании того, что реально стоит в PRNG-строке.
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
                starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
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
                starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
            },
        ],
    );

    // Оба перевозчика направления — обычный сдвиг вправо, каждый несёт свой
    // dir_byte при переполнении (2=Left, 3=Right — см. rule_store.rs).
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
            starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
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
            starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
        }],
    );

    // Шесть перевозчиков фиксированных байт пакета — точно как в
    // strength_self_modification_computed.rs.
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
                starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
            }],
        );
    }

    idx
}

/// Возвращает (реально стоявший в PRNG бит на момент чтения,
/// сгенерированное решёткой направление сдвига) для данного посева.
fn run_experiment(seed_x: usize) -> Option<(u8, Direction)> {
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

    // Фаза 1: PRNG "разогревается" сам по себе — строка 1 пуста, ничего
    // больше не происходит.
    for _ in 0..WARMUP_TICKS {
        engine.run_tick();
    }

    // Читаем бит ТОЛЬКО для собственной проверки результата ниже — сама
    // решётка прочитает тот же бит независимо, через pattern селектора.
    let observed_bit = if engine.grid().get_cell(SELECT_X, 0)?.value.0 .0 == ON { 1u8 } else { 0u8 };

    // Фаза 2: ставим селектор и шесть фиксированных перевозчиков одним
    // поколением — дальше решётка сама решает, каким типом станет селектор.
    let gen = engine.grid().generation();
    engine.grid_mut().set_cell(SELECT_X, 1, Cell { value: CellValue::new(SELECTOR_TYPE), born_at: gen });
    for (i, &(byte_index, _)) in PACKET_FIXED.iter().enumerate() {
        let offset = if byte_index < 4 { 2 * (4 - byte_index) } else { 2 * (byte_index - 4) };
        let x = if byte_index < 4 { SELECT_X + offset } else { SELECT_X - offset };
        engine.grid_mut().set_cell(x, 1, Cell { value: CellValue::new(CARRIER_TYPES[i]), born_at: gen });
    }

    for _ in 0..TRANSMIT_TICKS {
        engine.run_tick();
        if let Some(rule) = engine.rule_index().get(&CellType(GEN_ID)).and_then(|v| v.first()) {
            return Some((observed_bit, rule.shifts[0][0].direction));
        }
    }
    None
}

fn main() {
    let seeds = [CENTER - 9, CENTER - 4, CENTER, CENTER + 3, CENTER + 8, CENTER + 15];

    let mut saw_left = false;
    let mut saw_right = false;

    for &seed in &seeds {
        match run_experiment(seed) {
            Some((bit, dir)) => {
                let expected = if bit == 1 { Direction::Right } else { Direction::Left };
                println!(
                    "seed_x={seed}: PRNG-бит после прогрева = {bit}, решётка сама сгенерировала направление {dir:?} — {}",
                    if dir == expected { "СОВПАДАЕТ с ожидаемым по правилу бит=1->Right/бит=0->Left" } else { "НЕ СОВПАДАЕТ (!)" }
                );
                assert_eq!(dir, expected, "направление в сгенерированном правиле должно однозначно определяться PRNG-битом");
                match dir {
                    Direction::Left => saw_left = true,
                    Direction::Right => saw_right = true,
                    other => panic!("непредвиденное направление в сгенерированном правиле: {other:?}"),
                }
            }
            None => panic!("пакет не собрался для seed_x={seed} — см. диагностику"),
        }
    }

    assert!(saw_left && saw_right, "среди посевов должны встретиться ОБЕ ветви — иначе ветвление не доказано, только один путь проверен");

    println!(
        "\nВывод: направление сдвига в правиле, которое решётка НАСТОЯЩИМ образом устанавливает себе через \
RuleStore, определяется значением PRNG-клетки (Rule 30, часть A), а не константой в Rust-коде и не счётчиком. \
Выбор между Left/Right — обычное сопоставление pattern двух конкурирующих правил селектора, дальше — уже \
доказанный механизм переноса байта через границу (strength_self_modification_computed.rs). Стохастическая \
самомодификация' — не голое слово: источник хаоса (A) реально управляет решением, которое физически \
транслируется в новое правило движка (B), оба шага проверены автоматическими assert, не 'должно сработать'."
    );
}
