//! Эксперимент: превращаем ДОКАЗАННОЕ свойство (локальный арбитраж = глобальному,
//! см. tests/local_arbitration_research.rs) в РЕАЛЬНОЕ многопоточное исполнение,
//! а не просто теорему в тестовом коде.
//!
//! Идея (та же, что в domain decomposition в научных вычислениях): решётка
//! режется на N полос. Совпадение, чей "радиус записи" целиком укладывается
//! внутри своей полосы (не ближе `MARGIN` к её краю), НИКОГДА не может
//! конфликтовать ни с чем из другой полосы — это гарантируется геометрией,
//! а не верой. Такие совпадения каждый воркер арбитрирует полностью
//! независимо, в отдельном потоке. Совпадения у самых границ полос (их
//! немного) сверяются отдельно, последовательно, в конце.
//!
//! Проверка: результат многопоточного decentralized-арбитража побитово
//! сравнивается с результатом обычного централизованного `arbitrate()` на
//! ВСЕХ совпадениях сразу. Если они совпадают — доказанная теорема реально
//! работает как инфраструктура, а не только на бумаге. Если нет — эксперимент
//! честно это покажет, а не спрячет.
//!
//! Сценарий с РЕАЛЬНЫМИ конфликтами (не Game of Life, где каждая клетка
//! пишет только в себя — там конфликтов вообще нет): чередующиеся "R" и "L"
//! маркеры, R едет вправо, L — влево. Каждая соседняя пара (R,L) хочет
//! записать в ОБЕ свои клетки одновременно — настоящий конфликт, который
//! арбитраж обязан разрешить (побеждает по приоритету/тай-брейку).

use std::collections::HashMap;
use std::thread;
use std::time::Instant;

use cellaria::conflict_analyzer::build_rule_data_cache;
use cellaria::engine::{arbitrate, detect_matches};
use cellaria::types::{Cell, CellType, CellValue, Direction, Rule, RuleMatch, ShiftSpec};
use cellaria::{Grid, VecStorage};

const R_MOVER: u8 = 1;
const L_MOVER: u8 = 2;

fn build_rules() -> Vec<Rule> {
    vec![
        Rule {
            id: vec![CellType(R_MOVER)], pattern: vec![],
            shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]], changes: vec![],
            active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
        },
        Rule {
            id: vec![CellType(L_MOVER)], pattern: vec![],
            shifts: vec![vec![ShiftSpec::new(Direction::Left, 1)]], changes: vec![],
            active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None,
        },
    ]
}

fn build_grid(n: usize) -> Grid<VecStorage> {
    let storage = VecStorage::new(n, 1);
    let mut grid = Grid::new(storage, Default::default());
    for x in 0..n {
        let t = if x % 2 == 0 { R_MOVER } else { L_MOVER };
        grid.set_cell(x, 0, Cell { value: CellValue(CellType(t)), born_at: 0 });
    }
    grid
}

fn sorted_key(m: &RuleMatch) -> (u32, u32, u8, usize) {
    (m.x, m.y, m.head.0, m.rule_idx)
}

fn main() {
    let n = 4_000_000usize;
    let num_workers = 8usize;
    // Каждый конфликт строго локален паре соседних клеток (R@2k, L@2k+1) —
    // ни одно правило не дотягивается дальше чем на 1 клетку. MARGIN=4
    // (чётный — чтобы порог никогда не резал пару R,L пополам) даёт
    // комфортный запас прочности сверх реально нужного минимума.
    const MARGIN: usize = 4;

    let grid = build_grid(n);
    let rules = build_rules();
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for r in rules {
        rule_index.entry(r.id[0]).or_default().push(r);
    }
    let rule_cache = build_rule_data_cache(&rule_index);
    let bounds = (grid.width(), grid.height());

    let active: Vec<(usize, usize)> = (0..n).map(|x| (x, 0)).collect();
    let matches = detect_matches(&grid, &rule_index, &active);
    println!("Решётка: {} клеток, совпадений: {}\n", n, matches.len());

    // ── (a) Централизованный арбитраж — весь список сразу, один поток ──
    let t0 = Instant::now();
    let mut accepted_central = arbitrate(matches.clone(), &rule_index, &rule_cache, bounds, |_, _| 0);
    let central_time = t0.elapsed();

    // ── (b) Децентрализованный: делим по полосам, каждую — в свой поток ──
    let band_width = n / num_workers;
    let mut core_by_band: Vec<Vec<RuleMatch>> = vec![Vec::new(); num_workers];
    let mut boundary: Vec<RuleMatch> = Vec::new();

    for &m in &matches {
        let x = m.x as usize;
        let band = (x / band_width).min(num_workers - 1);
        let band_start = band * band_width;
        let band_end = if band == num_workers - 1 { n } else { (band + 1) * band_width };
        let is_boundary = x < band_start + MARGIN || x + MARGIN >= band_end;
        if is_boundary {
            boundary.push(m);
        } else {
            core_by_band[band].push(m);
        }
    }
    let boundary_frac = 100.0 * boundary.len() as f64 / matches.len() as f64;

    let t1 = Instant::now();
    let core_accepted: Vec<Vec<RuleMatch>> = thread::scope(|s| {
        let handles: Vec<_> = core_by_band
            .into_iter()
            .map(|band_matches| {
                let rule_index = &rule_index;
                let rule_cache = &rule_cache;
                s.spawn(move || arbitrate(band_matches, rule_index, rule_cache, bounds, |_, _| 0))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let boundary_accepted = arbitrate(boundary, &rule_index, &rule_cache, bounds, |_, _| 0);
    let decentralized_time = t1.elapsed();

    let mut accepted_decentralized: Vec<RuleMatch> = core_accepted.into_iter().flatten().collect();
    accepted_decentralized.extend(boundary_accepted);

    // ── Сверка: побитовое совпадение множеств принятых matches ──
    accepted_central.sort_by_key(sorted_key);
    accepted_decentralized.sort_by_key(sorted_key);
    let identical = accepted_central == accepted_decentralized;

    println!("Централизованный арбитраж (1 поток):     {:>10?}, принято {}", central_time, accepted_central.len());
    println!("Децентрализованный ({} потоков):          {:>10?}, принято {}", num_workers, decentralized_time, accepted_decentralized.len());
    println!("Доля матчей на границах полос (последовательно): {:.2}%", boundary_frac);
    println!("Ускорение: {:.2}x", central_time.as_secs_f64() / decentralized_time.as_secs_f64());
    println!(
        "\nРезультаты ПОБИТОВО ИДЕНТИЧНЫ централизованному арбитражу: {}",
        if identical { "ДА" } else { "НЕТ — расхождение!" }
    );
}
