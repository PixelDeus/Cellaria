use super::super::*;
use super::common::*;
use crate::types::{CamSearch, Cell, CellType, Direction, RecursionSpec, Rule};

const MAGNET: u8 = 40;
const TARGET: u8 = 41;

fn magnet_rule(radius: u8, priority: u32) -> Rule {
    Rule {
        id: vec![CellType(MAGNET)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![],
        active_only: false,
        priority,
        min_age: 0,
        overflow: Default::default(),
        cam: Some(CamSearch {
            radius,
            target_type: CellType(TARGET),
        }),
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    }
}

/// Одиночный магнит без конфликтов: находит ближайшую цель в радиусе,
/// притягивает её — цель очищается, магнит становится типом цели.
#[test]
fn test_cam_single_magnet_pulls_nearest_target() {
    let mut grid = make_grid(10, 1);
    grid.set_cell(0, 0, Cell::new(MAGNET));
    grid.set_cell(4, 0, Cell::new(TARGET));
    let ri = make_rule_index(vec![magnet_rule(5, 0)]);
    let mut engine = Engine::new(grid, ri);

    engine.run_tick();

    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(TARGET),
        "magnet must become the target type"
    );
    assert_eq!(
        engine.grid().get_cell(4, 0).map(|c| c.value.0 .0),
        Some(0),
        "found cell must be cleared to default"
    );
}

/// Цель вне радиуса — магнит не находит ничего, остаётся собой.
#[test]
fn test_cam_target_outside_radius_no_match() {
    let mut grid = make_grid(10, 1);
    grid.set_cell(0, 0, Cell::new(MAGNET));
    grid.set_cell(6, 0, Cell::new(TARGET));
    let ri = make_rule_index(vec![magnet_rule(5, 0)]);
    let mut engine = Engine::new(grid, ri);

    engine.run_tick();

    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(MAGNET),
        "magnet unchanged: target out of reach"
    );
    assert_eq!(
        engine.grid().get_cell(6, 0).map(|c| c.value.0 .0),
        Some(TARGET),
        "target untouched"
    );
}

/// Два магнита претендуют на ОДНУ цель (обе в радиусе обоих) — арбитраж по
/// priority решает all-or-nothing, ровно как и для обычных сдвигов
/// (см. `test_gpu_engine_arbitrated_write_conflict_all_or_nothing`'s
/// CPU-аналог этого же принципа).
#[test]
fn test_cam_two_magnets_conflict_resolved_by_priority() {
    let mut grid = make_grid(10, 1);
    grid.set_cell(1, 0, Cell::new(MAGNET)); // низкий priority
    grid.set_cell(8, 0, Cell::new(MAGNET)); // высокий priority — должен выиграть
    grid.set_cell(4, 0, Cell::new(TARGET));

    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    idx.insert(CellType(MAGNET), vec![magnet_rule(5, 9), magnet_rule(5, 1)]);
    // rule_idx=0 (priority=9) и rule_idx=1 (priority=1) — арбитраж должен
    // предпочесть rule_idx=0 независимо от того, какая клетка его выбрала;
    // здесь обе клетки могут матчить ОБА rule_idx одной головы MAGNET, так
    // что тай-брейк реально решает priority самого правила, не позицию.
    let mut engine = Engine::new(grid, idx);

    engine.run_tick();

    let winner_at_1 = engine.grid().get_cell(1, 0).map(|c| c.value.0 .0) == Some(TARGET);
    let winner_at_8 = engine.grid().get_cell(8, 0).map(|c| c.value.0 .0) == Some(TARGET);
    assert!(
        winner_at_1 ^ winner_at_8,
        "ровно один магнит должен выиграть цель (all-or-nothing)"
    );
    assert_eq!(
        engine.grid().get_cell(4, 0).map(|c| c.value.0 .0),
        Some(0),
        "цель в любом случае забрана"
    );
}

/// Несколько тиков подряд: магнит без цели рядом остаётся собой сколько
/// угодно тиков, затем "видит" цель, как только она появляется в радиусе —
/// проверяет, что `max_pattern_radius`/dirty-tracking корректно расширяет
/// кандидатов на CAM-радиус (см. её doc-комментарий в `engine/mod.rs`),
/// а не только на радиус обычных паттернов.
#[test]
fn test_cam_detects_target_appearing_later_without_touching_magnet() {
    let mut grid = make_grid(10, 1);
    grid.set_cell(0, 0, Cell::new(MAGNET));
    let ri = make_rule_index(vec![magnet_rule(5, 0)]);
    let mut engine = Engine::new(grid.clone(), ri.clone());

    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(MAGNET),
        "no target yet"
    );

    // Цель появляется на 3-м тике — НЕ трогая сам магнит, только записывая
    // клетку TARGET напрямую в решётку (имитирует "что-то ещё" появившееся
    // рядом, не связанное с магнитом).
    engine.grid_mut().set_cell(4, 0, Cell::new(TARGET));
    engine.run_tick();

    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(TARGET),
        "magnet must detect the newly-appeared target even though the magnet cell itself never changed"
    );
}

// ──────────────────────────────────────────────────────────────
// `cam` + `recursion` (каскад независимых магнитов вдоль
// `recursion.direction`, см. `applicator::apply_cam_buffered`'s
// doc-комментарий и `conflict_analyzer::compute_rule_data`'s Corollary D)
// ──────────────────────────────────────────────────────────────

const MAGNET_A: u8 = 42;
const MAGNET_B: u8 = 43;

/// Как `magnet_rule`, но с настраиваемым типом головы — нужен, чтобы A и B
/// в тесте на коллизию каскадов были РАЗНЫМИ типами клеток (иначе одно
/// правило CAM сопоставилось бы с обоими магнитами и их нельзя было бы
/// независимо адресовать).
fn magnet_rule_typed(id_type: u8, radius: u8, priority: u32) -> Rule {
    Rule {
        id: vec![CellType(id_type)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![],
        active_only: false,
        priority,
        min_age: 0,
        overflow: Default::default(),
        cam: Some(CamSearch {
            radius,
            target_type: CellType(TARGET),
        }),
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    }
}

fn magnet_recursion_rule(id_type: u8, radius: u8, priority: u32, direction: Direction, max_depth: u8) -> Rule {
    Rule {
        id: vec![CellType(id_type)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![],
        active_only: false,
        priority,
        min_age: 0,
        overflow: Default::default(),
        cam: Some(CamSearch {
            radius,
            target_type: CellType(TARGET),
        }),
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: Some(RecursionSpec { max_depth, direction }),
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    }
}

/// Уровень 0 притягивает свою цель как обычный CAM, затем каскад
/// продолжается на уровень 1 — НЕЗАВИСИМЫЙ магнит на позиции
/// `level0.magnet + direction`.
///
/// Раскладка нарочно НЕ даёт клетке продолжения (x=3) собственной
/// достижимой pre-tick цели (её единственная реальная цель, x=0, лежит
/// ВНЕ её радиуса, хотя и внутри радиуса магнита уровня 0 на x=2) — иначе
/// x=3 сама стала бы НЕЗАВИСИМЫМ top-level CAM-матчем (detect_cam_matches
/// видит КАЖДУЮ клетку типа `id[0]` независимо от каскада), что превратило
/// бы тест в конкуренцию за арбитраж между двумя матчами одного правила,
/// а не в чистую демонстрацию каскада. Вместо этого уровень 1 находит
/// СВОЮ цель ТОЛЬКО через эффективное чтение — саму клетку магнита
/// уровня 0, которая стала TARGET уже В ЭТОМ ТИКЕ (см. `apply_cam_buffered`'s
/// doc-комментарий про `read_cell_effective`/`search_nearest_effective`) —
/// pre-tick она была MAGNET, так что top-level детект её не видел вообще.
/// Итог: x=2 транзитно становится TARGET (уровень 0), затем тут же
/// повторно потребляется уровнем 1 и возвращается в default — наблюдаемый
/// финальный результат для x=2 такой же, как если бы там никогда ничего
/// не произошло, а x=3 (не x=2!) несёт финальное свидетельство каскада.
#[test]
fn test_cam_recursion_cascades_independent_magnets_along_direction() {
    let mut grid = make_grid(6, 1);
    grid.set_cell(2, 0, Cell::new(MAGNET)); // магнит уровня 0
    grid.set_cell(0, 0, Cell::new(TARGET)); // цель уровня 0 (dist 2, вне радиуса продолжения x=3)
    grid.set_cell(3, 0, Cell::new(MAGNET)); // клетка продолжения каскада (магнит уровня 1)
    let ri = make_rule_index(vec![magnet_recursion_rule(MAGNET, 2, 0, Direction::Right, 1)]);
    let mut engine = Engine::new(grid, ri);

    engine.run_tick();

    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(0),
        "level-0 found cell cleared"
    );
    assert_eq!(
        engine.grid().get_cell(2, 0).map(|c| c.value.0 .0),
        Some(0),
        "level-0 magnet transiently becomes target, then is itself consumed by level-1's effective-read search"
    );
    assert_eq!(
        engine.grid().get_cell(3, 0).map(|c| c.value.0 .0),
        Some(TARGET),
        "level-1 cascade magnet becomes target, having found level-0's own just-written cell via effective read"
    );
}

/// Регрессионный тест на реальный баг, найденный при мёрже cam+recursion:
/// `arbitrator::get_match_affected_cells`'s точный (exact-cells) путь для
/// CAM-матчей возвращал ТОЛЬКО [found, magnet] уровня 0, игнорируя каскад
/// уровней 1..=max_depth — так что cam+recursion матч B, чей диск уровня 0
/// не пересекается с обычным (не-recursion) CAM-матчем A, но чей каскад
/// уровня 1 СХОДИТСЯ на клетке, которую A тоже хочет, ошибочно считался
/// НЕ конфликтующим с A и применялся без арбитража — тихая порча
/// состояния (двойная запись одной клетки в общий write_buffer) мимо
/// системы приоритетов.
///
/// Раскладка (1D, radius=5 у обоих правил): magnetB (MAGNET_B, priority 5,
/// direction Right) на x=0 находит СВОЮ ближайшую цель targetB0 на x=2
/// (dist 2) — единственную реальную цель в его собственном радиусе
/// (x∈[-5,5], клетка C на x=6 вне досягаемости). Каскад продолжается
/// магнитом на x=1: его эффективный поиск (после того как уровень 0 уже
/// потребил targetB0) достигает x=6 (dist 5) — клетки `C`, единственной
/// оставшейся цели в его радиусе x∈[-4,6]. magnetA (MAGNET_A, priority 10,
/// обычный CAM БЕЗ recursion) на x=8 находит ТУ ЖЕ клетку C на x=6
/// (dist 2, единственная цель в его радиусе x∈[3,13]) — прямой конфликт
/// с B каскадом на уровне 1, притом что уровень-0 диски A (вокруг x=8) и
/// B (вокруг x=0) вообще не пересекаются (расстояние 8 при радиусе 5) —
/// старый баг видел бы только это и признал бы A и B независимыми.
///
/// Ожидание: приоритет решает конфликт ЦЕЛОГО матча B — A (выше приоритет)
/// применяется, B не применяется ВООБЩЕ (ни уровень 0, ни продолжение),
/// включая его формально бесконфликтную находку targetB0 — целостность
/// матча важнее локальности конфликта.
#[test]
fn test_cam_recursion_cascade_collision_resolved_by_priority_not_silently_corrupted() {
    let mut grid = make_grid(14, 1);
    grid.set_cell(0, 0, Cell::new(MAGNET_B));
    grid.set_cell(2, 0, Cell::new(TARGET)); // targetB0
    grid.set_cell(1, 0, Cell::new(MAGNET_B)); // продолжение каскада B

    grid.set_cell(8, 0, Cell::new(MAGNET_A));
    grid.set_cell(6, 0, Cell::new(TARGET)); // C — единственная цель, достижимая И A, И каскадом B

    let ri = make_rule_index(vec![
        magnet_rule_typed(MAGNET_A, 5, 10),
        magnet_recursion_rule(MAGNET_B, 5, 5, Direction::Right, 1),
    ]);
    let mut engine = Engine::new(grid, ri);

    engine.run_tick();

    // A выигрывает арбитраж — его единственный матч применился.
    assert_eq!(
        engine.grid().get_cell(8, 0).map(|c| c.value.0 .0),
        Some(TARGET),
        "A magnet must become target"
    );
    assert_eq!(
        engine.grid().get_cell(6, 0).map(|c| c.value.0 .0),
        Some(0),
        "shared cell C must be claimed by A, cleared"
    );

    // B проигрывает арбитраж ЦЕЛИКОМ — ничего из B не применилось, включая
    // уровень 0 (targetB0), который сам по себе ни с кем не конфликтовал.
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(MAGNET_B),
        "B must not apply at all: level-0 magnet unchanged"
    );
    assert_eq!(
        engine.grid().get_cell(2, 0).map(|c| c.value.0 .0),
        Some(TARGET),
        "B must not apply at all: level-0 target untouched"
    );
    assert_eq!(
        engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
        Some(MAGNET_B),
        "B must not apply at all: level-1 cascade magnet unchanged"
    );
}

// ──────────────────────────────────────────────────────────────
// Broadcast-сдвиг (`ShiftSpec::broadcast`)
// ──────────────────────────────────────────────────────────────
