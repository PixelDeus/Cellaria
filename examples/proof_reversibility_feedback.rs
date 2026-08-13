//! Обратимость + `Rule.feedback`: держится ли Теорема 9 (paper2.md §7.3),
//! если в наборе правил участвует правило с `feedback: Some(FeedbackSpec)`?
//!
//! Формальный повод для сомнения: Теорема 9 доказывает, что тик — это
//! БИЕКЦИЯ `config_t → config_{t+1}`, то есть функция ТОЛЬКО от решётки.
//! `Rule.feedback` ломает это на более базовом уровне, чем "инъективность
//! локальной карты" (Definition 6) — у правила с `feedback` эффект НА ЭТОМ
//! ТИКЕ зависит не только от `config_t`, а ЕЩЁ и от `Engine::feedback_counters`
//! — скрытого состояния, накопленного за ВСЮ предыдущую историю прогона и
//! НЕ являющегося частью решётки (приватное поле `Engine`, недоступное
//! снаружи, см. doc-комментарий `Engine::feedback_counters`). Формально:
//! тик уже не `config_t → config_{t+1}`, а `(config_t, counters_t) →
//! (config_{t+1}, counters_{t+1})` — то же самое ослабление, что Theorem 10
//! (paper4.md §12) сделало явным для граничных буферов `B`, но `feedback_counters`
//! — это НЕ `B`: `B` — конечная явная очередь, которую можно ЗАФИКСИРОВАТЬ и
//! ВОСПРОИЗВЕСТИ (см. `proof_reversibility_boundary_io.rs`). `feedback_counters`
//! — ЗАЩЁЛКА (`saturating_add`, НИКОГДА не сбрасывается, см.
//! `Engine::feedback_counters`'s doc-комментарий) — считает только ВВЕРХ.
//! Чтобы правильно "отмотать" переключение направления назад, обратному
//! прогону нужен был бы счётчик, считающий ВНИЗ от значения, которое имел
//! прямой прогон на финальном тике — но (а) этого значения не существует
//! ни в решётке, ни где-либо ещё в публичном API `Engine`, и (б) даже если
//! бы существовало, механизм защёлки СТРУКТУРНО не умеет считать вниз.
//!
//! Этот прогон строит наивный `R⁻¹` (симметричный трюк из
//! `proof_reversibility.rs`: направления развёрнуты, `timeout` тот же) и
//! показывает, что происходит на самом деле, а не что "должно быть по
//! аналогии" — с двумя независимыми проверками:
//!
//! 1. Позиция маркера-токена: где он оказывается после обратного прогона —
//!    в исходной точке или нет.
//! 2. "Приманка" (декой) — неподвижная клетка постороннего типа, не
//!    участвующая ни в одном правиле, помещённая ЗАВЕДОМО ВНЕ пути прямого
//!    прогона. Если обратный прогон (из-за неверного порядка "сначала
//!    новое направление, потом старое" вместо правильного "сначала
//!    отменить последнее, потом первое") физически пересекает клетку
//!    декоя, она будет молча уничтожена обычной перезаписью при сдвиге
//!    (`shift` не проверяет, что было в целевой клетке раньше) — а значит
//!    восстановленная решётка НЕ совпадёт с исходной побитово, даже если
//!    сам токен каким-то образом вернётся в правильный x (что, как
//!    показано ниже, действительно происходит — чистое перемещение без
//!    декоя суммируется коммутативно независимо от порядка направлений, но
//!    ПУТЬ при этом другой, и именно путь решает судьбу декоя).

use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{Cell, CellType, CellValue, Direction, FeedbackSpec, Rule, ShiftSpec};
use cellaria::{Grid, VecStorage};

const WIDTH: usize = 40;
const TOKEN: u8 = 50;
const DECOY: u8 = 77;
const START_X: usize = 20;
const DECOY_X: usize = 12; // заведомо вне пути прямого прогона (см. ниже)
const TIMEOUT: u64 = 5;
const TICKS: u32 = 15;

fn reverse_direction(d: Direction) -> Direction {
    match d {
        Direction::Up => Direction::Down,
        Direction::Down => Direction::Up,
        Direction::Left => Direction::Right,
        Direction::Right => Direction::Left,
    }
}

fn feedback_rules(normal: Direction, new_direction: Direction) -> HashMap<CellType, Vec<Rule>> {
    let mut idx = HashMap::new();
    idx.insert(
        CellType(TOKEN),
        vec![Rule {
            id: vec![CellType(TOKEN)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec::new(normal, 1)]],
            changes: vec![],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: Some(FeedbackSpec {
                timeout: TIMEOUT,
                new_direction,
            }),
            recursion: None,
            memory: None,
            max_activations: None,
            cross_layer_reads: Vec::new(),
        }],
    );
    idx
}

fn build_grid(token_x: usize, decoy_x: Option<usize>) -> Grid<VecStorage> {
    let mut grid = Grid::new(VecStorage::new(WIDTH, 1), Default::default());
    grid.set_cell(
        token_x,
        0,
        Cell {
            value: CellValue(CellType(TOKEN)),
            born_at: 0,
        },
    );
    if let Some(dx) = decoy_x {
        grid.set_cell(
            dx,
            0,
            Cell {
                value: CellValue(CellType(DECOY)),
                born_at: 0,
            },
        );
    }
    grid
}

fn grid_from_snapshot(snap: &[u8]) -> Grid<VecStorage> {
    let mut g = Grid::new(VecStorage::new(WIDTH, 1), Default::default());
    for (x, &v) in snap.iter().enumerate() {
        if v != 0 {
            g.set_cell(
                x,
                0,
                Cell {
                    value: CellValue(CellType(v)),
                    born_at: 0,
                },
            );
        }
    }
    g
}

fn snapshot(engine: &Engine<VecStorage>) -> Vec<u8> {
    (0..WIDTH)
        .map(|x| engine.grid().get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(0))
        .collect()
}

fn token_x_of(snap: &[u8]) -> Option<usize> {
    snap.iter().position(|&v| v == TOKEN)
}

fn main() {
    let initial_snapshot: Vec<u8> = {
        let g = build_grid(START_X, Some(DECOY_X));
        (0..WIDTH)
            .map(|x| g.get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(0))
            .collect()
    };
    println!("Исходная решётка:  {:?}", initial_snapshot);
    println!(
        "Токен на x={START_X}, декой (посторонняя неподвижная клетка, без правил) на x={DECOY_X}.\n\
Правило: `feedback` с timeout={TIMEOUT}: {TICKS} тиков вперёд = {TIMEOUT} тиков нормальным \
направлением (Right) + {} тиков направлением-после-переключения (Left, ЗАЩЁЛКА — навсегда).",
        TICKS - TIMEOUT as u32
    );

    // ── Прямой прогон — записываем позицию токена ПОСЛЕ каждого тика (это
    // уже наблюдаемо через публичный API, не нужен доступ к приватному
    // `feedback_counters`) — из последовательности позиций ниже выводится,
    // каким направлением реально шёл КАЖДЫЙ тик, без подглядывания в
    // внутренний счётчик ─────────────────────────────────────────────────
    let mut forward = Engine::new(
        build_grid(START_X, Some(DECOY_X)),
        feedback_rules(Direction::Right, Direction::Left),
    );
    let mut positions: Vec<usize> = vec![START_X];
    for _ in 1..=TICKS {
        forward.run_tick();
        positions.push(token_x_of(&snapshot(&forward)).expect("токен обязан остаться на решётке"));
    }
    let final_snapshot = snapshot(&forward);
    let final_token_x = token_x_of(&final_snapshot).expect("токен обязан остаться на решётке");
    println!("\nПосле {TICKS} тиков вперёд: {:?}", final_snapshot);
    println!(
        "Токен: x={START_X} -> x={final_token_x} (путь прямого прогона: x∈[15,25], декой на x={DECOY_X} НЕ затронут — вне этого диапазона)."
    );
    assert_eq!(
        final_snapshot[DECOY_X], DECOY,
        "декой обязан пережить прямой прогон нетронутым — прямой путь токена никогда не заходит на x=12"
    );

    // ── Наивный R⁻¹: направления развёрнуты (тот же приём, что и в
    // proof_reversibility.rs), timeout тот же, свежий Engine (счётчик
    // feedback стартует с нуля — другого пути нет, поле приватное) ────────
    let mut reverse = Engine::new(
        grid_from_snapshot(&final_snapshot),
        feedback_rules(reverse_direction(Direction::Right), reverse_direction(Direction::Left)),
    );
    for _ in 1..=TICKS {
        reverse.run_tick();
    }
    let recovered_snapshot = snapshot(&reverse);
    let recovered_token_x = token_x_of(&recovered_snapshot);
    println!("\nПосле {TICKS} тиков наивного реверса: {:?}", recovered_snapshot);
    println!(
        "Токен вернулся на исходный x={START_X}: {} (позиция токена — это ЧИСТОЕ 1D-перемещение, сумма шагов \
коммутативна независимо от порядка направлений, поэтому чистая позиция может случайно совпасть)",
        match recovered_token_x {
            Some(x) if x == START_X => "ДА".to_string(),
            Some(x) => format!("НЕТ (x={x})"),
            None => "НЕТ (токен потерян)".to_string(),
        }
    );
    println!(
        "Декой на x={DECOY_X} пережил обратный прогон: {} — {}",
        if recovered_snapshot[DECOY_X] == DECOY {
            "ДА"
        } else {
            "НЕТ"
        },
        if recovered_snapshot[DECOY_X] == DECOY {
            "неожиданно — обратный путь не должен был пересечь x=12 в этом прогоне"
        } else {
            "как и предсказано: наивный реверс (защёлка считает только вверх, свежая с нуля) идёт \
СНАЧАЛА обращённым НОВЫМ направлением, ПОТОМ обращённым обычным — в порядке, ОБРАТНОМ правильному \
(сначала отменить ПОСЛЕДНЕЕ, что случилось на прямом прогоне), из-за чего путь обратного прогона \
физически проходит через x=12 и молча стирает декой обычной перезаписью при сдвиге"
        }
    );
    println!(
        "\nВосстановленная решётка совпадает с исходной клетка в клетку: {}",
        if recovered_snapshot == initial_snapshot {
            "ДА"
        } else {
            "НЕТ"
        }
    );
    assert_ne!(
        recovered_snapshot, initial_snapshot,
        "если побитовое совпадение ВСЁ-ТАКИ произошло, вывод примера неверен — нужно пересмотреть построение декоя"
    );

    println!(
        "\nВывод (наивный R⁻¹, тот же feedback-рецепт назад): не работает — счётчик не умеет считать вниз, \
а без него сегменты применяются в неверном порядке. Это ограничение КОНКРЕТНОГО рецепта \
(\"прогнать ТОТ ЖЕ feedback-набор с развёрнутыми направлениями\"), а не доказательство, что обратимость \
недостижима в принципе — ниже другой рецепт, не использующий `feedback` в обратном прогоне вовсе."
    );

    // ── А если не наивно? Рецепт без feedback в обратном проходе ───────────
    //
    // Ключевое наблюдение: единственный эффект `feedback` на РЕШЁТКУ — это
    // КАКОЕ направление сдвига применяется на КАЖДОМ конкретном тике. Эта
    // последовательность направлений УЖЕ полностью наблюдаема через
    // публичный API — она восстанавливается из записанных выше `positions`
    // (чистое 1D-перемещение: dx>0 -> Right, dx<0 -> Left), без единого
    // обращения к приватному `feedback_counters`. R⁻¹ строится не как
    // "тот же feedback-набор назад", а как ЗАПИСАННАЯ последовательность
    // ОБЫЧНЫХ (без feedback) одно-направленных сдвиговых правил, по одному
    // тику на шаг, в ОБРАТНОМ порядке записи — счётчик обхода не нужен
    // вообще, потому что мы не пересчитываем его, а ЧИТАЕМ уже случившуюся
    // историю и проигрываем её назад. Тот же принцип, что уже использует
    // Theorem 10 для граничных буферов (`proof_reversibility_boundary_io.rs`):
    // обратимость скрытого состояния требует ЗАПИСИ его истории, а не
    // попытки пересчитать его "в уме" задним числом.
    fn single_direction_rule(direction: Direction) -> HashMap<CellType, Vec<Rule>> {
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
                overflow: Default::default(),
                cam: None,
                tie_break: 0,
                starvation_after: None,
                feedback: None,
                recursion: None,
                memory: None,
                max_activations: None,
                cross_layer_reads: Vec::new(),
            }],
        );
        idx
    }

    let mut smart_snapshot = final_snapshot.clone();
    for i in (1..=TICKS as usize).rev() {
        let forward_dx = positions[i] as i32 - positions[i - 1] as i32;
        let forward_direction = if forward_dx > 0 {
            Direction::Right
        } else {
            Direction::Left
        };
        let mut step_engine = Engine::new(
            grid_from_snapshot(&smart_snapshot),
            single_direction_rule(reverse_direction(forward_direction)),
        );
        step_engine.run_tick();
        smart_snapshot = snapshot(&step_engine);
    }
    println!(
        "\nПосле {TICKS} тиков УМНОГО реверса (записанная история направлений, без feedback в обратном проходе): {:?}",
        smart_snapshot
    );
    println!(
        "Восстановленная решётка совпадает с исходной клетка в клетку: {}",
        if smart_snapshot == initial_snapshot {
            "ДА"
        } else {
            "НЕТ"
        }
    );
    assert_eq!(
        smart_snapshot, initial_snapshot,
        "записанная-история-рецепт ОБЯЗАН точно восстановить исходную решётку, включая декой -- если нет, конструкция неверна"
    );

    println!(
        "\nИтоговый вывод: `Rule.feedback` несовместим конкретно с рецептом \"тот же feedback-набор назад\" \
(Теорема 9 в буквальном виде) — но это ограничение РЕЦЕПТА, не факт, что прогон с `feedback` необратим в \
принципе. Обратимость ДЕРЖИТСЯ при другом рецепте: не пересчитывать скрытый счётчик, а ЗАПИСАТЬ \
фактически применённую последовательность направлений (уже наблюдаемую через публичный API) и \
проиграть её в обратном порядке ОБЫЧНЫМИ (без feedback) правилами — проверено конструктивно выше, включая \
декой. Это тот же общий приём, что paper4.md §12 (Theorem 10) уже использует для граничных буферов: \
скрытое состояние не мешает обратимости САМО ПО СЕБЕ, если его историю ЗАПИСАТЬ и ЗАМЕНИТЬ обратный \
пересчёт воспроизведением записи. Открытый вопрос (не проверен здесь): в этом примере история была видна \
через ПОЗИЦИЮ токена (feedback тут двигает, значит эффект наблюдаем как перемещение); для конфигурации, \
где feedback переключает НЕ единственный видимый сдвиг, а что-то менее прямо наблюдаемое, потребовался бы \
явный публичный геттер `feedback_counters`/лог переключений, которого сегодня нет — этот пример НЕ \
доказывает, что таковой не нужен в общем случае, только что для движения по одной оси хватает уже \
существующего публичного API."
    );
}
