//! Шестая сила, часть 1: замыкаем протокол самомодификации (`RuleStore`),
//! который был в замысле проекта с самого начала, но никогда не подключался
//! к реально работающему `Engine` целиком — только существовал изолированно
//! (см. `rule_store_tests.rs`), плюс отдельно был проверен внешний путь
//! (`strength_live_rules.rs`: правило добавляет СНАРУЖИ обычный Rust-код).
//!
//! Здесь правило добавляется НЕ снаружи — решётка сама, своими клетками,
//! выталкивает байты пакета протокола `AddRule` через выходной порт
//! (`OverflowAction::Write` при уходе клетки за край — см.
//! `applicator.rs`), а `Engine::run_tick` САМ (после `Engine::enable_self_modification`)
//! их собирает, декодирует и устанавливает обратно в свой `rule_index` —
//! без единого внешнего вызова между тиками. Раньше (первая версия этого
//! демо) эту склейку приходилось делать вручную после каждого тика; теперь
//! это часть `run_tick` самого движка — самомодификация "всегда включена",
//! а не разовая ручная сверка в конце прогона.
//!
//! Транспорт байт по решётке и итоговое ЗНАЧЕНИЕ байта на выходе — два
//! независимых механизма: `OverflowAction::Write(v)` c v != 0 кладёт в
//! очередь ЛИТЕРАЛ v (заданный в самом правиле-перевозчике), а не то, что
//! физически несёт клетка. Это специально: если бы байт id генерируемого
//! правила транспортировался клеткой ТОГО ЖЕ типа, что и сам id, движку
//! пришлось бы уже ЗАРАНЕЕ иметь правило-перевозчик с тем же id — что сделало
//! бы демонстрацию "до/после" бессмысленной. Разделение "тип
//! клетки-перевозчика" / "значение на выходе" снимает эту путаницу.
//!
//! Перевозчики стоят с зазором 2 между собой (а не впритык) — иначе цель
//! сдвига одного совпадает с исходной клеткой соседа, и консервативный
//! анализатор конфликтов (справедливо) считает это столкновением движущихся
//! объектов — то же доказанное ограничение, что и у челнока в `big_world.rs`.

use cellaria::engine::Engine;
use cellaria::types::{BoundaryBuffer, Cell, CellType, CellValue, Direction, OverflowAction, Rule, ShiftSpec};
use cellaria::{Grid, VecStorage};

const WIDTH: usize = 80;
const TARGET_ID: u8 = 55; // id правила, которое решётка сгенерирует сама

fn main() {
    // Пакет протокола AddRule (см. rule_store.rs::deserialize_packet):
    // [priority, id_len, id_byte×id_len, SHIFT_FLAG(0xFE), dir_byte, steps, 0xFF]
    // Описывает: "правило id=[55], сдвиг вправо на 1 клетку за тик".
    let packet: [u8; 7] = [10, 1, TARGET_ID, 0xFE, 3 /*Right*/, 1, 0xFF];

    let storage = VecStorage::new(WIDTH, 1);
    let mut grid = Grid::new(storage, Default::default());

    let mut output_buf = BoundaryBuffer::new();
    output_buf.direction = "output".to_string();
    grid.set_boundary(WIDTH - 1, 0, output_buf);

    // 7 клеток-перевозчиков, каждая — свой зарезервированный тип (101..107),
    // никак не пересекающийся с байтами самого пакета. Каждая едет вправо и,
    // уходя за край, кладёт в очередь СВОЙ литерал — конкретный байт пакета,
    // а не собственное значение.
    let mut rule_index = std::collections::HashMap::new();
    for (i, &byte) in packet.iter().enumerate() {
        let carrier = CellType((101 + i) as u8);
        let start_x = 2 * (packet.len() - 1 - i);
        grid.set_cell(start_x, 0, Cell { value: CellValue(carrier), born_at: 0 });
        rule_index.insert(
            carrier,
            vec![Rule {
                id: vec![carrier],
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

    let mut engine = Engine::new(grid, rule_index);
    engine.enable_self_modification();

    println!("Пакет для передачи (решётка сама его вытолкнет клетка за клеткой): {:?}", packet);
    println!(
        "До передачи: правило для типа {} существует? {}\n",
        TARGET_ID,
        engine.rule_index().contains_key(&CellType(TARGET_ID))
    );

    let mut was_installed = false;
    for tick in 1..=100u32 {
        engine.run_tick();

        let now_installed = engine.rule_index().contains_key(&CellType(TARGET_ID));
        if now_installed && !was_installed {
            println!(
                "тик {:>2}: правило для типа {} появилось в rule_index — решётка сама его сгенерировала и передала, run_tick сам его подхватил, без единой внешней строчки между тиками",
                tick, TARGET_ID
            );
            // Прямо ЗДЕСЬ, посреди того же прогона (не после цикла) — ставим
            // свежую клетку типа TARGET_ID, которая раньше никогда не
            // существовала на решётке, и продолжаем тикать как ни в чём не
            // бывало.
            engine.grid_mut().set_cell(0, 0, Cell { value: CellValue(CellType(TARGET_ID)), born_at: 0 });
            println!("тик {:>2}: тут же ставим свежую клетку типа {} на x=0", tick, TARGET_ID);
        }
        was_installed = now_installed;

        if was_installed {
            if let Some(x) = (0..WIDTH).find(|&x| engine.grid().get_cell(x, 0).map(|c| c.value.0 .0) == Some(TARGET_ID)) {
                if x > 0 {
                    println!(
                        "тик {:>2}: клетка типа {} сдвинулась на x={} — сгенерированное решёткой правило реально работает, всё в одном непрерывном прогоне",
                        tick, TARGET_ID, x
                    );
                    break;
                }
            }
        }
    }
}
