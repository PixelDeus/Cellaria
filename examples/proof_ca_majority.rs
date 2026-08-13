//! paper3.md §4.1: конструктивное вложение 1D CA (k=3) в Cellaria
//! (`ca_to_cellaria`). `configs/ca_majority.yaml` кодирует CA-правило
//! "большинство" (`f(a,b,c) = 1`, если `a+b+c >= 2`) через маркер (тип 99),
//! идущий вправо по строке данных.
//!
//! Результат оказывается сдвинут на -1 относительно исходной нумерации CA:
//! маркер стартует на позиции 0 (данные — с позиции 1), и каждое правило
//! пишет "позади себя" ПОСЛЕ сдвига — так что итог для CA-позиции `i`
//! (`f(data[i], data[i+1], data[i+2])`) оказывается записан в клетку сетки
//! `i-1`, а не `i`. Это фиксированное, детерминированное смещение (не баг
//! конфига — тот же вывод и в его собственном комментарии), а не
//! отклонение от §4.1.2.
use cellaria::config::load_config;
use cellaria::Engine;

fn majority(a: u8, b: u8, c: u8) -> u8 {
    if a + b + c >= 2 {
        1
    } else {
        0
    }
}

fn main() {
    let data = [0u8, 1, 0, 1, 0, 0, 1, 1, 1, 0];
    let n = data.len();

    let (grid, rule_index) = load_config("configs/ca_majority.yaml").expect("load ca_majority.yaml");
    let mut engine = Engine::new(grid, rule_index);
    loop {
        let (matches, _) = engine.run_tick();
        if matches.is_empty() {
            break;
        }
    }

    let g = engine.grid();
    let actual: Vec<u8> = (0..n).map(|x| g.get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(0)).collect();

    // f(data[i], data[i+1], data[i+2]) для i=1..=n (1-индексация CA, вне
    // диапазона данных читается как 0 — граница строки).
    let d = |i: usize| -> u8 {
        if i >= 1 && i <= n {
            data[i - 1]
        } else {
            0
        }
    };
    let expected: Vec<u8> = (1..=n).map(|i| majority(d(i), d(i + 1), d(i + 2))).collect();
    // Смещение на -1: итог для CA-позиции i лежит в клетке сетки i-1.
    let expected_at_grid_pos: Vec<u8> = expected.clone();

    println!("data:     {:?}", data);
    println!("actual:   {:?}", actual);
    println!("expected: {:?} (= f(data[i..i+3]) для i=1..={}, записано со сдвигом -1)", expected_at_grid_pos, n);

    assert_eq!(actual, expected_at_grid_pos, "CA majority mapping mismatch");
    println!("\nOK: результат совпадает с f(data[i], data[i+1], data[i+2]) для каждой позиции.");
}
