use cellaria::config::load_config;
use cellaria::engine::Engine;
use cellaria::VecStorage;
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

fn print_grid_line(grid: &cellaria::Grid<VecStorage>) {
    let w = grid.width();
    let h = grid.height();
    for y in 0..h {
        for x in 0..w {
            let cell = grid
                .get_cell(x, y)
                .expect("print_grid_line: ячейка должна существовать в пределах решётки");
            print!("{:3}", cell.value.0 .0);
        }
        println!();
    }
}

fn print_grid_json(grid: &cellaria::Grid<VecStorage>) {
    let w = grid.width();
    let h = grid.height();
    let mut cells: Vec<Vec<u8>> = Vec::with_capacity(h);
    for y in 0..h {
        let mut row = Vec::with_capacity(w);
        for x in 0..w {
            let cell = grid
                .get_cell(x, y)
                .expect("print_grid_json: ячейка должна существовать в пределах решётки");
            row.push(cell.value.0 .0);
        }
        cells.push(row);
    }
    let output = serde_json::json!({
        "width": w,
        "height": h,
        "cells": cells,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&output)
            .expect("print_grid_json: ошибка сериализации JSON")
    );
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
        println!();
        println!("Тик 0:");
        print_grid_line(engine.grid());
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
            print_grid_line(engine.grid());
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
        print_grid_json(engine.grid());
    }
}