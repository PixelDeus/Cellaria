//! # Cellaria Benchmark Suite
//!
//! Точка входа для всех бенчмарков.
//!
//! ## Запуск
//!
//! Это `[[bench]]`-таргет (не `[[bin]]`) — запускается через
//! `cargo bench --bench cellaria_bench -- <флаги>`, не `cargo run`.
//! `cargo bench` сама всегда дописывает `--bench` в конец argv (стандартное
//! поведение Cargo для bench-таргетов) — поэтому переключатель на
//! Criterion называется `--criterion`, а не `--bench`, во избежание
//! коллизии с тем, что подставляет сама Cargo.
//!
//! ```bash
//! # Все тесты производительности + peak-тесты (кастомный репортер)
//! cargo bench --bench cellaria_bench
//!
//! # Вывод в JSON
//! cargo bench --bench cellaria_bench -- --json
//!
//! # Сохранить результаты в файл
//! cargo bench --bench cellaria_bench -- --json --output bench_results.json
//!
//! # Сравнить с эталоном
//! cargo bench --bench cellaria_bench -- --check bench_baseline.json
//!
//! # Только Criterion-бенчмарки
//! cargo bench --bench cellaria_bench -- --criterion
//!
//! # Criterion + фильтрация по имени группы
//! cargo bench --bench cellaria_bench -- --criterion throughput
//! ```

mod helpers;
mod existing;
mod throughput;
mod rules;
mod reporter;

use reporter::{Reporter, BenchResult};

// ============================================================================
// ФАЗА 1: Максимальный throughput
// ============================================================================

fn run_phase1(reporter: &mut Reporter) {
    reporter.print_header("ФАЗА 1: Максимальный throughput");

    // 1A: Без сдвига
    reporter.print_subheader("1A: Без сдвига (N×N, чередование, apply)");
    let mut results_1a = Vec::new();
    for &n in &[10, 50, 100, 200, 500] {
        let (elapsed_us, ticks) = throughput::max_throughput_no_shift(n);
        let tps = if elapsed_us > 0 {
            (ticks as u128 * 1_000_000) / elapsed_us
        } else {
            0
        };
        let mut r = BenchResult::new("1A", &format!("N={}", n), &ticks.to_string(), "тиков")
            .with_extra("tps", &reporter::format_number(tps))
            .pass();
        // Пик throughput
        if n == 500 {
            r = r.peak();
        }
        results_1a.push(r);
    }
    reporter.add_all(results_1a.clone());
    reporter.print_table(&results_1a, &["Параметр", "Значение", "Дополнительно", "Статус"]);

    // 1B: Со сдвигом
    reporter.print_subheader("1B: Со сдвигом (TM-лента длины N)");
    let mut results_1b = Vec::new();
    for &n in &[10, 100, 1000, 5000] {
        let (elapsed_us, ticks) = throughput::max_throughput_with_shift(n);
        let tps = if elapsed_us > 0 {
            (ticks as u128 * 1_000_000) / elapsed_us
        } else {
            0
        };
        let r = BenchResult::new("1B", &format!("N={}", n), &ticks.to_string(), "тиков")
            .with_extra("tps", &reporter::format_number(tps))
            .pass();
        results_1b.push(r);
    }
    reporter.add_all(results_1b.clone());
    reporter.print_table(&results_1b, &["Параметр", "Значение", "Дополнительно", "Статус"]);

    // 1C: Конфликт
    reporter.print_subheader("1C: Конфликт (M правил на одной ячейке)");
    let mut results_1c = Vec::new();
    for &m in &[10, 50, 100, 200] {
        let (elapsed_us, ticks) = throughput::max_throughput_conflict(m);
        let tps = if elapsed_us > 0 {
            (ticks as u128 * 1_000_000) / elapsed_us
        } else {
            0
        };
        let r = BenchResult::new("1C", &format!("M={}", m), &ticks.to_string(), "тиков")
            .with_extra("tps", &reporter::format_number(tps))
            .pass();
        results_1c.push(r);
    }
    reporter.add_all(results_1c.clone());
    reporter.print_table(&results_1c, &["Параметр", "Значение", "Дополнительно", "Статус"]);

    // 1D: Пустой тик
    reporter.print_subheader("1D: Пустой тик (0 активных ячеек)");
    let (elapsed_ns, total_ticks) = throughput::empty_tick_bench();
    let avg_ns = if total_ticks > 0 { elapsed_ns / total_ticks } else { 0 };
    let r = BenchResult::new("1D", "пустой тик", &reporter::format_number(total_ticks), "тиков")
        .with_extra("ns/tick", &format!("{}", avg_ns))
        .pass();
    reporter.add(r.clone());
    reporter.print_table(&[r], &["Параметр", "Значение", "Дополнительно", "Статус"]);

    // 1E: Одна ячейка
    reporter.print_subheader("1E: Одна ячейка, одно правило");
    let (elapsed_us, ticks) = throughput::single_cell_max_tps();
    let tps = if elapsed_us > 0 {
        (ticks as u128 * 1_000_000) / elapsed_us
    } else {
        0
    };
    let avg_ns = if ticks > 0 {
        (elapsed_us as u128 * 1000) / ticks as u128
    } else {
        0
    };
    let r = BenchResult::new("1E", "1 ячейка", &ticks.to_string(), "тиков")
        .with_extra("tps", &reporter::format_number(tps))
        .with_extra("ns/tick", &format!("{}", avg_ns))
        .pass();
    reporter.add(r.clone());
    reporter.print_table(&[r], &["Параметр", "Значение", "Дополнительно", "Статус"]);

    // 1F: Длинная цепочка сдвигов
    reporter.print_subheader("1F: Длинная цепочка сдвигов (N)");
    let mut results_1f = Vec::new();
    for &n in &[10, 100, 500] {
        let (elapsed_us, ticks) = throughput::long_shift_chain_bench(n);
        let tps = if elapsed_us > 0 {
            (ticks as u128 * 1_000_000) / elapsed_us
        } else {
            0
        };
        let r = BenchResult::new("1F", &format!("N={}", n), &ticks.to_string(), "тиков")
            .with_extra("tps", &reporter::format_number(tps))
            .pass();
        results_1f.push(r);
    }
    reporter.add_all(results_1f.clone());
    reporter.print_table(&results_1f, &["Параметр", "Значение", "Дополнительно", "Статус"]);
}

// ============================================================================
// ФАЗА 2: Разложение по фазам
// ============================================================================

fn run_phase2(reporter: &mut Reporter) {
    reporter.print_header("ФАЗА 2: Разложение по фазам + полный tick");

    for &n in &[10, 100, 500] {
        reporter.print_subheader(&format!("N={}", n));
        let phases = phase_breakdown(n);
        let total_phases: u128 = phases.iter().map(|(_, t)| t).sum();

        let mut results = Vec::new();
        for (name, t) in &phases {
            let pct = if total_phases > 0 { (*t as f64 / total_phases as f64) * 100.0 } else { 0.0 };
            let r = BenchResult::new(&format!("2:N={}", n), name, &t.to_string(), "ns")
                .with_extra("%", &reporter::format_f64(pct, 1))
                .pass();
            results.push(r);
        }

        // Сумма фаз
        let r_sum = BenchResult::new(&format!("2:N={}", n), "Сумма фаз", &total_phases.to_string(), "ns")
            .pass();
        results.push(r_sum);

        // Полный run_tick
        let full = full_tick_bench(n);
        let r_full = BenchResult::new(&format!("2:N={}", n), "Полный run_tick", &full.to_string(), "ns")
            .pass();
        results.push(r_full);

        // Overhead
        if full > 0 {
            let overhead = if total_phases > full { total_phases - full } else { full - total_phases };
            let pct = (overhead as f64 / full as f64) * 100.0;
            let r_over = BenchResult::new(&format!("2:N={}", n), "Overhead", &overhead.to_string(), "ns")
                .with_extra("%", &reporter::format_f64(pct, 1))
                .pass();
            results.push(r_over);
        }

        reporter.add_all(results.clone());
        reporter.print_table(&results, &["Параметр", "Значение", "Дополнительно", "Статус"]);
    }
}

// ============================================================================
// ФАЗА 3: Память
// ============================================================================

fn run_phase3(reporter: &mut Reporter) {
    reporter.print_header("ФАЗА 3: Память");

    reporter.print_subheader("VecStorage: оценочный размер N×N");
    let mut results_vec = Vec::new();
    for &n in &[10, 100, 500, 1000] {
        let bytes = memory_vec(n);
        let mb = bytes as f64 / 1_000_000.0;
        let r = BenchResult::new("3A:Vec", &format!("N={}", n), &reporter::format_f64(mb, 2), "MB")
            .pass();
        results_vec.push(r);
    }
    reporter.add_all(results_vec.clone());
    reporter.print_table(&results_vec, &["Параметр", "Значение", "Статус"]);

    reporter.print_subheader("ChunkStorage: оценочный размер N×N (32×32 chunks)");
    let mut results_chunk = Vec::new();
    for &n in &[10, 100, 500, 1000, 5000] {
        let bytes = memory_chunk_estimate(n);
        let kb = bytes / 1024;
        let unit = if kb >= 1024 {
            format!("{:.1} MB", kb as f64 / 1024.0)
        } else {
            format!("{} KB", kb)
        };
        let r = BenchResult::new("3B:Chunk", &format!("N={}", n), &unit, "")
            .pass();
        results_chunk.push(r);
    }
    reporter.add_all(results_chunk.clone());
    reporter.print_table(&results_chunk, &["Параметр", "Значение", "Статус"]);
}

// ============================================================================
// ФАЗА 4: Сложность правил
// ============================================================================

fn run_phase4(reporter: &mut Reporter) {
    reporter.print_header("ФАЗА 4: Сложность правил");

    reporter.print_subheader("4A: Размер паттерна");
    let mut results_4a = Vec::new();
    for &size in &[1, 2, 4, 9] {
        let elapsed = rules::pattern_size_bench(size);
        let r = BenchResult::new("4A", &format!("size={}", size), &elapsed.to_string(), "µs")
            .pass();
        results_4a.push(r);
    }
    reporter.add_all(results_4a.clone());
    reporter.print_table(&results_4a, &["Параметр", "Значение", "Статус"]);

    reporter.print_subheader("4B: Правил на один head-тип");
    let mut results_4b = Vec::new();
    for &k in &[1, 10, 50, 100, 200] {
        let elapsed = rules::rules_per_head_bench(k);
        let r = BenchResult::new("4B", &format!("K={}", k), &elapsed.to_string(), "µs")
            .pass();
        results_4b.push(r);
    }
    reporter.add_all(results_4b.clone());
    reporter.print_table(&results_4b, &["Параметр", "Значение", "Статус"]);
}

// ============================================================================
// ФАЗА 5: Профилирование find_rule
// ============================================================================

fn run_phase5(reporter: &mut Reporter) {
    reporter.print_header("ФАЗА 5: Профилирование find_rule");

    reporter.print_subheader("Среднее время поиска правила (10000 итераций)");
    let mut results = Vec::new();
    for &k in &[1, 10, 50, 100, 200, 500] {
        let (avg_ns, count) = rules::profile_find_rule(k);
        let r = BenchResult::new("5", &format!("K={}", k), &avg_ns.to_string(), "ns/поиск")
            .with_extra("найдено", &count.to_string())
            .pass();
        results.push(r);
    }
    reporter.add_all(results.clone());
    reporter.print_table(&results, &["Параметр", "Значение", "Дополнительно", "Статус"]);
}

// ============================================================================
// ФАЗА 2: Разложение времени тика по фазам (внутренняя)
// ============================================================================

/// Разложить run_tick на фазы и замерить каждую отдельно.
fn phase_breakdown(n: usize) -> Vec<(&'static str, u128)> {
    use std::time::Instant;
    use std::collections::HashSet;
    use cellaria::Grid;
    use cellaria::VecStorage;
    use cellaria::GridStorage;
    use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Rule};
    
    let storage = VecStorage::new(n, n);
    let mut grid = Grid::new(storage, HashSet::new());

    // Шахматный паттерн
    for y in 0..n {
        for x in 0..n {
            let val = if (x + y) % 2 == 0 { 1 } else { 2 };
            grid.set_cell(x, y, Cell {
                value: CellValue(CellType(val)),
                born_at: grid.generation(),
            });
        }
    }

    let rule = Rule {
        id: vec![CellType(1), CellType(2)],
        pattern: vec![(0i8, 0i8, CellType(1)), (1i8, 0i8, CellType(2))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(3)), (1, 0, ChangeValue::Literal(4))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
    };
    let rule_index = helpers::make_rule_index(vec![rule]);
    let rule_cache = cellaria::conflict_analyzer::build_rule_data_cache(&rule_index);

    // Первый тик (может быть outlier из-за аллокаций)
    cellaria::engine::run_tick(&mut grid, &rule_index);

    // Второй тик — измеряем

    // detect_matches
    let t0 = Instant::now();
    let matches = cellaria::engine::detect_matches(&grid, &rule_index, grid.active_coords());
    let t_detect = t0.elapsed().as_nanos() as u128;

    // arbitrate
    let t1 = Instant::now();
    let accepted = cellaria::engine::arbitrate(matches, &rule_index, &rule_cache, (grid.width(), grid.height()), |x, y| {
        grid.get_age(x, y) as u32
    });
    let t_arbitrate = t1.elapsed().as_nanos() as u128;

    // apply_matches
    let t2 = Instant::now();
    let (regions, _outputs) = if !accepted.is_empty() {
        cellaria::engine::apply_matches(&mut grid, accepted, &rule_index, &rule_cache)
    } else {
        (Vec::new(), Vec::new())
    };
    let t_apply = t2.elapsed().as_nanos() as u128;

    // advance_age
    let t3 = Instant::now();
    // advance_age — O(1): просто инкремент generation
    grid.advance_age();
    let t_age = t3.elapsed().as_nanos() as u128;

    // reset_age
    let t4 = Instant::now();
    for region in &regions {
        for y in region.y_start..region.y_end {
            for x in region.x_start..region.x_end {
                if let Some(cell) = grid.get_cell(x as usize, y as usize) {
                    grid.storage.set(x as usize, y as usize, Cell {
                        value: cell.value,
                        born_at: grid.generation(),
                    });
                }
            }
        }
    }
    let t_reset = t4.elapsed().as_nanos() as u128;

    vec![
        ("detect_matches", t_detect),
        ("arbitrate", t_arbitrate),
        ("apply_matches", t_apply),
        ("advance_age", t_age),
        ("reset_age", t_reset),
    ]
}

// ============================================================================
// Полный tick (для сравнения с суммой фаз)
// ============================================================================

/// Замерить полный run_tick от начала до конца (создаёт новую решётку, как phase_breakdown).
fn full_tick_bench(n: usize) -> u128 {
    use std::time::Instant;
    use std::collections::HashSet;
    use cellaria::Grid;
    use cellaria::VecStorage;
    use cellaria::types::{Cell, CellType, CellValue, ChangeValue, Rule};
    
    let storage = VecStorage::new(n, n);
    let mut grid = Grid::new(storage, HashSet::new());

    for y in 0..n {
        for x in 0..n {
            let val = if (x + y) % 2 == 0 { 1 } else { 2 };
            grid.set_cell(x, y, Cell {
                value: CellValue(CellType(val)),
                born_at: grid.generation(),
            });
        }
    }

    let rule = Rule {
        id: vec![CellType(1), CellType(2)],
        pattern: vec![(0i8, 0i8, CellType(1)), (1i8, 0i8, CellType(2))],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(3)), (1, 0, ChangeValue::Literal(4))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
    };
    let rule_index = helpers::make_rule_index(vec![rule]);

    // Прогрев
    cellaria::engine::run_tick(&mut grid, &rule_index);

    // Измерение
    let t0 = Instant::now();
    cellaria::engine::run_tick(&mut grid, &rule_index);
    t0.elapsed().as_nanos() as u128
}

// ============================================================================
// ФАЗА 3: Память — Vec vs Chunk, max size
// ============================================================================

/// Замерить потребление памяти VecStorage.
fn memory_vec(n: usize) -> u128 {
    use cellaria::types::Cell;
    let cell_size = std::mem::size_of::<Cell>(); // 12 bytes (u8+padding, u32)
    (n * n) as u128 * cell_size as u128
}

/// Замерить потребление ChunkStorage для заполненной N×N области.
fn memory_chunk_estimate(n: usize) -> usize {
    let chunks_per_side = (n + 31) / 32;
    let num_chunks = chunks_per_side * chunks_per_side;
    num_chunks * 4096 // примерный оверхед на чанк
}

// ============================================================================
// Точка входа
// ============================================================================

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let is_json = args.iter().any(|a| a == "--json");
    let is_quick = args.iter().any(|a| a == "--quick");
    let is_quiet = args.iter().any(|a| a == "--quiet");
    let is_compact = args.iter().any(|a| a == "--compact");
    // `cargo bench --bench cellaria_bench -- <...>` ВСЕГДА дописывает
    // `--bench` в конец argv сама — это стандартное поведение Cargo для
    // bench-таргетов, даже при harness = false в Cargo.toml. Раньше именно
    // этот флаг выбирал ветку Criterion — то есть кастомный репортер
    // (--compact/--quiet/--json) был недостижим ЛЮБЫМ запуском через
    // `cargo bench`: --bench там есть всегда, независимо от того, что
    // ввёл пользователь после `--`. Отдельный `--criterion` не пересекается
    // с тем, что cargo подставляет сама.
    let is_criterion = args.iter().any(|a| a == "--criterion");

    // Если --criterion: запускаем Criterion-бенчмарки с быстрой конфигурацией
    if is_criterion {
        let mut criterion = {
            let mut cb = criterion::Criterion::default();
            // Быстрый режим для Criterion
            if is_quick {
                cb = cb
                    .warm_up_time(std::time::Duration::from_secs(1))
                    .measurement_time(std::time::Duration::from_secs(2))
                    .sample_size(10)
                    .noise_threshold(0.10);
            }
            cb.configure_from_args()
        };
        existing::register_all(&mut criterion);
        throughput::register_all(&mut criterion);
        rules::register_all(&mut criterion);
        criterion.final_summary();
        return;
    }

    // --quick для обычного (не-Criterion) пути: раньше флаг парсился, но
    // нигде не читался вне ветки is_bench — окна throughput::*_bench
    // оставались жёстко 100ms/1s независимо от него. Теперь делит их на 10.
    if is_quick {
        throughput::QUICK.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    let mut reporter = Reporter::new();
    reporter.set_json(is_json);
    reporter.set_compact(is_compact);
    reporter.set_quiet(is_quiet);

    // Проверка на --check
    if let Some(pos) = args.iter().position(|a| a == "--check") {
        if let Some(path) = args.get(pos + 1) {
            if let Err(e) = reporter.load_baseline(path) {
                eprintln!("Ошибка загрузки baseline: {}", e);
            }
        }
    }

    // Проверка на --output
    if let Some(pos) = args.iter().position(|a| a == "--output") {
        if let Some(path) = args.get(pos + 1) {
            reporter.set_output_path(path);
        }
    }

    // Peak-тесты
    run_phase1(&mut reporter);
    run_phase2(&mut reporter);
    run_phase3(&mut reporter);
    run_phase4(&mut reporter);
    run_phase5(&mut reporter);

    // Проверка с baseline, если загружен
    let baseline_warnings = reporter.check_baseline();
    if !baseline_warnings.is_empty() {
        reporter.print_blank();
        reporter.print_info("Предупреждения от baseline-проверки:");
        for w in &baseline_warnings {
            reporter.print_info(&format!("  ⚠ {}: {} ({})", w.group, w.param, w.status.label()));
        }
    }

    // Финальная сводка
    reporter.print_summary();
}
