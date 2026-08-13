//! Доказывает, что PRNG строится из ОБЫЧНЫХ правил Cellaria (pattern →
//! literal, без арифметики) — единственный недостающий кирпич для
//! "стохастической самомодификации + внешней фитнес-селекции" (см. память
//! проекта, финальная формулировка пункта 6 из block-F дискуссии): решётка
//! не может обратиться к внешнему `rand`, но Turing-полнота (§9)
//! гарантирует, что ХАОТИЧНАЯ (детерминированная, но невоспроизводимая без
//! честного прогона) последовательность реализуема — и не нужно даже
//! реального Turing-machine конструирования: elementary CA "Rule 30"
//! (Wolfram) — это буквально таблица подстановки (сосед-слева, сам,
//! сосед-справа) → новое значение, ровно то, что `pattern`/`changes` уже
//! делают нативно.
//!
//! Rule 30 — не моя эвристика: это тот же генератор, что использует
//! Wolfram Mathematica для `RandomInteger` (`Method -> "CellularAutomaton"`)
//! — проверенный на практике источник псевдослучайности, не игрушечный
//! пример.
//!
//! Три проверки:
//! 1. Читаем ОДНУ фиксированную клетку (центр) каждый тик — получаем
//!    битовую последовательность; статистически она должна быть близка к
//!    сбалансированной (не тождественный 0 или 1, не короткий период).
//! 2. Детерминизм: ДВА независимых прогона с ОДНИМ и тем же посевом дают
//!    ПОБИТОВО идентичную последовательность — то же самое свойство,
//!    которое делает "стохастическую" самомодификацию воспроизводимой по
//!    сиду, а не настоящей энтропией (см. обсуждение — модель полностью
//!    детерминирована, Аксиома 2).
//! 3. Разные посевы (разное начальное положение единственной "1") дают
//!    РАЗНЫЕ последовательности — подтверждает, что "посев" реально влияет
//!    на исход, а не игнорируется где-то по пути.

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Rule};
use cellaria::{Grid, VecStorage};

// Волна Rule 30 расходится на ~1 клетку/тик в каждую сторону от посева —
// решётка должна быть заведомо шире, чем `2×TICKS`, иначе край "заморозит"
// граничные клетки (см. doc-комментарий `build_seeded_grid`) задолго до
// конца прогона, и статистика перестаёт отражать хаотичное ядро.
const WIDTH: usize = 1001;
const CENTER: usize = WIDTH / 2;
const OFF: u8 = 1; // намеренно НЕ 0 (default_cell_type) — см. ниже
const ON: u8 = 2;
const TICKS: u32 = 400;

/// Rule 30: (сосед-слева, сам, сосед-справа) -> новое значение центра.
/// Официальная таблица (XOR(left, center OR right)):
/// 111->0 110->0 101->0 100->1 011->1 010->1 001->1 000->0
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

/// Кодируем 0/1 из таблицы в OFF/ON (не 0, чтобы не путаться с
/// `default_cell_type` — незаданная клетка вне решётки читается как
/// `default_cell_type=0`, а не как OFF, так что таблица должна оперировать
/// именно теми типами, что реально стоят в решётке).
fn ct(bit: u8) -> CellType {
    CellType(if bit == 1 { ON } else { OFF })
}

fn build_rule30_index() -> HashMap<CellType, Vec<Rule>> {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
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
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        });
    }
    idx
}

/// Строит начальную решётку: вся строка OFF, кроме одной клетки-посева
/// (классическая инициализация Rule 30 — единственная "1" по центру или
/// в произвольной позиции `seed_x`).
fn build_seeded_grid(seed_x: usize) -> Grid<VecStorage> {
    let mut grid = Grid::new(VecStorage::new(WIDTH, 1), Default::default());
    for x in 0..WIDTH {
        grid.set_cell(
            x,
            0,
            Cell {
                value: CellValue::new(OFF),
                born_at: 0,
            },
        );
    }
    grid.set_cell(
        seed_x,
        0,
        Cell {
            value: CellValue::new(ON),
            born_at: 0,
        },
    );
    grid
}

/// Прогоняет `ticks` тиков, возвращает битовую последовательность значений
/// клетки `CENTER` (0/1) на каждом тике.
fn run_and_sample(seed_x: usize, ticks: u32) -> Vec<u8> {
    let grid = build_seeded_grid(seed_x);
    let rules = build_rule30_index();
    let mut engine = Engine::new(grid, rules);

    let mut bits = Vec::with_capacity(ticks as usize);
    for _ in 0..ticks {
        engine.run_tick();
        let v = engine.grid().get_cell(CENTER, 0).map(|c| c.value.0 .0).unwrap_or(OFF);
        bits.push(if v == ON { 1u8 } else { 0u8 });
    }
    bits
}

fn main() {
    // ── Проверка 1: статистическая сбалансированность ───────────────────
    let bits = run_and_sample(CENTER, TICKS);
    let ones = bits.iter().filter(|&&b| b == 1).count();
    let ratio = ones as f64 / TICKS as f64;
    println!(
        "[1] {TICKS} тиков, центральная клетка: {ones} единиц ({:.1}%) — {}",
        ratio * 100.0,
        if (0.40..=0.60).contains(&ratio) {
            "в пределах ожидаемого разброса ✓"
        } else {
            "ПОДОЗРИТЕЛЬНО НЕСБАЛАНСИРОВАНО"
        }
    );
    assert!(
        (0.40..=0.60).contains(&ratio),
        "битовая последовательность слишком смещена для псевдослучайного источника"
    );

    // Нет короткого периода в разумном окне (наивная проверка: последнее
    // окно длиной WINDOW не совпадает ПОЛНОСТЬЮ ни с одним предыдущим окном
    // той же длины — не строгое доказательство апериодичности, но ловит
    // тривиальные короткие циклы).
    const WINDOW: usize = 50;
    let tail = &bits[bits.len() - WINDOW..];
    let mut short_cycle_found = false;
    for start in 0..(bits.len() - WINDOW) {
        if &bits[start..start + WINDOW] == tail {
            short_cycle_found = true;
            break;
        }
    }
    println!(
        "[1b] Короткий цикл (окно {WINDOW} бит) за {TICKS} тиков: {}",
        if short_cycle_found {
            "НАЙДЕН (плохо)"
        } else {
            "не найден ✓"
        }
    );
    assert!(
        !short_cycle_found,
        "последовательность зациклилась в разумном окне — не годится как источник хаоса"
    );

    // ── Проверка 2: детерминизм — один и тот же посев даёт побитово
    // идентичный результат в двух НЕЗАВИСИМЫХ прогонах ──────────────────
    let run_a = run_and_sample(CENTER, 300);
    let run_b = run_and_sample(CENTER, 300);
    assert_eq!(
        run_a, run_b,
        "один и тот же посев должен давать побитово идентичную последовательность"
    );
    println!("[2] Два независимых прогона с одним посевом: побитово идентичны ✓ (детерминизм, не настоящая энтропия — Аксиома 2)");

    // ── Проверка 3: разные посевы дают разные последовательности ────────
    let seed1 = run_and_sample(CENTER - 5, 300);
    let seed2 = run_and_sample(CENTER + 7, 300);
    assert_ne!(
        seed1, seed2,
        "разные посевы должны давать разные последовательности — иначе посев ничего не определяет"
    );
    println!("[3] Разные посевы -> разные последовательности ✓ (посев реально управляет исходом)");

    println!(
        "\nВывод: PRNG строится из ОБЫЧНЫХ правил Cellaria (pattern -> literal, Rule 30), без единой строчки \
арифметики. Детерминированно-хаотичный (не настоящая энтропия), воспроизводимо по посеву, статистически \
сбалансирован. Достаточно, чтобы 'стохастическая самомодификация' (мутация, выбранная по значению этой \
последовательности) не была голым словом — источник для неё есть, и он полностью укладывается в модель."
    );
}
