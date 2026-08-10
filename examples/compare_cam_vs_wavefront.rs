//! Сравнивает CAM-поиск (радиус R, один тик, O(R²) сканирование —
//! см. `proof_cam_radius_search.rs`) с тем, что УЖЕ достижимо существующими
//! примитивами Cellaria: волновой поиск через обычные `shifts`/`changes`.
//!
//! Волновой поиск — сигнал SEEK сдвигается на 1 клетку в тик в сторону цели;
//! если следующая клетка — искомый тип, SEEK становится FOUND и едет назад к
//! магниту тем же способом. Реализовано как настоящие `Rule` через настоящий
//! `cellaria::engine::run_tick` — не приближение, а то же самое, что мог бы
//! написать пользователь библиотеки сегодня, без единой правки `src/`.
//!
//! Упрощение: 1D-поиск вдоль одной оси, не полный 2D-диск CAM — честно для
//! сравнения ЗАДЕРЖКИ (тиков до результата) и СТОИМОСТИ ЗА ТИК, не полное
//! воспроизведение геометрии CAM.
//!
//! Профили принципиально разные:
//! - CAM: 1 тик, O(R²) работы разом (burst).
//! - Волна: 2R тиков (туда+обратно), O(1) работы за тик.
//!
//! Вопрос не "что быстрее" вообще, а "что быстрее ДЛЯ ЧЕГО" — задержка до
//! результата или суммарная работа/тактовая нагрузка на тик.

use std::collections::HashMap;
use std::time::Instant;

use cellaria::engine::Engine;
use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Direction, Rule, ShiftSpec};
use cellaria::{Grid, VecStorage};

const SEEK: u8 = 1;
const FOUND: u8 = 2;
const TARGET: u8 = 3;

/// Правила волнового поиска: SEEK едет вправо, пока следующая клетка не
/// TARGET; как только следующая — TARGET, SEEK становится FOUND НА МЕСТЕ
/// (без сдвига); FOUND едет влево обратно к магниту (позиция 0).
fn wavefront_rules() -> HashMap<CellType, Vec<Rule>> {
    let mut idx: HashMap<CellType, Vec<Rule>> = HashMap::new();
    idx.insert(CellType(SEEK), vec![
        // Более специфичный паттерн — выше приоритет, побеждает "слепой" сдвиг.
        Rule {
            id: vec![CellType(SEEK)],
            pattern: vec![(0, 0, CellType(SEEK)), (1, 0, CellType(TARGET))],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(FOUND))],
            active_only: false, priority: 10, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
        },
        Rule {
            id: vec![CellType(SEEK)],
            pattern: vec![(0, 0, CellType(SEEK))],
            shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
            changes: vec![],
            active_only: false, priority: 0, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
        },
    ]);
    idx.insert(CellType(FOUND), vec![Rule {
        id: vec![CellType(FOUND)],
        pattern: vec![],
        shifts: vec![vec![ShiftSpec::new(Direction::Left, 1)]],
        changes: vec![],
        active_only: false, priority: 0, min_age: 0, overflow: Default::default(), cam: None, tie_break: 0, starvation_after: None, feedback: None, recursion: None, memory: None, max_activations: None,
    }]);
    idx
}

/// Прогоняет волновой поиск до тех пор, пока FOUND не вернётся в позицию 0
/// (магнит), возвращает (число тиков, потраченное время). `Engine::run_tick`
/// (не свободная функция `engine::run_tick`) — переиспользует `rule_cache`/
/// `group_cache` между вызовами, как и должен любой реальный горячий цикл
/// (свободная функция пересобирает их КАЖДЫЙ раз — специально
/// задокументированная ловушка, см. её doc-комментарий в `engine/mod.rs`).
fn run_wavefront(radius: usize) -> (u32, std::time::Duration) {
    let width = radius + 2;
    let storage = VecStorage::new(width, 1);
    let mut grid = Grid::new(storage, Default::default());
    grid.set_cell(0, 0, Cell { value: CellValue(CellType(SEEK)), born_at: 0 });
    grid.set_cell(radius, 0, Cell { value: CellValue(CellType(TARGET)), born_at: 0 });
    let mut engine = Engine::new(grid, wavefront_rules());

    let start = Instant::now();
    let mut ticks = 0u32;
    loop {
        engine.run_tick();
        ticks += 1;
        if engine.grid().get_cell(0, 0).map(|c| c.value.0 .0) == Some(FOUND) {
            break;
        }
        if ticks > (radius as u32) * 4 {
            panic!("волновой поиск не вернулся за разумное число тиков — ошибка в правилах");
        }
    }
    (ticks, start.elapsed())
}

/// Та же логика поиска, что в `proof_cam_radius_search.rs`, но с реальным
/// таймингом — один сплошной проход по диску радиуса R.
fn run_cam_scan(radius: i32, n_targets: usize) -> std::time::Duration {
    let targets: Vec<(i32, i32)> = (0..n_targets)
        .map(|i| ((i as i32 * 37) % (radius * 4 + 1) - radius * 2, (i as i32 * 53) % (radius * 4 + 1) - radius * 2))
        .collect();
    let magnet = (0i32, 0i32);

    let start = Instant::now();
    let _found = targets
        .iter()
        .filter(|&&(tx, ty)| (tx - magnet.0).abs() <= radius && (ty - magnet.1).abs() <= radius)
        .min_by_key(|&&(tx, ty)| {
            let dist = (tx - magnet.0).abs().max((ty - magnet.1).abs());
            (dist, ty, tx)
        });
    start.elapsed()
}

fn main() {
    println!("Радиус | CAM (1 тик, O(R²)) | Волна (2R тиков, реальный движок) | тиков в волне | вывод");
    println!("-------|---------------------|-------------------------------------|---------------|------");

    for &radius in &[5usize, 10, 20, 50, 100] {
        let cam_time = run_cam_scan(radius as i32, (2 * radius + 1).pow(2).min(2000));
        let (wave_ticks, wave_time) = run_wavefront(radius);

        let verdict = if cam_time < wave_time { "CAM быстрее" } else { "волна быстрее" };
        println!(
            "{radius:>6} | {:>17.2?} | {:>35.2?} | {wave_ticks:>13} | {verdict}",
            cam_time, wave_time,
        );
    }

    println!(
        "\nОжидаемо: CAM выигрывает по ЗАДЕРЖКЕ (1 тик против 2R) на любом R — это единственный тик \
против множества. Но волна делает O(1) работы за тик (дешёвая, предсказуемая нагрузка), а CAM — всю \
O(R²) работу разом, одним burst'ом. Если важна задержка до результата (интерактивность, реактивность) \
— CAM. Если важна равномерная нагрузка на тик на плотной решётке с МНОГИМИ одновременными магнитами \
(та же логика, что и во всей этой сессии про GPU: burst-стоимость на клетку умножается на число \
активных магнитов ЭТОГО тика) — волна может оказаться безопаснее, потому что не создаёт пиков."
    );
}
