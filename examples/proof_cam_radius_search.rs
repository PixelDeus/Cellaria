//! Доказывает теоретическую состоятельность "content-addressable memory
//! с ограниченным радиусом" (CAM-R) как расширения модели Cellaria —
//! БЕЗ проводки через реальный движок (см. doc-обсуждение: полная проводка
//! требует правок в `Rule`/`matcher.rs`/`conflict_analyzer.rs`/`arbitrator.rs`,
//! это отдельная, большая задача; здесь — только вопрос "совместимо ли это
//! с моделью в принципе", не "как это встроить").
//!
//! Идея: правило-"магнит" ищет ближайшую клетку типа X в радиусе R (не по
//! всей решётке) и притягивает её к себе. Неограниченный поиск ломает
//! главное свойство модели — статическую разрешимость конфликтов (affected
//! region зависит от содержимого решётки в рантайме, не от координат
//! правила). Ограничение радиусом R восстанавливает разрешимость: affected
//! region ограничен диском радиуса R вокруг магнита — тем же приёмом, что
//! уже использует `arbitrate_spatial`'s bucket-hashing (`max_radius`).
//!
//! Три независимые проверки:
//!
//! 1. **Поиск действительно ограничен радиусом.** Найденная клетка (если
//!    найдена) всегда лежит внутри диска R вокруг магнита — по построению
//!    поиска, но проверяем эмпирически на случайных решётках, а не
//!    полагаемся на "должно быть так".
//!
//! 2. **Консервативная граница графа конфликтов состоятельна (sound).**
//!    "Магниты", чьи диски радиуса R НЕ пересекаются (расстояние между
//!    центрами > 2R), не могут конфликтовать НИ ПРИ КАКОМ содержимом
//!    решётки — проверяем на множестве случайных решёток: если граница
//!    сказала "не пересекаются", реальные affected-cells и правда никогда
//!    не пересекаются. Это именно то свойство, которое требуется от графа
//!    конфликтов для его использования в `spatial_bypass_split`-подобной
//!    оптимизации.
//!
//! 3. **Арбитраж по КОНКРЕТНОЙ найденной позиции работает как обычный
//!    сдвиг.** Один раз позиция найдена на реальном состоянии решётки —
//!    дальше это ничем не отличается от `arbitrator::arbitrate`: два
//!    магнита, реально претендующих на одну и ту же клетку, разрешаются тем
//!    же тай-брейком (priority → age → id → x → y), all-or-nothing.

const RADIUS: i32 = 3;
const GRID_W: i32 = 40;
const GRID_H: i32 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pos {
    x: i32,
    y: i32,
}

#[derive(Debug, Clone, Copy)]
struct Magnet {
    pos: Pos,
    radius: i32,
    priority: u32,
    age: u32,
    id: u32,
}

/// Ближайшая клетка типа X в радиусе `magnet.radius` вокруг магнита —
/// линейный скан диска (Chebyshev-расстояние, как offset'ы паттерна в
/// реальном движке), детерминированный тай-брейк по (y, x) при равном
/// расстоянии — не зависит от порядка обхода `targets`.
fn search_nearest(magnet: &Magnet, targets: &[Pos]) -> Option<Pos> {
    targets
        .iter()
        .filter(|t| (t.x - magnet.pos.x).abs() <= magnet.radius && (t.y - magnet.pos.y).abs() <= magnet.radius)
        .copied()
        .min_by_key(|t| {
            let dist = (t.x - magnet.pos.x).abs().max((t.y - magnet.pos.y).abs());
            (dist, t.y, t.x)
        })
}

/// Консервативная СТАТИЧЕСКАЯ граница для графа конфликтов: два магнита
/// МОГУТ конфликтовать, только если их диски радиуса R пересекаются — это
/// единственное, что известно ДО просмотра содержимого решётки (сама
/// найденная клетка зависит от рантайм-состояния).
fn discs_could_conflict(a: &Magnet, b: &Magnet) -> bool {
    let dx = (a.pos.x - b.pos.x).abs();
    let dy = (a.pos.y - b.pos.y).abs();
    dx <= a.radius + b.radius && dy <= a.radius + b.radius
}

/// Реальные affected-cells КОНКРЕТНОГО матча на КОНКРЕТНОЙ решётке —
/// источник (найденная клетка) и цель (сама позиция магнита), как обычный
/// сдвиг с динамически определённым источником.
fn actual_affected_cells(magnet: &Magnet, targets: &[Pos]) -> Option<[Pos; 2]> {
    search_nearest(magnet, targets).map(|found| [found, magnet.pos])
}

fn cells_overlap(a: &[Pos; 2], b: &[Pos; 2]) -> bool {
    a.iter().any(|p| b.contains(p))
}

/// Тай-брейк ровно как `arbitrator::arbitrate`: priority → age → id → x → y
/// (rule_idx опущен — тут одно "правило" на магнит, различаются только id).
fn magnet_is_better(a: &Magnet, b: &Magnet) -> bool {
    (a.priority, a.age, a.id, a.pos.y, a.pos.x) > (b.priority, b.age, b.id, b.pos.y, b.pos.x)
}

/// Жадный арбитраж: как `arbitrator::arbitrate` — сортируем по тай-брейку,
/// принимаем непересекающиеся по affected-cells, all-or-nothing.
fn arbitrate(magnets: &[Magnet], targets: &[Pos]) -> Vec<(u32, [Pos; 2])> {
    let mut candidates: Vec<(&Magnet, [Pos; 2])> = magnets
        .iter()
        .filter_map(|m| actual_affected_cells(m, targets).map(|cells| (m, cells)))
        .collect();
    candidates.sort_by(|(a, _), (b, _)| magnet_is_better(a, b).cmp(&magnet_is_better(b, a)).reverse());

    let mut used: Vec<Pos> = Vec::new();
    let mut accepted = Vec::new();
    for (m, cells) in candidates {
        if cells.iter().any(|c| used.contains(c)) {
            continue;
        }
        used.extend(cells);
        accepted.push((m.id, cells));
    }
    accepted
}

// xorshift32 — детерминированный ГПСЧ, без внешних крейтов, только для
// построения случайных решёток теста; не связан с содержимым модели.
struct Rng(u32);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo) as u32) as i32
    }
}

fn random_targets(rng: &mut Rng, n: usize) -> Vec<Pos> {
    (0..n)
        .map(|_| Pos {
            x: rng.range(0, GRID_W),
            y: rng.range(0, GRID_H),
        })
        .collect()
}

fn main() {
    let mut rng = Rng(0xC0FFEE ^ 1);

    // ── Проверка 1: найденная клетка всегда внутри радиуса ──────────────
    let mut check1_trials = 0u32;
    let mut check1_found = 0u32;
    for _ in 0..20_000 {
        let magnet = Magnet {
            pos: Pos {
                x: rng.range(0, GRID_W),
                y: rng.range(0, GRID_H),
            },
            radius: RADIUS,
            priority: 0,
            age: 0,
            id: 0,
        };
        let targets = random_targets(&mut rng, 20);
        check1_trials += 1;
        if let Some(found) = search_nearest(&magnet, &targets) {
            check1_found += 1;
            let dx = (found.x - magnet.pos.x).abs();
            let dy = (found.y - magnet.pos.y).abs();
            assert!(
                dx <= RADIUS && dy <= RADIUS,
                "найденная клетка вне радиуса: dx={dx} dy={dy} R={RADIUS}"
            );
        }
    }
    println!(
        "[1] Поиск ограничен радиусом R={RADIUS}: {check1_trials} испытаний, {check1_found} находок, \
         0 нарушений границы ✓"
    );

    // ── Проверка 2: консервативная граница sound (нет ложноотрицательных) ──
    let mut check2_pairs = 0u32;
    let mut check2_far_pairs = 0u32;
    let mut check2_real_conflicts_within_bound = 0u32;
    for _ in 0..5_000 {
        let m1 = Magnet {
            pos: Pos {
                x: rng.range(0, GRID_W),
                y: rng.range(0, GRID_H),
            },
            radius: RADIUS,
            priority: 0,
            age: 0,
            id: 1,
        };
        let m2 = Magnet {
            pos: Pos {
                x: rng.range(0, GRID_W),
                y: rng.range(0, GRID_H),
            },
            radius: RADIUS,
            priority: 0,
            age: 0,
            id: 2,
        };
        let targets = random_targets(&mut rng, 15);
        check2_pairs += 1;

        let could_conflict = discs_could_conflict(&m1, &m2);
        let cells1 = actual_affected_cells(&m1, &targets);
        let cells2 = actual_affected_cells(&m2, &targets);
        let real_overlap = match (cells1, cells2) {
            (Some(c1), Some(c2)) => cells_overlap(&c1, &c2),
            _ => false,
        };

        if !could_conflict {
            check2_far_pairs += 1;
            // Главное утверждение: граница сказала "не пересекаются" ⇒
            // реального пересечения НЕ БЫВАЕТ, ни при каком содержимом
            // решётки — если это нарушится хоть раз, граница несостоятельна.
            assert!(
                !real_overlap,
                "ложноотрицательный результат: диски не пересекались, но affected-cells пересеклись"
            );
        } else if real_overlap {
            check2_real_conflicts_within_bound += 1;
        }
    }
    println!(
        "[2] Граница графа конфликтов sound: {check2_pairs} пар, {check2_far_pairs} с непересекающимися дисками \
         (0 ложноотрицательных), {check2_real_conflicts_within_bound} реальных конфликтов внутри границы \
         (граница не вакуумна — реально ловит конфликты) ✓"
    );

    // ── Проверка 3: арбитраж по конкретной позиции — как обычный сдвиг ──
    // Два магнита рядом (радиусы пересекаются), одна общая цель типа X —
    // оба реально претендуют на одну и ту же найденную клетку.
    let shared_target = Pos { x: 20, y: 20 };
    let targets = vec![shared_target];
    let magnets = vec![
        Magnet {
            pos: Pos { x: 18, y: 20 },
            radius: RADIUS,
            priority: 5,
            age: 0,
            id: 100,
        }, // ниже priority
        Magnet {
            pos: Pos { x: 22, y: 20 },
            radius: RADIUS,
            priority: 9,
            age: 0,
            id: 200,
        }, // выше priority — должен победить
    ];
    assert!(
        discs_could_conflict(&magnets[0], &magnets[1]),
        "тестовый сценарий должен попадать внутрь границы конфликта"
    );

    let accepted = arbitrate(&magnets, &targets);
    assert_eq!(accepted.len(), 1, "ровно один магнит должен победить (all-or-nothing)");
    assert_eq!(accepted[0].0, 200, "магнит с более высоким priority должен победить");
    println!(
        "[3] Арбитраж по найденной позиции: 2 магнита претендуют на клетку {shared_target:?}, \
         магнит id=200 (priority=9) победил, id=100 (priority=5) отклонён целиком — \
         тот же all-or-nothing тай-брейк, что и arbitrator::arbitrate ✓"
    );

    println!(
        "\nВывод: CAM с ограничением радиуса R теоретически совместим с моделью — статическая \
разрешимость конфликтов сохраняется, арбитраж конкретных матчей не требует новой логики. \
Цена: новый вид сопоставления (не текущий pattern), O(R²) на клетку-кандидата, и более \
консервативный (не обязательно более медленный) граф конфликтов для правил, использующих этот механизм."
    );
}
