//! Property-based тесты для расширений с персистентным (межтиковым)
//! состоянием: `starvation_after`/`feedback`/`memory`/`max_activations`.
//!
//! Существующие property-тесты (`property_arbitration.rs`) проверяют
//! инварианты арбитража на ОДНОМ тике — этого достаточно для structural
//! свойств, но оба реальных бага, найденных в этой сессии (некорректный
//! тайминг `feedback_counters` между арбитражем и apply; переиспользование
//! `rule_idx` при изменении состава правил) — это МНОГОТИКОВЫЕ проблемы
//! согласованности состояния, которые single-tick unit-тесты пропускают,
//! пока кто-то не придумает написать именно такой сценарий руками.
//!
//! Здесь — два инварианта, которые должны держаться на ЛЮБОМ наборе правил
//! с этими расширениями, а не только на конкретных примерах:
//!
//! 1. `prop_repeated_runs_are_deterministic` — сам фундамент модели
//!    (§3.4/детерминизм): два движка, построенные ОДИНАКОВО и прогнанные на
//!    ОДИНАКОВОЕ число тиков, обязаны дать побитово идентичный результат.
//! 2. `prop_snapshot_restore_continues_identically` — стресс-тест
//!    `Engine::snapshot`/`from_snapshot` (сессия 2026-08-09): снимок,
//!    сделанный посреди прогона, и продолженный с него движок обязаны дать
//!    ТОТ ЖЕ результат, что и движок, прогнанный без остановки.
//!
//! `recursion`/`cam` покрыты отдельно — `property_recursion_cam.rs`, тот же
//! каркас, генератор построен заново под их структурные ограничения (пустые
//! `shifts`, доп. параметры) — не расширение этого файла, отдельная задача,
//! как и планировалось.

use std::collections::{HashMap, HashSet};

use proptest::prelude::*;

use cellaria::engine::Engine;
use cellaria::types::{
    Cell, CellType, CellValue, Direction, FeedbackSpec, MemorySpec, RecordTrigger, RecordedValue, Rule, ShiftSpec,
};
use cellaria::{Grid, VecStorage};

const CELL_ALPHABET: u8 = 3;
const SIDE: usize = 5;
const MAX_TICKS: u32 = 12;

fn direction_strategy() -> impl Strategy<Value = Direction> {
    prop_oneof![
        Just(Direction::Up),
        Just(Direction::Down),
        Just(Direction::Left),
        Just(Direction::Right),
    ]
}

/// Одно расширение на правило, СОВМЕСТИМОЕ с тем, есть ли у него сдвиг —
/// `feedback` требует РОВНО один сдвиг (см. `config.rs::build_rule`),
/// остальные (`starvation_after`/`memory`/`max_activations`) сдвигов не
/// требуют вовсе.
fn extensions_strategy(
    has_shift: bool,
) -> impl Strategy<Value = (Option<u32>, Option<FeedbackSpec>, Option<MemorySpec>, Option<u32>)> {
    let starvation = prop_oneof![Just(None), (1u32..=3).prop_map(Some)];
    let max_activations = prop_oneof![Just(None), (1u32..=3).prop_map(Some)];
    let memory = prop_oneof![
        Just(None),
        (direction_strategy(), 1u8..=CELL_ALPHABET).prop_map(|(dir, t)| Some(MemorySpec {
            window: 1,
            record_trigger: RecordTrigger::NeighborType(dir),
            match_pattern: vec![RecordedValue::Type(CellType(t))],
        })),
        Just(Some(MemorySpec {
            window: 1,
            record_trigger: RecordTrigger::RuleOutcome,
            match_pattern: vec![RecordedValue::Applied]
        })),
    ];
    if has_shift {
        let feedback = prop_oneof![
            Just(None),
            (1u64..=3, direction_strategy())
                .prop_map(|(timeout, new_direction)| Some(FeedbackSpec { timeout, new_direction })),
        ];
        (starvation, feedback, memory, max_activations).boxed()
    } else {
        (starvation, Just(None), memory, max_activations).boxed()
    }
}

fn rule_strategy(head: u8) -> impl Strategy<Value = Rule> {
    (prop::option::of((direction_strategy(), 1u16..=1)), 1u32..=10).prop_flat_map(move |(shift, priority)| {
        let has_shift = shift.is_some();
        extensions_strategy(has_shift).prop_map(move |(starvation_after, feedback, memory, max_activations)| Rule {
            id: vec![CellType(head)],
            pattern: vec![],
            shifts: shift
                .map(|(dir, steps)| vec![vec![ShiftSpec::new(dir, steps)]])
                .unwrap_or_default(),
            changes: vec![],
            active_only: false,
            priority,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after,
            feedback,
            recursion: None,
            memory,
            max_activations,
            cross_layer_reads: Vec::new(),
        })
    })
}

/// 1-2 головы, 1-2 конкурирующих правила на голову — достаточно для
/// реальной конкуренции в арбитраже (нужной для `starvation_after`), не
/// разрастаясь до размера, где proptest-шринкинг станет непрактичным.
fn rule_index_strategy() -> impl Strategy<Value = HashMap<CellType, Vec<Rule>>> {
    prop::collection::vec(1u8..=CELL_ALPHABET, 1..=2).prop_flat_map(|heads| {
        let mut heads: Vec<u8> = heads;
        heads.sort_unstable();
        heads.dedup();
        let per_head: Vec<_> = heads
            .iter()
            .map(|&h| prop::collection::vec(rule_strategy(h), 1..=2))
            .collect();
        per_head.prop_map(move |groups| {
            let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
            for (h, rules) in heads.iter().zip(groups) {
                idx.insert(CellType(*h), rules);
            }
            idx
        })
    })
}

fn grid_strategy() -> impl Strategy<Value = Vec<(usize, usize, u8)>> {
    prop::collection::vec((0..SIDE, 0..SIDE, 1u8..=CELL_ALPHABET), 1..=4)
}

fn build_engine(rule_index: &HashMap<CellType, Vec<Rule>>, cells: &[(usize, usize, u8)]) -> Engine<VecStorage> {
    let mut grid = Grid::new(VecStorage::new(SIDE, SIDE), HashSet::new());
    for &(x, y, t) in cells {
        grid.set_cell(
            x,
            y,
            Cell {
                value: CellValue::new(t),
                born_at: 0,
            },
        );
    }
    Engine::new(grid, rule_index.clone())
}

fn dump_grid(engine: &Engine<VecStorage>) -> Vec<Cell> {
    (0..SIDE)
        .flat_map(|y| (0..SIDE).map(move |x| (x, y)))
        .map(|(x, y)| engine.grid().get_cell(x, y).copied().unwrap_or_default())
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// Фундамент модели: два ОДИНАКОВО построенных движка, прогнанные на
    /// одинаковое число тиков, обязаны дать побитово идентичный результат —
    /// держится для ЛЮБОГО набора правил, включая любую комбинацию
    /// `starvation_after`/`feedback`/`memory`/`max_activations`.
    #[test]
    fn prop_repeated_runs_are_deterministic(
        rule_index in rule_index_strategy(),
        cells in grid_strategy(),
        ticks in 1u32..=MAX_TICKS,
    ) {
        let mut engine_a = build_engine(&rule_index, &cells);
        let mut engine_b = build_engine(&rule_index, &cells);
        for _ in 0..ticks {
            engine_a.run_tick();
            engine_b.run_tick();
        }
        prop_assert_eq!(dump_grid(&engine_a), dump_grid(&engine_b));
    }

    /// `Engine::snapshot`/`from_snapshot` (сессия 2026-08-09): движок,
    /// остановленный на середине прогона, сериализованный (YAML — см.
    /// doc-комментарий `EngineSnapshot` про то, почему не JSON),
    /// десериализованный и продолженный, обязан дать ТОТ ЖЕ результат, что
    /// и движок, прогнанный все тики без остановки.
    #[test]
    fn prop_snapshot_restore_continues_identically(
        rule_index in rule_index_strategy(),
        cells in grid_strategy(),
        ticks_before in 1u32..=6,
        ticks_after in 1u32..=6,
    ) {
        let mut straight = build_engine(&rule_index, &cells);
        for _ in 0..(ticks_before + ticks_after) {
            straight.run_tick();
        }

        let mut interrupted = build_engine(&rule_index, &cells);
        for _ in 0..ticks_before {
            interrupted.run_tick();
        }
        let yaml = serde_yaml::to_string(&interrupted.snapshot()).expect("snapshot must serialize");
        let restored_snapshot = serde_yaml::from_str(&yaml).expect("snapshot must deserialize");
        let mut restored = Engine::from_snapshot(restored_snapshot);
        for _ in 0..ticks_after {
            restored.run_tick();
        }

        prop_assert_eq!(dump_grid(&straight), dump_grid(&restored));
    }
}
