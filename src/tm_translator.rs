// ============================================================================
// translate_tm — формальная редукция TM → Cellaria
// ============================================================================
//
// Функция translate_tm строит набор правил Cellaria по спецификации
// машины Тьюринга M = (Q, Σ, δ, q₀, F). Доказательство корректности:
// один шаг TM ≡ один тик Cellaria.
//
// Кодирование:
//   Состояние q ∈ Q         → тип ячейки из states[]
//   Символ a ∈ Σ            → тип ячейки из symbols[]
//   Пробел _                → тип 0 (default)
//   Головка над символом    → паттерн [q_type, a_type] (d=R) или [a_type, q_type] (d=L)
//
// Правила для δ(q, a) = (q', a', d):
//   d=R: id: [q_type, a_type]
//        shifts: [{east, 1}]
//        changes: [(-1, 0, a'_type), (0, 0, q'_type)]
//   d=L: id: [a_type, q_type]
//        shifts: [{west, 1}]
//        changes: [(0, 0, q'_type), (1, 0, a'_type)]
//   q' ∈ F: id: [q_type, a_type]  (без shifts)
//        changes: [(0, 0, a'_type)]  — головка поглощается, симуляция останавливается

use crate::types::{CellType, ChangeValue, Direction, Rule, ShiftSpec};

/// Построить набор правил Cellaria по спецификации машины Тьюринга.
///
/// # Аргументы
///
/// * `states` — типы для состояний Q (например, [10, 11, 12])
/// * `symbols` — типы для символов Σ (например, [1, 2] для {0, 1})
/// * `delta` — функция перехода: (q_idx, a_idx, q'_idx, a'_idx, 'L'|'R')
/// * `start_state` — индекс начального состояния в states
/// * `final_states` — индексы конечных состояний в states
///
/// # Возврат
///
/// Вектор правил Cellaria, симулирующих TM.
pub fn translate_tm(
    states: &[u8],
    symbols: &[u8],
    delta: &[(usize, usize, usize, usize, char)],
    _start_state: usize,
    final_states: &[usize],
) -> Vec<Rule> {
    let mut rules: Vec<Rule> = Vec::with_capacity(delta.len());

    for &(q_idx, a_idx, qp_idx, ap_idx, direction) in delta {
        let q_type = states[q_idx];
        let a_type = symbols[a_idx];
        let qp_type = states[qp_idx];
        let ap_type = symbols[ap_idx];

        let is_final = final_states.contains(&qp_idx);

        let (id, shifts, changes) = if is_final {
            // Конечное состояние: головка останавливается.
            // Правило без сдвигов, только запись нового символа.
            // После применения правило для (q'_type, _) не найдётся → остановка.
            // После L-шага: [q', a'] на (cx-1, cx). После R-шага: [q', a'] на (cx, cx+1).
            // В обоих случаях паттерн [q', a'].
            let id = vec![CellType::new(q_type), CellType::new(a_type)];
            let shifts: Vec<Vec<ShiftSpec>> = vec![];
            let changes = vec![(0i32, 0i32, ChangeValue::Literal(ap_type))];
            (id, shifts, changes)
        } else {
            match direction {
                'R' | 'r' => {
                    // Головка справа от символа: [q, a]
                    let id = vec![CellType::new(q_type), CellType::new(a_type)];
                    let shifts = vec![vec![ShiftSpec::new(Direction::Right, 1)]];
                    let changes = vec![
                        (-1i32, 0i32, ChangeValue::Literal(ap_type)),
                        (0i32, 0i32, ChangeValue::Literal(qp_type)),
                    ];
                    (id, shifts, changes)
                }
                'L' | 'l' => {
                    // Головка слева от символа: [a, q]
                    let id = vec![CellType::new(a_type), CellType::new(q_type)];
                    let shifts = vec![vec![ShiftSpec::new(Direction::Left, 1)]];
                    let changes = vec![
                        (0i32, 0i32, ChangeValue::Literal(qp_type)),
                        (1i32, 0i32, ChangeValue::Literal(ap_type)),
                    ];
                    (id, shifts, changes)
                }
                _ => panic!("translate_tm: unknown direction '{}'", direction),
            }
        };

        // Строим pattern из id для обратной совместимости
        let pattern: Vec<(i8, i8, CellType)> = id.iter().enumerate()
            .map(|(i, &ct)| (i as i8, 0i8, ct))
            .collect();

        rules.push(Rule {
            id,
            pattern,
            shifts,
            changes,
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None, cross_layer_reads: Vec::new(),
        });
    }

    rules
}

#[cfg(test)]
#[path = "tm_translator_tests.rs"]
mod tests;
