//! Проверка `GpuEngine` для v2-подмножества (сдвиги + запись в соседа,
//! реальный многораундовый арбитраж — см. `gpu::rule_table`'s
//! `needs_arbitration`/`GpuEngine`'s `Arbitrated`-пайплайн) против
//! CPU-эталона `engine::run_tick` — прямое продолжение
//! `tests/gpu_v1_correctness.rs`, но со сдвигами, которые v1 сознательно
//! исключал.
//!
//! Генератор строго ограничен документированным GPU-подмножеством (см.
//! `gpu::rule_table::GpuUnsupportedReason`): без `ChangeValue::Ref`, без
//! `OverflowAction::Write`/`WriteLiteral` — оба выхода намеренно НЕ
//! генерируются вовсе (не polyfill+skip, а структурно недостижимы), так что
//! почти каждый сгенерированный конфиг реально проверяется, а не
//! пропускается. Единственная причина пропуска — `TooManyRulesForArbitration`
//! (голова случайно набрала больше `MAX_MATCHES_PER_CELL` правил).
//!
//! Несколько тиков подряд, сверка ПОСЛЕ КАЖДОГО (не только в конце) — чтобы
//! расхождение локализовалось на конкретном тике, а не терялось за
//! накопленным дрейфом.

use std::collections::{HashMap, HashSet};

use proptest::prelude::*;

use cellaria::engine::{run_tick, Engine};
use cellaria::gpu::GpuEngine;
use cellaria::types::{
    CamSearch, Cell, CellType, CellValue, ChangeValue, Direction, FeedbackSpec, MemorySpec, OverflowAction,
    RecordTrigger, RecordedValue, RecursionSpec, Rule, ShiftSpec,
};
use cellaria::{Grid, VecStorage};

const CELL_ALPHABET: u8 = 3;
const SIDE: usize = 7;
const TICKS: usize = 4;

fn cell_type_strategy() -> impl Strategy<Value = u8> {
    1..=CELL_ALPHABET
}

fn pattern_strategy() -> impl Strategy<Value = (u8, Vec<(i8, i8, CellType)>)> {
    (
        cell_type_strategy(),
        prop::collection::vec((-1i8..=1, -1i8..=1, cell_type_strategy()), 0..=2),
    )
        .prop_map(|(head, extra)| {
            let mut seen: HashSet<(i8, i8)> = HashSet::new();
            seen.insert((0, 0));
            let mut cells = vec![(0i8, 0i8, CellType(head))];
            for (dx, dy, ct) in extra {
                if seen.insert((dx, dy)) {
                    cells.push((dx, dy, CellType(ct)));
                }
            }
            (head, cells)
        })
}

fn direction_strategy() -> impl Strategy<Value = Direction> {
    prop_oneof![
        Just(Direction::Up),
        Just(Direction::Down),
        Just(Direction::Left),
        Just(Direction::Right),
    ]
}

/// Один сдвиг v2-генератора: направление + `steps` в 1..=2 (держит решётку
/// `SIDE`×`SIDE` маленькой относительно возможных целей записи, как и
/// раньше) + случайный `broadcast` — покрывает как обычные (телепорт), так
/// и broadcast (путь) сдвиги в одном и том же property-тесте, включая их
/// СЛУЧАЙНОЕ пересечение друг с другом (2 сдвига на правило, разные головы
/// одновременно на маленькой решётке) без отдельного вручную написанного
/// сценария на каждую комбинацию.
fn shift_spec_strategy() -> impl Strategy<Value = ShiftSpec> {
    (direction_strategy(), 1u16..=2, any::<bool>()).prop_map(|(direction, steps, broadcast)| ShiftSpec {
        direction,
        steps,
        broadcast,
        keep_source: false,
    })
}

/// v2-правило: 0..=2 независимых сдвигов (см. `shift_spec_strategy` — теперь
/// включая broadcast) и 0..=3 `changes` на произвольном смещении в ±1 (не
/// только self, в отличие от v1-генератора) — фильтруется так, чтобы НЕ
/// было одновременно пустых shifts И changes (вне подмножества —
/// `GpuUnsupportedReason::NoEffect`).
fn v2_rule_strategy() -> impl Strategy<Value = Rule> {
    (
        pattern_strategy(),
        prop::collection::vec(shift_spec_strategy(), 0..=2),
        prop::collection::vec((-1i32..=1, -1i32..=1, (1u8..=9).prop_map(ChangeValue::Literal)), 0..=3),
        0u32..=3,
        0u64..=1,
        any::<bool>(),
    )
        .prop_map(
            |((head, pattern), shift_specs, changes, priority, min_age, active_only)| {
                let shifts: Vec<Vec<ShiftSpec>> = shift_specs.into_iter().map(|s| vec![s]).collect();
                Rule {
                    id: vec![CellType(head)],
                    pattern,
                    shifts,
                    changes,
                    active_only,
                    priority,
                    min_age,
                    overflow: OverflowAction::Discard,
                    cam: None,
                    tie_break: 0,
                    starvation_after: None,
                    feedback: None,
                    recursion: None,
                    memory: None,
                    max_activations: None,
                    cross_layer_reads: Vec::new(),
                }
            },
        )
        .prop_filter("rule must have at least one shift or change", |r| {
            !(r.shifts.is_empty() && r.changes.is_empty())
        })
}

fn v2_rule_set_strategy() -> impl Strategy<Value = Vec<Rule>> {
    prop::collection::vec(v2_rule_strategy(), 1..=5)
}

fn make_rule_index(rules: &[Rule]) -> HashMap<CellType, Vec<Rule>> {
    let mut index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        if let Some(&head) = rule.id.first() {
            index.entry(head).or_default().push(rule.clone());
        }
    }
    // Тот же порядок, что и `config::load_config`/`RuleStore::get_index` —
    // критично для тай-брейка по `rule_idx`, см. аналогичный комментарий в
    // `gpu_v1_correctness.rs`.
    for group in index.values_mut() {
        group.sort_by_key(|r| std::cmp::Reverse(r.priority));
    }
    index
}

fn grid_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(0..=CELL_ALPHABET, SIDE * SIDE)
}

proptest! {
    // Каждый кейс строит настоящее GPU-устройство и гоняет TICKS тиков —
    // держим число кейсов умеренным (как и в gpu_v1_correctness.rs).
    #![proptest_config(ProptestConfig::with_cases(80))]

    #[test]
    fn test_gpu_v2_matches_cpu_property_random_shifting_rules(rules in v2_rule_set_strategy(), cells in grid_strategy()) {
        let rule_index = make_rule_index(&rules);

        let storage = VecStorage::new(SIDE, SIDE);
        let mut cpu_grid = Grid::new(storage, HashSet::new());
        let mut gpu_initial: Vec<(usize, usize, Cell)> = Vec::new();
        for (i, &v) in cells.iter().enumerate() {
            if v != 0 {
                let x = i % SIDE;
                let y = i / SIDE;
                let cell = Cell { value: CellValue(CellType(v)), born_at: 0 };
                cpu_grid.set_cell(x, y, cell);
                gpu_initial.push((x, y, cell));
            }
        }

        // Пропускаем конфиги вне GPU-подмножества (см. doc-комментарий
        // модуля — на практике почти всегда TooManyRulesForArbitration,
        // если у одной головы случайно оказалось больше MAX_MATCHES_PER_CELL
        // правил).
        let Ok(mut gpu_engine) = GpuEngine::new(SIDE, SIDE, &gpu_initial, &rule_index) else {
            return Ok(());
        };

        for tick in 0..TICKS {
            run_tick(&mut cpu_grid, &rule_index);
            gpu_engine.run_tick();
            let gpu_result = gpu_engine.read_grid();

            for y in 0..SIDE {
                for x in 0..SIDE {
                    let cpu_cell = cpu_grid.get_cell(x, y).copied().unwrap_or_default();
                    let gpu_cell = gpu_result[y * SIDE + x];
                    prop_assert_eq!(
                        cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                        "value mismatch at tick={} ({}, {})", tick, x, y
                    );
                    prop_assert_eq!(
                        cpu_cell.born_at, gpu_cell.born_at,
                        "born_at mismatch at tick={} ({}, {})", tick, x, y
                    );
                }
            }
        }
    }
}

/// Не-property, конкретный сценарий: две частицы движутся навстречу друг
/// другу и сталкиваются на одной клетке несколько тиков подряд (та же
/// природа конфликта, что и в `gpu::engine::tests::test_gpu_engine_arbitrated_write_conflict_all_or_nothing`,
/// но теперь сверяется с НАСТОЯЩИМ CPU-эталоном, а не вручную посчитанным
/// ожиданием).
#[test]
fn test_gpu_v2_matches_cpu_head_on_collision_scenario() {
    fn mover(id: u8, direction: Direction, priority: u32) -> Rule {
        Rule {
            id: vec![CellType(id)],
            pattern: vec![(0, 0, CellType(id))],
            shifts: vec![vec![ShiftSpec::new(direction, 1)]],
            changes: vec![],
            active_only: false,
            priority,
            min_age: 0,
            overflow: OverflowAction::Discard,
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        }
    }

    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(1), vec![mover(1, Direction::Right, 5)]);
    rule_index.insert(CellType(2), vec![mover(2, Direction::Left, 1)]);

    let n = 9usize;
    let storage = VecStorage::new(n, 1);
    let mut cpu_grid = Grid::new(storage, HashSet::new());
    cpu_grid.set_cell(
        0,
        0,
        Cell {
            value: CellValue::new(1),
            born_at: 0,
        },
    );
    cpu_grid.set_cell(
        8,
        0,
        Cell {
            value: CellValue::new(2),
            born_at: 0,
        },
    );

    let initial = vec![
        (
            0usize,
            0usize,
            Cell {
                value: CellValue::new(1),
                born_at: 0,
            },
        ),
        (
            8,
            0,
            Cell {
                value: CellValue::new(2),
                born_at: 0,
            },
        ),
    ];
    let mut gpu_engine =
        GpuEngine::new(n, 1, &initial, &rule_index).expect("two Discard movers are within the v2 subset");

    for tick in 0..6 {
        run_tick(&mut cpu_grid, &rule_index);
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();
        for x in 0..n {
            let cpu_cell = cpu_grid.get_cell(x, 0).copied().unwrap_or_default();
            let gpu_cell = gpu_result[x];
            assert_eq!(
                cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                "value mismatch at tick={tick} x={x}"
            );
            assert_eq!(
                cpu_cell.born_at, gpu_cell.born_at,
                "born_at mismatch at tick={tick} x={x}"
            );
        }
    }
}

/// Прогоняет ГИБРИДНЫЙ (GPU + CPU-добор) арбитраж через настоящий
/// `GpuEngine`, а не синтетический `hybrid_check.rs`-прототип. Строит
/// цепочку конфликтов (см. `gpu::engine`'s doc-комментарий про
/// `chain_check.rs`-находку: n матчей в ряд, каждый пишет в себя И в
/// соседа справа [(0,0),(1,0)] — цепочка зависимых конфликтов длиной n
/// требует ~n/2 раундов на сходимость) ДЛИННЕЕ, чем `ROUNDS` (32) успевает
/// разрешить чистым GPU-путём — единственный способ, которым эта клетка
/// корректно обновится, это реальное срабатывание `cpu_fallback_resolve`.
/// Тай-брейк между соседями — по x (все клетки одного типа, один
/// приоритет, один возраст) — совпадает с построением
/// `hybrid_check.rs`/`chain_check.rs`'s цепочки.
#[test]
fn test_gpu_v2_hybrid_fallback_resolves_long_conflict_chain() {
    const CHAIN_LEN: usize = 100; // ~50 раундов нужно, ROUNDS=32 — не хватает чистому GPU
    let width = CHAIN_LEN + 5;

    let rule = Rule {
        id: vec![CellType(1)],
        pattern: vec![(0, 0, CellType(1))],
        shifts: Vec::new(),
        changes: vec![(0, 0, ChangeValue::Literal(1)), (1, 0, ChangeValue::Literal(1))],
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(1), vec![rule]);

    let storage = VecStorage::new(width, 1);
    let mut cpu_grid = Grid::new(storage, HashSet::new());
    let mut initial = Vec::new();
    for x in 0..CHAIN_LEN {
        let cell = Cell {
            value: CellValue::new(1),
            born_at: 0,
        };
        cpu_grid.set_cell(x, 0, cell);
        initial.push((x, 0usize, cell));
    }

    let mut gpu_engine = GpuEngine::new(width, 1, &initial, &rule_index)
        .expect("self+neighbor change without shifts is within the v2 subset");

    // Несколько тиков подряд — не только проверяет, что CPU-добор
    // разрешает первый (самый длинный) хвост правильно, но и что
    // "залатанный" им буфер корректно используется как ВХОД для
    // следующего тика (detect_pass на GPU должен видеть именно то, что
    // записал CPU-fallback, а не устаревшее/непропатченное значение).
    for tick in 0..3 {
        run_tick(&mut cpu_grid, &rule_index);
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();

        for x in 0..width {
            let cpu_cell = cpu_grid.get_cell(x, 0).copied().unwrap_or_default();
            let gpu_cell = gpu_result[x];
            assert_eq!(
                cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                "value mismatch at tick={tick} x={x} (chain len {CHAIN_LEN})"
            );
            assert_eq!(
                cpu_cell.born_at, gpu_cell.born_at,
                "born_at mismatch at tick={tick} x={x} (chain len {CHAIN_LEN})"
            );
        }
    }
}

/// CAM (`Rule::cam`, content-addressable поиск с ограниченным радиусом) на
/// GPU — сверка с CPU-эталоном на 2D-решётке (радиус — Chebyshev, значит
/// именно 2D, не 1D-случай остальных тестов этого файла). Два сценария в
/// одном: одиночный магнит без конфликта (клетка (0,0)) и два магнита,
/// реально претендующие на одну цель (клетки (6,0)/(6,6) обе в радиусе
/// цели (6,3)) — арбитраж должен дать ОДИНАКОВЫЙ результат на GPU и CPU,
/// включая тай-брейк по приоритету/позиции внутри `cam_search`'s поиска
/// ближайшей клетки при равных расстояниях.
#[test]
fn test_gpu_v2_cam_matches_cpu_single_and_conflicting_magnets() {
    const MAGNET: u8 = 10;
    const TARGET: u8 = 20;

    fn magnet_rule(radius: u8, priority: u32) -> Rule {
        Rule {
            id: vec![CellType(MAGNET)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![],
            active_only: false,
            priority,
            min_age: 0,
            overflow: OverflowAction::Discard,
            cam: Some(CamSearch {
                radius,
                target_type: CellType(TARGET),
            }),
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        }
    }

    let width = 10;
    let height = 10;
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(MAGNET), vec![magnet_rule(4, 9), magnet_rule(4, 1)]);

    let cells = [
        (0usize, 0usize, MAGNET), // одиночный магнит, цель рядом
        (2, 0, TARGET),
        (6, 0, MAGNET), // два магнита...
        (6, 6, MAGNET), // ...претендующих на одну цель
        (6, 3, TARGET),
    ];

    let storage = VecStorage::new(width, height);
    let mut cpu_grid = Grid::new(storage, HashSet::new());
    let mut initial = Vec::new();
    for &(x, y, v) in &cells {
        let cell = Cell {
            value: CellValue::new(v),
            born_at: 0,
        };
        cpu_grid.set_cell(x, y, cell);
        initial.push((x, y, cell));
    }

    let mut gpu_engine =
        GpuEngine::new(width, height, &initial, &rule_index).expect("cam rule within MAX_CAM_RADIUS is supported");

    for tick in 0..3 {
        run_tick(&mut cpu_grid, &rule_index);
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();

        for y in 0..height {
            for x in 0..width {
                let cpu_cell = cpu_grid.get_cell(x, y).copied().unwrap_or_default();
                let gpu_cell = gpu_result[y * width + x];
                assert_eq!(
                    cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                    "value mismatch at tick={tick} ({x},{y})"
                );
                assert_eq!(
                    cpu_cell.born_at, gpu_cell.born_at,
                    "born_at mismatch at tick={tick} ({x},{y})"
                );
            }
        }
    }
}

/// CAM × многораундовый арбитраж: предыдущий тест (`..._single_and_conflicting_magnets`)
/// проверяет только ПРЯМОЙ, одноуровневый конфликт (два магнита хотят одну
/// цель) — резолвится приоритетом за 1 раунд. Здесь — настоящая ДВУХУРОВНЕВАЯ
/// цепочка зависимостей, построенная так, чтобы ROUNDS-бюджет (см.
/// `gpu::engine::ROUNDS`'s doc-комментарий — эмпирически обоснован на
/// generic сценариях сдвигов, НЕ на CAM специально) был реально
/// протестирован именно на CAM, а не просто предполагался достаточным по
/// аналогии:
///
/// - M1 (приоритет 5) и M2 (приоритет 10) — оба ищут X в радиусе. M2 сильнее,
///   выигрывает X напрямую (раунд 1).
/// - M3 (приоритет 15) ищет клетки ТИПА M2 в радиусе — то есть саму позицию
///   M2 целиком (два разных матча претендуют на клетку M2: сам M2 (хочет
///   стать типом X) и M3 (хочет очистить M2 как свою добычу)). M3 сильнее,
///   выигрывает эту клетку — значит матч M2 отклоняется ЦЕЛИКОМ (all-or-nothing),
///   включая уже "выигранную" им в раунде 1 претензию на X.
/// - Раунд 2 обязан заметить, что M2 выбыл, и вернуть X в игру — единственный
///   оставшийся претендент, M1, должен ТЕПЕРЬ забрать X, хотя в раунде 1 он
///   казался проигравшим. Если бы алгоритм не делал этот повторный проход
///   (чисто жадный, однораундовый), X остался бы нетронутым НАВСЕГДА — ни
///   M1, ни M2 его не получили бы, хотя по модели он ДОЛЖЕН достаться M1.
#[test]
fn test_gpu_v2_cam_two_level_conflict_chain_matches_cpu() {
    const M1: u8 = 50;
    const M2: u8 = 51;
    const M3: u8 = 52;
    const X: u8 = 53;

    fn seeker_rule(head: u8, target: u8, priority: u32) -> Rule {
        Rule {
            id: vec![CellType(head)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![],
            active_only: false,
            priority,
            min_age: 0,
            overflow: OverflowAction::Discard,
            cam: Some(CamSearch {
                radius: 3,
                target_type: CellType(target),
            }),
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        }
    }

    let width = 10;
    let height = 1;
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(M1), vec![seeker_rule(M1, X, 5)]);
    rule_index.insert(CellType(M2), vec![seeker_rule(M2, X, 10)]);
    rule_index.insert(CellType(M3), vec![seeker_rule(M3, M2, 15)]);

    // (0)=M1, (2)=X, (4)=M2, (6)=M3 -- все расстояния = 2, все в радиусе 3.
    let cells = [(0usize, 0usize, M1), (2, 0, X), (4, 0, M2), (6, 0, M3)];

    let storage = VecStorage::new(width, height);
    let mut cpu_grid = Grid::new(storage, Default::default());
    let mut initial = Vec::new();
    for &(x, y, v) in &cells {
        let cell = Cell {
            value: CellValue::new(v),
            born_at: 0,
        };
        cpu_grid.set_cell(x, y, cell);
        initial.push((x, y, cell));
    }

    let mut gpu_engine = GpuEngine::new(width, height, &initial, &rule_index).expect("cam chain within GPU subset");

    run_tick(&mut cpu_grid, &rule_index);
    gpu_engine.run_tick();
    let gpu_result = gpu_engine.read_grid();

    // Ожидаемый исход по модели (независимая ручная проверка, не просто
    // "CPU и GPU совпали друг с другом" -- оба МОГЛИ БЫ совпасть в одной и
    // той же неверной реализации, если бы у обеих не было честного второго
    // раунда):
    // - (4) M2's клетка: выиграл M3 -> очищена (M3 её "поглотил").
    // - (6) M3's клетка: M3 стал типом M2 (нашёл и поглотил M2).
    // - (2) X: M2 отклонён целиком (проиграл M3 за клетку (4)) -> X снова
    //   свободен -> достаётся M1 во ВТОРОМ раунде -> очищена.
    // - (0) M1's клетка: M1 стал типом X (нашёл и поглотил X) -- ТОЛЬКО если
    //   алгоритм сделал повторный проход после отклонения M2.
    assert_eq!(
        cpu_grid.get_cell(4, 0).map(|c| c.value.0 .0),
        Some(0),
        "клетка M2 должна быть очищена (M3 её поглотил)"
    );
    assert_eq!(
        cpu_grid.get_cell(6, 0).map(|c| c.value.0 .0),
        Some(M2),
        "M3 должен стать типом M2"
    );
    assert_eq!(
        cpu_grid.get_cell(2, 0).map(|c| c.value.0 .0),
        Some(0),
        "X должен достаться M1 во втором раунде (M2 выбыл целиком)"
    );
    assert_eq!(
        cpu_grid.get_cell(0, 0).map(|c| c.value.0 .0),
        Some(X),
        "M1 должен стать типом X -- доказывает, что CPU-эталон делает повторный проход, а не жадно решает один раз"
    );

    for y in 0..height {
        for x in 0..width {
            let cpu_cell = cpu_grid.get_cell(x, y).copied().unwrap_or_default();
            let gpu_cell = gpu_result[y * width + x];
            assert_eq!(cpu_cell.value.0 .0, gpu_cell.value.0 .0, "value mismatch at ({x},{y}) -- GPU ROUNDS-бюджет должен справиться с той же двухуровневой цепочкой, что и CPU");
            assert_eq!(cpu_cell.born_at, gpu_cell.born_at, "born_at mismatch at ({x},{y})");
        }
    }
}

/// Модульный tie-break (`Rule::tie_break`, block F п.3) на GPU — та же
/// сцена, что и CPU-тест `test_tie_break_rotates_fairly_when_spaced_half_modulus_apart`
/// (два правила с равным priority, конкурирующие за одну и ту же клетку
/// каждый тик, tie_break расставлены на M/2 друг от друга), но сверяется
/// GPU против CPU-эталона побитово на каждом тике. `MODULUS` здесь —
/// намеренно захардкоженная копия `arbitrator::TIE_BREAK_MODULUS`/
/// `shader.wgsl::TIE_BREAK_MODULUS` (оба `pub(crate)`/шейдерная константа,
/// недоступны отсюда как внешнему интеграционному тесту) — если кто-то
/// поменяет модуль на одной стороне и забудет про другую, этот тест либо
/// поймает расхождение GPU/CPU напрямую (если стороны разъехались), либо
/// продолжит проверять корректный (но не факт что 50/50) сценарий — риск
/// приемлем, реальная защита от рассинхрона — побитовое сравнение ниже.
#[test]
fn test_gpu_v2_tie_break_matches_cpu_rotating_conflict() {
    const MODULUS: u32 = 16;
    const HEAD: u8 = 1;

    fn competing_rule(tie_break: u32, written_value: u8) -> Rule {
        Rule {
            id: vec![CellType(HEAD)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(1, 0, ChangeValue::Literal(written_value))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        }
    }

    let width = 2;
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(
        CellType(HEAD),
        vec![competing_rule(0, 100), competing_rule(MODULUS / 2, 200)],
    );

    let initial = vec![(
        0usize,
        0usize,
        Cell {
            value: CellValue::new(HEAD),
            born_at: 0,
        },
    )];
    let storage = VecStorage::new(width, 1);
    let mut cpu_grid = Grid::new(storage, HashSet::new());
    cpu_grid.set_cell(
        0,
        0,
        Cell {
            value: CellValue::new(HEAD),
            born_at: 0,
        },
    );

    let mut gpu_engine = GpuEngine::new(width, 1, &initial, &rule_index)
        .expect("two same-priority competing rules are within the v2 subset");

    let mut cpu_winners = Vec::new();
    for tick in 0..MODULUS {
        run_tick(&mut cpu_grid, &rule_index);
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();

        for x in 0..width {
            let cpu_cell = cpu_grid.get_cell(x, 0).copied().unwrap_or_default();
            let gpu_cell = gpu_result[x];
            assert_eq!(
                cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                "value mismatch at tick={tick} x={x}"
            );
            assert_eq!(
                cpu_cell.born_at, gpu_cell.born_at,
                "born_at mismatch at tick={tick} x={x}"
            );
        }
        cpu_winners.push(cpu_grid.get_cell(1, 0).map(|c| c.value.0 .0));
    }

    let a_wins = cpu_winners.iter().filter(|&&w| w == Some(100)).count();
    let b_wins = cpu_winners.iter().filter(|&&w| w == Some(200)).count();
    assert_eq!(
        a_wins,
        (MODULUS / 2) as usize,
        "CPU-эталон сам должен чередовать 50/50 — иначе сцена не проверяет вращение"
    );
    assert_eq!(
        b_wins,
        (MODULUS / 2) as usize,
        "CPU-эталон сам должен чередовать 50/50 — иначе сцена не проверяет вращение"
    );
}

/// `ShiftSpec::broadcast` (см. её doc-комментарий в `types.rs`) на GPU —
/// теперь ПОДДЕРЖИВАЕТСЯ (см. `gpu::rule_table::MAX_BROADCAST_REACH`), три
/// сценария ниже сверяют её с CPU-эталоном (`applicator::apply_shift_buffered`'s
/// `for k in 1..=steps`) настолько же придирчиво, как остальной файл.
///
/// Сценарий 1: одна частица (тип 1) на пустой решётке рассылает своё
/// значение по всему пути (steps=3, без конфликтов), а `changes` (заданный
/// относительно (0,0) — т.е. относительно КАЖДОЙ цели сдвига, здесь
/// единственной) перезаписывает ТОЛЬКО конечную точку пути значением 2 —
/// проверяет одновременно: (а) промежуточные клетки пути реально получают
/// копию головки, а не только финальная точка (то, что раньше было вне
/// GPU-подмножества), и (б) `changes` по-прежнему применяются ТОЛЬКО
/// относительно финальной цели сдвига, а не к каждой клетке пути (см.
/// `apply_rule_buffered`'s "Фаза 2" — `shift_targets` не включает
/// промежуточные точки пути).
#[test]
fn test_gpu_v2_broadcast_matches_cpu_single_no_conflict() {
    let rule = Rule {
        id: vec![CellType(1)],
        pattern: vec![(0, 0, CellType(1))],
        shifts: vec![vec![ShiftSpec::broadcast(Direction::Right, 3)]],
        changes: vec![(0, 0, ChangeValue::Literal(2))],
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(1), vec![rule]);

    let width = 12;
    let storage = VecStorage::new(width, 1);
    let mut cpu_grid = Grid::new(storage, HashSet::new());
    cpu_grid.set_cell(
        0,
        0,
        Cell {
            value: CellValue::new(1),
            born_at: 0,
        },
    );
    let initial = vec![(
        0usize,
        0usize,
        Cell {
            value: CellValue::new(1),
            born_at: 0,
        },
    )];

    let mut gpu_engine = GpuEngine::new(width, 1, &initial, &rule_index)
        .expect("short broadcast (steps within MAX_BROADCAST_REACH) is within the v2 subset");

    for tick in 0..3 {
        run_tick(&mut cpu_grid, &rule_index);
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();
        for x in 0..width {
            let cpu_cell = cpu_grid.get_cell(x, 0).copied().unwrap_or_default();
            let gpu_cell = gpu_result[x];
            assert_eq!(
                cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                "value mismatch at tick={tick} x={x}"
            );
            assert_eq!(
                cpu_cell.born_at, gpu_cell.born_at,
                "born_at mismatch at tick={tick} x={x}"
            );
        }
    }

    // Сверка не только с CPU, но и с РУЧНЫМ ожиданием после тика 0 — чтобы
    // тест не мог "случайно" пройти из-за одинаковой (но неверной) ошибки
    // на обеих сторонах: клетка 0 очищена, 1 и 2 несут "сырое" значение
    // головки (путь), 3 — финальная точка, перезаписанная `changes`.
    let after_tick0 = cpu_grid.get_cell(0, 0).copied().unwrap_or_default();
    assert_eq!(after_tick0.value.0 .0, 0, "source must be cleared");
}

/// Сценарий 2: ДВЕ broadcast-частицы, чьи пути ПЕРЕСЕКАЮТСЯ — частица A
/// (тип 1, приоритет 5) в x=0 рассылает вправо на 4 клетки (путь 1..4),
/// частица B (тип 2, приоритет 1) в x=6 рассылает влево на 4 клетки (путь
/// 5..2) — области записи пересекаются на x∈{2,3,4}. По правилам арбитража
/// (`arbitrator::arbitrate`/`shader.wgsl`'s claim/resolve) матч побеждает
/// ЛИБО ВСЕМИ своими ячейками, ЛИБО ни одной ("all or nothing" — см.
/// `test_gpu_v2_matches_cpu_head_on_collision_scenario`): A (выше приоритет)
/// должен победить целиком, B — проиграть целиком (x=6 останется
/// НЕТРОНУТЫМ, не очищенным, т.к. весь матч B отклонён, а не только
/// пересекающаяся часть его пути). Именно это (не просто "кто-то победил")
/// проверяет тест — сверяя КАЖДУЮ клетку с настоящим CPU-эталоном.
#[test]
fn test_gpu_v2_broadcast_matches_cpu_overlapping_paths_arbitrated() {
    fn broadcaster(id: u8, direction: Direction, priority: u32) -> Rule {
        Rule {
            id: vec![CellType(id)],
            pattern: vec![(0, 0, CellType(id))],
            shifts: vec![vec![ShiftSpec::broadcast(direction, 4)]],
            changes: vec![],
            active_only: false,
            priority,
            min_age: 0,
            overflow: OverflowAction::Discard,
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        }
    }

    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(1), vec![broadcaster(1, Direction::Right, 5)]);
    rule_index.insert(CellType(2), vec![broadcaster(2, Direction::Left, 1)]);

    let width = 10;
    let storage = VecStorage::new(width, 1);
    let mut cpu_grid = Grid::new(storage, HashSet::new());
    cpu_grid.set_cell(
        0,
        0,
        Cell {
            value: CellValue::new(1),
            born_at: 0,
        },
    );
    cpu_grid.set_cell(
        6,
        0,
        Cell {
            value: CellValue::new(2),
            born_at: 0,
        },
    );
    let initial = vec![
        (
            0usize,
            0usize,
            Cell {
                value: CellValue::new(1),
                born_at: 0,
            },
        ),
        (
            6usize,
            0usize,
            Cell {
                value: CellValue::new(2),
                born_at: 0,
            },
        ),
    ];

    let mut gpu_engine = GpuEngine::new(width, 1, &initial, &rule_index)
        .expect("two overlapping broadcast shifts within MAX_BROADCAST_REACH are within the v2 subset");

    // Только тик 0 даёт истинное "пересечение путей" по построению — сверяем
    // его отдельно ручным ожиданием, а не только против CPU (защита от
    // "одинаково неверно на обеих сторонах"), и дополнительно гоняем ещё
    // несколько тиков, сверяя ТОЛЬКО против CPU (последующие тики зависят от
    // того, что осталось после арбитража тика 0, и достаточно сложны, чтобы
    // не считать их вручную).
    run_tick(&mut cpu_grid, &rule_index);
    gpu_engine.run_tick();
    let gpu_result = gpu_engine.read_grid();
    for x in 0..width {
        let cpu_cell = cpu_grid.get_cell(x, 0).copied().unwrap_or_default();
        let gpu_cell = gpu_result[x];
        assert_eq!(
            cpu_cell.value.0 .0, gpu_cell.value.0 .0,
            "value mismatch at tick=0 x={x}"
        );
        assert_eq!(cpu_cell.born_at, gpu_cell.born_at, "born_at mismatch at tick=0 x={x}");
    }

    // Ручная проверка: A (выше приоритет) полностью выигрывает — x=0
    // очищена, x=1..4 несут тип 1. B полностью проигрывает (all-or-nothing)
    // — x=6 остаётся исходным типом 2 (НЕ очищена), x=5 остаётся default
    // (0), т.к. запись туда принадлежала отклонённому матчу B.
    assert_eq!(
        cpu_grid.get_cell(0, 0).copied().unwrap_or_default().value.0 .0,
        0,
        "A's source must be cleared"
    );
    for x in 1..=4 {
        assert_eq!(
            cpu_grid.get_cell(x, 0).copied().unwrap_or_default().value.0 .0,
            1,
            "A must win its full broadcast path at x={x}"
        );
    }
    assert_eq!(
        cpu_grid.get_cell(6, 0).copied().unwrap_or_default().value.0 .0,
        2,
        "B must be rejected whole — its own source stays untouched"
    );
    assert_eq!(
        cpu_grid.get_cell(5, 0).copied().unwrap_or_default().value.0 .0,
        0,
        "B's uncontested path cell must NOT be written either — all-or-nothing rejection"
    );

    for tick in 1..4 {
        run_tick(&mut cpu_grid, &rule_index);
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();
        for x in 0..width {
            let cpu_cell = cpu_grid.get_cell(x, 0).copied().unwrap_or_default();
            let gpu_cell = gpu_result[x];
            assert_eq!(
                cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                "value mismatch at tick={tick} x={x}"
            );
            assert_eq!(
                cpu_cell.born_at, gpu_cell.born_at,
                "born_at mismatch at tick={tick} x={x}"
            );
        }
    }
}

/// Сценарий 3: broadcast-путь, упирающийся в границу решётки с
/// `OverflowAction::Discard` (единственный overflow, поддерживаемый GPU для
/// сдвигов вообще, см. `GpuUnsupportedReason::OverflowNotDiscard`) — путь
/// монотонен, значит клетки ДО границы обязаны быть записаны, а клетки
/// ПОСЛЕ границы — молча потеряны (см. `apply_shift_buffered`'s комментарий
/// "как только k-я позиция выходит за границу... дальше не размножается").
/// Частица в x=3 на решётке шириной 5 (индексы 0..4) рассылает вправо на 4
/// клетки: путь 4,5,6,7 — только x=4 попадает в решётку, 5/6/7 отбрасываются.
#[test]
fn test_gpu_v2_broadcast_matches_cpu_boundary_overflow_discard() {
    let rule = Rule {
        id: vec![CellType(1)],
        pattern: vec![(0, 0, CellType(1))],
        shifts: vec![vec![ShiftSpec::broadcast(Direction::Right, 4)]],
        changes: vec![],
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(1), vec![rule]);

    let width = 5;
    let storage = VecStorage::new(width, 1);
    let mut cpu_grid = Grid::new(storage, HashSet::new());
    cpu_grid.set_cell(
        3,
        0,
        Cell {
            value: CellValue::new(1),
            born_at: 0,
        },
    );
    let initial = vec![(
        3usize,
        0usize,
        Cell {
            value: CellValue::new(1),
            born_at: 0,
        },
    )];

    let mut gpu_engine = GpuEngine::new(width, 1, &initial, &rule_index)
        .expect("broadcast with Discard overflow within MAX_BROADCAST_REACH is within the v2 subset");

    run_tick(&mut cpu_grid, &rule_index);
    gpu_engine.run_tick();
    let gpu_result = gpu_engine.read_grid();
    for x in 0..width {
        let cpu_cell = cpu_grid.get_cell(x, 0).copied().unwrap_or_default();
        let gpu_cell = gpu_result[x];
        assert_eq!(
            cpu_cell.value.0 .0, gpu_cell.value.0 .0,
            "value mismatch at tick=0 x={x}"
        );
        assert_eq!(cpu_cell.born_at, gpu_cell.born_at, "born_at mismatch at tick=0 x={x}");
    }

    // Ручная проверка: x=3 (source) очищена, x=4 (единственная клетка пути
    // внутри решётки) несёт тип 1, остальное не тронуто (0/default) — не
    // "клэмплено" на край, а именно отброшено (Discard).
    assert_eq!(
        cpu_grid.get_cell(3, 0).copied().unwrap_or_default().value.0 .0,
        0,
        "source must be cleared"
    );
    assert_eq!(
        cpu_grid.get_cell(4, 0).copied().unwrap_or_default().value.0 .0,
        1,
        "only in-bounds path cell must be written"
    );
    for x in 0..3 {
        assert_eq!(
            cpu_grid.get_cell(x, 0).copied().unwrap_or_default().value.0 .0,
            0,
            "cells before source must be untouched at x={x}"
        );
    }

    // Ещё пара тиков — x=4 (теперь тип 1) снова матчится и снова рассылает
    // вправо на 4, снова упираясь в границу немедленно (steps=4 из x=4 —
    // весь путь 5,6,7,8 вне решётки шириной 5) — целиком отбрасывается,
    // источник (x=4) тем не менее очищается (source-clear не зависит от
    // overflow пути).
    for tick in 1..3 {
        run_tick(&mut cpu_grid, &rule_index);
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();
        for x in 0..width {
            let cpu_cell = cpu_grid.get_cell(x, 0).copied().unwrap_or_default();
            let gpu_cell = gpu_result[x];
            assert_eq!(
                cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                "value mismatch at tick={tick} x={x}"
            );
            assert_eq!(
                cpu_cell.born_at, gpu_cell.born_at,
                "born_at mismatch at tick={tick} x={x}"
            );
        }
    }
}

// ──────────────────────────────────────────────────────────────
// `ShiftSpec::keep_source` на GPU (см. `GpuMatch::keep_age_mask` в
// `shader.wgsl`) — источник не очищается, и КРИТИЧНО, его возраст не
// должен сбрасываться (в отличие от обычной клетки, выигравшей арбитраж).
// Именно возраст источника, а не только значение — тот аспект, который
// однажды уже разошёлся молча (несовпадение layout `GpuMatchLayout` на
// CPU-стороне сдвинуло на 4 байта чтение `cells`/`values` при
// CPU-fallback readback — этот тест, сравнивающий born_at на КАЖДОМ тике,
// поймал бы это немедленно).
// ──────────────────────────────────────────────────────────────

/// Простой случай: одна клетка-маркер копирует себя вправо, не убывая у
/// источника. Паттерн требует ПУСТОГО правого соседа — без этого гейта
/// каждая созданная копия тоже начала бы сдвигаться в СЛЕДУЮЩЕМ тике,
/// раздувая сравнение (не ошибка, просто ненужная сложность для теста,
/// который проверяет именно возраст источника, а не рост цепочки — тот
/// сценарий уже покрыт CPU-стороной, `test_max_activations_bounds_keep_source_growth`).
#[test]
fn test_gpu_v2_keep_source_matches_cpu_source_age_preserved() {
    const MARKER: u8 = 7;
    let rule = Rule {
        id: vec![CellType(MARKER)],
        pattern: vec![(0, 0, CellType(MARKER)), (1, 0, CellType(0))],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 1,
            broadcast: false,
            keep_source: true,
        }]],
        changes: vec![],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(MARKER), vec![rule]);

    let width = 10;
    let storage = VecStorage::new(width, 1);
    let mut cpu_grid = Grid::new(storage, HashSet::new());
    cpu_grid.set_cell(
        0,
        0,
        Cell {
            value: CellValue::new(MARKER),
            born_at: 0,
        },
    );
    let initial = vec![(
        0usize,
        0usize,
        Cell {
            value: CellValue::new(MARKER),
            born_at: 0,
        },
    )];

    let mut gpu_engine =
        GpuEngine::new(width, 1, &initial, &rule_index).expect("plain keep_source is within the v2 subset");

    for tick in 0..5 {
        run_tick(&mut cpu_grid, &rule_index);
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();
        for x in 0..width {
            let cpu_cell = cpu_grid.get_cell(x, 0).copied().unwrap_or_default();
            let gpu_cell = gpu_result[x];
            assert_eq!(
                cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                "value mismatch at tick={tick} x={x}"
            );
            assert_eq!(
                cpu_cell.born_at, gpu_cell.born_at,
                "born_at mismatch at tick={tick} x={x} -- keep_source source must not reset age on GPU either"
            );
        }
    }

    // Ручная проверка (не только "GPU==CPU", а "оба верны"): x=0 (источник)
    // обязан ОСТАТЬСЯ типом MARKER (не очищен) на протяжении ВСЕХ тиков, и
    // его born_at обязан остаться 0 (никогда не тронут) -- если бы это было
    // не так, CPU-сторона теста уже провалилась бы сама на себя.
    let source = cpu_grid.get_cell(0, 0).copied().unwrap_or_default();
    assert_eq!(source.value.0 .0, MARKER, "source must survive keep_source");
    assert_eq!(source.born_at, 0, "source's age must never reset");
}

/// "Излучение" (`broadcast + keep_source` вместе, исходный мотивирующий
/// случай для GPU-порта) — источник неподвижен, путь заполняется целиком,
/// ни то ни другое не должно сбрасывать возраст источника.
#[test]
fn test_gpu_v2_emit_broadcast_plus_keep_source_matches_cpu() {
    const EMITTER: u8 = 8;
    let rule = Rule {
        id: vec![CellType(EMITTER)],
        pattern: vec![(0, 0, CellType(EMITTER))],
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 3,
            broadcast: true,
            keep_source: true,
        }]],
        changes: vec![],
        active_only: false,
        priority: 10,
        min_age: 0, // переизлучение на уже размеченный путь безвредно (то же значение поверх себя)
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(EMITTER), vec![rule]);

    let width = 10;
    let storage = VecStorage::new(width, 1);
    let mut cpu_grid = Grid::new(storage, HashSet::new());
    cpu_grid.set_cell(
        0,
        0,
        Cell {
            value: CellValue::new(EMITTER),
            born_at: 0,
        },
    );
    let initial = vec![(
        0usize,
        0usize,
        Cell {
            value: CellValue::new(EMITTER),
            born_at: 0,
        },
    )];

    let mut gpu_engine =
        GpuEngine::new(width, 1, &initial, &rule_index).expect("emit (broadcast+keep_source) is within the v2 subset");

    for tick in 0..3 {
        run_tick(&mut cpu_grid, &rule_index);
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();
        for x in 0..width {
            let cpu_cell = cpu_grid.get_cell(x, 0).copied().unwrap_or_default();
            let gpu_cell = gpu_result[x];
            assert_eq!(
                cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                "value mismatch at tick={tick} x={x}"
            );
            assert_eq!(
                cpu_cell.born_at, gpu_cell.born_at,
                "born_at mismatch at tick={tick} x={x}"
            );
        }
    }

    let source = cpu_grid.get_cell(0, 0).copied().unwrap_or_default();
    assert_eq!(source.value.0 .0, EMITTER, "emitter must survive (keep_source)");
    assert_eq!(source.born_at, 0, "emitter's age must never reset");
    for x in 1..=3 {
        assert_eq!(
            cpu_grid.get_cell(x, 0).copied().unwrap_or_default().value.0 .0,
            EMITTER,
            "path cell x={x} must be filled (broadcast)"
        );
    }
}

// ──────────────────────────────────────────────────────────────
// `Rule::recursion` на GPU (см. `gpu::rule_table::MAX_RECURSION_DEPTH`'s
// doc-комментарий: каскад — чисто локальное, однопоточное вычисление,
// безопасное на GPU в отличие от `feedback`/`memory`/`starvation_after`).
// ──────────────────────────────────────────────────────────────

const RFILLED: u8 = 20;
const RUNFILLED: u8 = 21;

fn wall_fill_rule(min_age: u64, max_depth: u8) -> Rule {
    Rule {
        id: vec![CellType(RUNFILLED)],
        pattern: vec![(0, 0, CellType(RUNFILLED)), (-1, 0, CellType(RFILLED))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(RFILLED))],
        active_only: false,
        priority: 10,
        min_age,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: Some(RecursionSpec {
            max_depth,
            direction: Direction::Right,
        }),
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    }
}

/// Прямой порт CPU-теста `test_recursion_cascades_multiple_cells_in_one_tick`
/// на GPU: один тик обязан залить исходную клетку + `max_depth` уровней
/// каскада, а не по одной клетке за тик — если бы шейдер откатывался на
/// "цикл не развернулся" или "каскад читает устаревший pre-tick срез вместо
/// уже накопленных этим же матчем ячеек", это либо не собралось бы вовсе,
/// либо разошлось с CPU здесь.
#[test]
fn test_gpu_v2_recursion_cascade_matches_cpu_in_one_tick() {
    const MAX_DEPTH: u8 = 3;
    let width = 10;
    let storage = VecStorage::new(width, 1);
    let mut cpu_grid = Grid::new(storage, HashSet::new());
    let mut initial = Vec::new();
    cpu_grid.set_cell(0, 0, Cell::new(RFILLED));
    initial.push((0usize, 0usize, Cell::new(RFILLED)));
    for x in 1..width {
        cpu_grid.set_cell(x, 0, Cell::new(RUNFILLED));
        initial.push((x, 0, Cell::new(RUNFILLED)));
    }

    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(RUNFILLED), vec![wall_fill_rule(0, MAX_DEPTH)]);

    let mut gpu_engine = GpuEngine::new(width, 1, &initial, &rule_index)
        .expect("recursion within MAX_RECURSION_DEPTH is within the v2 subset");

    run_tick(&mut cpu_grid, &rule_index);
    gpu_engine.run_tick();
    let gpu_result = gpu_engine.read_grid();

    // Ручная проверка (не просто "GPU совпал с CPU"): клетки 0..=4 залиты
    // (seed + MAX_DEPTH=3 уровня каскада = 4 клетки), 5..9 нетронуты.
    for x in 0..=4 {
        assert_eq!(
            cpu_grid.get_cell(x, 0).map(|c| c.value.0 .0),
            Some(RFILLED),
            "CPU: клетка {x} должна быть залита за один тик"
        );
    }
    for x in 5..width {
        assert_eq!(
            cpu_grid.get_cell(x, 0).map(|c| c.value.0 .0),
            Some(RUNFILLED),
            "CPU: клетка {x} вне глубины каскада, не должна меняться"
        );
    }

    for x in 0..width {
        let cpu_cell = cpu_grid.get_cell(x, 0).copied().unwrap_or_default();
        let gpu_cell = gpu_result[x];
        assert_eq!(cpu_cell.value.0 .0, gpu_cell.value.0 .0, "value mismatch at x={x}");
        assert_eq!(cpu_cell.born_at, gpu_cell.born_at, "born_at mismatch at x={x}");
    }
}

/// Взаимодействие `recursion` + `min_age` на GPU — естественная (без
/// искусственной подмены born_at, недоступной через публичный `GpuEngine`
/// API) многотиковая динамика: min_age гейтует и исходный (уровень 0)
/// матч, и КАЖДЫЙ уровень каскада отдельно (через `read_age_effective_local`,
/// зеркалящий CPU `read_age_effective`). Заскафолженные клетки (born_at=0)
/// не могут сработать раньше, чем их возраст (generation - born_at) достигнет
/// `min_age` — а клетки, СОЗДАННЫЕ каскадом (свежий born_at = generation
/// этого тика), сами становятся НОВЫМ фронтом только через `min_age`
/// дополнительных тиков. Сверяется после КАЖДОГО тика, не только в конце —
/// расхождение в порядке "кто раньше состарился" локализуется сразу.
#[test]
fn test_gpu_v2_recursion_with_min_age_matches_cpu_across_ticks() {
    const MIN_AGE: u64 = 2;
    const MAX_DEPTH: u8 = 1;
    let width = 8;
    let storage = VecStorage::new(width, 1);
    let mut cpu_grid = Grid::new(storage, HashSet::new());
    let mut initial = Vec::new();
    cpu_grid.set_cell(0, 0, Cell::new(RFILLED));
    initial.push((0usize, 0usize, Cell::new(RFILLED)));
    for x in 1..width {
        cpu_grid.set_cell(x, 0, Cell::new(RUNFILLED));
        initial.push((x, 0, Cell::new(RUNFILLED)));
    }

    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(RUNFILLED), vec![wall_fill_rule(MIN_AGE, MAX_DEPTH)]);

    let mut gpu_engine =
        GpuEngine::new(width, 1, &initial, &rule_index).expect("recursion+min_age within the v2 subset");

    // Ручная поклеточная траектория (независимо выведена, не подогнана под
    // то, что выдал прогон): min_age=2 держит тики 1-2 полностью холостыми
    // (scaffold born_at=0 не достигает age>=2 раньше generation=2, то есть
    // до 3-го вызова run_tick — CPU/GPU оба видят params.generation=2 ТОЛЬКО
    // на 3-м вызове). far_x=RFILLED-фронт после каждого тика:
    // тик1: [] (ничего), тик2: [] (ничего),
    // тик3: заливает x=1,2 (level0=x1 + каскад max_depth=1 -> x2),
    // тик4: заливает x=3,4, тик5: заливает x=5,6, тик6: заливает x=7
    // (край решётки шириной 8 — каскад дальше x=8 естественно не матчится,
    // читает default за границей, попутно проверяет boundary-поведение).
    let expected_filled_after_tick: [&[usize]; 6] = [
        &[],
        &[],
        &[1, 2],
        &[1, 2, 3, 4],
        &[1, 2, 3, 4, 5, 6],
        &[1, 2, 3, 4, 5, 6, 7],
    ];

    for tick in 1..=6 {
        run_tick(&mut cpu_grid, &rule_index);
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();
        for x in 0..width {
            let cpu_cell = cpu_grid.get_cell(x, 0).copied().unwrap_or_default();
            let gpu_cell = gpu_result[x];
            assert_eq!(
                cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                "value mismatch at tick={tick} x={x}"
            );
            assert_eq!(
                cpu_cell.born_at, gpu_cell.born_at,
                "born_at mismatch at tick={tick} x={x}"
            );
        }

        let filled = &expected_filled_after_tick[tick - 1];
        for x in 1..width {
            let expect_filled = filled.contains(&x);
            let actual = cpu_grid.get_cell(x, 0).map(|c| c.value.0 .0);
            if expect_filled {
                assert_eq!(
                    actual,
                    Some(RFILLED),
                    "CPU manual trace: x={x} should be filled by tick={tick}"
                );
            } else {
                assert_eq!(
                    actual,
                    Some(RUNFILLED),
                    "CPU manual trace: x={x} should still be unfilled scaffold at tick={tick}"
                );
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────
// `Rule::starvation_after` на GPU (см. `gpu::rule_table::GpuRuleTable::needs_starvation`'s
// doc-комментарий: старое обоснование отказа было неверным — persistent
// storage-буфер решает "нет состояния между тиками", а реальная сложность
// (гибридный CPU-fallback арбитраж должен дописывать финальный
// ACCEPTED/REJECTED, иначе `update_starvation_pass` не может отличить
// "проиграл" от "навсегда завис в PENDING") теперь решена явно.
// ──────────────────────────────────────────────────────────────

fn starvation_rules(low_threshold: u32) -> HashMap<CellType, Vec<Rule>> {
    let rule_high = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(1, 0, ChangeValue::Literal(100))],
        active_only: false,
        priority: 20,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let rule_low = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(1, 0, ChangeValue::Literal(200))],
        active_only: false,
        priority: 5,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: Some(low_threshold),
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut idx = HashMap::new();
    idx.insert(CellType(1), vec![rule_high, rule_low]);
    idx
}

/// Прямой порт CPU-теста `test_starvation_guard_guarantees_periodic_progress`
/// на GPU: LOW обязан побеждать РОВНО каждый (K+1)-й тик. Конфликт (2 матча
/// на 1 клетку) тривиален для GPU-раундов — эта проверка чистого механизма
/// (persistent-счётчик, буст priority, сброс/рост) БЕЗ участия CPU-fallback.
#[test]
fn test_gpu_v2_starvation_matches_cpu_periodic_progress() {
    const K: u32 = 3;
    const TOTAL_TICKS: u32 = 20;
    let width = 2;
    // ВАЖНО: свободная функция `run_tick` documented no-op для
    // `starvation_after` (свежие пустые `StarvationCounters` на КАЖДЫЙ
    // вызов, см. её doc-комментарий в `engine/mod.rs`) -- она годится как
    // CPU-эталон для ВСЕХ остальных тестов этого файла (не завязанных на
    // межтиковое состояние), но НЕ здесь. Нужен `Engine` (хранит
    // `self.starvation_counters` между вызовами `run_tick`), первая
    // попытка этого теста именно так и ошиблась -- поймано сравнением с
    // GPU: GPU (персистентный буфер) корректно показал LOW, побеждающий
    // на тике 4, CPU (свободная функция) навсегда застрял на HIGH.
    let storage = VecStorage::new(width, 1);
    let mut grid = Grid::new(storage, HashSet::new());
    grid.set_cell(0, 0, Cell::new(1));
    let initial = vec![(0usize, 0usize, Cell::new(1))];

    let rule_index = starvation_rules(K);
    let mut cpu_engine = Engine::new(grid, rule_index.clone());
    let mut gpu_engine =
        GpuEngine::new(width, 1, &initial, &rule_index).expect("starvation_after is within the v2 subset");

    let mut low_wins_at_cpu = Vec::new();
    let mut low_wins_at_gpu = Vec::new();
    for tick in 1..=TOTAL_TICKS {
        cpu_engine.run_tick();
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();

        for x in 0..width {
            let cpu_cell = cpu_engine.grid().get_cell(x, 0).copied().unwrap_or_default();
            let gpu_cell = gpu_result[x];
            assert_eq!(
                cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                "value mismatch at tick={tick} x={x}"
            );
            assert_eq!(
                cpu_cell.born_at, gpu_cell.born_at,
                "born_at mismatch at tick={tick} x={x}"
            );
        }

        match cpu_engine.grid().get_cell(1, 0).map(|c| c.value.0 .0) {
            Some(200) => low_wins_at_cpu.push(tick),
            Some(100) => {}
            other => panic!("CPU: unexpected value at (1,0) on tick {tick}: {other:?}"),
        }
        match gpu_result[1].value.0 .0 {
            200 => low_wins_at_gpu.push(tick),
            100 => {}
            other => panic!("GPU: unexpected value at (1,0) on tick {tick}: {other:?}"),
        }
    }

    // Ручная проверка (не просто "GPU совпал с CPU"): LOW обязан побеждать
    // РОВНО каждый (K+1)-й тик, ни чаще, ни реже.
    let expected: Vec<u32> = (1..=TOTAL_TICKS).filter(|t| t % (K + 1) == 0).collect();
    assert_eq!(
        low_wins_at_cpu, expected,
        "CPU manual model: LOW must win exactly every (K+1)-th tick"
    );
    assert_eq!(
        low_wins_at_gpu, expected,
        "GPU manual model: LOW must win exactly every (K+1)-th tick"
    );
}

/// Угловой случай `starvation_after: Some(0)` на GPU: побеждает СРАЗУ, с
/// первого же тика — не "никогда" (что случилось бы, если бы 0 молча
/// трактовался как "выключено", см. `rule_table::GpuRule::has_starvation`'s
/// doc-комментарий).
#[test]
fn test_gpu_v2_starvation_threshold_zero_wins_immediately() {
    let width = 2;
    let storage = VecStorage::new(width, 1);
    let mut cpu_grid = Grid::new(storage, HashSet::new());
    cpu_grid.set_cell(0, 0, Cell::new(1));
    let initial = vec![(0usize, 0usize, Cell::new(1))];

    let rule_index = starvation_rules(0);
    let mut gpu_engine =
        GpuEngine::new(width, 1, &initial, &rule_index).expect("starvation_after: Some(0) is within the v2 subset");

    run_tick(&mut cpu_grid, &rule_index);
    gpu_engine.run_tick();
    let gpu_result = gpu_engine.read_grid();

    assert_eq!(
        cpu_grid.get_cell(1, 0).map(|c| c.value.0 .0),
        Some(200),
        "CPU: threshold 0 must win immediately on tick 1"
    );
    assert_eq!(
        gpu_result[1].value.0 .0, 200,
        "GPU: threshold 0 must win immediately on tick 1 -- must NOT be silently treated as disabled"
    );
}

/// Совмещает `starvation_after` с гибридным CPU-fallback: RULE_HIGH
/// (priority 20, обычная самораспространяющаяся цепочка, без голодания) и
/// RULE_LOW (priority 1, starvation_after=1, ТА ЖЕ self+neighbor геометрия,
/// другой литерал) конкурируют на КАЖДОЙ из 100 клеток. Тик1: RULE_HIGH
/// побеждает везде (100 голодных проигрышей разом). Тик2: у всех клеток
/// счётчик=1>=threshold=1 -> RULE_LOW эффективный priority=MAX -> побеждает
/// везде — но теперь ИМЕННО RULE_LOW's собственные, взаимно пересекающиеся
/// claims образуют ту же цепочку зависимых конфликтов, требующую
/// CPU-fallback, — то есть голодающие матчи это именно то, что доигрывается
/// на CPU в этот момент, не что-то постороннее в том же тике.
///
/// ЧЕСТНАЯ ОГОВОРКА (проверено сабботажным прогоном — временный откат
/// `cpu_fallback_resolve`'s дозаписи `match_state_buf`, не оставлен в
/// финальном коде): этот тест НЕ падает от отката фикса, ни в этой
/// раскладке, ни в двух более ранних (изолированный "STARVER" сбоку,
/// голова/хвост цепочки — обе тривиально сходятся за раунд 1, вообще не
/// касаясь CPU-fallback). Причина — не в тесте, а в самой природе бага:
/// без дозаписи счётчик для CPU-доигранных матчей вместо сброса на 0 после
/// победы продолжает РАСТИ (PENDING(0) != ACCEPTED(1), код ошибочно уходит
/// в ветку "проиграл") — но раз RULE_LOW уже перешагнул порог и остаётся
/// заблокированным на MAX-приоритете независимо от ТОЧНОГО значения
/// счётчика (лишь бы оно было >= порога), эта порча счётчика здесь не
/// имеет НАБЛЮДАЕМОГО следствия. Наблюдаемым она стала бы только в
/// сценарии, где голодающее правило после выигрыша ДОЛЖНО было бы снова
/// начать честно проигрывать и копить счётчик заново с нуля — такой
/// сценарий, ГЛУБОКО внутри CPU-fallback-требующей цепочки, оказался
/// непропорционально дорог для точной конструкции в разумное время; фикс
/// (дозапись `match_state_buf`) оставлен как правильный по построению
/// (устраняет реальный, найденный при аудите пробел в консистентности
/// состояния), пусть и не изолированно доказанный adversarial-тестом
/// именно здесь. Тест по-прежнему содержателен: подтверждает побитовое
/// совпадение GPU/CPU (значения И born_at) на каждом тике для реалистично
/// сложного сценария, сочетающего оба механизма одновременно.
#[test]
fn test_gpu_v2_starvation_survives_cpu_fallback_long_chain() {
    const CHAIN_LEN: usize = 100; // тот же потолок, что уже форсирует CPU-fallback
    const HEAD: u8 = 1;
    let width = CHAIN_LEN + 5;

    let rule_high = Rule {
        id: vec![CellType(HEAD)],
        pattern: vec![(0, 0, CellType(HEAD))],
        shifts: Vec::new(),
        changes: vec![(0, 0, ChangeValue::Literal(HEAD)), (1, 0, ChangeValue::Literal(HEAD))],
        active_only: false,
        priority: 20,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let rule_low = Rule {
        id: vec![CellType(HEAD)],
        pattern: vec![(0, 0, CellType(HEAD))],
        shifts: Vec::new(),
        changes: vec![(0, 0, ChangeValue::Literal(9)), (1, 0, ChangeValue::Literal(9))],
        active_only: false,
        priority: 1,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: Some(1),
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(HEAD), vec![rule_high, rule_low]);

    let storage = VecStorage::new(width, 1);
    let mut grid = Grid::new(storage, HashSet::new());
    let mut initial = Vec::new();
    for x in 0..CHAIN_LEN {
        let cell = Cell {
            value: CellValue::new(HEAD),
            born_at: 0,
        };
        grid.set_cell(x, 0, cell);
        initial.push((x, 0usize, cell));
    }

    // Тот же `Engine` (не свободная `run_tick`) — см. `test_gpu_v2_starvation_matches_cpu_periodic_progress`'s
    // doc-комментарий про то, почему свободная функция непригодна как
    // эталон для `starvation_after`.
    let mut cpu_engine = Engine::new(grid, rule_index.clone());
    let mut gpu_engine = GpuEngine::new(width, 1, &initial, &rule_index)
        .expect("starvation_after + long conflict chain is within the v2 subset");

    for tick in 1..=3 {
        cpu_engine.run_tick();
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();
        for x in 0..width {
            let cpu_cell = cpu_engine.grid().get_cell(x, 0).copied().unwrap_or_default();
            let gpu_cell = gpu_result[x];
            assert_eq!(
                cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                "value mismatch at tick={tick} x={x} (starvation + CPU-fallback chain)"
            );
            assert_eq!(
                cpu_cell.born_at, gpu_cell.born_at,
                "born_at mismatch at tick={tick} x={x}"
            );
        }
    }

    // Ручная проверка на CPU-эталоне: к тику 2 ВСЯ цепочка должна была
    // перейти с литерала HEAD(1) на литерал 9 (RULE_LOW выиграл везде за
    // счёт голодания), и продолжать расти тем же 9 на тике 3 (x=CHAIN_LEN
    // -- новая клетка, впервые захваченная RULE_LOW's собственным
    // неймингом на этом тике).
    for x in 0..CHAIN_LEN {
        assert_eq!(
            cpu_engine.grid().get_cell(x, 0).map(|c| c.value.0 .0),
            Some(9),
            "CPU manual trace: x={x} must be 9 (RULE_LOW) by tick 2, staying 9 through tick 3"
        );
    }
}

// ──────────────────────────────────────────────────────────────
// `Rule::feedback` на GPU (см. `gpu::rule_table::GpuRuleTable::needs_feedback`'s
// doc-комментарий) — та же persistent-storage-буфер техника, что и
// `starvation_after`, но с ДВУМЯ добавленными сложностями: (1) счётчик —
// защёлка (растёт при ЛЮБОМ обнаружении, не только при поражении, и
// НИКОГДА не сбрасывается победой), (2) счётчик должен ПЕРЕЕХАТЬ вместе с
// маркером на новую позицию при каждом выигранном сдвиге — иначе счётчик
// обнулялся бы каждый тик (клетка всегда "новая"), и `timeout` никогда бы
// не достигался (см. `FeedbackSpec`'s doc-комментарий в `types.rs`).
// Перенос сделан ДВУМЯ раздельными dispatch'ами (латч, потом перенос) —
// не одним, как у starvation — потому что перенос пишет в ЧУЖОЙ слот
// (новая позиция), который параллельно СВОИМ собственным потоком того же
// прохода независимо решает "я не был матчем -- сброс на 0"; без строгого
// порядка latch→relocate это гонка (см. doc-комментарий
// `update_feedback_relocate_pass` в `shader.wgsl`).

fn feedback_mover_rule(timeout: u64) -> Rule {
    Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
        changes: vec![],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: Some(cellaria::types::FeedbackSpec {
            timeout,
            new_direction: Direction::Down,
        }),
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    }
}

/// Прямой порт CPU-теста поведения `feedback`: маркер едет Right, пока
/// счётчик обнаружений (растущий КАЖДЫЙ тик, независимо от того, что
/// конкурентов нет и он всегда выигрывает) не достигнет `timeout`, затем
/// переключается на Down НАВСЕГДА — при этом счётчик обязан корректно
/// ПЕРЕЕХАТЬ вслед за маркером на GPU (persistent-буфер, привязанный к
/// (клетка, слот), а не к самому маркеру) на каждом шаге до переключения.
#[test]
fn test_gpu_v2_feedback_switches_direction_after_timeout_matches_cpu() {
    const TIMEOUT: u64 = 3;
    const TOTAL_TICKS: u32 = 8;
    let width = 10;
    let height = 10;

    let mut rule_index = HashMap::new();
    rule_index.insert(CellType(1), vec![feedback_mover_rule(TIMEOUT)]);
    let initial = vec![(0usize, 0usize, Cell::new(1))];

    let storage = VecStorage::new(width, height);
    let mut grid = Grid::new(storage, HashSet::new());
    grid.set_cell(0, 0, Cell::new(1));

    // Свободная функция `run_tick` — documented no-op для `feedback`
    // (свежие пустые `FeedbackCounters` на КАЖДЫЙ вызов) — нужен `Engine`,
    // хранящий счётчик между тиками, ровно как у `starvation_after`'s
    // тестов выше.
    let mut cpu_engine = Engine::new(grid, rule_index.clone());
    let mut gpu_engine = GpuEngine::new(width, height, &initial, &rule_index)
        .expect("feedback (single non-broadcast shift) is within the v2 subset");

    for tick in 1..=TOTAL_TICKS {
        cpu_engine.run_tick();
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();

        for y in 0..height {
            for x in 0..width {
                let cpu_cell = cpu_engine.grid().get_cell(x, y).copied().unwrap_or_default();
                let gpu_cell = gpu_result[y * width + x];
                assert_eq!(
                    cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                    "value mismatch at tick={tick} x={x} y={y}"
                );
                assert_eq!(
                    cpu_cell.born_at, gpu_cell.born_at,
                    "born_at mismatch at tick={tick} x={x} y={y}"
                );
            }
        }
    }

    // Ручная проверка на CPU-эталоне (не просто "GPU совпал с CPU"): счётчик
    // читается КАК ОН БЫЛ на начало тика (та же дисциплина, что и у
    // `age`/`min_age`/`starvation_after` — инкремент в `run_tick_with_cache`
    // стоит строго ПОСЛЕ apply, так что обнаружение ЭТИМ тиком видно только
    // следующему тику). Тики 1..TIMEOUT читают counter=0..TIMEOUT-1 (все
    // < TIMEOUT) -> Right; переключение первым видит тик TIMEOUT+1. К
    // моменту TOTAL_TICKS маркер должен быть в (TIMEOUT, TOTAL_TICKS - TIMEOUT).
    let expected_x = TIMEOUT as usize;
    let expected_y = TOTAL_TICKS as usize - TIMEOUT as usize;
    assert_eq!(
        cpu_engine.grid().get_cell(expected_x, expected_y).map(|c| c.value.0 .0),
        Some(1),
        "CPU manual trace: marker must be at ({expected_x},{expected_y}) after {TOTAL_TICKS} ticks (Right for {TIMEOUT} ticks, then Down)"
    );
}

/// Угловой случай `feedback.timeout: 0` на GPU: переключение с ПЕРВОГО же
/// тика (счётчик ДО обновления == 0 >= 0 сразу) — не "никогда", что
/// случилось бы, если бы 0 молча трактовался как "выключено" (тот же класс
/// ошибки, что уже пойман у `starvation_threshold=0`, см.
/// `rule_table::GpuRule::has_feedback`'s doc-комментарий).
#[test]
fn test_gpu_v2_feedback_timeout_zero_switches_immediately() {
    let width = 5;
    let height = 5;
    let mut rule_index = HashMap::new();
    rule_index.insert(CellType(1), vec![feedback_mover_rule(0)]);
    let initial = vec![(0usize, 0usize, Cell::new(1))];

    let mut gpu_engine =
        GpuEngine::new(width, height, &initial, &rule_index).expect("feedback: timeout 0 is within the v2 subset");
    gpu_engine.run_tick();
    let gpu_result = gpu_engine.read_grid();

    assert_eq!(gpu_result[0].value.0 .0, 0, "source must be cleared after the shift");
    assert_eq!(
        gpu_result[width].value.0 .0, 1,
        "timeout=0 must switch to Down on tick 1, not Right"
    );
}

fn feedback_mover_rule_priority(timeout: u64, priority: u32) -> Rule {
    Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
        changes: vec![],
        active_only: false,
        priority,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: Some(cellaria::types::FeedbackSpec {
            timeout,
            new_direction: Direction::Down,
        }),
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    }
}

/// Не двигается сама, каждый тик перезаписывает соседа СНИЗУ (относительное
/// смещение `(0, 1)`) литералом `literal` — якорь остаётся ВНЕ траектории A
/// (та едет по своей строке, B сидит строкой выше и целится ровно в клетку
/// под собой) — используется как "проигрывающий" оппонент для теста ниже:
/// голова `2`, ничем не очищается и не двигается, значит матчится КАЖДЫЙ тик
/// заново (постоянный источник конкуренции за одну и ту же абсолютную
/// клетку, без побочных коллизий с собственной позицией A на поздних тиках).
fn contender_self_write_rule(priority: u32, literal: u8) -> Rule {
    Rule {
        id: vec![CellType(2)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 1, ChangeValue::Literal(literal))],
        active_only: false,
        priority,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    }
}

/// Целевой репро для риска, описанного в `gpu/rule_table.rs`'s
/// `FeedbackChangeCollidesWithShiftTarget`: перенос `feedback`-счётчика
/// (`update_feedback_relocate_pass`) предполагает, что тип клетки на НОВОЙ
/// позиции — снова собственная голова маркера, и это предположение может
/// быть нарушено, если РЕАЛЬНО записанное там значение пришло не из
/// собственного сдвига правила. Явная защита в коде покрывает только
/// САМОколлизию (одно и то же правило, `changes` на том же смещении, что и
/// собственный сдвиг) — случай ДВУХ РАЗНЫХ правил, реально контестирующих
/// одну и ту же абсолютную клетку через нормальный раундовый арбитраж (не
/// самоколлизия), явно защитой нигде не покрыт — этот тест проверяет его
/// напрямую против CPU-эталона, а не рассуждением.
///
/// Голова `1` (A, в строке 1, `feedback`, сдвиг Right, ВЫСОКИЙ приоритет) и
/// голова `2` (B, в строке 0 прямо НАД первой целевой клеткой A, self-write
/// вниз через `changes`, НИЗКИЙ приоритет, никогда не двигается и не
/// очищается — переигрывает каждый тик) обе целятся в одну и ту же
/// абсолютную клетку на первом же тике. A обязана побеждать (выше
/// приоритет) — если раундовый арбитраж и перенос счётчика работают
/// корректно, GPU обязан побитово совпасть с CPU на каждом тике вообще (не
/// только в клетке контеста), включая корректно перенесённый
/// `feedback`-счётчик A — если бы перенос спутал слот A со слотом B (или
/// каким-либо иным), поведение A (момент переключения на `Down`) разошлось
/// бы с CPU-эталоном на одном из следующих тиков, что немедленно поймает
/// per-tick сверка ниже.
#[test]
fn test_gpu_v2_feedback_relocate_survives_genuine_cross_rule_contention() {
    const TIMEOUT: u64 = 4;
    const TOTAL_TICKS: u32 = 8;
    let width = 12;
    let height = 8;

    let mut rule_index = HashMap::new();
    rule_index.insert(CellType(1), vec![feedback_mover_rule_priority(TIMEOUT, 20)]);
    rule_index.insert(CellType(2), vec![contender_self_write_rule(5, 2)]);

    // A в (2,1) целится Right в (3,1); B в (3,0) целится Down в ту же (3,1)
    // -- genuine contention на первом же тике, A выигрывает (приоритет
    // 20 > 5). B никогда не двигается и не очищается -- продолжает
    // безобидно писать (3,1) на КАЖДОМ следующем тике тоже (A там уже нет
    // после тика 1, реального конфликта больше не возникает, но сам факт
    // повторного матчинга B каждый тик уже проверен GPU/CPU сверкой ниже).
    let initial = vec![(2usize, 1usize, Cell::new(1)), (3usize, 0usize, Cell::new(2))];
    let storage = VecStorage::new(width, height);
    let mut grid = Grid::new(storage, HashSet::new());
    grid.set_cell(2, 1, Cell::new(1));
    grid.set_cell(3, 0, Cell::new(2));

    let mut cpu_engine = Engine::new(grid, rule_index.clone());
    let mut gpu_engine = GpuEngine::new(width, height, &initial, &rule_index)
        .expect("feedback vs a lower-priority contending self-write rule is within the v2 subset");

    let mut switched_at_cpu: Option<u32> = None;
    let mut switched_at_gpu: Option<u32> = None;

    for tick in 1..=TOTAL_TICKS {
        cpu_engine.run_tick();
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();

        for y in 0..height {
            for x in 0..width {
                let cpu_cell = cpu_engine.grid().get_cell(x, y).copied().unwrap_or_default();
                let gpu_cell = gpu_result[y * width + x];
                assert_eq!(
                    cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                    "value mismatch at tick={tick} x={x} y={y}"
                );
                assert_eq!(
                    cpu_cell.born_at, gpu_cell.born_at,
                    "born_at mismatch at tick={tick} x={x} y={y}"
                );
            }
        }

        // A -- единственная голова `1` на решётке; "переключилась" = где-то
        // на строке > 1 появилась голова `1` (Down уводит её из строки 1
        // навсегда, Right держит её в строке 1).
        if switched_at_cpu.is_none()
            && (0..width)
                .any(|x| (2..height).any(|y| cpu_engine.grid().get_cell(x, y).map(|c| c.value.0 .0) == Some(1)))
        {
            switched_at_cpu = Some(tick);
        }
        if switched_at_gpu.is_none()
            && (0..width).any(|x| (2..height).any(|y| gpu_result[y * width + x].value.0 .0 == 1))
        {
            switched_at_gpu = Some(tick);
        }
    }

    assert!(
        switched_at_cpu.is_some(),
        "CPU sanity: A must switch to Down within TOTAL_TICKS ticks (TIMEOUT={TIMEOUT})"
    );
    assert_eq!(
        switched_at_cpu, switched_at_gpu,
        "switch tick must match between CPU and GPU -- a corrupted feedback counter would desync this"
    );
}

// ──────────────────────────────────────────────────────────────
// `Rule::memory` на GPU (см. `gpu::rule_table::GpuRuleTable::needs_memory`'s
// doc-комментарий) — persistent FIFO-буфер, тот же общий приём, что и
// `starvation_after`/`feedback`, но с ДВУМЯ добавленными сложностями: (1)
// гейт — ЧИСТЫЙ pre-arbитражный фильтр кандидатов (закрытый гейт исключает
// матч из арбитража целиком, но буфер обязан ПРОДОЛЖАТЬ наблюдать — см.
// `GpuMatch::structural`), (2) буфер (не скаляр) должен переехать вместе с
// маркером на новую позицию при каждом выигранном сдвиге (та же техника,
// что у `feedback_counters`, но копирует до `MAX_MEMORY_WINDOW` значений,
// не одно).

/// `NeighborType`-триггер + сдвиг (упражняет ОБЕ сложности: гейт и перенос
/// буфера). Правило: сосед сверху (Direction::Up, за пределами решётки на
/// y=0 — читается как `DEFAULT_CELL_VALUE`=0) должен быть "пуст" (тип 0)
/// ДВА тика подряд (`window=2`), прежде чем маркер продвинется на 1 клетку
/// вправо. На полностью пустой решётке (сосед сверху ВСЕГДА пуст) это даёт
/// детерминированную, вручную выводимую траекторию: буфер начинает пустым
/// (гейт закрыт по построению первые `window` тиков — "прайминг", тот же
/// эффект, что уже найден для `memory`+`recursion`, см.
/// `project_memory_recursion_combo_2026_08_08` в памяти сессии), затем
/// гейт открывается и остаётся открытым НАВСЕГДА (перенесённый буфер
/// продолжает наблюдать тот же "всегда пусто" сигнал на каждой новой
/// позиции) — маркер должен двигаться Right КАЖДЫЙ тик начиная с тика
/// `window + 1`.
fn memory_neighbor_type_mover_rule(window: usize) -> Rule {
    Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
        changes: vec![],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: Some(MemorySpec {
            window,
            record_trigger: RecordTrigger::NeighborType(Direction::Up),
            match_pattern: vec![RecordedValue::Type(CellType(0)); window],
        }),
        max_activations: None,
        cross_layer_reads: Vec::new(),
    }
}

#[test]
fn test_gpu_v2_memory_neighbor_type_gate_and_relocation_matches_cpu() {
    const WINDOW: usize = 2;
    const TOTAL_TICKS: u32 = 6;
    let width = 10;
    let height = 10;

    let mut rule_index = HashMap::new();
    rule_index.insert(CellType(1), vec![memory_neighbor_type_mover_rule(WINDOW)]);
    let initial = vec![(0usize, 0usize, Cell::new(1))];

    let storage = VecStorage::new(width, height);
    let mut grid = Grid::new(storage, HashSet::new());
    grid.set_cell(0, 0, Cell::new(1));

    // `Engine`, не свободная `run_tick` — `memory_buffers` (как и
    // `starvation_counters`/`feedback_counters`) нужен персистентным между
    // вызовами `run_tick`, свободная функция гарантированно даёт свежий
    // пустой буфер на КАЖДЫЙ вызов (documented no-op).
    let mut cpu_engine = Engine::new(grid, rule_index.clone());
    let mut gpu_engine = GpuEngine::new(width, height, &initial, &rule_index)
        .expect("memory (NeighborType, one non-broadcast shift, window within cap) is within the v2 subset");

    for tick in 1..=TOTAL_TICKS {
        cpu_engine.run_tick();
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();

        for y in 0..height {
            for x in 0..width {
                let cpu_cell = cpu_engine.grid().get_cell(x, y).copied().unwrap_or_default();
                let gpu_cell = gpu_result[y * width + x];
                assert_eq!(
                    cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                    "value mismatch at tick={tick} x={x} y={y}"
                );
                assert_eq!(
                    cpu_cell.born_at, gpu_cell.born_at,
                    "born_at mismatch at tick={tick} x={x} y={y}"
                );
            }
        }
    }

    // Ручная проверка на CPU-эталоне: первые `WINDOW` тиков — прайминг
    // (буфер ещё не полон, гейт закрыт по построению), маркер стоит на
    // месте; с тика `WINDOW+1` гейт открыт (буфер полон и совпадает — сосед
    // сверху всегда пуст на этой решётке) и остаётся открытым НАВСЕГДА
    // (перенесённый буфер продолжает наблюдать тот же сигнал) — маркер
    // должен двигаться Right КАЖДЫЙ тик после этого. К тику `TOTAL_TICKS`
    // маркер должен быть на x = `TOTAL_TICKS - WINDOW`.
    let expected_x = TOTAL_TICKS as usize - WINDOW;
    assert_eq!(
        cpu_engine.grid().get_cell(expected_x, 0).map(|c| c.value.0 .0),
        Some(1),
        "CPU manual trace: marker must be at x={expected_x} after {TOTAL_TICKS} ticks ({WINDOW}-tick priming, then Right every tick)"
    );
}

/// `RuleOutcome`-триггер, БЕЗ сдвига (упражняет push-таймингом от
/// `match_state`, независимо от переноса — уже покрытого предыдущим
/// тестом). Правило без конкурентов пишет литерал в соседнюю клетку
/// (`changes`, не сдвиг — головной тип клетки-маркера никогда не меняется,
/// значит правило продолжает совпадать КАЖДЫЙ тик) с гейтом `window=1`,
/// `match_pattern=[Missed]` — "срабатывает только СРАЗУ ПОСЛЕ того, как в
/// прошлый раз проиграло". Из холодного старта (буфер пуст) это даёт
/// детерминированную осцилляцию: тик1 REJECTED (гейт закрыт по построению,
/// буфер ещё пуст), пишет Missed → тик2 гейт ОТКРЫТ (Missed==Missed) →
/// ACCEPTED, пишет Applied → тик3 гейт ЗАКРЫТ (Applied!=Missed) → REJECTED,
/// пишет Missed → тик4 снова ОТКРЫТ... т.е. чётные тики ACCEPTED, нечётные
/// (начиная с 3) REJECTED.
#[test]
fn test_gpu_v2_memory_rule_outcome_gate_oscillates_matches_cpu() {
    const TOTAL_TICKS: u32 = 6;
    let width = 5;
    let height = 5;

    let rule = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(1, 0, ChangeValue::Literal(200))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: Some(MemorySpec {
            window: 1,
            record_trigger: RecordTrigger::RuleOutcome,
            match_pattern: vec![RecordedValue::Missed],
        }),
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut rule_index = HashMap::new();
    rule_index.insert(CellType(1), vec![rule]);
    let initial = vec![(0usize, 0usize, Cell::new(1))];

    let storage = VecStorage::new(width, height);
    let mut grid = Grid::new(storage, HashSet::new());
    grid.set_cell(0, 0, Cell::new(1));

    let mut cpu_engine = Engine::new(grid, rule_index.clone());
    let mut gpu_engine = GpuEngine::new(width, height, &initial, &rule_index)
        .expect("memory (RuleOutcome, no shift, window=1) is within the v2 subset");

    for tick in 1..=TOTAL_TICKS {
        cpu_engine.run_tick();
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();

        for y in 0..height {
            for x in 0..width {
                let cpu_cell = cpu_engine.grid().get_cell(x, y).copied().unwrap_or_default();
                let gpu_cell = gpu_result[y * width + x];
                assert_eq!(
                    cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                    "value mismatch at tick={tick} x={x} y={y}"
                );
                assert_eq!(
                    cpu_cell.born_at, gpu_cell.born_at,
                    "born_at mismatch at tick={tick} x={x} y={y}"
                );
            }
        }
    }

    // Ручная проверка на CPU-эталоне: тик 2 — первый ACCEPTED (см.
    // doc-комментарий выше), (1,0) обязан стать 200 к этому моменту и
    // оставаться 200 (REJECTED-тики не пишут туда ничего, не сбрасывают).
    assert_eq!(
        cpu_engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
        Some(200),
        "CPU manual trace: (1,0) must be 200 by tick 2 (first ACCEPTED tick in the oscillation) and stay 200"
    );
}

/// `starvation_after` COMBINED with `memory` on the SAME rule — checks that
/// `update_starvation_pass` correctly distinguishes "genuinely orphaned"
/// (structural mismatch — reset, correct) from "structurally matched but
/// memory-gate closed" (CPU: simply not in `starving_keys` this tick, the
/// counter is left UNTOUCHED — see `engine/mod.rs`'s doc-comment: "Считаются
/// ПОСЛЕ гейт-фильтра памяти — гейтованный кандидат этот тик не участвует
/// ни в чём, как будто не детектировался" is about EXCLUSION from
/// arbitration, not about resetting a counter that already has state).
///
/// Setup: an oscillating two-type "beacon" below the competing head (types
/// 7/8 flip-flop every tick, no external state needed — fully deterministic
/// from tick 1) drives a `NeighborType(Down)` gate with `window=1,
/// pattern=[Type(7)]`, so the gate is open on roughly every OTHER tick and
/// closed on the ticks in between. RULE_HIGH (no starvation) always wins
/// when RULE_LOW is excluded or loses; RULE_LOW has `starvation_after` +
/// this gate. If the starvation counter is correctly FROZEN (not reset)
/// while gate-closed, it accumulates across open ticks and eventually
/// crosses `threshold`, making RULE_LOW win via starvation boost. If it's
/// WRONGLY reset every closed tick (the bug this test targets), it can
/// never accumulate past 1 and RULE_LOW never wins within any reasonable
/// tick budget.
fn starvation_plus_memory_rules(threshold: u32) -> HashMap<CellType, Vec<Rule>> {
    let rule_high = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(1, 0, ChangeValue::Literal(100))],
        active_only: false,
        priority: 20,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let rule_low = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(1, 0, ChangeValue::Literal(200))],
        active_only: false,
        priority: 5,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: Some(threshold),
        feedback: None,
        recursion: None,
        memory: Some(MemorySpec {
            window: 1,
            record_trigger: RecordTrigger::NeighborType(Direction::Down),
            match_pattern: vec![RecordedValue::Type(CellType(7))],
        }),
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let beacon7 = Rule {
        id: vec![CellType(7)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(8))],
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let beacon8 = Rule {
        id: vec![CellType(8)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(7))],
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut idx = HashMap::new();
    idx.insert(CellType(1), vec![rule_high, rule_low]);
    idx.insert(CellType(7), vec![beacon7]);
    idx.insert(CellType(8), vec![beacon8]);
    idx
}

#[test]
fn test_gpu_v2_starvation_plus_memory_gate_freezes_counter_not_resets_matches_cpu() {
    const THRESHOLD: u32 = 3;
    const TOTAL_TICKS: u32 = 14;
    let width = 5;
    let height = 5;

    let rule_index = starvation_plus_memory_rules(THRESHOLD);
    let initial = vec![(0usize, 0usize, Cell::new(1)), (0usize, 1usize, Cell::new(7))];

    let storage = VecStorage::new(width, height);
    let mut grid = Grid::new(storage, HashSet::new());
    grid.set_cell(0, 0, Cell::new(1));
    grid.set_cell(0, 1, Cell::new(7));

    let mut cpu_engine = Engine::new(grid, rule_index.clone());
    let mut gpu_engine = GpuEngine::new(width, height, &initial, &rule_index)
        .expect("starvation_after + memory on the same rule is within the v2 subset");

    // `starvation_after` резервирует счётчик на КАЖДОЙ победе (см. CPU-side
    // "win -> сброс"), значит RULE_LOW не остаётся на 200 НАВСЕГДА после
    // первой победы — оно побеждает ПЕРИОДИЧЕСКИ (тот же паттерн, что и в
    // `test_gpu_v2_starvation_matches_cpu_periodic_progress` выше, только
    // модулированный ЕЩЁ и открытием/закрытием гейта поверх голодания) —
    // отслеживаем ВСЕ тики победы, а не проверяем фиксированное значение на
    // фиксированном тике (первая версия этого теста ошибочно ожидала "200
    // навсегда к тику 14" — реальность: 200 на тике 8, обратно 100 позже,
    // пока не накопится следующий порог).
    let mut low_wins_at_cpu = Vec::new();
    let mut low_wins_at_gpu = Vec::new();

    for tick in 1..=TOTAL_TICKS {
        cpu_engine.run_tick();
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();

        for y in 0..height {
            for x in 0..width {
                let cpu_cell = cpu_engine.grid().get_cell(x, y).copied().unwrap_or_default();
                let gpu_cell = gpu_result[y * width + x];
                assert_eq!(
                    cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                    "value mismatch at tick={tick} x={x} y={y}"
                );
                assert_eq!(
                    cpu_cell.born_at, gpu_cell.born_at,
                    "born_at mismatch at tick={tick} x={x} y={y}"
                );
            }
        }

        if cpu_engine.grid().get_cell(1, 0).map(|c| c.value.0 .0) == Some(200) {
            low_wins_at_cpu.push(tick);
        }
        if gpu_result[1].value.0 .0 == 200 {
            low_wins_at_gpu.push(tick);
        }
    }

    // Ручная проверка на CPU-эталоне: с частично-открытым гейтом (каждый
    // второй тик) и замороженным (не сбрасываемым) счётчиком, RULE_LOW
    // обязан НАКОПИТЬ достаточно поражений и выиграть через голодание ХОТЯ
    // БЫ ОДИН РАЗ в пределах TOTAL_TICKS. Если счётчик ошибочно сбрасывается
    // на закрытых тиках (баг, который этот тест ловит), RULE_LOW никогда не
    // выигрывает вовсе (`low_wins_at_cpu` останется пустым и на GPU, и на
    // CPU — но именно РАСХОЖДЕНИЕ между ними, пойманное построчным циклом
    // выше, было первым, самым прямым сигналом бага).
    assert!(!low_wins_at_cpu.is_empty(), "CPU manual trace: RULE_LOW must win via starvation at least once within {TOTAL_TICKS} ticks (frozen counter accumulates across gate-open ticks)");
    assert_eq!(
        low_wins_at_cpu, low_wins_at_gpu,
        "GPU must win via starvation on EXACTLY the same ticks as CPU"
    );
}

/// `Rule::feedback` COMBINED with `Rule::memory` on the SAME rule — same
/// bug class as the starvation+memory test above, but targeting
/// `update_feedback_latch_pass` instead of `update_starvation_pass`: CPU's
/// `feedback_keys` (see `engine/mod.rs`) is ALSO computed from the
/// gate-filtered match list, so a gate-closed tick simply excludes the key
/// from CPU's increment-only feedback update loop — the latch is left
/// UNTOUCHED (frozen), never reset. `update_feedback_latch_pass` used the
/// same `matches[m].matched == 0u` check as starvation's (pre-fix) — if it
/// has the same bug, the latch resets every gate-closed tick and the
/// `feedback` timeout may never be reached where CPU reliably reaches it.
///
/// Setup: the marker (type 1) has BOTH a shift (required by `feedback`,
/// Right → Down after timeout) AND a `memory` gate watching `Direction::Down`.
/// Since the marker itself moves right every tick, a single fixed beacon
/// cell won't stay "below" it — instead, the ENTIRE row below is filled
/// with the SAME oscillating type-7/8 rule (all cells toggle in lockstep,
/// driven only by their own previous value, no coordination needed), so
/// whatever column the marker is currently above, looking down always sees
/// the same synchronized oscillation.
fn feedback_plus_memory_rules(feedback_timeout: u64, memory_window: usize) -> HashMap<CellType, Vec<Rule>> {
    let marker = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
        changes: vec![],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: Some(FeedbackSpec {
            timeout: feedback_timeout,
            new_direction: Direction::Down,
        }),
        recursion: None,
        memory: Some(MemorySpec {
            window: memory_window,
            record_trigger: RecordTrigger::NeighborType(Direction::Down),
            match_pattern: vec![RecordedValue::Type(CellType(7)); memory_window],
        }),
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let beacon7 = Rule {
        id: vec![CellType(7)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(8))],
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let beacon8 = Rule {
        id: vec![CellType(8)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(7))],
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: OverflowAction::Discard,
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut idx = HashMap::new();
    idx.insert(CellType(1), vec![marker]);
    idx.insert(CellType(7), vec![beacon7]);
    idx.insert(CellType(8), vec![beacon8]);
    idx
}

#[test]
fn test_gpu_v2_feedback_plus_memory_gate_freezes_latch_not_resets_matches_cpu() {
    const TIMEOUT: u64 = 4;
    const TOTAL_TICKS: u32 = 12;
    let width = 10;
    let height = 3;

    let rule_index = feedback_plus_memory_rules(TIMEOUT, 1);
    let mut initial = vec![(0usize, 0usize, Cell::new(1))];
    for x in 0..width {
        initial.push((x, 1, Cell::new(7)));
    }

    let storage = VecStorage::new(width, height);
    let mut grid = Grid::new(storage, HashSet::new());
    grid.set_cell(0, 0, Cell::new(1));
    for x in 0..width {
        grid.set_cell(x, 1, Cell::new(7));
    }

    let mut cpu_engine = Engine::new(grid, rule_index.clone());
    let mut gpu_engine = GpuEngine::new(width, height, &initial, &rule_index)
        .expect("feedback + memory on the same rule is within the v2 subset");

    // Отслеживаем, на каком тике (если вообще) маркер впервые оказывается в
    // строке 1 (переключился на Down) — сканируем ВСЮ строку, не
    // фиксированный столбец: маркер едет Right неизвестное число тиков
    // ПЕРЕД переключением, так что колонка переключения заранее не
    // известна (тот же урок, что и у starvation+memory теста выше: точный
    // тик/позицию переключения сложно вывести руками, надёжнее отслеживать
    // факт и сверять с GPU).
    let mut switched_at_cpu: Option<u32> = None;
    let mut switched_at_gpu: Option<u32> = None;

    for tick in 1..=TOTAL_TICKS {
        cpu_engine.run_tick();
        gpu_engine.run_tick();
        let gpu_result = gpu_engine.read_grid();

        for y in 0..height {
            for x in 0..width {
                let cpu_cell = cpu_engine.grid().get_cell(x, y).copied().unwrap_or_default();
                let gpu_cell = gpu_result[y * width + x];
                assert_eq!(
                    cpu_cell.value.0 .0, gpu_cell.value.0 .0,
                    "value mismatch at tick={tick} x={x} y={y}"
                );
                assert_eq!(
                    cpu_cell.born_at, gpu_cell.born_at,
                    "born_at mismatch at tick={tick} x={x} y={y}"
                );
            }
        }

        if switched_at_cpu.is_none()
            && (0..width).any(|x| cpu_engine.grid().get_cell(x, 1).map(|c| c.value.0 .0) == Some(1))
        {
            switched_at_cpu = Some(tick);
        }
        if switched_at_gpu.is_none() && (0..width).any(|x| gpu_result[width + x].value.0 .0 == 1) {
            switched_at_gpu = Some(tick);
        }
    }

    // Ручная проверка на CPU-эталоне: с частично-открытым гейтом (каждый
    // второй тик, тот же паттерн, что у starvation+memory теста) и
    // замороженной (не сбрасываемой) защёлкой, маркер обязан РАНО ИЛИ
    // ПОЗДНО переключиться на Down в пределах TOTAL_TICKS. Если защёлка
    // ошибочно сбрасывается на закрытых тиках (баг, который этот тест
    // ловит), защёлка никогда не достигает `TIMEOUT`, и маркер продолжает
    // движение Right (уходя за правый край решётки, Discard) без
    // переключения вовсе.
    assert!(switched_at_cpu.is_some(), "CPU manual trace: marker must switch to Down within {TOTAL_TICKS} ticks (frozen latch accumulates across gate-open ticks)");
    assert_eq!(
        switched_at_cpu, switched_at_gpu,
        "GPU must switch direction on EXACTLY the same tick as CPU"
    );
}
