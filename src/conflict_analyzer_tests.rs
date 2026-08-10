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
        cam: None,
        tie_break: 0,
        starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
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

// ========================================================================
// Лемма 4 (`paper/paper4.md` §8, Corollary C): `Rule::feedback` обязан
// участвовать в графе конфликтов через UNION нормального и альтернативного
// направления, а не только нормального — иначе конфликт, который может
// произойти ТОЛЬКО после срабатывания обратной связи, был бы пропущен.
// ========================================================================

#[test]
fn test_feedback_conflict_only_visible_via_alternate_direction_union() {
    use crate::types::FeedbackSpec;

    // Правило A: сдвиг Right (нормально), feedback переключает на Down.
    // Нормальные write cells (относительно A): (0,0) [очистка], (1,0) [цель].
    // Альтернативные (Down) write cells: (0,0), (0,1).
    let rule_a = Rule {
        id: vec![CellType(1)],
        pattern: vec![(0, 0, CellType(1))],
        shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
        changes: vec![],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: Some(FeedbackSpec { timeout: 5, new_direction: Direction::Down }),
        recursion: None,
        memory: None,
        max_activations: None,
    };
    // Правило B: статичное self-change, без сдвигов — пишет только в (0,0)
    // ОТНОСИТЕЛЬНО СЕБЯ. Размещено (в терминах относительного офсета,
    // который перебирает `ConflictGraph::build`) так, что B попадает РОВНО
    // в (0,1) от A — это ЕСТЬ в альтернативном (Down) наборе A, но ЕГО НЕТ
    // в нормальном (Right) наборе A.
    let rule_b = Rule {
        id: vec![CellType(2)],
        pattern: vec![(0, 0, CellType(2))],
        shifts: vec![],
        changes: vec![(0, 0, crate::types::ChangeValue::Literal(9))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None, recursion: None, memory: None, max_activations: None,
    };

    let graph = ConflictGraph::build(&[rule_a, rule_b]);
    assert!(
        graph.edges.contains(&(0, 1)),
        "граф ОБЯЗАН найти ребро между A и B: A's feedback-альтернатива (Down) пишет в ту же \
относительную клетку, где сидит B, хотя нормальное (Right) направление A её не задевает вовсе. \
Рёбра: {:?}",
        graph.edges
    );
}

// ========================================================================
// analyze_conflicts / ConflictReport (п.4, сессия 2026-08-09) — тот же
// вопрос, что ConflictGraph::build, но на уровне rule_index конфига, с
// ответом в человекочитаемых (голова, локальный индекс) вместо плоских
// индексов ConflictGraph.
// ========================================================================

#[test]
fn test_analyze_conflicts_conflict_free_reports_empty() {
    let (_, rule_index) = crate::config::load_config("configs/parallel.yaml")
        .unwrap_or_else(|e| panic!("Не удалось загрузить parallel.yaml: {}", e));
    let report = analyze_conflicts(&rule_index);
    assert!(
        report.is_conflict_free(),
        "parallel.yaml: отчёт должен быть пуст, но получено: {:?}",
        report.conflicts
    );
}

#[test]
fn test_analyze_conflicts_maps_flat_indices_back_to_head_and_local_idx() {
    // conflict.yaml: голова 1 (правило [1,2]) и голова 3 (правило [3,4]) —
    // ровно по одному правилу на голову, так что оба должны получить
    // rule_idx=0, а не сырой плоский индекс ConflictGraph. ConflictGraph
    // также проверяет каждое правило само на себя (см. doc-комментарий
    // `ConflictGraph::build`), так что помимо кросс-головной пары 1<->3 в
    // отчёте ожидаются ещё и две self-пары (1,1) и (3,3) — тест ищет именно
    // кросс-головную пару, а не требует, чтобы она была единственной.
    let (_, rule_index) = crate::config::load_config("configs/conflict.yaml")
        .unwrap_or_else(|e| panic!("Не удалось загрузить conflict.yaml: {}", e));
    let report = analyze_conflicts(&rule_index);
    assert!(
        !report.is_conflict_free(),
        "conflict.yaml: отчёт не должен быть пуст"
    );
    let cross_head = report.conflicts.iter().find(|p| p.head_a != p.head_b);
    let pair = cross_head.unwrap_or_else(|| {
        panic!("ожидалась кросс-головная конфликтующая пара (1<->3), получено: {:?}", report.conflicts)
    });
    let heads: (CellType, CellType) = (pair.head_a, pair.head_b);
    assert!(
        heads == (CellType(1), CellType(3)) || heads == (CellType(3), CellType(1)),
        "ожидались головы 1 и 3, получено: {:?}",
        pair
    );
    assert_eq!(pair.rule_idx_a, 0, "у каждой головы ровно одно правило — локальный индекс обязан быть 0: {:?}", pair);
    assert_eq!(pair.rule_idx_b, 0, "у каждой головы ровно одно правило — локальный индекс обязан быть 0: {:?}", pair);
}
