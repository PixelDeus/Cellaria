use cellaria::conflict_analyzer::analyze_conflicts;
use cellaria::config::load_config;
use cellaria::engine::Engine;
use cellaria::render::{render_grid, render_grid_json};
use clap::Parser;

/// Cellaria — вычислительная модель на принципе локальной редукции.
#[derive(Parser)]
#[command(name = "cellaria")]
#[command(about = "Запуск симуляции Cellaria", long_about = None)]
struct Args {
    /// Путь к YAML-конфигу
    #[arg(default_value = "configs/collision.yaml")]
    config: String,

    /// Количество тиков симуляции
    #[arg(long, default_value_t = 10)]
    ticks: u32,

    /// Вывод финального состояния решётки в JSON
    #[arg(long)]
    json: bool,
}

fn main() {
    let args = Args::parse();
    let config_path = &args.config;
    let max_ticks = args.ticks;

    let (grid, rule_index) = load_config(config_path).expect("Не удалось загрузить конфиг");

    let has_input = grid.iter_boundaries().any(|(_, b)| b.direction == "input");
    let has_output = grid.iter_boundaries().any(|(_, b)| b.direction == "output");
    let has_io = has_input || has_output;

    let total_rules: usize = rule_index.values().map(|v| v.len()).sum();

    let mut engine = Engine::new(grid, rule_index);

    if !args.json && !has_io {
        println!(
            "Конфиг: {} ({} правил, решётка {}×{})",
            config_path,
            total_rules,
            engine.grid().width(),
            engine.grid().height()
        );
        let report = analyze_conflicts(engine.rule_index());
        if report.is_conflict_free() {
            println!("CF: арбитраж не нужен (конфликтов между правилами не обнаружено)");
        } else {
            println!("Обнаружены потенциальные конфликты правил ({}):", report.conflicts.len());
            for pair in &report.conflicts {
                println!(
                    "  {:?}[{}] <-> {:?}[{}]",
                    pair.head_a, pair.rule_idx_a, pair.head_b, pair.rule_idx_b
                );
            }
        }
        println!();
        println!("Тик 0:");
        render_grid(engine.grid());
        println!();
    }

    for tick in 1..=max_ticks {
        let (accepted, outputs) = engine.run_tick();

        // Вывод IO-данных
        if has_output {
            for (ch, cell) in &outputs {
                let byte = cell.value.0 .0;
                print!("IO[{}]={}", ch, byte as char);
            }
        }

        if !args.json && !has_io {
            println!("Тик {} ({} совпадений):", tick, accepted.len());
            render_grid(engine.grid());
            println!();
            if accepted.is_empty() && tick > 1 {
                println!("Система достигла устойчивого состояния. Остановка.");
                break;
            }
        } else if accepted.is_empty() && tick > 1 {
            break;
        }
    }

    if args.json {
        println!("{}", render_grid_json(engine.grid()));
    }
}