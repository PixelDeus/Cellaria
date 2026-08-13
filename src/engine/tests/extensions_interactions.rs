use super::super::*;
use super::common::*;
use crate::types::{
    CamSearch, Cell, CellType, CellValue, ChangeValue, Direction, FeedbackSpec, MemorySpec, RecordTrigger,
    RecordedValue, RecursionSpec, Rule, ShiftSpec,
};
use crate::VecStorage;
use std::collections::VecDeque;

// Тесты в этом файле намеренно НЕ разделены строго "по одному расширению
// на файл" -- feedback/recursion/memory здесь активно комбинируются друг с
// другом (см. например test_cam_magnet_respects_memory_gate,
// test_memory_neighbor_type_plus_recursion_cascade_level_gate_primes_across_ticks) --
// искусственное разнесение по "основному" расширению было бы менее
// честным отражением того, что реально проверяется, чем один связный файл
// про межтиковые расширения и их взаимодействия.
const MAGNET: u8 = 40;
const TARGET: u8 = 41;
const TIMER: u8 = 60;
const FIRED: u8 = 61;

/// Block G, п.3 ("грубость повторной проверки min_age"): регрессионный тест
/// на РЕАЛЬНЫЙ механизм, который эту "грубость" устраняет —
/// `min_age_gated_types` в `SearchRadiusCache` заставляет
/// `resolve_search_coords_advance` каждый тик безусловно досканировать ВСЕ
/// активные клетки типов, у которых есть хоть одно правило с `min_age > 0`,
/// независимо от dirty-состояния (см. `build_candidates` в `engine/mod.rs`).
///
/// Клетка-таймер стоит ОДНА на большой (50×1) решётке, далеко от края и без
/// единой другой активной клетки рядом — обычное dirty-расширение (радиус
/// вокруг недавних изменений) в принципе не может её найти, ей неоткуда
/// взяться в кандидатах, КРОМЕ безусловного пересканирования по типу.
/// Если бы этот механизм был сломан или удалён, клетка осталась бы TIMER
/// НАВСЕГДА (её тип никогда не помечается dirty, никто рядом не меняется) —
/// тест ловит именно такую регрессию, а не просто "min_age вообще работает"
/// (для этого хватило бы уже существующего теста на 1×1 решётке).
#[test]
fn test_min_age_gated_cell_matures_exactly_on_time_when_isolated_on_sparse_grid() {
    const THRESHOLD: u64 = 7;
    const ISOLATED_X: usize = 40;

    let mut grid = make_grid(50, 1);
    grid.set_cell(ISOLATED_X, 0, Cell::new(TIMER));
    let rule = Rule {
        id: vec![CellType(TIMER)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(FIRED))],
        active_only: false,
        priority: 0,
        min_age: THRESHOLD,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    for tick in 0..THRESHOLD {
        engine.run_tick();
        assert_eq!(
            engine.grid().get_cell(ISOLATED_X, 0).map(|c| c.value.0 .0),
            Some(TIMER),
            "изолированная клетка должна оставаться TIMER до порога (тик {tick}) — если это не так, вероятно \
             сработал ложный full-scan или клетка вообще не переоценивалась и осталась бы TIMER навсегда"
        );
    }
    engine.run_tick(); // tick == THRESHOLD
    assert_eq!(
        engine.grid().get_cell(ISOLATED_X, 0).map(|c| c.value.0 .0),
        Some(FIRED),
        "изолированная клетка обязана дозреть РОВНО на пороговом тике, несмотря на отсутствие какой-либо \
         соседней активности, которая могла бы её случайно пометить dirty"
    );
}

// ──────────────────────────────────────────────────────────────
// `Rule::feedback` (block-обсуждение "обратная связь", п.1) — маркер едет
// East, после `timeout` тиков подряд (независимо от исхода арбитража —
// считаются попытки) переключается на `new_direction` НАВСЕГДА (защёлка,
// не сбрасывается — см. её doc-комментарий).
// ──────────────────────────────────────────────────────────────

const MARKER: u8 = 70;

#[test]
fn test_feedback_latches_new_direction_after_timeout_and_stays() {
    const TIMEOUT: u64 = 3;
    let mut grid = make_grid(10, 10);
    grid.set_cell(2, 2, Cell::new(MARKER));

    let rule = Rule {
        id: vec![CellType(MARKER)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
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
            new_direction: Direction::Up,
        }),
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    fn find_marker(engine: &Engine<VecStorage>) -> (usize, usize) {
        for y in 0..10 {
            for x in 0..10 {
                if engine.grid().get_cell(x, y).map(|c| c.value.0 .0) == Some(MARKER) {
                    return (x, y);
                }
            }
        }
        panic!("маркер не найден на решётке");
    }

    // Счётчик читается КАК ОН БЫЛ на начало тика (та же дисциплина, что и
    // у `age`/`min_age`/`starvation_after` — обнаружение ЭТИМ тиком не
    // засчитывается для решения ЭТОГО же тика, инкремент виден только со
    // СЛЕДУЮЩЕГО). При TIMEOUT=3: тики 1,2,3 читают counter=0,1,2
    // (все < 3) -> Right; счётчик становится 3 только ПОСЛЕ apply тика 3.
    // Тик 4 первым читает counter=3 (>= 3) -> переключение на Up.

    // Счётчик читается КАК ОН БЫЛ на начало тика (та же дисциплина, что и
    // у `age`/`min_age`/`starvation_after` — обнаружение ЭТИМ тиком не
    // засчитывается для решения ЭТОГО же тика, инкремент виден только со
    // СЛЕДУЮЩЕГО). При TIMEOUT=3: тики 1,2,3 читают counter=0,1,2
    // (все < 3) -> Right; счётчик становится 3 только ПОСЛЕ apply тика 3.
    // Тик 4 первым читает counter=3 (>= 3) -> переключение на Up.

    // Тик 1: counter (на начало тика) = 0 < 3 -> Right.
    engine.run_tick();
    assert_eq!(
        find_marker(&engine),
        (3, 2),
        "тик 1: должен ехать East (счётчик на начало тика = 0)"
    );
    // Тик 2: counter = 1 < 3 -> всё ещё Right.
    engine.run_tick();
    assert_eq!(
        find_marker(&engine),
        (4, 2),
        "тик 2: должен ехать East (счётчик на начало тика = 1)"
    );
    // Тик 3: counter = 2 < 3 -> ВСЁ ЕЩЁ Right (это тик, ПОСЛЕ которого
    // счётчик станет 3 — само пересечение порога видно только следующему
    // тику, не этому).
    engine.run_tick();
    assert_eq!(
        find_marker(&engine),
        (5, 2),
        "тик 3: должен ехать East (счётчик на начало тика = 2, ещё не пересёк порог)"
    );
    // Тик 4: counter (на начало тика) = 3 >= 3 -> защёлка сработала, едет Up.
    engine.run_tick();
    assert_eq!(
        find_marker(&engine),
        (5, 1),
        "тик 4: должен ПЕРЕКЛЮЧИТЬСЯ на North — первый тик, читающий уже пересечённый порог"
    );
    // Тик 5: защёлка не сбрасывается — по-прежнему Up, не East.
    engine.run_tick();
    assert_eq!(
        find_marker(&engine),
        (5, 0),
        "тик 5: защёлка не должна сбрасываться — маркер продолжает ехать North"
    );
}

/// Найден при аудите GPU-порта `feedback` (см. память сессии,
/// `project_gpu_memory_support_2026_08_08`): `arbitrator::get_match_affected_cells`
/// (вызывается ИЗНУТРИ арбитража) и `applicator::apply_rule_buffered`
/// (вызывается ПОСЛЕ) ОБА читают `feedback_counters`, чтобы решить, какое
/// направление (декларированное или `new_direction`) реально становится
/// affected-cells/фактической записью — раньше инкремент счётчика стоял
/// МЕЖДУ этими двумя чтениями, так что на тике, где счётчик матча
/// ПЕРЕСЕКАЕТ `timeout` ИМЕННО на этом тике, арбитраж резервировал/проверял
/// конфликты для ОДНОГО направления, а apply реально писал в ДРУГОЕ — цель,
/// которую арбитраж НИКОГДА не проверял на конфликт с другими матчами.
///
/// ВАЖНО (переработано после повторного аудита, см. память сессии): счётчик
/// ОБЯЗАН читаться КАК ОН БЫЛ на начало тика — обнаружение ЭТИМ тиком не
/// засчитывается для решения ЭТОГО же тика (та же дисциплина, что и у
/// `age`/`min_age`/`starvation_after`). При `TIMEOUT=1` порог пересекается
/// НЕ на первом тике (тик 1 читает counter=0 < 1 — ещё Right), а становится
/// видимым НАЧИНАЯ со второго (тик 2 читает counter=1 >= 1 — переключение).
/// Маркер сначала едет Right (тик 1: (0,0)->(1,0)), ЗАТЕМ на тике 2
/// пытается переключиться на Down из своей НОВОЙ позиции (1,0)->(1,1) —
/// конкурент стоит именно там, а не на исходной Down-цели (0,1), которая
/// маркеру больше не актуальна после переезда.
///
/// Конкурент стоит ИМЕННО на клетке `new_direction`-цели ВТОРОГО тика (не
/// Right-цели) — так что баг проявляется ТОЛЬКО при реальной рассинхронизации
/// между "что видел арбитраж" и "что реально написал apply": маркер обязан
/// либо (а) выиграть у конкурента и переехать Down, либо (б) остаться на
/// месте (не эта ветка — Right ничем не занят). Тест ловит ИМЕННО
/// невозможный третий исход: маркер тихо ИСЧЕЗАЕТ (source-clear проходит,
/// целевая запись проигрывает необнаруженную гонку с конкурентом).
#[test]
fn test_feedback_counter_crossing_threshold_this_tick_matches_arbitration_and_apply() {
    const MARKER2: u8 = 98;
    const COMPETITOR: u8 = 99;
    const TIMEOUT: u64 = 1;

    let mut grid = make_grid(3, 3);
    grid.set_cell(0, 0, Cell::new(MARKER2));
    grid.set_cell(1, 1, Cell::new(COMPETITOR)); // Down-цель ВТОРОГО тика (маркер к тому моменту уже на (1,0))

    let marker_rule = Rule {
        id: vec![CellType(MARKER2)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
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
            new_direction: Direction::Down,
        }),
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let competitor_rule = Rule {
        id: vec![CellType(COMPETITOR)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(COMPETITOR))],
        active_only: false,
        priority: 1,
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
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![marker_rule, competitor_rule]));

    // Тик 1: counter (на начало тика) = 0 < TIMEOUT=1 -> Right, (0,0)->(1,0).
    engine.run_tick();
    let after_tick1: Vec<(usize, usize)> = (0..3)
        .flat_map(|y| (0..3).map(move |x| (x, y)))
        .filter(|&(x, y)| engine.grid().get_cell(x, y).map(|c| c.value.0 .0) == Some(MARKER2))
        .collect();
    assert_eq!(
        after_tick1,
        vec![(1, 0)],
        "тик 1: маркер едет Right (счётчик на начало тика = 0, ещё не пересёк порог)"
    );

    // Тик 2: counter (на начало тика) = 1 >= TIMEOUT=1 -> переключение на
    // Down, ИМЕННО тот тик, где рассинхронизация арбитража/apply могла бы
    // проявиться (счётчик пересёк порог ПОСЛЕ тика 1, впервые виден тику 2).
    engine.run_tick();
    let marker_positions: Vec<(usize, usize)> = (0..3)
        .flat_map(|y| (0..3).map(move |x| (x, y)))
        .filter(|&(x, y)| engine.grid().get_cell(x, y).map(|c| c.value.0 .0) == Some(MARKER2))
        .collect();
    assert_eq!(
        marker_positions,
        vec![(1, 1)],
        "тик 2: маркер обязан ПОБЕДИТЬ конкурента и переехать Down (приоритет 10 > 1) — если он вместо этого исчез (пустой список), арбитраж и apply разошлись во мнениях о направлении"
    );
}

// ──────────────────────────────────────────────────────────────
// `Rule::recursion` (block-обсуждение "рекурсивные правила", п.4) —
// ограниченный каскад ВНУТРИ одного тика: заливка на несколько клеток
// сразу, а не за N тиков.
// ──────────────────────────────────────────────────────────────

const RFILLED: u8 = 80;
const RUNFILLED: u8 = 81;

#[test]
fn test_recursion_cascades_multiple_cells_in_one_tick() {
    const MAX_DEPTH: u8 = 3;
    let mut grid = make_grid(10, 1);
    grid.set_cell(0, 0, Cell::new(RFILLED));
    for x in 1..10 {
        grid.set_cell(x, 0, Cell::new(RUNFILLED));
    }

    let rule = Rule {
        id: vec![CellType(RUNFILLED)],
        pattern: vec![(0, 0, CellType(RUNFILLED)), (-1, 0, CellType(RFILLED))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(RFILLED))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: Some(RecursionSpec {
            max_depth: MAX_DEPTH,
            direction: Direction::Right,
        }),
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // ОДИН тик — должен залить исходную клетку (1) плюс MAX_DEPTH=3
    // дополнительных уровня каскада (2, 3, 4), итого 4 клетки, не за 4 тика.
    engine.run_tick();

    for x in 0..=4 {
        assert_eq!(
            engine.grid().get_cell(x, 0).map(|c| c.value.0 .0),
            Some(RFILLED),
            "клетка {x} должна быть залита за ОДИН тик (каскад глубины {MAX_DEPTH})"
        );
    }
    for x in 5..10 {
        assert_eq!(
            engine.grid().get_cell(x, 0).map(|c| c.value.0 .0),
            Some(RUNFILLED),
            "клетка {x} НЕ должна быть залита — каскад ограничен max_depth={MAX_DEPTH}"
        );
    }
}

/// Лемма 4 (`paper/paper4.md` §8, Corollary B): каскад `recursion` обязан
/// участвовать в графе конфликтов через union по ВСЕМ уровням k=0..=max_depth,
/// а не только k=0 — иначе конфликт, достижимый ТОЛЬКО на глубине каскада,
/// был бы пропущен.
#[test]
fn test_recursion_conflict_only_visible_via_cascade_depth_union() {
    // Правило A: recursion max_depth=2, direction=Right. Нормальный (k=0)
    // write cell — только (0,0). Union по k=0..=2 добавляет (1,0) и (2,0).
    let rule_a = Rule {
        id: vec![CellType(1)],
        pattern: vec![(0, 0, CellType(1)), (-1, 0, CellType(9))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(2))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: Some(RecursionSpec {
            max_depth: 2,
            direction: Direction::Right,
        }),
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    // Правило B: пишет в (0,0) относительно себя. Размещённое (в терминах
    // относительного офсета, который перебирает `ConflictGraph::build`) на
    // (2,0) от A — недостижимо на k=0, достижимо ТОЛЬКО на глубине каскада k=2.
    let rule_b = Rule {
        id: vec![CellType(3)],
        pattern: vec![(0, 0, CellType(3))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(9))],
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
    };

    let graph = crate::ConflictGraph::build(&[rule_a, rule_b]);
    assert!(
        graph.edges.contains(&(0, 1)),
        "граф ОБЯЗАН найти ребро между A и B: каскад A на глубине k=2 пишет в ту же \
относительную клетку, где сидит B, хотя k=0 (без каскада) её не задевает. Рёбра: {:?}",
        graph.edges
    );
}

// ──────────────────────────────────────────────────────────────
// `Rule::memory` (тема "правила с памятью", п.3) — гейт по ТОЧНОЙ
// последовательности прошлых наблюдений (FIFO-буфер), а не по скалярному
// счётчику (`starvation_after`/`feedback`). Два триггера, один механизм
// (см. её doc-комментарий в `types.rs`): `NeighborType` — до арбитража,
// `RuleOutcome` — после.
// ──────────────────────────────────────────────────────────────

const MEM_WATCHER: u8 = 90;
const MEM_FIRED: u8 = 91;
const MEM_NEIGH_A: u8 = 92;
const MEM_NEIGH_B: u8 = 93;

#[test]
fn test_memory_neighbor_type_gate_opens_exactly_after_matching_sequence() {
    let mut grid = make_grid(5, 1);
    grid.set_cell(2, 0, Cell::new(MEM_WATCHER));
    grid.set_cell(3, 0, Cell::new(MEM_NEIGH_A)); // (2,0) + Right = (3,0)

    let rule = Rule {
        id: vec![CellType(MEM_WATCHER)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(MEM_FIRED))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: Some(MemorySpec {
            window: 3,
            record_trigger: RecordTrigger::NeighborType(Direction::Right),
            match_pattern: vec![
                RecordedValue::Type(CellType(MEM_NEIGH_A)),
                RecordedValue::Type(CellType(MEM_NEIGH_B)),
                RecordedValue::Type(CellType(MEM_NEIGH_A)),
            ],
        }),
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Тик 1: буфер получает Type(A) (сосед на (3,0)), но len=1 != window=3
    // -> гейт закрыт (проверяет буфер КАК ОН БЫЛ до этого тика — пустой).
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(2, 0).map(|c| c.value.0 .0),
        Some(MEM_WATCHER),
        "тик 1: гейт ещё закрыт — буфер не полон"
    );

    // Тик 2: сосед -> B. Буфер [A, B], len=2 != 3 -> гейт всё ещё закрыт.
    engine.grid_mut().set_cell(3, 0, Cell::new(MEM_NEIGH_B));
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(2, 0).map(|c| c.value.0 .0),
        Some(MEM_WATCHER),
        "тик 2: гейт ещё закрыт — буфер не полон"
    );

    // Тик 3: сосед -> A. К КОНЦУ этого тика буфер станет [A, B, A] (точно
    // совпадает с match_pattern), но гейт ЭТОГО тика проверяется ДО этой
    // записи (буфер "как он был на конец тика 2" = [A, B], не полон) ->
    // WATCHER ещё не срабатывает, хотя буфер вот-вот совпадёт.
    engine.grid_mut().set_cell(3, 0, Cell::new(MEM_NEIGH_A));
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(2, 0).map(|c| c.value.0 .0),
        Some(MEM_WATCHER),
        "тик 3: гейт всё ещё закрыт — буфер полнится ПОСЛЕ арбитража этого тика, не ДО"
    );

    // Тик 4: гейт теперь проверяет буфер [A, B, A] (каким он стал к концу
    // тика 3) — точное совпадение -> гейт открывается РОВНО на этом тике.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(2, 0).map(|c| c.value.0 .0),
        Some(MEM_FIRED),
        "тик 4: гейт обязан открыться ровно на тик после накопления полной совпадающей последовательности"
    );
}

/// Лемма-4-класса вопрос "нужны ли изменения в conflict_analyzer" здесь не
/// стоит — `memory` не меняет заявленную зону записи правила (гейт только
/// решает, участвует ли матч в арбитраже ВООБЩЕ, changes/shifts остаются
/// теми же, что и без memory). См. `types::MemorySpec`'s doc-комментарий:
/// `conflict_analyzer.rs` не тронут ни строчкой ради этой темы.
#[test]
fn test_memory_rule_outcome_gate_fires_on_exact_mixed_sequence() {
    const R_MARKER: u8 = 94;
    const R_FIRED: u8 = 95;

    let mut grid = make_grid(1, 1);
    grid.set_cell(0, 0, Cell::new(R_MARKER));

    let rule = Rule {
        id: vec![CellType(R_MARKER)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(R_FIRED))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: Some(MemorySpec {
            window: 3,
            record_trigger: RecordTrigger::RuleOutcome,
            match_pattern: vec![RecordedValue::Missed, RecordedValue::Applied, RecordedValue::Missed],
        }),
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Белый ящик (законно: `tests` — дочерний модуль `engine`, у него есть
    // доступ к приватным полям `Engine`): подсаживаем буфер напрямую. Это
    // ЕДИНСТВЕННЫЙ честный способ проверить гейт на СМЕШАННОЙ (не
    // однородной) последовательности — правило, гейтующее САМО СЕБЯ по
    // своему же исходу арбитража, не может естественно накопить такую
    // историю через симуляцию с нуля: "гейт закрыт" ВСЕГДА означает
    // "проиграл" (матч исключён из арбитража целиком, а не проиграл
    // по-честному), так что с нуля накопимая история умеет быть ТОЛЬКО
    // однородной ([Missed; window] после N тиков простоя) — ровно то, что
    // `starvation_after` и так умеет выразить. Это не баг теста и не баг
    // механизма — чисто структурное свойство self-referential
    // RuleOutcome-гейта, стоящее отдельного документирования (см.
    // `paper/paper4.md`), а не то, что этот тест обязан воспроизводить
    // "с нуля" через `run_tick`.
    engine.state.mutate().memory_buffers_mut().insert(
        (0, 0, 0),
        VecDeque::from(vec![
            RecordedValue::Missed,
            RecordedValue::Applied,
            RecordedValue::Missed,
        ]),
    );

    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(R_FIRED),
        "гейт обязан открыться немедленно: подсаженный буфер уже точно совпадает с match_pattern"
    );
}

/// Тот же multiset исходов (2×Missed, 1×Applied), но ДРУГОЙ порядок —
/// [Missed, Missed, Applied], а не [Missed, Applied, Missed]. Скалярный
/// счётчик (`Rule::starvation_after: Option<u32>`) в принципе не может
/// различить эти два случая: он хранит ОДНО число (сколько раз подряд
/// проиграно), а не порядок исходов. Память обязана их различить — это и
/// есть доказательство, что `memory` — не переобёртка `starvation_after`.
#[test]
fn test_memory_rule_outcome_gate_rejects_reordered_sequence_with_same_multiset() {
    const R_MARKER: u8 = 96;
    const R_FIRED: u8 = 97;

    let mut grid = make_grid(1, 1);
    grid.set_cell(0, 0, Cell::new(R_MARKER));

    let rule = Rule {
        id: vec![CellType(R_MARKER)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(R_FIRED))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: Some(MemorySpec {
            window: 3,
            record_trigger: RecordTrigger::RuleOutcome,
            match_pattern: vec![RecordedValue::Missed, RecordedValue::Applied, RecordedValue::Missed],
        }),
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    engine.state.mutate().memory_buffers_mut().insert(
        (0, 0, 0),
        VecDeque::from(vec![
            RecordedValue::Missed,
            RecordedValue::Missed,
            RecordedValue::Applied,
        ]),
    );

    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(R_MARKER),
        "гейт ДОЛЖЕН остаться закрытым: тот же multiset исходов, но другой порядок — не \
совпадает с match_pattern поэлементно. Скаляр (starvation_after) эту разницу в принципе не видит."
    );
}

// ──────────────────────────────────────────────────────────────
// Аудит взаимодействий: `keep_source` × `feedback`/`memory`, гейт `memory`
// × `starvation_after`. Не переписанные из головы предположения — каждый
// тест реально проверяет конкретную, потенциально ломающуюся комбинацию.
// ──────────────────────────────────────────────────────────────

/// Правило с ОБОИМИ `feedback` И `memory` НА `keep_source`-сдвиге:
/// источник физически никогда не двигается (`keep_source` не даёт его
/// очистить), так что БЕЗ фикса ("пропустить перенос при keep_source", см.
/// `applicator::apply_shift_buffered`) старый код всё равно пытался бы
/// ПЕРЕНЕСТИ оба состояния (`feedback_counters` и `memory_buffers`) на
/// позицию ЦЕЛИ ИЗЛУЧЕНИЯ (которая НЕ является тем же маркером — это
/// НЕЗАВИСИМАЯ копия) — история оригинала терялась бы на каждом тике,
/// который что-то реально излучил. Проверяем, что состояние ИСТОЧНИКА
/// переживает несколько тиков нетронутым.
#[test]
fn test_emit_preserves_feedback_and_memory_state_at_source_across_ticks() {
    const MARKER: u8 = 210;

    let mut grid = make_grid(5, 1);
    grid.set_cell(0, 0, Cell::new(MARKER));

    let rule = Rule {
        id: vec![CellType(MARKER)],
        pattern: vec![],
        // Точечное излучение: копия ТОЛЬКО в (1,0), источник (0,0) не трогается.
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 1,
            broadcast: false,
            keep_source: true,
        }]],
        changes: vec![],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: Some(FeedbackSpec {
            timeout: 100,
            new_direction: Direction::Up,
        }), // высокий timeout — не должен успеть сработать за 3 тика, тест не про feedback-переключение, а про сохранность счётчика
        recursion: None,
        memory: Some(MemorySpec {
            window: 2,
            record_trigger: RecordTrigger::RuleOutcome,
            // Достижимо с нуля (см. doc-комментарий про self-referential
            // bootstrap deadlock в `test_memory_rule_outcome_gate_fires_on_exact_mixed_sequence`):
            // однородный [Missed, Missed] естественно накопится, пока гейт закрыт.
            match_pattern: vec![RecordedValue::Missed, RecordedValue::Missed],
        }),
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Тик 1: буфер пуст -> гейт закрыт -> матч исключён из арбитража ->
    // Missed записывается (см. doc-комментарий гейта), апдейт не применяется.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(MARKER),
        "тик 1: гейт закрыт, применения не было"
    );
    assert_eq!(
        engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
        Some(0),
        "тик 1: излучения не было (гейт закрыт)"
    );

    // Тик 2: буфер [Missed], всё ещё не полон (window=2) -> гейт всё ещё закрыт.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
        Some(0),
        "тик 2: гейт всё ещё закрыт"
    );

    // Тик 3: буфер [Missed, Missed] (как он был к концу тика 2) == match_pattern
    // -> гейт открывается -> единственный претендент побеждает арбитраж без
    // сравнений -> излучение реально применяется.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(MARKER),
        "тик 3: источник ДОЛЖЕН сохранить значение (keep_source)"
    );
    assert_eq!(
        engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
        Some(MARKER),
        "тик 3: цель излучения получила копию"
    );

    // Белый ящик: состояние ОБОИХ расширений должно жить у ИСТОЧНИКА
    // (0,0,0), не быть перенесено (и тем более не потеряно) на цель (1,0,0).
    assert_eq!(
        engine.state.snapshot().feedback_counters().get(&(0, 0, 0)),
        Some(&1),
        "счётчик feedback ДОЛЖЕН пережить 3 тика на позиции источника — \
без фикса keep_source он был бы (ошибочно) перенесён на (1,0) при первом же реальном применении"
    );
    assert_eq!(
        engine.state.snapshot().memory_buffers().get(&(0, 0, 0)),
        Some(&VecDeque::from(vec![RecordedValue::Missed, RecordedValue::Applied])),
        "буфер memory ДОЛЖЕН остаться у источника и корректно обновиться на Applied после тика 3"
    );
    assert!(
        engine.state.snapshot().feedback_counters().get(&(1, 0, 0)).is_none()
            && engine.state.snapshot().memory_buffers().get(&(1, 0, 0)).is_none(),
        "у ЦЕЛИ излучения НЕ должно быть состояния — это независимая свежая копия, не наследующая историю оригинала"
    );
}

/// Гейт `memory` закрывает матч ДО того, как считаются `starving_keys` (см.
/// порядок в `run_tick_with_cache`). Проверяем это напрямую: правило с
/// ПОСТОЯННО закрытым `memory`-гейтом (наблюдает соседа, который никогда не
/// станет нужным типом) и `starvation_after` ОДНОВРЕМЕННО — если бы порядок
/// был перепутан, счётчик голодания рос бы для матча, который на самом деле
/// ни разу не участвовал в арбитраже.
#[test]
fn test_memory_gate_closed_excludes_from_starvation_accounting() {
    const WATCHER: u8 = 211;
    const NEVER_A: u8 = 212; // сосед всегда этого типа
    const WANTED_B: u8 = 213; // а гейт ждёт этот — никогда не появится

    let mut grid = make_grid(3, 1);
    grid.set_cell(0, 0, Cell::new(WATCHER));
    grid.set_cell(1, 0, Cell::new(NEVER_A));

    let rule = Rule {
        id: vec![CellType(WATCHER)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(WATCHER))], // no-op change если бы применилось — не про эффект, про сам факт участия в арбитраже
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: Some(2),
        feedback: None,
        recursion: None,
        memory: Some(MemorySpec {
            window: 1,
            record_trigger: RecordTrigger::NeighborType(Direction::Right),
            match_pattern: vec![RecordedValue::Type(CellType(WANTED_B))], // недостижимо — сосед всегда NEVER_A
        }),
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    for tick in 1..=5 {
        engine.run_tick();
        assert!(
            engine.state.snapshot().starvation_counters().get(&(0, 0, 0)).is_none(),
            "тик {tick}: счётчик голодания НЕ должен даже появиться в карте — гейт memory \
исключает матч из арбитража КАЖДЫЙ тик, значит он никогда по-настоящему не 'проигрывал'"
        );
    }
}

/// `cam` × `memory`: CAM-матчи входят в `matches` отдельным путём
/// (`detect_cam_matches`, слитый в общий список ДО гейт-фильтра) — гейт
/// работает с ними УНИФИЦИРОВАННО (резолвит правило по `m.head`/`m.rule_idx`,
/// без спец-случая для CAM) или нет? Проверяем напрямую: магнит с закрытым
/// на тик 1 гейтом НЕ должен притянуть цель; тот же магнит на тик 2
/// (гейт открылся) — должен.
#[test]
fn test_cam_magnet_respects_memory_gate() {
    const GATE_NEIGHBOR_VALUE: u8 = 214;

    let mut grid = make_grid(10, 1);
    grid.set_cell(0, 0, Cell::new(MAGNET));
    grid.set_cell(1, 0, Cell::new(GATE_NEIGHBOR_VALUE)); // магнит смотрит сюда через NeighborType(Right)
    grid.set_cell(4, 0, Cell::new(TARGET));

    let rule = Rule {
        id: vec![CellType(MAGNET)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![],
        active_only: false,
        priority: 0,
        min_age: 0,
        overflow: Default::default(),
        cam: Some(CamSearch {
            radius: 5,
            target_type: CellType(TARGET),
        }),
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: Some(MemorySpec {
            window: 1,
            record_trigger: RecordTrigger::NeighborType(Direction::Right),
            match_pattern: vec![RecordedValue::Type(CellType(GATE_NEIGHBOR_VALUE))],
        }),
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Тик 1: буфер пуст -> не полон -> гейт закрыт (даже хотя сосед УЖЕ
    // нужного типа с самого начала) -> CAM-матч исключён из арбитража ->
    // притяжения не происходит.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(MAGNET),
        "тик 1: гейт закрыт -- магнит не должен был притянуть цель"
    );
    assert_eq!(
        engine.grid().get_cell(4, 0).map(|c| c.value.0 .0),
        Some(TARGET),
        "тик 1: цель должна остаться на месте"
    );

    // Тик 2: буфер [Type(GATE_NEIGHBOR_VALUE)] (записан в тике 1, независимо
    // от гейта) == match_pattern -> гейт открывается -> CAM реально применяется.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(TARGET),
        "тик 2: гейт открылся -- магнит должен был притянуть цель"
    );
    assert_eq!(
        engine.grid().get_cell(4, 0).map(|c| c.value.0 .0),
        Some(0),
        "тик 2: найденная клетка должна быть очищена"
    );
}

// ──────────────────────────────────────────────────────────────
// `memory` × `keep_source` (БЕЗ `feedback`), триггер `NeighborType`
// (не `RuleOutcome` — та комбинация уже покрыта
// `test_emit_preserves_feedback_and_memory_state_at_source_across_ticks`).
// Три вопроса: (1) копия получает свежий независимый буфер, источник верно
// копится ДОЛЬШЕ 3 тиков; (2) `NeighborType` копии читает СВОЕГО соседа, а не
// соседа оригинала; (3) не остаётся ли осиротевшая запись в
// `Engine::memory_buffers`, когда клетка перестаёт совпадать.
// ──────────────────────────────────────────────────────────────

/// (1)+(2) в одном сценарии: WATCHER на (0,0), точечное излучение
/// (`keep_source`, БЕЗ `feedback`) в (2,0) при открытии гейта на тик 4.
/// Копия на (2,0) с тика 5 сама становится независимым матчем ТОГО ЖЕ
/// правила (тот же `head`/`rule_idx`) — проверяем, что её запись в
/// `memory_buffers` (а) появляется только тогда, когда она реально
/// продетектирована (не раньше — сама копия физически не существовала в
/// решётке до конца тика 4), (b) НЕ содержит ничего от истории источника,
/// (c) читает соседа ОТНОСИТЕЛЬНО СВОЕЙ позиции (3,0), а не позиции
/// источника (1,0) — который к этому моменту сознательно выставлен в ДРУГОЕ
/// значение, чтобы совпадение было бы легко спутать, будь чтение
/// перепутано. Источник тем временем продолжает копить СВОЙ буфер ещё
/// несколько тиков после эмиссии — дольше 3 тиков, которые покрывал
/// предыдущий тест.
#[test]
fn test_emit_memory_neighbor_type_copy_gets_independent_buffer_own_position() {
    const WATCHER: u8 = 215;
    const TYPE_A: u8 = 216;
    const TYPE_B: u8 = 217;
    const TYPE_C: u8 = 218; // сосед копии — константа, никогда не откроет её гейт
    const TYPE_D: u8 = 219; // сосед источника после эмиссии — заведомо не A/B/C

    let mut grid = make_grid(6, 1);
    grid.set_cell(0, 0, Cell::new(WATCHER)); // источник
    grid.set_cell(1, 0, Cell::new(TYPE_A)); // сосед источника (Right)

    let rule = Rule {
        id: vec![CellType(WATCHER)],
        pattern: vec![], // матчится безусловно по типу головы, без требования к соседям
        // Точечное излучение (keep_source, БЕЗ feedback): копия в (cx+2, cy).
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 2,
            broadcast: false,
            keep_source: true,
        }]],
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
        memory: Some(MemorySpec {
            window: 3,
            record_trigger: RecordTrigger::NeighborType(Direction::Right),
            match_pattern: vec![
                RecordedValue::Type(CellType(TYPE_A)),
                RecordedValue::Type(CellType(TYPE_B)),
                RecordedValue::Type(CellType(TYPE_A)),
            ],
        }),
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Тик 1: буфер источника пуст -> гейт закрыт -> эмиссии нет. После тика
    // буфер = [A] (сосед на тик 1).
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(2, 0).map(|c| c.value.0 .0),
        Some(0),
        "тик 1: гейт закрыт, эмиссии не было"
    );
    assert_eq!(
        engine.state.snapshot().memory_buffers().get(&(0, 0, 0)),
        Some(&VecDeque::from(vec![RecordedValue::Type(CellType(TYPE_A))]))
    );

    // Тик 2: сосед -> B. Буфер [A] не полон -> гейт закрыт. После тика [A,B].
    engine.grid_mut().set_cell(1, 0, Cell::new(TYPE_B));
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(2, 0).map(|c| c.value.0 .0),
        Some(0),
        "тик 2: гейт всё ещё закрыт"
    );
    assert_eq!(
        engine.state.snapshot().memory_buffers().get(&(0, 0, 0)),
        Some(&VecDeque::from(vec![
            RecordedValue::Type(CellType(TYPE_A)),
            RecordedValue::Type(CellType(TYPE_B))
        ]))
    );

    // Тик 3: сосед -> A. Буфер [A,B] (len=2) всё ещё не полон -> гейт закрыт
    // на ЭТОМ тике (проверяется буфер ДО обновления). После тика — [A,B,A],
    // полон и совпадает с match_pattern, но это станет видно гейту только
    // СЛЕДУЮЩЕГО тика.
    engine.grid_mut().set_cell(1, 0, Cell::new(TYPE_A));
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(2, 0).map(|c| c.value.0 .0),
        Some(0),
        "тик 3: гейт всё ещё закрыт (буфер заполнится только к концу тика)"
    );
    assert_eq!(
        engine.state.snapshot().memory_buffers().get(&(0, 0, 0)),
        Some(&VecDeque::from(vec![
            RecordedValue::Type(CellType(TYPE_A)),
            RecordedValue::Type(CellType(TYPE_B)),
            RecordedValue::Type(CellType(TYPE_A))
        ]))
    );

    // Тик 4: буфер [A,B,A] (каким он был к концу тика 3) == match_pattern ->
    // гейт открывается -> keep_source-эмиссия реально применяется: источник
    // (0,0) СОХРАНЯЕТ значение, копия появляется в (2,0). Буфер источника
    // продолжает копиться дальше (FIFO): сосед на этот тик = B (выставлен
    // ниже перед тиком) -> [B,A,B].
    engine.grid_mut().set_cell(1, 0, Cell::new(TYPE_B));
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(WATCHER),
        "тик 4: источник ДОЛЖЕН сохранить значение (keep_source)"
    );
    assert_eq!(
        engine.grid().get_cell(2, 0).map(|c| c.value.0 .0),
        Some(WATCHER),
        "тик 4: копия должна была появиться в (2,0)"
    );
    assert_eq!(
        engine.state.snapshot().memory_buffers().get(&(0, 0, 0)),
        Some(&VecDeque::from(vec![
            RecordedValue::Type(CellType(TYPE_B)),
            RecordedValue::Type(CellType(TYPE_A)),
            RecordedValue::Type(CellType(TYPE_B))
        ])),
        "тик 4: источник продолжает копить СВОЙ буфер (дольше 3 тиков предыдущего теста)"
    );
    // Копия физически появилась только к КОНЦУ тика 4 (в write_buffer) — она
    // ещё НЕ была продетектирована как отдельный матч на этом тике (детект
    // читает pre-tick срез), поэтому у неё пока не должно быть записи вовсе.
    assert!(
        engine.state.snapshot().memory_buffers().get(&(2, 0, 0)).is_none(),
        "тик 4: у копии не должно быть записи ДО того, как она хоть раз реально продетектирована"
    );

    // Перед тиком 5: выставляем РАЗНЫХ соседей источнику (D — заведомо не
    // A/B/C) и копии (C — константа, гейт копии никогда не откроется).
    engine.grid_mut().set_cell(1, 0, Cell::new(TYPE_D));
    engine.grid_mut().set_cell(3, 0, Cell::new(TYPE_C)); // сосед КОПИИ, Right от (2,0)
    engine.run_tick();

    // Копия (2,0) теперь тоже матчится как независимый матч ТОГО ЖЕ правила.
    // Её буфер должен появиться ВПЕРВЫЕ и содержать РОВНО [C] — не унаследовав
    // НИЧЕГО от истории источника (которая на этот момент [A,B,D] — старое
    // [B,A,B] минус A, плюс D).
    assert_eq!(
        engine.state.snapshot().memory_buffers().get(&(2, 0, 0)),
        Some(&VecDeque::from(vec![RecordedValue::Type(CellType(TYPE_C))])),
        "тик 5: буфер копии должен быть СВЕЖИМ (только что увиденное значение), не унаследованным от источника"
    );
    assert_eq!(
        engine.state.snapshot().memory_buffers().get(&(0, 0, 0)),
        Some(&VecDeque::from(vec![
            RecordedValue::Type(CellType(TYPE_A)),
            RecordedValue::Type(CellType(TYPE_B)),
            RecordedValue::Type(CellType(TYPE_D))
        ])),
        "тик 5: источник продолжает копить свой буфер независимо от копии"
    );
    // Копия ещё не могла сама излучить дальше — её гейт никогда не откроется
    // (сосед константно C, паттерн требует A,B,A) — (4,0) должно остаться пустым.
    assert_eq!(
        engine.grid().get_cell(4, 0).map(|c| c.value.0 .0),
        Some(0),
        "тик 5: гейт копии закрыт — каскада излучения быть не должно"
    );

    // Ещё два тика (6,7): сосед копии остаётся C (гейт копии так и не
    // откроется — [C,C,C] никогда не совпадёт с [A,B,A]), сосед источника
    // держим D. Проверяем, что оба буфера продолжают расти корректно и
    // НЕЗАВИСИМО друг от друга ещё дальше (суммарно > 3 тиков с момента
    // появления копии).
    engine.run_tick(); // тик 6
    engine.run_tick(); // тик 7
    assert_eq!(
        engine.state.snapshot().memory_buffers().get(&(2, 0, 0)),
        Some(&VecDeque::from(vec![
            RecordedValue::Type(CellType(TYPE_C)),
            RecordedValue::Type(CellType(TYPE_C)),
            RecordedValue::Type(CellType(TYPE_C))
        ])),
        "тик 7: буфер копии — три одинаковых наблюдения СВОЕГО соседа, копия жива и наблюдает независимо"
    );
    assert_eq!(
        engine.state.snapshot().memory_buffers().get(&(0, 0, 0)),
        Some(&VecDeque::from(vec![
            RecordedValue::Type(CellType(TYPE_D)),
            RecordedValue::Type(CellType(TYPE_D)),
            RecordedValue::Type(CellType(TYPE_D))
        ])),
        "тик 7: источник продолжает копить свой буфер (D) независимо от копии — суммарно 7 тиков, не 3"
    );
    // Гейт копии так и не открылся -> каскада на (4,0) по-прежнему нет.
    assert_eq!(
        engine.grid().get_cell(4, 0).map(|c| c.value.0 .0),
        Some(0),
        "тик 7: гейт копии всё ещё закрыт"
    );

    // Ровно ДВЕ живые записи в карте — источник и копия, никаких лишних или
    // фантомных ключей не появилось за 7 тиков активной эмиссии.
    assert_eq!(
        engine.state.snapshot().memory_buffers().len(),
        2,
        "в карте должно быть ровно 2 записи: источник (0,0,0) и копия (2,0,0)"
    );
}

/// (3): бывшая "осиротевшая запись" в `Engine::memory_buffers` — ТЕПЕРЬ
/// ФИКС, не задокументированный компромисс. Раньше, если клетка, которая
/// матчилась (и потому обзавелась записью в буфере), переставала совпадать
/// с правилом по ВНЕШНЕЙ причине (тип меняется чем-то посторонним —
/// например, проигрывает конфликт другому правилу в другой части конфига,
/// что здесь смоделировано прямой записью в решётку, а не отдельным
/// конкурирующим правилом, ради простоты и детерминизма), ничто не убирало
/// её запись из `Engine::memory_buffers` — она росла НАВСЕГДА.
///
/// Теперь (см. блок "осиротевшие записи" в `run_tick_with_cache`) это
/// вычищается ДЁШЕВО и КОРРЕКТНО, используя уже посчитанный на этот тик
/// `search_coords` (тот же dirty-based инвариант, на котором держится весь
/// инкрементальный матчер) — не требует ни полного скана карты, ни хранения
/// снимка кандидатов прошлого тика. `keep_source` тут не хуже и не лучше
/// обычного сдвига: тот же фикс покрывает оба случая одинаково (см. также
/// `test_feedback_counter_pruned_after_match_stops_existing` — тот же класс
/// для `feedback_counters`).
#[test]
fn test_memory_buffer_entry_pruned_after_match_stops_existing() {
    const WATCHER: u8 = 220;
    const NEIGH_OK: u8 = 221;
    const UNRELATED: u8 = 222; // то, во что "внешне" превращается источник

    let mut grid = make_grid(4, 1);
    grid.set_cell(0, 0, Cell::new(WATCHER));
    grid.set_cell(1, 0, Cell::new(NEIGH_OK)); // сосед постоянно нужного типа

    let rule = Rule {
        id: vec![CellType(WATCHER)],
        pattern: vec![],
        // keep_source-эмиссия НАМЕРЕННО за пределы решётки (steps=10 на
        // ширине 4, overflow=Discard по умолчанию) — цель никогда никуда не
        // попадает, значит НЕ появляется второй независимый матч этого же
        // правила (копия), который иначе завёл бы СВОЮ запись в
        // `memory_buffers` и мешал бы проверять именно сценарий источника в
        // изоляции (см. отдельный тест
        // `test_emit_memory_neighbor_type_copy_gets_independent_buffer_own_position`
        // про то, что копия — это ожидаемо-корректный независимый матч).
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 10,
            broadcast: false,
            keep_source: true,
        }]],
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
        memory: Some(MemorySpec {
            window: 1,
            record_trigger: RecordTrigger::NeighborType(Direction::Right),
            match_pattern: vec![RecordedValue::Type(CellType(NEIGH_OK))],
        }),
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Тик 1: буфер пуст -> гейт закрыт -> буфер после тика = [NEIGH_OK].
    engine.run_tick();
    assert_eq!(
        engine.state.snapshot().memory_buffers().get(&(0, 0, 0)),
        Some(&VecDeque::from(vec![RecordedValue::Type(CellType(NEIGH_OK))]))
    );

    // Тик 2: гейт открыт (буфер [NEIGH_OK] == match_pattern) -> эмиссия
    // применяется, источник (0,0) остаётся WATCHER (keep_source).
    engine.run_tick();
    assert_eq!(engine.grid().get_cell(0, 0).map(|c| c.value.0 .0), Some(WATCHER));
    assert!(
        engine.state.snapshot().memory_buffers().contains_key(&(0, 0, 0)),
        "запись источника должна существовать после того, как он реально совпал"
    );

    // Внешнее событие: (0,0) перестаёт быть WATCHER (имитирует проигрыш
    // конфликта другому правилу/оверрайд извне — сам механизм конфликта тут
    // не важен, важен только факт: клетка, которая раньше матчилась, больше
    // никогда не будет продетектирована этим правилом). `set_cell` метит
    // (0,0) "грязной" безусловно (см. `Grid::set_cell`) — этого достаточно,
    // чтобы следующий тик пересмотрел её.
    engine.grid_mut().set_cell(0, 0, Cell::new(UNRELATED));

    // Тик 3: (0,0) больше не входит в `matches` этого правила (тип не
    // совпадает) — запись ДОЛЖНА быть вычищена уже на ЭТОМ тике.
    engine.run_tick();
    assert!(
        engine.state.snapshot().memory_buffers().get(&(0, 0, 0)).is_none(),
        "запись ДОЛЖНА быть вычищена сразу же, как только позиция перестала матчиться — фикс осиротевших записей"
    );

    // Ещё несколько тиков — запись не должна воскреснуть сама по себе.
    for _ in 0..5 {
        engine.run_tick();
    }
    assert!(
        engine.state.snapshot().memory_buffers().get(&(0, 0, 0)).is_none(),
        "запись не должна появиться вновь сама по себе"
    );
    assert_eq!(
        engine.state.snapshot().memory_buffers().len(),
        0,
        "карта должна быть полностью пуста — никаких фантомных остатков"
    );
}

/// Точность момента чистки: запись НЕ должна пропасть раньше срока (пока
/// клетка ещё честно матчится — тики 1..3) и НЕ должна пережить дольше
/// одного тика после того, как перестала матчиться (ровно тик 4, не тик 5
/// или позже) — то есть не "слишком рано" и не "слишком поздно", а именно
/// на первом тике, где инкрементальный матчер физически МОГ это заметить
/// (см. doc-комментарий блока "осиротевшие записи" в `run_tick_with_cache`
/// про то, почему `search_coords` этого тика гарантированно включает эту
/// позицию).
#[test]
fn test_memory_buffer_entry_pruned_exactly_on_tick_match_stops_existing() {
    const WATCHER: u8 = 223;
    const NEIGH_OK: u8 = 224;
    const UNRELATED: u8 = 225;
    const NEVER_MATCHES: u8 = 250; // гейт никогда не откроется — тест не про арбитраж, только про сам факт трекинга

    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(WATCHER));
    grid.set_cell(1, 0, Cell::new(NEIGH_OK));

    let rule = Rule {
        id: vec![CellType(WATCHER)],
        pattern: vec![],
        shifts: vec![],
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
        memory: Some(MemorySpec {
            window: 1,
            record_trigger: RecordTrigger::NeighborType(Direction::Right),
            match_pattern: vec![RecordedValue::Type(CellType(NEVER_MATCHES))],
        }),
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Тики 1..3: клетка стабильно матчится (тип не меняется) — буфер
    // продолжает наблюдать независимо от гейта (см. `memory_targets`'s
    // doc-комментарий), запись НЕ должна быть тронута ни на одном из этих
    // тиков ("не слишком рано").
    for tick in 1..=3 {
        engine.run_tick();
        assert!(
            engine.state.snapshot().memory_buffers().contains_key(&(0, 0, 0)),
            "тик {tick}: клетка всё ещё матчится — запись НЕ должна быть удалена"
        );
    }

    // Внешнее событие ровно ПЕРЕД тиком 4.
    engine.grid_mut().set_cell(0, 0, Cell::new(UNRELATED));

    // Тик 4: первый тик, на котором инкрементальный матчер видит изменение —
    // запись ДОЛЖНА исчезнуть именно теперь ("не слишком поздно").
    engine.run_tick();
    assert!(
        engine.state.snapshot().memory_buffers().get(&(0, 0, 0)).is_none(),
        "тик 4: клетка перестала матчиться — запись должна быть вычищена ИМЕННО на этом тике"
    );

    // Тики 5..7: остаётся вычищенной.
    for tick in 5..=7 {
        engine.run_tick();
        assert!(
            engine.state.snapshot().memory_buffers().get(&(0, 0, 0)).is_none(),
            "тик {tick}: запись не должна вернуться сама по себе"
        );
    }
    assert_eq!(
        engine.state.snapshot().memory_buffers().len(),
        0,
        "карта должна быть полностью пуста"
    );
}

/// Тот же класс фикса, что и у `memory_buffers` (см.
/// `test_memory_buffer_entry_pruned_after_match_stops_existing`), но для
/// `Engine::feedback_counters` — доказывает, что дешёвая чистка
/// (`ExtensionFlags::extension_rule_indices`) действительно покрывает ОБЕ
/// карты, не только `memory_buffers`, ради которой была написана.
#[test]
fn test_feedback_counter_pruned_after_match_stops_existing() {
    const WATCHER: u8 = 226;
    const UNRELATED: u8 = 227;

    let mut grid = make_grid(3, 1);
    grid.set_cell(0, 0, Cell::new(WATCHER));

    let rule = Rule {
        id: vec![CellType(WATCHER)],
        pattern: vec![],
        // Без сдвигов/изменений вовсе — детекция (и, следовательно, рост
        // `feedback_counters`) не зависит от `shifts`/`changes`, только от
        // того, что паттерн продолжает совпадать (см. `feedback_keys`'s
        // построение в `run_tick_with_cache`: фильтр по `matches`, посчитан
        // ДО фазы применения).
        shifts: vec![],
        changes: vec![],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        // Заведомо недостижимый timeout — тест не про переключение
        // направления, а про сам факт накопления/чистки счётчика.
        feedback: Some(FeedbackSpec {
            timeout: 1000,
            new_direction: Direction::Up,
        }),
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    engine.run_tick();
    assert_eq!(
        engine.state.snapshot().feedback_counters().get(&(0, 0, 0)),
        Some(&1),
        "тик 1: счётчик должен вырасти на 1"
    );
    engine.run_tick();
    assert_eq!(
        engine.state.snapshot().feedback_counters().get(&(0, 0, 0)),
        Some(&2),
        "тик 2: счётчик продолжает расти"
    );

    // Внешнее событие: клетка перестаёт быть WATCHER.
    engine.grid_mut().set_cell(0, 0, Cell::new(UNRELATED));
    engine.run_tick();

    assert!(
        engine.state.snapshot().feedback_counters().get(&(0, 0, 0)).is_none(),
        "запись ДОЛЖНА быть вычищена ровно на тике, следующем за тем, когда клетка перестала матчиться — тот же фикс, что и у memory_buffers"
    );
    assert_eq!(
        engine.state.snapshot().feedback_counters().len(),
        0,
        "карта должна быть полностью пуста"
    );
}

// ──────────────────────────────────────────────────────────────
// Part B (аудит взаимодействий с `min_age`, по прецеденту
// `recursion`+`min_age` в `config.rs`): проверяем `memory`'s гейт-фильтр и
// `keep_source`'s пропуск переноса на СУЩЕСТВОВАНИЕ того же класса дыры —
// "клетка, ещё не созревшая до `min_age`, всё равно как-то участвует в
// расширении".
// ──────────────────────────────────────────────────────────────

/// `memory` + `min_age > 0`: незрелая клетка (age < min_age) не должна даже
/// ПОПАСТЬ в `Engine::memory_buffers` — гейт-фильтр `memory` работает НАД
/// списком `matches`, который матчер (`matcher::match_cell`) уже
/// отфильтровал по `min_age` ДО того, как `run_tick_with_cache` вообще
/// узнаёт о существовании этого матча (см. `memory_targets`'s построение:
/// `matches.iter().filter(...)`, где `matches` -- пост-min_age список). Это
/// белый ящик, реально проверяющий карту `Engine::memory_buffers`
/// напрямую, а не только конечное значение клетки -- если бы это было
/// нарушено (аналогично найденному багу `recursion`+`min_age`, где каскадные
/// уровни проверяли только ТИП, не возраст), буфер начал бы заполняться
/// РАНЬШЕ, чем клетка формально имеет право участвовать в арбитраже вообще.
#[test]
fn test_memory_gate_does_not_track_immature_cell_before_min_age() {
    const WATCHER: u8 = 220;
    const FIRED: u8 = 221;
    const NEIGH: u8 = 222;
    const THRESHOLD: u64 = 3;

    let mut grid = make_grid(3, 1);
    grid.set_cell(0, 0, Cell::new(WATCHER));
    grid.set_cell(1, 0, Cell::new(NEIGH)); // (0,0) + Right = (1,0), NeighborType target

    let rule = Rule {
        id: vec![CellType(WATCHER)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(FIRED))],
        active_only: false,
        priority: 10,
        min_age: THRESHOLD,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: Some(MemorySpec {
            window: 1,
            record_trigger: RecordTrigger::NeighborType(Direction::Right),
            match_pattern: vec![RecordedValue::Type(CellType(NEIGH))],
        }),
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Тики 1..=THRESHOLD: возраст клетки на момент проверки (ДО advance_age
    // этого же тика) -- 0, 1, ..., THRESHOLD-1, все < THRESHOLD. Матчер
    // (`match_cell`) обязан исключить такую клетку из `matches` целиком, так
    // что она физически не может попасть в `memory_targets` -- буфер должен
    // оставаться ПУСТЫМ (отсутствовать в карте) все эти тики.
    for tick in 1..=THRESHOLD {
        engine.run_tick();
        assert!(
            engine.state.snapshot().memory_buffers().get(&(0, 0, 0)).is_none(),
            "тик {tick}: незрелая клетка (age < min_age) НЕ должна быть memory-gate-tracked -- \
если бы была, буфер начал бы копиться раньше, чем клетке формально разрешено матчиться"
        );
        assert_eq!(
            engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
            Some(WATCHER),
            "тик {tick}: правило не должно сработать до созревания"
        );
    }

    // Тик THRESHOLD+1: возраст клетки == THRESHOLD теперь (созрела) -- ВПЕРВЫЕ
    // попадает в `matches`, а значит и в `memory_targets` -- буфер должен
    // начать заполняться РОВНО с этого тика, не раньше.
    engine.run_tick();
    assert_eq!(
        engine.state.snapshot().memory_buffers().get(&(0, 0, 0)),
        Some(&VecDeque::from(vec![RecordedValue::Type(CellType(NEIGH))])),
        "тик {}: клетка только что созрела (age == min_age) -- обязана начать отслеживаться именно теперь",
        THRESHOLD + 1
    );
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(WATCHER),
        "гейт всё ещё закрыт в этот тик (буфер проверяется ДО обновления этого же тика)"
    );

    // Тик THRESHOLD+2: буфер [Type(NEIGH)] (window=1, как он стал к концу
    // предыдущего тика) точно совпадает с match_pattern -> гейт открывается.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(FIRED),
        "гейт обязан открыться ровно на тик после того, как буфер созревшей клетки заполнился"
    );
}

/// `keep_source` + `min_age > 0`: источник ("излучение", `keep_source: true`)
/// физически не перемещается и не очищается, поэтому продолжает удовлетворять
/// `min_age` на каждом следующем тике без повторного "созревания" -- это
/// ожидаемо и уже задокументировано. Адверсариальная часть теста -- ДРУГАЯ
/// сторона того же взаимодействия: цель излучения получает СВЕЖУЮ копию
/// (born_at = текущее поколение) при КАЖДОМ срабатывании источника, поэтому
/// НИКОГДА не накапливает возраст, пока источник продолжает её перезаписывать
/// -- если бы `apply_shift_buffered`/флеш-фаза `apply_matches_with_cam`
/// когда-нибудь "просочили" старое `born_at` источника в цель (например, если
/// бы флеш перестал безусловно переустанавливать `born_at = gen` для каждой
/// записи из `write_buffer`), клетка-цель с тем же типом мгновенно
/// удовлетворяла бы `min_age`, унаследовав чужую историю -- ровно тот класс
/// тихой дыры, что и `recursion`+`min_age`.
#[test]
fn test_keep_source_emit_target_never_inherits_source_age_for_min_age() {
    const MARKER: u8 = 223;
    const THRESHOLD: u64 = 4;

    let mut grid = make_grid(5, 1);
    grid.set_cell(0, 0, Cell::new(MARKER));

    let rule = Rule {
        id: vec![CellType(MARKER)],
        pattern: vec![],
        // Точечное излучение: копия только в (1,0), источник (0,0) не трогается.
        shifts: vec![vec![ShiftSpec {
            direction: Direction::Right,
            steps: 1,
            broadcast: false,
            keep_source: true,
        }]],
        changes: vec![],
        active_only: false,
        priority: 10,
        min_age: THRESHOLD,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Тики 1..=THRESHOLD: источник ещё не созрел -- никакого излучения, цель
    // остаётся дефолтной.
    for tick in 1..=THRESHOLD {
        engine.run_tick();
        assert_eq!(
            engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
            Some(0),
            "тик {tick}: источник ещё не созрел до min_age -- излучения быть не должно"
        );
    }

    // Тик THRESHOLD+1: источник созрел (age == THRESHOLD) -- первое
    // излучение. Цель получает MARKER со свежим born_at (== текущее
    // поколение), значит её ВОЗРАСТ должен быть 0 сразу после этого тика --
    // НЕ унаследованный возраст источника (который к этому моменту зрелый).
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
        Some(MARKER),
        "тик {}: первое излучение должно было применить копию",
        THRESHOLD + 1
    );
    assert_eq!(
        engine.grid().get_age(1, 0),
        0,
        "цель излучения обязана иметь возраст 0 сразу после копирования -- born_at должен быть переустановлен на текущее поколение, а НЕ унаследован от зрелого источника"
    );
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(MARKER),
        "источник (keep_source) должен сохранить значение"
    );

    // Тик THRESHOLD+2: источник продолжает удовлетворять min_age (его
    // возраст никогда не сбрасывался -- keep_source не включает источник в
    // written_cells) и излучает СНОВА -- цель перезаписывается свежей копией
    // и её возраст остаётся 0, а НЕ растёт до 1 -- она никогда не "видит"
    // непрерывного течения времени, пока источник её каждый тик перезаписывает.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_age(1, 0),
        0,
        "цель продолжает получать свежие копии каждый тик -- её возраст обязан оставаться 0, не расти, пока источник её непрерывно перезаписывает"
    );
}

/// `recursion` + `min_age > 0` (ранее взаимоисключающая комбинация — см. её
/// историю в `config.rs`, теперь разрешена): каждый уровень каскада обязан
/// САМ проверить `min_age` против ЭФФЕКТИВНОГО (с учётом уже накопленного
/// `write_buffer`) возраста СВОЕЙ клетки-анкера `(ox, oy)`, а не только тип
/// её паттерна — см. `applicator::read_age_effective`.
///
/// Раскладка: seed (RFILLED) в x=0, затем RUNFILLED в x=1..9 с РАЗНЫМ
/// заранее выставленным `born_at` при generation=5:
///   x=1 born_at=0 → age=5 (обычный k=0 матч, проходит min_age=2)
///   x=2 born_at=0 → age=5 (уровень каскада k=1 — тоже старый, должен пройти)
///   x=3 born_at=4 → age=1 (уровень каскада k=2 — СЛИШКОМ МОЛОДОЙ: 1 < 2)
///   x=4 born_at=0 → age=5 (старый, но каскад обязан остановиться РАНЬШЕ —
///                          на x=3 — так что сюда очередь дойти не должна)
///
/// Тип клетки в x=3 сам по себе полностью совпадает с паттерном (RUNFILLED,
/// сосед слева после k=1 стал RFILLED) — без проверки возраста наивная
/// (только-типовая) версия `pattern_matches_effective` продолжила бы каскад
/// и залила бы x=3 (и, скорее всего, x=4 тоже, вплоть до `max_depth`).
/// Единственная причина, по которой каскад обязан остановиться именно на
/// x=3, — `min_age`, так что финальное состояние решётки однозначно
/// свидетельствует, сработала проверка возраста на уровне каскада или нет.
#[test]
fn test_recursion_with_min_age_blocks_cascade_at_too_young_cell() {
    const MIN_AGE: u64 = 2;
    const MAX_DEPTH: u8 = 5;

    let mut grid = make_grid(10, 1);
    // Продвигаем поколение решётки НАПРЯМУЮ (без реальных тиков — метод
    // ничего не трогает, кроме счётчика) до generation=5, чтобы можно было
    // детерминированно расставить born_at ячеек ниже generation и получить
    // заранее выбранный возраст (generation - born_at) для каждой из них.
    for _ in 0..5 {
        grid.advance_age();
    }

    grid.set_cell(0, 0, Cell::new(RFILLED));
    grid.set_cell(
        1,
        0,
        Cell {
            value: CellValue::new(RUNFILLED),
            born_at: 0,
        },
    ); // age 5
    grid.set_cell(
        2,
        0,
        Cell {
            value: CellValue::new(RUNFILLED),
            born_at: 0,
        },
    ); // age 5
    grid.set_cell(
        3,
        0,
        Cell {
            value: CellValue::new(RUNFILLED),
            born_at: 4,
        },
    ); // age 1 < MIN_AGE
    grid.set_cell(
        4,
        0,
        Cell {
            value: CellValue::new(RUNFILLED),
            born_at: 0,
        },
    ); // age 5, но недостижим
    for x in 5..10 {
        grid.set_cell(x, 0, Cell::new(RUNFILLED));
    }

    let rule = Rule {
        id: vec![CellType(RUNFILLED)],
        pattern: vec![(0, 0, CellType(RUNFILLED)), (-1, 0, CellType(RFILLED))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(RFILLED))],
        active_only: false,
        priority: 10,
        min_age: MIN_AGE,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: Some(RecursionSpec {
            max_depth: MAX_DEPTH,
            direction: Direction::Right,
        }),
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    engine.run_tick();

    // x=0 (seed) не участвует в паттерне как анкер — не проверяется на возраст, не меняется.
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(RFILLED),
        "seed x=0 не должен меняться"
    );
    // x=1: обычный (k=0) матч, age=5 >= min_age=2 — должен сработать.
    assert_eq!(
        engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
        Some(RFILLED),
        "x=1: обычный матч должен пройти min_age"
    );
    // x=2: уровень каскада k=1, age=5 >= min_age=2 — должен сработать.
    assert_eq!(
        engine.grid().get_cell(2, 0).map(|c| c.value.0 .0),
        Some(RFILLED),
        "x=2: k=1 каскад должен пройти min_age (старая клетка)"
    );
    // x=3: уровень каскада k=2, age=1 < min_age=2 — ДОЛЖЕН быть заблокирован, несмотря на совпадающий тип.
    assert_eq!(
        engine.grid().get_cell(3, 0).map(|c| c.value.0 .0),
        Some(RUNFILLED),
        "x=3: k=2 каскад ДОЛЖЕН остановиться здесь — клетка слишком молода (age=1 < min_age=2)"
    );
    // x=4..9: недостижимы — каскад уже остановился на x=3.
    for x in 4..10 {
        assert_eq!(
            engine.grid().get_cell(x, 0).map(|c| c.value.0 .0),
            Some(RUNFILLED),
            "x={x} не должен быть затронут — каскад остановился на x=3"
        );
    }
}

const MEM_RECUR_MARKER: u8 = 94;

/// `memory` (`NeighborType`) + `recursion` вместе — раньше запрещённая
/// комбинация (см. `config.rs`'s старую валидацию), теперь разрешена (тот
/// же приём, что уже сработал для `cam`+`recursion` и `recursion`+`min_age`
/// — найти обход блэнкет-запрета, а не оставить его как есть).
///
/// Ключевое структурное следствие проверено здесь конструктивно: у СВЕЖЕЙ
/// позиции (буфер ещё ни разу не наблюдался) гейт ВСЕГДА закрыт на первом
/// визите (проверка происходит ДО пуша — 0 записей никогда не равно
/// window), причём это касается и обычного (level 0) матча, и КАЖДОГО
/// уровня каскада одинаково — у `memory` нет отдельного "level 0 матчится
/// безусловно". Позиция, чей гейт закрыт, тем не менее ПОЛУЧАЕТ новое
/// наблюдение (буфер продолжает копить историю, даже пока гейт закрыт — та
/// же семантика, что и у обычного top-level матча) — и на СЛЕДУЮЩЕМ тике,
/// когда та же позиция снова оценивается (либо как level 0 нового тика,
/// либо как уровень каскада, либо как независимый top-level матч — не
/// важно, откуда именно), она использует ТОТ ЖЕ, уже частично заполненный
/// буфер. При `window=1` одного такого "прогревочного" тика достаточно,
/// чтобы гейт открылся на следующем шаге — что и даёт цепочке расти РОВНО
/// на одну клетку каждый тик, начиная со ВТОРОГО (первый тик целиком уходит
/// на прогрев самого level 0).
#[test]
fn test_memory_neighbor_type_plus_recursion_cascade_level_gate_primes_across_ticks() {
    let mut grid = make_grid(6, 2);
    const WALL_ROW: usize = 1;
    const WATCH_ROW: usize = 0;
    grid.set_cell(0, WALL_ROW, Cell::new(RFILLED)); // seed, статичен, без правил
    grid.set_cell(1, WALL_ROW, Cell::new(RUNFILLED)); // level 0
    grid.set_cell(2, WALL_ROW, Cell::new(RUNFILLED)); // будущий каскадный/top-level уровень
    grid.set_cell(3, WALL_ROW, Cell::new(RUNFILLED)); // будущий каскадный/top-level уровень
    for x in 1..4 {
        grid.set_cell(x, WATCH_ROW, Cell::new(MEM_RECUR_MARKER)); // статичные маркеры над всей цепочкой
    }

    let rule = Rule {
        id: vec![CellType(RUNFILLED)],
        pattern: vec![(0, 0, CellType(RUNFILLED)), (-1, 0, CellType(RFILLED))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(RFILLED))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: Some(RecursionSpec {
            max_depth: 1,
            direction: Direction::Right,
        }),
        memory: Some(MemorySpec {
            window: 1,
            record_trigger: RecordTrigger::NeighborType(Direction::Up),
            match_pattern: vec![RecordedValue::Type(CellType(MEM_RECUR_MARKER))],
        }),
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    // Тик 1 (прогрев level0): x=1 структурно матчится (behind=RFILLED с
    // самого начала), но её буфер памяти пуст -- гейт закрыт, она НЕ
    // выигрывает арбитраж вовсе (значит, и её каскад не запускается: Фаза 3
    // -- часть apply уже выигравшего матча). x=1 получает первое наблюдение.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(0, WALL_ROW).map(|c| c.value.0 .0),
        Some(RFILLED),
        "тик 1: seed не меняется"
    );
    assert_eq!(
        engine.grid().get_cell(1, WALL_ROW).map(|c| c.value.0 .0),
        Some(RUNFILLED),
        "тик 1: level0 НЕ должен сработать -- буфер памяти пуст на первом визите"
    );
    assert_eq!(
        engine.grid().get_cell(2, WALL_ROW).map(|c| c.value.0 .0),
        Some(RUNFILLED),
        "тик 1: x=2 недостижима -- x=1 не выиграла, каскада не было"
    );
    assert_eq!(
        engine.grid().get_cell(3, WALL_ROW).map(|c| c.value.0 .0),
        Some(RUNFILLED),
        "тик 1: x=3 недостижима"
    );

    // Тик 2: x=1 теперь имеет заполненный (с тика 1) буфер -- гейт открыт,
    // x=1 срабатывает. Её каскад пытается level1=x=2: pattern совпадает
    // (behind эффективно RFILLED из этого же каскада), но буфер x=2 ПУСТ на
    // первом визите -- гейт закрыт, каскад останавливается на x=2 (не
    // конвертируя её), x=2 получает первое наблюдение.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(1, WALL_ROW).map(|c| c.value.0 .0),
        Some(RFILLED),
        "тик 2: level0 обязан сработать -- буфер уже заполнен с тика 1"
    );
    assert_eq!(
        engine.grid().get_cell(2, WALL_ROW).map(|c| c.value.0 .0),
        Some(RUNFILLED),
        "тик 2: каскадный уровень x=2 НЕ должен сработать -- её буфер пуст на первом визите"
    );
    assert_eq!(
        engine.grid().get_cell(3, WALL_ROW).map(|c| c.value.0 .0),
        Some(RUNFILLED),
        "тик 2: x=3 недостижима"
    );

    // Тик 3: x=1 -- RFILLED, больше не матчится (её каскад из тика 2 тоже
    // не существует, она сама больше не RUNFILLED). x=2 (RUNFILLED,
    // behind=x=1=RFILLED) теперь НЕЗАВИСИМЫЙ top-level матч -- её буфер уже
    // заполнен с тика 2 -- гейт открыт через ОБЫЧНЫЙ (не каскадный)
    // memory-механизм -- x=2 срабатывает. Её собственный каскад (level1=x=3)
    // повторяет ту же историю: буфер x=3 пуст -- гейт закрыт, не
    // конвертируется, получает первое наблюдение.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(2, WALL_ROW).map(|c| c.value.0 .0),
        Some(RFILLED),
        "тик 3: x=2 обязана сработать через обычный top-level путь -- буфер уже заполнен с тика 2"
    );
    assert_eq!(
        engine.grid().get_cell(3, WALL_ROW).map(|c| c.value.0 .0),
        Some(RUNFILLED),
        "тик 3: x=3 (новый каскадный уровень от x=2) НЕ должна сработать -- её буфер пуст на первом визите"
    );

    // Тик 4: та же история повторяется на x=3 -- теперь она независимый
    // top-level матч (x=2 уже RFILLED) с уже заполненным с тика 3 буфером.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(3, WALL_ROW).map(|c| c.value.0 .0),
        Some(RFILLED),
        "тик 4: x=3 обязана сработать -- её буфер заполнен с тика 3"
    );
}
