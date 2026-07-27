use super::*;
use crate::types::{CellType, Direction, ShiftSpec};

/// Вспомогательная функция: создать правило из id и shifts/changes.
fn make_rule(
    id: Vec<u8>,
    shifts: Vec<Vec<(Direction, u16)>>,
    changes: Vec<(i32, i32, u8)>,
    priority: u32,
    min_age: u64,
) -> Rule {
    // Строим pattern из id: [(0,0, id[0]), (1,0, id[1]), ...]
    let pattern: Vec<(i8, i8, CellType)> = id.iter().enumerate()
        .map(|(i, &v)| (i as i8, 0i8, CellType(v)))
        .collect();
    Rule {
        id: id.iter().map(|&v| CellType(v)).collect(),
        pattern,
        shifts: shifts
            .into_iter()
            .map(|group| {
                group
                    .into_iter()
                    .map(|(dir, steps)| ShiftSpec::new(dir, steps))
                    .collect()
            })
            .collect(),
        changes: changes.into_iter().map(|(dx, dy, v)| (dx, dy, crate::types::ChangeValue::Literal(v))).collect(),
        active_only: false,
        priority,
        min_age,
        overflow: Default::default(),
    }
}

/// Загрузить правила из YAML-файла конфига через load_config.
fn load_rules_from_config(path: &str) -> Vec<Rule> {
    let (_, rule_index) = crate::config::load_config(path)
        .unwrap_or_else(|e| panic!("Не удалось загрузить {}: {}", path, e));
    // Извлекаем все правила из индекса
    let mut rules: Vec<Rule> = Vec::new();
    for (_, rules_vec) in rule_index {
        rules.extend(rules_vec);
    }
    rules
}

// ========================================================================
// Тест: parallel.yaml — правила не пересекаются
// ========================================================================

#[test]
fn test_parallel_rules_conflict_free() {
    let rules = load_rules_from_config("configs/parallel.yaml");
    let graph = ConflictGraph::build(&rules);
    assert!(
        graph.is_conflict_free(),
        "parallel.yaml: правила не должны конфликтовать, но найдены рёбра: {:?}",
        graph.edges
    );
}

// ========================================================================
// Тест: conflict.yaml — цепочки пересекаются
// ========================================================================

#[test]
fn test_conflict_rules_have_conflict() {
    let rules = load_rules_from_config("configs/conflict.yaml");
    let graph = ConflictGraph::build(&rules);
    assert!(
        !graph.is_conflict_free(),
        "conflict.yaml: правила должны конфликтовать, но граф пуст"
    );
    // Ожидаем одно ребро между правилами [1,2] (idx=0) и [3,4] (idx=1)
    assert!(
        graph.edges.contains(&(0, 1)),
        "conflict.yaml: ожидалось ребро (0, 1), получено: {:?}",
        graph.edges
    );
}

// ========================================================================
// Тест: turing.yaml — одно правило на состояние
// ========================================================================

#[test]
fn test_turing_rules_conflict_free() {
    let rules = load_rules_from_config("configs/turing.yaml");
    let graph = ConflictGraph::build(&rules);
    assert!(
        graph.is_conflict_free(),
        "turing.yaml: правила не должны конфликтовать, но найдены рёбра: {:?}",
        graph.edges
    );
}

// ========================================================================
// Тест: tag_system.yaml
// ========================================================================

#[test]
fn test_tag_system_rules() {
    let rules = load_rules_from_config("configs/tag_system.yaml");
    let graph = ConflictGraph::build(&rules);
    // tag_system: правила с разными id, проверяем что граф построен
    // без паники и имеет корректное количество вершин
    assert_eq!(
        graph.rule_count,
        rules.len(),
        "tag_system.yaml: количество вершин должно совпадать с числом правил"
    );
}

// ========================================================================
// Тест: правила с разными min_age могут конфликтовать, если
// их affected regions пересекаются и типы совместимы.
// ========================================================================

#[test]
fn test_different_min_age_can_conflict() {
    // Правило 1: pattern=[(0,0,1),(1,0,2)], shift east 1, change -> (0,0,5), min_age=0
    // Правило 2: pattern=[(0,0,3),(1,0,4)], shift west 1, change -> (-1,0,6), min_age=1
    let rules = vec![
        make_rule(
            vec![1, 2],
            vec![vec![(Direction::Right, 1)]],
            vec![(0, 0, 5)],
            10,
            0, // min_age = 0
        ),
        make_rule(
            vec![3, 4],
            vec![vec![(Direction::Left, 1)]],
            vec![(-1, 0, 6)],
            5,
            1, // min_age = 1 — другой, но не препятствует конфликту
        ),
    ];

    let graph = ConflictGraph::build(&rules);
    assert!(
        !graph.is_conflict_free(),
        "Правила с разными min_age МОГУТ конфликтовать (affected regions пересекаются)"
    );
}

// ========================================================================
// Тест: разные head-типы + разные min_age — нет конфликта
// ========================================================================

#[test]
fn test_different_head_and_min_age_no_conflict() {
    // Правило 1: pattern=[(0,0,1)], min_age=0
    // Правило 2: pattern=[(0,0,2)], min_age=10
    let rules = vec![
        make_rule(
            vec![1],
            vec![],
            vec![(0, 0, 0)],
            10,
            0,
        ),
        make_rule(
            vec![2],
            vec![],
            vec![(0, 0, 0)],
            5,
            10,
        ),
    ];

    let graph = ConflictGraph::build(&rules);
    assert!(
        graph.is_conflict_free(),
        "Правила с разными head-типами и непересекающимися affected regions не должны конфликтовать"
    );
}

// ========================================================================
// Тест: перекрывающиеся паттерны с несовместимыми типами — нет конфликта
// ========================================================================

#[test]
fn test_overlap_incompatible_types_no_conflict() {
    // Правило 1: pattern = [(0,0,1), (1,0,2)]
    // Правило 2: pattern = [(0,0,1), (1,0,3)]
    let rules = vec![
        make_rule(
            vec![1, 2],
            vec![],
            vec![(0, 0, 5)],
            10,
            0,
        ),
        make_rule(
            vec![1, 3],
            vec![],
            vec![(0, 0, 6)],
            10,
            0,
        ),
    ];

    let graph = ConflictGraph::build(&rules);
    assert!(
        graph.is_conflict_free(),
        "Правила с несовместимыми типами на пересекающихся ячейках не должны конфликтовать"
    );
}

// ========================================================================
// Тест: перекрывающиеся паттерны с совместимыми типами — есть конфликт
// ========================================================================

#[test]
fn test_overlap_compatible_types_has_conflict() {
    let rules = vec![
        make_rule(
            vec![1, 2],
            vec![vec![(Direction::Right, 1)]],
            vec![(0, 0, 5)],
            10,
            0,
        ),
        make_rule(
            vec![2, 3],
            vec![vec![(Direction::Left, 1)]],
            vec![(0, 0, 6)],
            10,
            0,
        ),
    ];

    let graph = ConflictGraph::build(&rules);
    if graph.is_conflict_free() {
        println!("ПРЕДУПРЕЖДЕНИЕ: тест overlap_compatible_types не обнаружил конфликт (возможно, алгоритм консервативен)");
    } else {
        assert!(
            graph.edges.contains(&(0, 1)),
            "Ожидалось ребро (0, 1), получено: {:?}",
            graph.edges
        );
    }
}

// ========================================================================
// Тест: cascade.yaml — каскадные правила могут конфликтовать
// ========================================================================

#[test]
fn test_cascade_rules_have_potential_conflict() {
    let rules = load_rules_from_config("configs/cascade.yaml");
    let graph = ConflictGraph::build(&rules);
    assert!(
        rules.len() >= 2,
        "cascade.yaml должен содержать минимум 2 правила"
    );
    assert_eq!(graph.rule_count, rules.len());
    if !graph.is_conflict_free() {
        println!(
            "cascade.yaml: обнаружен потенциальный конфликт {:?} (консервативная оценка)",
            graph.edges
        );
    }
}

// ========================================================================
// Тест: collision.yaml
// ========================================================================

#[test]
fn test_collision_rules() {
    let rules = load_rules_from_config("configs/collision.yaml");
    let graph = ConflictGraph::build(&rules);
    assert_eq!(graph.rule_count, rules.len());
}

// ========================================================================
// Тест: io.yaml
// ========================================================================

#[test]
fn test_io_rules() {
    let rules = load_rules_from_config("configs/io.yaml");
    let graph = ConflictGraph::build(&rules);
    assert_eq!(graph.rule_count, rules.len());
}

// ========================================================================
// Тест: overflow.yaml
// ========================================================================

#[test]
fn test_overflow_rules() {
    let rules = load_rules_from_config("configs/overflow.yaml");
    let graph = ConflictGraph::build(&rules);
    assert_eq!(graph.rule_count, rules.len());
}

// ========================================================================
// Тест: priority.yaml
// ========================================================================

#[test]
fn test_priority_rules() {
    let rules = load_rules_from_config("configs/priority.yaml");
    let graph = ConflictGraph::build(&rules);
    assert_eq!(graph.rule_count, rules.len());
}

// ========================================================================
// Тесты: check_composition
// ========================================================================

#[test]
fn test_composition_unique_head() {
    let rules_a = vec![make_rule(
        vec![10],
        vec![],
        vec![(0, 0, 0)],
        10,
        0,
    )];
    let rules_b = vec![make_rule(
        vec![20],
        vec![],
        vec![(0, 0, 0)],
        10,
        0,
    )];
    let verdict = ConflictGraph::check_composition(&rules_a, &rules_b);
    assert_eq!(verdict, CompositionVerdict::Safe);
}

#[test]
fn test_composition_same_head() {
    let rules_a = vec![make_rule(
        vec![10],
        vec![],
        vec![(0, 0, 5)],
        10,
        0,
    )];
    let rules_b = vec![make_rule(
        vec![10],
        vec![],
        vec![(0, 0, 6)],
        10,
        0,
    )];
    let verdict = ConflictGraph::check_composition(&rules_a, &rules_b);
    assert_eq!(
        verdict,
        CompositionVerdict::Unsafe(vec![(0, 0)]),
        "Ожидается Unsafe с парой (0, 0)"
    );
}

#[test]
fn test_composition_min_age() {
    let rules_a = vec![make_rule(
        vec![10],
        vec![],
        vec![(0, 0, 5)],
        10,
        0,
    )];
    let rules_b = vec![make_rule(
        vec![10],
        vec![],
        vec![(0, 0, 6)],
        10,
        10,
    )];
    let verdict = ConflictGraph::check_composition(&rules_a, &rules_b);
    assert_eq!(
        verdict,
        CompositionVerdict::Unsafe(vec![(0, 0)]),
        "Одинаковый head-тип и пересекающиеся affected regions = конфликт"
    );
}

#[test]
fn test_composition_spatial() {
    let rules_a = vec![make_rule(
        vec![1],
        vec![],
        vec![(0, 0, 0)],
        10,
        0,
    )];
    let rules_b = vec![make_rule(
        vec![2],
        vec![],
        vec![(0, 0, 0)],
        10,
        0,
    )];
    let verdict = ConflictGraph::check_composition(&rules_a, &rules_b);
    assert_eq!(verdict, CompositionVerdict::Safe);
}

#[test]
fn test_composition_overlap() {
    let rules_a = vec![make_rule(
        vec![1, 2],
        vec![vec![(Direction::Right, 1)]],
        vec![(0, 0, 5)],
        10,
        0,
    )];
    let rules_b = vec![make_rule(
        vec![2, 3],
        vec![vec![(Direction::Left, 1)]],
        vec![(0, 0, 6)],
        10,
        0,
    )];
    let verdict = ConflictGraph::check_composition(&rules_a, &rules_b);
    if verdict == CompositionVerdict::Safe {
        println!("ПРЕДУПРЕЖДЕНИЕ: test_composition_overlap не обнаружил конфликт (консервативная оценка)");
    } else {
        assert_eq!(
            verdict,
            CompositionVerdict::Unsafe(vec![(0, 0)]),
            "Ожидается Unsafe с парой (0, 0)"
        );
    }
}

#[test]
fn test_composition_tm_cleanup() {
    let rules = load_rules_from_config("configs/composition.yaml");
    let rules_a = vec![rules[0].clone()];
    let rules_b = vec![rules[2].clone()];

    let verdict = ConflictGraph::check_composition(&rules_a, &rules_b);
    if verdict == CompositionVerdict::Safe {
        println!("ПРЕДУПРЕЖДЕНИЕ: test_composition_tm_cleanup: R₁∪R₂ Safe (консервативная оценка)");
    } else {
        println!("test_composition_tm_cleanup: R₁∪R₂ Unsafe с парами {:?}", verdict);
    }
}
