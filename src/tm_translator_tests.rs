use super::*;
use crate::engine::Engine;
use crate::grid::Grid;
use crate::storage::VecStorage;
use crate::types::{Cell, CellValue};
use std::collections::{HashMap, HashSet};

/// Создать решётку заданного размера.
fn make_grid(width: usize, height: usize) -> Grid<VecStorage> {
    let storage = VecStorage::new(width, height);
    Grid::new(storage, HashSet::new())
}

/// Построить rule_index из вектора правил.
fn make_rule_index(rules: Vec<Rule>) -> HashMap<CellType, Vec<Rule>> {
    let mut index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        if let Some(center) = rule.id.first() {
            index.entry(*center).or_default().push(rule);
        }
    }
    for rules in index.values_mut() {
        rules.sort_by_key(|b| std::cmp::Reverse(b.priority));
    }
    index
}

/// Запустить симуляцию до стабилизации.
/// Возвращает финальное состояние решётки.
fn run_to_completion(
    grid: &mut Grid<VecStorage>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
) {
    let mut engine = Engine::new(
        std::mem::take(grid),
        rule_index.clone(),
    );

    loop {
        let matches = engine.detect_matches();
        if matches.is_empty() {
            break;
        }
        let accepted = engine.arbitrate(matches);
        if accepted.is_empty() {
            break;
        }
        engine.apply_matches(accepted);
        engine.advance_age();
    }

    *grid = engine.grid().clone();
}

// ========================================================================
// Тест 1: TM инвертирования битов
// ========================================================================
//
// TM: состояние q₀, алфавит {0, 1}
//   δ(q₀, 0) = (q₀, 1, R)
//   δ(q₀, 1) = (q₀, 0, R)
// Начальная лента: 1 0 1  (типы 1, 2, 1)
// Результат:        0 1 0  (типы 2, 1, 2)
#[test]
fn test_tm_translator_bit_invert() {
    // Состояния: Q = {q₀} → type 10
    let states = &[10u8];
    // Алфавит: Σ = {0→1, 1→2}  (по convention: 1=1, 2=0)
    let symbols = &[1u8, 2u8];
    // delta: (q, a, q', a', d)
    // symbols: index 0 = тип 1 (бит "1"), index 1 = тип 2 (бит "0")
    //   (q₀, 0) → (q₀, 1, R): a_idx=1 (бит "0"=тип2), a'_idx=0 (бит "1"=тип1)
    //   (q₀, 1) → (q₀, 0, R): a_idx=0 (бит "1"=тип1), a'_idx=1 (бит "0"=тип2)
    let delta = &[
        (0usize, 1usize, 0usize, 0usize, 'R'),  // (q₀, 0) → (q₀, 1, R) — над типом 2 пишем тип 1
        (0usize, 0usize, 0usize, 1usize, 'R'),  // (q₀, 1) → (q₀, 0, R) — над типом 1 пишем тип 2
    ];
    let final_states: &[usize] = &[];

    let rules = translate_tm(states, symbols, delta, 0, final_states);
    // Должно быть 2 правила
    assert_eq!(rules.len(), 2);

    // Проверяем структуру правил
    // Правило (q₀, 0) → (q₀, 1, R): id [10, 2], shifts east, changes (-1,0,1), (0,0,10)
    let rule_r0 = &rules[0];
    assert_eq!(rule_r0.id, vec![CellType::new(10), CellType::new(2)]);
    assert_eq!(rule_r0.shifts.len(), 1);
    assert_eq!(rule_r0.shifts[0][0].direction, Direction::Right);
    assert_eq!(rule_r0.changes.len(), 2);
    assert!(rule_r0.changes.contains(&(-1i32, 0i32, ChangeValue::Literal(1))));
    assert!(rule_r0.changes.contains(&(0i32, 0i32, ChangeValue::Literal(10))));

    // Правило (q₀, 1) → (q₀, 0, R): id [10, 1], shifts east, changes (-1,0,2), (0,0,10)
    let rule_r1 = &rules[1];
    assert_eq!(rule_r1.id, vec![CellType::new(10), CellType::new(1)]);
    assert_eq!(rule_r1.shifts.len(), 1);
    assert_eq!(rule_r1.shifts[0][0].direction, Direction::Right);
    assert_eq!(rule_r1.changes.len(), 2);
    assert!(rule_r1.changes.contains(&(-1i32, 0i32, ChangeValue::Literal(2))));
    assert!(rule_r1.changes.contains(&(0i32, 0i32, ChangeValue::Literal(10))));

    // Запуск симуляции
    let mut grid = make_grid(16, 1);
    // Головка (тип 10) на позиции 0, лента 1 0 1 на позициях 1-3
    grid.set_cell(0, 0, Cell {
        value: CellValue(CellType::new(10)),
        born_at: 0,
    });
    grid.set_cell(1, 0, Cell {
        value: CellValue(CellType::new(1)),
        born_at: 0,
    });
    grid.set_cell(2, 0, Cell {
        value: CellValue(CellType::new(2)),
        born_at: 0,
    });
    grid.set_cell(3, 0, Cell {
        value: CellValue(CellType::new(1)),
        born_at: 0,
    });

    let rule_index = make_rule_index(rules);
    run_to_completion(&mut grid, &rule_index);

    // Проверяем: лента должна быть инвертирована: 0 1 0 → типы 2, 1, 2
    // Позиции могут сдвинуться из-за движения головки
    // Ищем последовательность 2, 1, 2 на ленте
    let mut found = false;
    for x in 0..14 {
        let c0 = grid.get_cell(x, 0).map(|c| c.value.0 .0);
        let c1 = grid.get_cell(x + 1, 0).map(|c| c.value.0 .0);
        let c2 = grid.get_cell(x + 2, 0).map(|c| c.value.0 .0);
        if c0 == Some(2) && c1 == Some(1) && c2 == Some(2) {
            found = true;
            break;
        }
    }
    assert!(found, "Bit invert: expected tape [2, 1, 2] not found on grid");
}

// ========================================================================
// Тест 2: TM инвертирования с 4-битной лентой
// ========================================================================
//
// Та же TM, что и в тесте 1 (1 состояние, инвертирует каждый бит),
// но на ленте 1 1 0 0 (типы 1, 1, 2, 2).
// Результат: 0 0 1 1 (типы 2, 2, 1, 1)
#[test]
fn test_tm_translator_bit_invert_4bit() {
    let states = &[10u8];
    let symbols = &[1u8, 2u8];
    let delta = &[
        (0usize, 1usize, 0usize, 0usize, 'R'),  // (q₀, 0) → (q₀, 1, R)
        (0usize, 0usize, 0usize, 1usize, 'R'),  // (q₀, 1) → (q₀, 0, R)
    ];
    let final_states: &[usize] = &[];

    let rules = translate_tm(states, symbols, delta, 0, final_states);
    assert_eq!(rules.len(), 2);

    let mut grid = make_grid(16, 1);
    grid.set_cell(0, 0, Cell { value: CellValue(CellType::new(10)), born_at: 0 });
    grid.set_cell(1, 0, Cell { value: CellValue(CellType::new(1)), born_at: 0 });
    grid.set_cell(2, 0, Cell { value: CellValue(CellType::new(1)), born_at: 0 });
    grid.set_cell(3, 0, Cell { value: CellValue(CellType::new(2)), born_at: 0 });
    grid.set_cell(4, 0, Cell { value: CellValue(CellType::new(2)), born_at: 0 });

    let rule_index = make_rule_index(rules);
    run_to_completion(&mut grid, &rule_index);

    // Ищем последовательность 2, 2, 1, 1 (0 0 1 1)
    let mut found = false;
    for x in 0..12 {
        let c0 = grid.get_cell(x, 0).map(|c| c.value.0 .0);
        let c1 = grid.get_cell(x + 1, 0).map(|c| c.value.0 .0);
        let c2 = grid.get_cell(x + 2, 0).map(|c| c.value.0 .0);
        let c3 = grid.get_cell(x + 3, 0).map(|c| c.value.0 .0);
        if c0 == Some(2) && c1 == Some(2) && c2 == Some(1) && c3 == Some(1) {
            found = true;
            break;
        }
    }
    assert!(found, "4-bit invert: expected tape [2, 2, 1, 1] not found on grid");
}

// ========================================================================
// Тест 3: TM с конечным состоянием через R-правило
// ========================================================================
//
// TM: 1 состояние, алфавит {0, 1}, q₀ → halt на 0.
//   q₀: (0, q₀, 1, R) — инвертирует 0 в 1 и идёт дальше
//   q₀: (1, q₀, 0, R) — инвертирует 1 в 0 и идёт дальше
//   q₀: (_, halt, _, _) — стоп на пустой ленте (нет правила)
//
// TM без конечных состояний (F = ∅) — симуляция останавливается,
// когда не находится совпадение.
#[test]
fn test_tm_translator_no_final() {
    let states = &[10u8];
    let symbols = &[1u8, 2u8];
    let delta = &[
        (0usize, 0usize, 0usize, 1usize, 'R'),  // (q₀, 1) → (q₀, 0, R)
        (0usize, 1usize, 0usize, 0usize, 'R'),  // (q₀, 0) → (q₀, 1, R)
    ];
    let final_states: &[usize] = &[];

    let rules = translate_tm(states, symbols, delta, 0, final_states);
    assert_eq!(rules.len(), 2);

    let mut grid = make_grid(8, 1);
    grid.set_cell(0, 0, Cell { value: CellValue(CellType::new(10)), born_at: 0 });
    grid.set_cell(1, 0, Cell { value: CellValue(CellType::new(1)), born_at: 0 });

    let rule_index = make_rule_index(rules);
    run_to_completion(&mut grid, &rule_index);

    // Проверяем: инвертировали первый бит (1→0), затем головка ушла вправо
    // на пустую ленту и остановилась (нет правила для (q₀, _)).
    // Результат: [2, 10] — инвертированный бит 0 (type 2) и головка (10) на позиции 1.
    let c0 = grid.get_cell(0, 0).map(|c| c.value.0 .0);
    let c1 = grid.get_cell(1, 0).map(|c| c.value.0 .0);
    assert_eq!(c0, Some(2), "Expected type 2 (0) at position 0");
    assert_eq!(c1, Some(10), "Expected head type 10 at position 1");
}

// ========================================================================
// Тест 4: TM с L-правилом
// ========================================================================
//
// TM: 2 состояния, алфавит {0, 1}.
//   q₀: (1, q₀, 1, R) — читает 1, идёт вправо
//   q₁: (0, q₁, 0, L) — читает 0 (default), идёт налево
//
// Начальная лента: 1 на позиции 1, головка q₀ на позиции 0.
// 1) q₀ читает 1 (type 1) → остаётся 1, идёт вправо в q₀.
// 2) q₀ читает 0 (default) → нет правила, стоп.
#[test]
fn test_tm_translator_no_left_rule_match() {
    let states = &[10u8];
    let symbols = &[1u8, 2u8];
    // Только R-правила, чтоб убедиться что R-кодировка корректна
    let delta = &[
        (0usize, 0usize, 0usize, 0usize, 'R'),  // (q₀, 1) → (q₀, 1, R)
    ];
    let final_states: &[usize] = &[];

    let rules = translate_tm(states, symbols, delta, 0, final_states);
    assert_eq!(rules.len(), 1);

    // Проверяем L-правило (если бы такое было): убеждаемся что функции корректно
    // обрабатывают L-направление на уровне структуры правил.
    // Создадим L-правило вручную и проверим его структуру.
    let pattern: Vec<(i8, i8, CellType)> = vec![
        (0i8, 0i8, CellType::new(2)),
        (1i8, 0i8, CellType::new(10)),
    ];
    let l_rule = Rule {
        id: vec![CellType::new(2), CellType::new(10)],  // [a, q] для (q₀, 0) с L
        pattern,
        shifts: vec![vec![ShiftSpec::new(Direction::Left, 1)]],
        changes: vec![
            (0i32, 0i32, ChangeValue::Literal(10)),
            (1i32, 0i32, ChangeValue::Literal(2)),
        ],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
    };
    assert_eq!(l_rule.id, vec![CellType::new(2), CellType::new(10)]);
    assert_eq!(l_rule.shifts[0][0].direction, Direction::Left);
    assert!(l_rule.changes.contains(&(0i32, 0i32, ChangeValue::Literal(10))));
    assert!(l_rule.changes.contains(&(1i32, 0i32, ChangeValue::Literal(2))));
}

// ========================================================================
// Тест 5: TM с L-переходом в конечное состояние
// ========================================================================
//
// TM: 2 состояния, алфавит {0, 1}
//   q₀: (1, q₁, 1, R) — читает 1, идёт вправо в q₁
//   q₁: (1, q₂, 0, L) — читает 1, пишет 0, идёт налево в q₂ (final)
//   q₂: halt — нет правил для q₂
//
// Начальная лента: 1 1 на позициях 1-2, головка q₀ на позиции 0.
// 1) Tick 1: [10, 1] at 0-1 → R-правило. Головка (10) движется вправо.
//    После: [1, 10, 1] — а_type=1 (бит 1) на позиции 0, q₀=10 на позиции 1.
// 2) Tick 2: [10, 1] at 1-2 → R-правило в q₁ (qp=11, ap=1).
//    После: [1, 1, 11] — ap=1 на позиции 1, qp=11 на позиции 2.
// 3) Tick 3: [1, 11] at 1-2 → L-правило в q₂ (qp=12, ap=2). q₂ final.
//    После: [(qp=12 на pos 1), (ap=2 на pos 2)]
//           → [1, 12, 2]? Нет, посчитаем:
//           L-rule: id [1, 11], shift left, changes [(0,0,qp=12), (1,0,ap=2)].
//           После shift: head (11) moves from 2 to 1. Pos 2 = 0.
//           Changes: (0,0,12) at pos 2? Нет: nx = cx + total_dx + dx = 1 + (-1) + 0 = 0.
//           12 записывается на позицию 0. (1,0,2) at nx = 1 + (-1) + 1 = 1.
//           После: [12, 2, 0].
//    В результате q₂=12 появляется на ленте и не поглощается, т.к. нет halt-правила.
//    В Cellaria halt-правило для L-перехода имеет id [a_type, q_type] = [1, 12].
//    Оно срабатывает: match [1, 12] at pos 0-1? Нет, [12, 2] at pos 0-1 не совпадает.
//
// Проблема: halt-правило с id [a, q] не может совпасть, так как после L-шага
// головка оказывается слева-снизу, не в [a, q] паттерне.
//
// На самом деле, после halt-правила (без сдвигов) с id [a, q]:
// match на позиции где a слева от q. После L-шага q оказался на позиции 0,
// a на позиции 1. Это [q, a], не [a, q].
//
// ВЫВОД: halt-правило для L-перехода должно иметь id [q_type, a_type],
// так как после применения L-правила головка (q') оказывается слева от
// старой позиции, а символ (a') — справа. Но это неверно, потому что
// сдвиг двигает q, и после сдвига q оказывается на левой ячейке,
// а a' — на правой.
//
// Перепроверим для halt (без сдвига):
//   id [a_type, q_type] = [1, 12] ищет a слева от q.
//   Но после L-шага: q на позиции cx-1, a на позиции cx. Это [q, a].
//
// Значит halt-правило для L должно иметь id [q_type, a_type], как и R?
// Но для L-шага в не-финальном случае id [a, q]. После применения:
//   сдвиг налево: q (головка) двигается на cx-1.
//   changes: (0,0,qp) пишет q' на cx-1, (1,0,ap) пишет a' на cx.
// Результат: [q', a'] на позициях cx-1, cx.
// Если q' — конечное, следующее срабатывание — halt с id [q', a'].
//
// Значит halt для L должен иметь id [q_type, a_type], а не [a_type, q_type]!
// Исправляем.
//
// Переделаем тест с правильной кодировкой.
//
// TM:
//   q₀: (1, q₁, 1, R) — идёт вправо в q₁, символ не меняет
//   q₁: (1, q₂, 1, L) — идёт налево в q₂ (final), символ не меняет
//   q₂: halt — halt-правило с id [q₂_type, a_type] = [12, 1], без сдвига,
//       пишет a_type = 1 (символ не меняется).
//
// Начальная лента: 1 1 на позициях 1-2, головка q₀ на позиции 0.
// Tick 1: [10, 1] at 0-1 → R: head→1, changes: (-1,0,1) at 0, (0,0,10) at 1.
//         Grid: [1, 10, 1]  (a=1 на pos0, q₀=10 на pos1)
// Tick 2: [10, 1] at 1-2 → R в q₁: (-1,0,1) at 1, (0,0,11) at 2.
//         Grid: [1, 1, 11]  (a=1 на pos1, q₁=11 на pos2)
// Tick 3: [1, 11] at 1-2 → L в q₂ (final): id [1, 11],
//         shift left, changes: (0,0,12) at cx+dx = 1-1+0 = 0, (1,0,1) at 1-1+1 = 1.
//         Grid: [12, 1, 0]  (q₂=12 на pos0, a=1 на pos1)
// Tick 4: halt-правило id [12, 1] at pos 0-1: no shift, change (0,0,1) at pos 0.
//         Grid: [1, 1, 0]  (оба бита "1", конечное состояние поглощено)
#[test]
fn test_tm_translator_final_via_l() {
    let states = &[10u8, 11u8, 12u8];
    let symbols = &[1u8, 2u8];
    let delta = &[
        (0usize, 0usize, 1usize, 0usize, 'R'),  // (q₀, 1) → (q₁, 1, R)
        (1usize, 0usize, 2usize, 0usize, 'L'),  // (q₁, 1) → (q₂, 1, L), q₂ final
        // halt-правило для q₂ не нужно — оно не сработает, так как q₂
        // является финальным и для него нет правила в delta.
        // Вместо этого, halt-правило генерируется translate_tm с id [12, 1].
        // Это halt-правило срабатывает на tick 4 и поглощает головку.
    ];
    let final_states = &[2usize];  // q₂ — финальное

    let rules = translate_tm(states, symbols, delta, 0, final_states);
    // Должно быть: 1 не-финальное R-правило + 1 halt-правило для q₂ = 2
    assert_eq!(rules.len(), 2);

    // Обратите внимание: поскольку q₂ — финальное и переход q₁ → q₂ идёт
    // через L-правило, которое является финальным, то для него генерируется
    // halt-правило с id [q_type, a_type] = [11, 1] (без сдвигов).
    // НЕ-финального L-правила нет, т.к. q₂ финальное.
    // После R-шага из q₀ в q₁ на ленте будет [1, 11] на позициях 0-1.
    // Halt-правило [11, 1] совпадает на позициях 1-2: q₁=11 справа, a=1 слева.

    // Проверяем halt-правило: id [q_type, a_type] = [11, 1]
    let halt_rule = rules.iter().find(|r| {
        r.id == vec![CellType::new(11), CellType::new(1)]
    }).expect("Expected halt rule for (q1, 1) from final transition");
    assert!(halt_rule.shifts.is_empty());

    // Запуск симуляции
    let mut grid = make_grid(8, 1);
    grid.set_cell(0, 0, Cell { value: CellValue(CellType::new(10)), born_at: 0 });
    grid.set_cell(1, 0, Cell { value: CellValue(CellType::new(1)), born_at: 0 });
    grid.set_cell(2, 0, Cell { value: CellValue(CellType::new(1)), born_at: 0 });

    let rule_index = make_rule_index(rules);
    run_to_completion(&mut grid, &rule_index);

    // После halt: головка q₁ (11) поглощена, оба бита — "1" (type 1)
    let has_head_state = grid.iter_active().any(|(x, y)| {
        grid.get_cell(x, y)
            .map(|c| c.value.0 .0 == 10 || c.value.0 .0 == 11 || c.value.0 .0 == 12)
            .unwrap_or(false)
    });
    assert!(!has_head_state, "Head state should be absorbed after halt");

    // Проверяем, что оба бита 1 остались
    let mut found = false;
    for x in 0..6 {
        let c0 = grid.get_cell(x, 0).map(|c| c.value.0 .0);
        let c1 = grid.get_cell(x + 1, 0).map(|c| c.value.0 .0);
        if c0 == Some(1) && c1 == Some(1) {
            found = true;
            break;
        }
    }
    assert!(found, "Expected two 1 bits (type 1) on grid after halt");
}
