//! Обратимость + КАСКАДНЫЙ `ShiftSpec.keep_source`: открытый вопрос, явно
//! оставленный непроверенным в конце `proof_reversibility_keep_source.rs`.
//!
//! Там доказано (конструктивно), что ОДНА эмиссия (`keep_source: true`,
//! `broadcast: false`) обратима — не рецептом Теоремы 9 ("развернуть
//! направление"), а "правилом-ластиком" (pattern читает пару
//! источник+копия, changes очищает только копию). Открытый вопрос: копия —
//! это клетка ТОГО ЖЕ `CellType`, что и источник, значит она сама
//! НЕЗАВИСИМО совпадает с ТЕМ ЖЕ правилом на следующем тике и может
//! эмитировать ДАЛЬШЕ — каскад, а не одна пара.
//!
//! ПЕРВАЯ находка при проверке (см. `dbg_cascade_repro.rs`, оставлен рядом
//! для воспроизводимости): "наивный" вариант правила (`id: [SRC]`, без
//! доп. паттерна) — то есть ЛЮБАЯ клетка типа SRC матчится, включая
//! исходный источник И его копию одновременно — НЕ каскадирует вообще, а
//! ЗАСТРЕВАЕТ на [source, copy] навсегда. Причина — НЕ конфликт по
//! `write_cells` сам по себе (это ожидаемо и разрешается арбитражем), а
//! КОНКРЕТНЫЙ порядок тай-брейка: `arbitrator::arbitrate_with_cam` сортирует
//! матчи по `(priority, age, tie_break, rule_id, x, y, rule_idx)` — `age`
//! ВТОРОЕ поле, раньше координат. Копия (STEPS от источника) каждый тик
//! получает СВЕЖИЙ `born_at` (apply всегда пишет `born_at: gen`, даже если
//! записываемое значение совпадает со старым — см. `apply_matches`), то
//! есть её age КАЖДЫЙ тик равен нулю, тогда как age самого источника растёт
//! неограниченно. Раз age доминирует над x/y в сравнении, старый источник
//! ВСЕГДА выигрывает арбитраж у своей же более молодой копии — копия
//! никогда не успевает первой применить СВОЙ сдвиг дальше, потому что её
//! конфликтующий с source сосед каждый раз просто повторно (избыточно)
//! перезаписывает её же позицию тем же значением, обнуляя её age снова.
//! Самоподдерживающийся тупик, не зависящий от длины цепочки (уже
//! воспроизводится на 2 клетках).
//!
//! ВТОРАЯ находка — это ИСПРАВИМО, а не фундаментальный тупик. Достаточно
//! добавить в `pattern` ОДНО дополнительное условие — "клетка
//! НЕПОСРЕДСТВЕННО впереди меня (моя собственная цель сдвига) сейчас
//! пуста" — и внутренние звенья цепочки перестают матчиться ВООБЩЕ (их цель
//! уже занята копией), так что на каждый тик существует РОВНО один матч
//! (самый передний), никакого конфликта/тай-брейка не возникает в
//! принципе. Ниже это проверяется конструктивно, затем — обратимость
//! получившегося (теперь по-настоящему растущего) каскада.

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Direction, Rule, ShiftSpec};
use cellaria::{Grid, VecStorage};

const WIDTH: usize = 40;
const SRC: u8 = 50;
const DECOY: u8 = 77;
const SOURCE_X: usize = 20;
const STEPS: i8 = 1; // точечная эмиссия, шаг 1 — копия сразу смежна с источником
const TICKS: u32 = 5; // сколько раз даём каскаду расти вперёд
const DECOY_X: usize = 14; // левее SOURCE_X — вне пути каскада и его реверса

fn reverse_direction(d: Direction) -> Direction {
    match d {
        Direction::Up => Direction::Down,
        Direction::Down => Direction::Up,
        Direction::Left => Direction::Right,
        Direction::Right => Direction::Left,
    }
}

/// Правило-"излучатель" с ФРОНТ-ГЕЙТОМ: pattern требует (0,0)=SRC (я сам)
/// И (STEPS,0)=default (моя собственная цель сдвига сейчас пуста). Второе
/// условие — это и есть исправление находки 1: внутреннее звено цепочки,
/// чья цель уже занята следующей копией, этому условию не удовлетворяет и
/// просто не матчится вообще, так что каждый тик существует РОВНО один
/// матч (самый передний), без конфликтов и без завязки на `age`.
fn emit_rule_front_gated(direction: Direction) -> HashMap<CellType, Vec<Rule>> {
    let mut idx = HashMap::new();
    idx.insert(
        CellType(SRC),
        vec![Rule {
            id: vec![],
            pattern: vec![(0, 0, CellType(SRC)), (STEPS, 0, CellType(0))],
            shifts: vec![vec![ShiftSpec { direction, steps: STEPS as u16, broadcast: false, keep_source: true }]],
            changes: vec![],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None, max_activations: None, cross_layer_reads: Vec::new(),
        }],
    );
    idx
}

/// "Ластик" из старого файла — БЕЗ распознавания фронта: pattern читает
/// ЛЮБУЮ смежную пару (источник на (0,0), копия на (STEPS,0)), changes
/// очищает только копию. Совпадёт со ВСЕМИ смежными парами цепочки
/// одновременно, если их дать все сразу.
fn plain_eraser_rule() -> HashMap<CellType, Vec<Rule>> {
    let mut idx = HashMap::new();
    idx.insert(
        CellType(SRC),
        vec![Rule {
            id: vec![CellType(SRC)],
            pattern: vec![(0, 0, CellType(SRC)), (STEPS, 0, CellType(SRC))],
            shifts: vec![],
            changes: vec![(STEPS as i32, 0, ChangeValue::Literal(0))],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None, max_activations: None, cross_layer_reads: Vec::new(),
        }],
    );
    idx
}

fn build_grid(src_x: usize, decoy_x: Option<usize>) -> Grid<VecStorage> {
    let mut grid = Grid::new(VecStorage::new(WIDTH, 1), Default::default());
    grid.set_cell(src_x, 0, Cell { value: CellValue(CellType(SRC)), born_at: 0 });
    if let Some(dx) = decoy_x {
        grid.set_cell(dx, 0, Cell { value: CellValue(CellType(DECOY)), born_at: 0 });
    }
    grid
}

fn grid_from_snapshot(snap: &[u8]) -> Grid<VecStorage> {
    let mut g = Grid::new(VecStorage::new(WIDTH, 1), Default::default());
    for (x, &v) in snap.iter().enumerate() {
        if v != 0 {
            g.set_cell(x, 0, Cell { value: CellValue(CellType(v)), born_at: 0 });
        }
    }
    g
}

fn snapshot(engine: &Engine<VecStorage>) -> Vec<u8> {
    (0..WIDTH).map(|x| engine.grid().get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(0)).collect()
}

fn occupied_src(snap: &[u8]) -> Vec<usize> {
    snap.iter().enumerate().filter(|&(_, &v)| v == SRC).map(|(x, _)| x).collect()
}

fn main() {
    let initial_snapshot: Vec<u8> = {
        let g = build_grid(SOURCE_X, Some(DECOY_X));
        (0..WIDTH).map(|x| g.get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(0)).collect()
    };
    println!("Исходная решётка: {:?}", initial_snapshot);
    println!(
        "Источник на x={SOURCE_X}, декой (посторонняя неподвижная клетка, без правил) на x={DECOY_X}.\n\
Правило (фронт-гейт): точечная эмиссия `keep_source=true`, `steps={STEPS}`, направление Right, {TICKS} тиков вперёд."
    );

    // ── Прямой прогон: каскад РАСТЁТ ровно на 1 клетку за тик ───────────────
    let mut forward = Engine::new(build_grid(SOURCE_X, Some(DECOY_X)), emit_rule_front_gated(Direction::Right));
    let mut wave_history: Vec<Vec<usize>> = vec![occupied_src(&initial_snapshot)];
    for t in 1..=TICKS {
        forward.run_tick();
        let occ = occupied_src(&snapshot(&forward));
        println!("После тика {t}: SRC-клетки = {:?}", occ);
        wave_history.push(occ);
    }
    let final_snapshot = snapshot(&forward);
    let final_occ = occupied_src(&final_snapshot);
    println!("\nИтоговая решётка после {TICKS} тиков: {:?}", final_snapshot);
    assert_eq!(final_snapshot[DECOY_X], DECOY, "декой обязан пережить прямой прогон — путь каскада вправо его не задевает");

    // Каскад обязан вырасти РОВНО до TICKS+1 клеток (одна новая клетка за
    // тик, ни больше, ни меньше — фронт-гейт гарантирует ровно 1 матч/тик).
    let expected_occ: Vec<usize> = (SOURCE_X..=SOURCE_X + TICKS as usize).collect();
    assert_eq!(final_occ, expected_occ, "фронт-гейт обязан давать ровно 1 новую клетку за тик, без пропусков и без застревания");
    println!(
        "\nКаскад РЕАЛЬНО растёт: за {TICKS} тиков популяция SRC выросла с 1 до {} клеток: {:?} — сплошная линия, \
ровно по одной новой клетке за тик. Это НЕ то же самое, что \"наивный\" вариант правила (`id: [SRC]`, без \
доп. условия на pattern): тот НЕ растёт вообще и намертво застревает на [source, copy] — см. \
`dbg_cascade_repro.rs`. Причина застревания наивного варианта — не конфликт `write_cells` сам по себе (это \
ожидаемо), а то, что тай-брейк арбитража сортирует матчи по `(priority, age, ...)`, и копия каждый тик \
получает СВЕЖИЙ born_at (apply всегда пишет born_at: gen, даже при повторной записи того же значения — см. \
`apply_matches`), так что её age вечно равен нулю и она НИКОГДА не выигрывает у своего куда более старого \
источника: тот каждый тик избыточно, но ПОБЕДНО перезаписывает её же позицию тем же значением, обнуляя её \
age заново — самоподдерживающийся тупик. Фикс — добавить в pattern условие \"моя цель сдвига сейчас пуста\": \
внутренние звенья (чья цель уже занята следующей копией) перестают матчиться вообще, так что на тик \
существует РОВНО один матч (самый передний) — тай-брейк по age становится не при делах, потому что \
конкурирующего матча просто нет.",
        final_occ.len(),
        final_occ
    );

    // ── A) Наивный R⁻¹ (Теорема 9): тот же набор правил, направление
    // развёрнуто, TICKS тиков — ожидаемо НЕ восстанавливает (растёт в другую
    // сторону, а не схлопывается) ──────────────────────────────────────────
    let mut naive_reverse = Engine::new(grid_from_snapshot(&final_snapshot), emit_rule_front_gated(reverse_direction(Direction::Right)));
    for _ in 1..=TICKS {
        naive_reverse.run_tick();
    }
    let naive_snapshot = snapshot(&naive_reverse);
    assert_ne!(naive_snapshot, initial_snapshot, "наивный реверс не должен был случайно восстановить решётку — иначе конструкцию нужно пересмотреть");
    println!(
        "\n[A: наивный реверс] После {TICKS} тиков: {:?} — НЕ совпадает с исходной, как и ожидалось (тот же \
фронт-гейт правил, но развёрнутое направление, растит НОВУЮ цепочку влево от левого конца старой, а не \
схлопывает старую).",
        naive_snapshot
    );

    // ── C) Плоский ластик (без фронт-детектора), ОДИН тик на всю цепочку
    // сразу — тот же рецепт, что уже работал для одной эмиссии ─────────────
    let mut plain_reverse = Engine::new(grid_from_snapshot(&final_snapshot), plain_eraser_rule());
    plain_reverse.run_tick();
    let plain_snapshot = snapshot(&plain_reverse);
    println!("\n[C: плоский ластик, 1 тик] После 1 тика: {:?}", plain_snapshot);
    println!(
        "[C] Восстановленная решётка совпадает с исходной клетка в клетку: {}",
        if plain_snapshot == initial_snapshot { "ДА" } else { "НЕТ" }
    );
    println!("[C] Декой на x={DECOY_X} пережил: {}", if plain_snapshot[DECOY_X] == DECOY { "ДА" } else { "НЕТ" });
    assert_eq!(
        plain_snapshot, initial_snapshot,
        "плоский ластик (без фронт-детектора), применённый ОДИН раз, обязан восстановить решётку целиком \
-- если нет, находка неверна"
    );
    println!(
        "\n[C] Ластик без фронт-детектора и без разбиения по уровням СХЛОПЫВАЕТ цепочку ЛЮБОЙ длины в исходную \
точку за ОДИН тик: pattern читает ЛЮБУЮ смежную пару источник+копия, changes очищает только копию — все \
{} внутренних пар цепочки матчатся ОДНОВРЕМЕННО и стирают РАЗНЫЕ клетки (у ластика нет сдвига, поэтому в его \
write_cells НЕ подмешивается консервативная (0,0) — см. `compute_write_cells`: ветка `shift_targets.is_empty()` \
включает только `changes`-клетки), так что конфликтов между парами вообще нет.",
        final_occ.len().saturating_sub(1)
    );

    println!(
        "\nИтоговый вывод: каскадный `keep_source` ОБРАТИМ, но у прямого направления был реальный, не \
надуманный тупик (наивное правило застревает навсегда из-за age-тай-брейка) — тупик оказался ИСПРАВИМЫМ \
(фронт-гейт в pattern), а не фундаментальным свойством `keep_source`. После починки прямого прогона обратный \
рецепт из `proof_reversibility_keep_source.rs` (плоский ластик, без учёта фронта/уровней) работает \
БУКВАЛЬНО без изменений — вся цепочка любой длины схлопывается за один тик, потому что у ластика (в отличие \
от самого излучателя) нет сдвига, а значит нет и консервативной клетки в write_cells, из-за которой соседние \
пары могли бы конфликтовать."
    );
}
