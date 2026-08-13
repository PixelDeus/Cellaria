use super::super::*;
use super::common::*;
use crate::types::{CamSearch, CellType, Direction, FeedbackSpec, Rule, ShiftSpec};
use std::collections::HashSet;

/// Тот же класс стресс-теста, что нашёл реальный баг boundary-vs-core для
/// `recursion` (см. CHANGELOG `[0.7.0] / Fixed`, `property_arbitration.rs`'s
/// `test_arbitrate_spatial_matches_centralized_recursion_dense_overlapping_writes`),
/// теперь для `cam` — единственного другого расширения с affected-регионом,
/// физически способным дотянуться дальше одной клетки от анкора.
/// `CamPositions` — `pub(crate)`, недоступен из `tests/` (внешние
/// интеграционные тесты) — поэтому здесь, не в `property_arbitration.rs`.
///
/// `cam`, БЕЗ `recursion`, использует ТОЧНЫЙ (не консервативный disk)
/// affected-регион — `[found, magnet]`, ровно 2 клетки (см.
/// `get_match_affected_cells`'s doc-комментарий) — принципиально другой
/// путь вычисления, чем у `recursion` (union дисков всех уровней,
/// консервативный `write_cells`). `reach` для band-margin по-прежнему
/// берётся из `RuleData::bbox`, построенного из КОНСЕРВАТИВНОГО
/// `cam_disc_cells(radius)` — теоретически должен оставаться корректной
/// верхней границей для точного `found`, раз поиск физически не может
/// найти цель дальше `radius` от анкора, но это НЕ проверялось эмпирически
/// ни разу до этого теста.
///
/// `radius=1`, `cam_positions` подставлены вручную (не через реальный
/// поиск по решётке) так, что anchor `x`'s найденная цель == anchor
/// `x+1`'s собственная позиция — гарантированная, точная 2-клеточная
/// коллизия для КАЖДОЙ соседней пары анкоров, той же плотности, что и
/// recursion-репро (что и должно максимально стрессировать границы полос).
#[test]
fn test_arbitrate_spatial_matches_centralized_cam_dense_overlapping_writes() {
    let rule = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: Some(CamSearch {
            radius: 1,
            target_type: CellType(2),
        }),
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let rule_index = make_rule_index(vec![rule]);
    let rule_cache = crate::conflict_analyzer::build_rule_data_cache(&rule_index);

    let reach: i32 = rule_cache
        .iter()
        .filter_map(|opt| opt.as_ref())
        .flat_map(|rules| rules.iter())
        .map(|data| {
            let (min_x, max_x, min_y, max_y) = data.bbox;
            min_x
                .unsigned_abs()
                .max(max_x.unsigned_abs())
                .max(min_y.unsigned_abs())
                .max(max_y.unsigned_abs()) as i32
        })
        .max()
        .unwrap_or(0);
    assert_eq!(reach, 1, "cam radius=1, без recursion -- reach обязан быть ровно 1");

    // >SPATIAL_THRESHOLD=4096 -- иначе arbitrate_spatial_with_cam падает
    // сразу в centralized fallback и весь тест становится вакуумным
    // (найдено экспериментально: 3000 анкоров × 1 rule_idx = 3000 < 4096,
    // тест проходил, но band-split вообще не запускался).
    const N_ANCHORS: u32 = 4500;
    let mut matches: Vec<RuleMatch> = Vec::new();
    let mut cam_positions: crate::engine::matcher::CamPositions = Default::default();
    for x in 0..N_ANCHORS {
        let m = RuleMatch {
            x,
            y: 0,
            head: CellType(1),
            rule_idx: 0,
        };
        // Anchor x находит цель РОВНО на позиции anchor x+1 -- гарантированная
        // 2-клеточная коллизия с соседом (found=x+1 совпадает с anchor(x+1)'s
        // собственной клеткой), не зависящая от реального содержимого решётки.
        cam_positions.insert((m.x, m.y, m.rule_idx), (x + 1, 0));
        matches.push(m);
    }

    let get_age = |x: usize, _y: usize| (x as u32).wrapping_mul(2654435761) % 7;

    let (centralized, _) = arbitrate_with_cam(
        matches.clone(),
        &rule_index,
        &rule_cache,
        (usize::MAX, usize::MAX),
        &cam_positions,
        0,
        &Default::default(),
        &Default::default(),
        &[],
        get_age,
    );
    let (spatial, _) = arbitrate_spatial_with_cam(
        matches,
        &rule_index,
        &rule_cache,
        (usize::MAX, usize::MAX),
        reach,
        &cam_positions,
        0,
        &Default::default(),
        &Default::default(),
        get_age,
    );

    let centralized_set: HashSet<(u32, u32, usize)> = centralized.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();
    let spatial_set: HashSet<(u32, u32, usize)> = spatial.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();

    assert_eq!(
        centralized.len(),
        spatial.len(),
        "разное число принятых матчей: {} vs {}",
        centralized.len(),
        spatial.len()
    );
    assert_eq!(centralized_set, spatial_set, "плотная цепочка cam-матчей (found соседа == anchor соседа) не должна расходиться с централизованным арбитражем");
}

/// Тот же класс стресс-теста для `feedback` — единственное расширение с
/// СОБСТВЕННОЙ (не через общий `rule_data.write_cells`) веткой в
/// `get_match_affected_cells` (см. её doc-комментарий): точные
/// `feedback_normal_write_cells`/`feedback_alt_write_cells`, выбираемые по
/// состоянию `FeedbackCounters` (защёлкнулся или нет), а не консервативный
/// union. `reach`/`bbox` строятся из UNION обоих направлений
/// (`compute_rule_data`, `conflict_analyzer.rs:483`) — теоретически
/// корректная верхняя граница для КАЖДОГО из точных направлений по
/// отдельности, но это не проверялось эмпирически на плотном масштабе ни
/// разу. `FeedbackCounters` — `pub(crate)`, тест внутренний.
#[test]
fn test_arbitrate_spatial_matches_centralized_feedback_dense_overlapping_writes() {
    let rule = Rule {
        id: vec![CellType(1)],
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
            timeout: 5,
            new_direction: Direction::Down,
        }),
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let rule_index = make_rule_index(vec![rule]);
    let rule_cache = crate::conflict_analyzer::build_rule_data_cache(&rule_index);

    let reach: i32 = rule_cache
        .iter()
        .filter_map(|opt| opt.as_ref())
        .flat_map(|rules| rules.iter())
        .map(|data| {
            let (min_x, max_x, min_y, max_y) = data.bbox;
            min_x
                .unsigned_abs()
                .max(max_x.unsigned_abs())
                .max(min_y.unsigned_abs())
                .max(max_y.unsigned_abs()) as i32
        })
        .max()
        .unwrap_or(0);
    assert_eq!(
        reach, 1,
        "declared Right + alt Down, оба на 1 клетку -- reach обязан быть ровно 1"
    );

    // >SPATIAL_THRESHOLD=4096. Все матчи НЕ защёлкнуты (feedback_counters
    // пуст, ниже timeout=5) -- используют "нормальное" направление (Right),
    // ту же плотную геометрию, что и обычный сдвиг, но идущую через
    // ОТДЕЛЬНУЮ feedback-ветку get_match_affected_cells, не общий путь.
    const N_ANCHORS: u32 = 4500;
    let mut matches: Vec<RuleMatch> = Vec::new();
    for x in 0..N_ANCHORS {
        matches.push(RuleMatch {
            x,
            y: 0,
            head: CellType(1),
            rule_idx: 0,
        });
    }

    let get_age = |x: usize, _y: usize| (x as u32).wrapping_mul(2654435761) % 7;
    let feedback_counters: crate::engine::arbitrator::FeedbackCounters = Default::default();

    let (centralized, _) = arbitrate_with_cam(
        matches.clone(),
        &rule_index,
        &rule_cache,
        (usize::MAX, usize::MAX),
        &Default::default(),
        0,
        &Default::default(),
        &feedback_counters,
        &[],
        get_age,
    );
    let (spatial, _) = arbitrate_spatial_with_cam(
        matches,
        &rule_index,
        &rule_cache,
        (usize::MAX, usize::MAX),
        reach,
        &Default::default(),
        0,
        &Default::default(),
        &feedback_counters,
        get_age,
    );

    let centralized_set: HashSet<(u32, u32, usize)> = centralized.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();
    let spatial_set: HashSet<(u32, u32, usize)> = spatial.iter().map(|m| (m.x, m.y, m.rule_idx)).collect();

    assert_eq!(
        centralized.len(),
        spatial.len(),
        "разное число принятых матчей: {} vs {}",
        centralized.len(),
        spatial.len()
    );
    assert_eq!(
        centralized_set, spatial_set,
        "плотная упаковка feedback-матчей (не защёлкнуты) не должна расходиться с централизованным арбитражем"
    );
}
