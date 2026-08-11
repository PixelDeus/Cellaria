//! Property-based тесты для `Rule::recursion`/`Rule::cam` — намеренно
//! оставленная дыра в `property_extensions.rs` (см. её doc-комментарий:
//! "Не покрыто намеренно... генератор под них — отдельная, самостоятельная
//! задача"). Те же два инварианта, тот же общий каркас (см. её же
//! doc-комментарий про то, почему именно эти два: детерминизм — фундамент
//! модели, snapshot/restore — многотиковая согласованность состояния,
//! которую single-tick unit-тесты пропускают), но генератор правил
//! построен заново под структурные ограничения `recursion`/`cam`
//! (`config.rs`'s валидация, см. комментарии ниже у каждого):
//!
//! - `cam` и `recursion` оба ТРЕБУЮТ `shifts.is_empty()` (взаимоисключающи
//!   со сдвигом, не друг с другом — `cam`+`recursion` вместе РАЗРЕШЕНЫ,
//!   реализовано как каскад независимых магнитов, см. `config.rs:495`).
//! - `feedback` требует РОВНО один сдвиг — значит несовместим с `cam`/
//!   `recursion` по построению (обе ветки ниже взаимоисключающи).
//! - `memory` с триггером `RuleOutcome` запрещён вместе с `recursion`
//!   (`config.rs:534` — у уровня каскада нет отдельного шага арбитража) —
//!   генератор избегает этой комбинации по построению, не полагаясь на
//!   то, что `Engine::new` (в отличие от `config::load_config`) её вообще
//!   бы отловил.

use std::collections::{HashMap, HashSet};

use proptest::prelude::*;

use cellaria::engine::Engine;
use cellaria::types::{
    CamSearch, Cell, CellType, CellValue, Direction, MemorySpec, RecordTrigger, RecordedValue, RecursionSpec, Rule, ShiftSpec,
};
use cellaria::{Grid, VecStorage};

const CELL_ALPHABET: u8 = 3;
const SIDE: usize = 6;
const MAX_TICKS: u32 = 10;

fn direction_strategy() -> impl Strategy<Value = Direction> {
    prop_oneof![
        Just(Direction::Up),
        Just(Direction::Down),
        Just(Direction::Left),
        Just(Direction::Right),
    ]
}

/// Форма "движения" правила — взаимоисключающие альтернативы по построению
/// конфига (см. doc-комментарий модуля): не генерируем недопустимые
/// комбинации, а НЕ ДАЁМ им появиться вообще, той же дисциплиной, что уже
/// применена в `property_extensions.rs`'s `has_shift`-развилке.
#[derive(Clone, Copy, Debug)]
enum Movement {
    None,
    Shift,
    Cam,
    Recursion,
    CamRecursion,
}

fn movement_strategy() -> impl Strategy<Value = Movement> {
    prop_oneof![
        Just(Movement::None),
        Just(Movement::Shift),
        Just(Movement::Cam),
        Just(Movement::Recursion),
        Just(Movement::CamRecursion),
    ]
}

fn cam_strategy() -> impl Strategy<Value = CamSearch> {
    (1u8..=2, 1u8..=CELL_ALPHABET).prop_map(|(radius, t)| CamSearch { radius, target_type: CellType(t) })
}

fn recursion_strategy() -> impl Strategy<Value = RecursionSpec> {
    (1u8..=3, direction_strategy()).prop_map(|(max_depth, direction)| RecursionSpec { max_depth, direction })
}

/// `memory` — доступен для ЛЮБОЙ формы движения, но `RuleOutcome`-триггер
/// исключается, когда движение включает `recursion` (см. doc-комментарий
/// модуля про `config.rs:534`). `NeighborType` разрешён всегда — и с
/// `recursion` (проверено, поддержано), и без.
fn memory_strategy(has_recursion: bool) -> impl Strategy<Value = Option<MemorySpec>> {
    let neighbor_type = (direction_strategy(), 1u8..=CELL_ALPHABET).prop_map(|(dir, t)| {
        Some(MemorySpec { window: 1, record_trigger: RecordTrigger::NeighborType(dir), match_pattern: vec![RecordedValue::Type(CellType(t))] })
    });
    if has_recursion {
        prop_oneof![Just(None), neighbor_type].boxed()
    } else {
        let rule_outcome = Just(Some(MemorySpec { window: 1, record_trigger: RecordTrigger::RuleOutcome, match_pattern: vec![RecordedValue::Applied] }));
        prop_oneof![Just(None), neighbor_type, rule_outcome].boxed()
    }
}

fn rule_strategy(head: u8) -> impl Strategy<Value = Rule> {
    (movement_strategy(), 1u32..=10).prop_flat_map(move |(movement, priority)| {
        let has_recursion = matches!(movement, Movement::Recursion | Movement::CamRecursion);
        let starvation = prop_oneof![Just(None), (1u32..=3).prop_map(Some)];
        let max_activations = prop_oneof![Just(None), (1u32..=3).prop_map(Some)];
        let cam = match movement {
            Movement::Cam | Movement::CamRecursion => cam_strategy().prop_map(Some).boxed(),
            _ => Just(None).boxed(),
        };
        let recursion = match movement {
            Movement::Recursion | Movement::CamRecursion => recursion_strategy().prop_map(Some).boxed(),
            _ => Just(None).boxed(),
        };
        let shifts_and_feedback = match movement {
            Movement::Shift => (direction_strategy(), prop_oneof![Just(false), Just(true)])
                .prop_map(|(dir, has_feedback)| {
                    let shifts = vec![vec![ShiftSpec::new(dir, 1)]];
                    let feedback = has_feedback.then(|| cellaria::types::FeedbackSpec { timeout: 2, new_direction: dir });
                    (shifts, feedback)
                })
                .boxed(),
            _ => Just((Vec::new(), None)).boxed(),
        };

        (cam, recursion, shifts_and_feedback, starvation, max_activations, memory_strategy(has_recursion)).prop_map(
            move |(cam, recursion, (shifts, feedback), starvation_after, max_activations, memory)| Rule {
                id: vec![CellType(head)],
                pattern: vec![],
                shifts,
                changes: vec![],
                active_only: false,
                priority,
                min_age: 0,
                overflow: Default::default(),
                cam,
                tie_break: 0,
                starvation_after,
                feedback,
                recursion,
                memory,
                max_activations,
                cross_layer_reads: Vec::new(),
            },
        )
    })
}

/// 1-2 головы, 1-2 конкурирующих правила на голову — то же обоснование, что
/// в `property_extensions.rs`'s `rule_index_strategy`.
fn rule_index_strategy() -> impl Strategy<Value = HashMap<CellType, Vec<Rule>>> {
    prop::collection::vec(1u8..=CELL_ALPHABET, 1..=2).prop_flat_map(|heads| {
        let mut heads: Vec<u8> = heads;
        heads.sort_unstable();
        heads.dedup();
        let per_head: Vec<_> = heads.iter().map(|&h| prop::collection::vec(rule_strategy(h), 1..=2)).collect();
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
    prop::collection::vec((0..SIDE, 0..SIDE, 1u8..=CELL_ALPHABET), 1..=6)
}

fn build_engine(rule_index: &HashMap<CellType, Vec<Rule>>, cells: &[(usize, usize, u8)]) -> Engine<VecStorage> {
    let mut grid = Grid::new(VecStorage::new(SIDE, SIDE), HashSet::new());
    for &(x, y, t) in cells {
        grid.set_cell(x, y, Cell { value: CellValue::new(t), born_at: 0 });
    }
    Engine::new(grid, rule_index.clone())
}

fn dump_grid(engine: &Engine<VecStorage>) -> Vec<Cell> {
    (0..SIDE).flat_map(|y| (0..SIDE).map(move |x| (x, y))).map(|(x, y)| engine.grid().get_cell(x, y).copied().unwrap_or_default()).collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// Тот же фундаментальный инвариант, что и `property_extensions.rs`'s
    /// одноимённый тест, но с генератором, включающим `cam`/`recursion`/
    /// `cam`+`recursion` — ни один из которых раньше не проверялся
    /// property-тестами вообще.
    #[test]
    fn prop_repeated_runs_are_deterministic_with_recursion_cam(
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

    /// Тот же snapshot/restore-инвариант, что и `property_extensions.rs`,
    /// с тем же расширенным генератором.
    #[test]
    fn prop_snapshot_restore_continues_identically_with_recursion_cam(
        rule_index in rule_index_strategy(),
        cells in grid_strategy(),
        ticks_before in 1u32..=5,
        ticks_after in 1u32..=5,
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
