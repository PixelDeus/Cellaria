//! Block G, п.2: доказательство обратимости (`proof_reversibility.rs`) не
//! касается ВООБЩЕ границы решётки — там `overflow: Default::default()`
//! (Discard) везде, ни один сдвиг никогда не пересекает край. Открытый
//! вопрос: остаётся ли обратимость в силе, когда клетка физически ПОКИДАЕТ
//! решётку через `OverflowAction::Write`/граничный буфер направления
//! "output"?
//!
//! Ответ — ДА, но только если рассматривать ПОЛНОЕ состояние системы как
//! (решётка + содержимое граничных каналов), а не решётку в одиночку:
//!
//! 1. НАИВНАЯ попытка: обратить сдвиг (было "вправо", стало "влево") и
//!    прогнать реверс-правила ТОЛЬКО на финальной решётке, игнорируя то,
//!    что было выведено в output — токен физически исчез из решётки, взять
//!    его для восстановления неоткуда, реверс-правила не находят, что
//!    двигать назад, и решётка остаётся пустой навсегда. Обратимость
//!    ЛОМАЕТСЯ, если "снаружи" не учитывать.
//! 2. ПРАВИЛЬНАЯ попытка: то же самое значение, что вышло через output на
//!    прямом прогоне, ВВОДИТСЯ обратно через ТУ ЖЕ границу в момент,
//!    зеркальный моменту выхода (реверс output — это input в ту же точку) —
//!    и тогда токен возвращается в точности на исходную позицию.
//!
//! Значит существующая теорема об обратимости (paper2.md, `proof_reversibility.rs`)
//! неявно предполагала ЗАКРЫТУЮ систему (без пересечения границы) — она не
//! ошибочна, а просто не покрывала открытый случай явно. Здесь это явно
//! проверено в обе стороны: и что наивный подход ломается, и что закрытый-
//! по-петле подход работает, с точным побитовым совпадением.

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{BoundaryBuffer, Cell, CellType, CellValue, Direction, OverflowAction, Rule, ShiftSpec};
use cellaria::{Grid, VecStorage};

const WIDTH: usize = 10;
const TOKEN: u8 = 42;
const START_X: usize = 2;
const EXIT_X: usize = WIDTH - 1; // граница, где стоит output-буфер
const TOTAL_TICKS: u32 = 12;

fn shift_rule(direction: Direction, overflow: OverflowAction) -> HashMap<CellType, Vec<Rule>> {
    let mut idx = HashMap::new();
    idx.insert(
        CellType(TOKEN),
        vec![Rule {
            id: vec![CellType(TOKEN)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec::new(direction, 1)]],
            changes: vec![],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow,
            cam: None,
            tie_break: 0,
            starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None, cross_layer_reads: Vec::new(),
        }],
    );
    idx
}

fn build_grid() -> Grid<VecStorage> {
    let mut grid = Grid::new(VecStorage::new(WIDTH, 1), Default::default());
    grid.set_cell(START_X, 0, Cell { value: CellValue::new(TOKEN), born_at: 0 });
    let mut out = BoundaryBuffer::new();
    out.direction = "output".to_string();
    grid.set_boundary(EXIT_X, 0, out);
    grid
}

fn snapshot(engine: &Engine<VecStorage>) -> Vec<u8> {
    (0..WIDTH).map(|x| engine.grid().get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(0)).collect()
}

fn main() {
    let initial_snapshot: Vec<u8> = {
        let g = build_grid();
        (0..WIDTH).map(|x| g.get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(0)).collect()
    };

    // ── Прямой прогон: токен едет вправо, на границе Write(0) выводит СВОЁ
    // значение в output и физически покидает решётку ──────────────────────
    let mut forward = Engine::new(build_grid(), shift_rule(Direction::Right, OverflowAction::Write(0)));
    let mut exit_tick: Option<u32> = None;
    let mut exit_value: Option<u8> = None;
    for tick in 1..=TOTAL_TICKS {
        forward.run_tick();
        let out = forward.pop_output();
        if let Some(&(_, cell)) = out.first() {
            exit_tick = Some(tick);
            exit_value = Some(cell.value.0 .0);
        }
    }
    let final_snapshot = snapshot(&forward);
    let (exit_tick, exit_value) = (
        exit_tick.expect("токен обязан был выйти за отведённые тики"),
        exit_value.expect("если токен вышел, значение обязано быть захвачено вместе с тиком"),
    );
    println!("Исходная решётка:     {:?}", initial_snapshot);
    println!("После {TOTAL_TICKS} тиков вперёд: {:?} (токен вышел на тике {exit_tick} со значением {exit_value})", final_snapshot);
    assert!(final_snapshot.iter().all(|&v| v == 0), "токен должен был физически покинуть решётку — она обязана быть полностью пустой");

    // ── Наивный реверс: обращаем сдвиг, прогоняем на финальной (пустой)
    // решётке, ИГНОРИРУЯ то, что было выведено ─────────────────────────────
    let mut naive_reverse = Engine::new(build_reverse_grid(&final_snapshot), shift_rule(Direction::Left, OverflowAction::Discard));
    for _ in 1..=TOTAL_TICKS {
        naive_reverse.run_tick();
    }
    let naive_result = snapshot(&naive_reverse);
    println!("\n[Наивный реверс, без учёта output] Восстановлено: {:?}", naive_result);
    println!(
        "Совпадает с исходной: {} — ожидаемо НЕТ: токен вышел через границу, реверс-правилам нечего двигать назад",
        if naive_result == initial_snapshot { "ДА" } else { "НЕТ" }
    );
    assert_ne!(naive_result, initial_snapshot, "наивный реверс, игнорирующий output, НЕ ДОЛЖЕН восстановить исходное состояние — иначе демонстрация ничего не показывает");

    // ── Правильный реверс: то же самое значение, что вышло через output на
    // прямом прогоне, вводится обратно через ТУ ЖЕ границу в момент,
    // зеркальный моменту выхода (реверс-тик = TOTAL_TICKS - exit_tick + 1) ─
    let reinject_at = TOTAL_TICKS - exit_tick + 1;
    let mut closed_reverse = Engine::new(build_reverse_grid(&final_snapshot), shift_rule(Direction::Left, OverflowAction::Discard));
    for tick in 1..=TOTAL_TICKS {
        closed_reverse.run_tick();
        if tick == reinject_at {
            // Инжектируем ПОСЛЕ run_tick этого тика — зеркальный тик выхода
            // должен ЗАКОНЧИТЬСЯ появлением токена на границе (симметрично
            // прямому тику 8, который НАЧАЛСЯ с токена на границе и
            // ЗАКОНЧИЛСЯ его выходом), а не двигать его в ТОМ ЖЕ тике, где
            // он появился — иначе реверс-правило сдвига успело бы
            // среагировать немедленно и увести токен на одну клетку дальше.
            let gen = closed_reverse.grid().generation();
            closed_reverse.grid_mut().set_cell(EXIT_X, 0, Cell { value: CellValue::new(exit_value), born_at: gen });
        }
    }
    let closed_result = snapshot(&closed_reverse);
    println!("\n[Закрытый реверс: output прямого прогона -> input обратного, на тике {reinject_at}] Восстановлено: {:?}", closed_result);
    println!(
        "Совпадает с исходной побитово: {}",
        if closed_result == initial_snapshot { "ДА" } else { "НЕТ (!)" }
    );
    assert_eq!(closed_result, initial_snapshot, "реверс, вернувший output прямого прогона как input в ту же точку, ОБЯЗАН побитово восстановить исходное состояние");

    println!(
        "\nВывод: обратимость (paper2.md, proof_reversibility.rs) держится не для решётки в одиночку, а для \
ПОЛНОЙ системы (решётка + граничные каналы) — прежнее доказательство было неявно про закрытую систему (нет \
пересечения границы) и честно не покрывало открытый случай, а не содержало ошибку. Открытый случай (данные \
покидают решётку) обратим ТОЛЬКО если то, что вышло через output, возвращается как input в ту же точку в \
зеркальный момент — иначе информация необратимо теряется, что и показал наивный реверс. Оба сценария проверены \
побитовым сравнением решётки целиком, не только там, где что-то менялось."
    );
}

/// Восстанавливает решётку из снапшота (для запуска реверс-прогона с той же
/// геометрией и output-буфером на прежнем месте).
fn build_reverse_grid(snapshot: &[u8]) -> Grid<VecStorage> {
    let mut grid = Grid::new(VecStorage::new(WIDTH, 1), Default::default());
    for (x, &v) in snapshot.iter().enumerate() {
        if v != 0 {
            grid.set_cell(x, 0, Cell { value: CellValue::new(v), born_at: 0 });
        }
    }
    let mut out = BoundaryBuffer::new();
    out.direction = "output".to_string();
    grid.set_boundary(EXIT_X, 0, out);
    grid
}
