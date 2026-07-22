use cellaria::config::load_config;
use cellaria::engine::run_tick;
use clap::Parser;

/// Cellaria — вычислительная модель на принципе локальной редукции.
#[derive(Parser)]
#[command(name = "cellaria")]
#[command(about = "Run Cellaria simulation", long_about = None)]
struct Args {
    /// Path to YAML config file (default: configs/collision.yaml)
    config: Option<String>,

    /// Number of ticks to simulate
    #[arg(long, default_value_t = 10)]
    ticks: u32,

    /// Output final grid state as JSON
    #[arg(long)]
    json: bool,
}

fn print_grid_line(grid: &cellaria::Grid<cellaria::VecStorage>) {
    let w = grid.width();
    let h = grid.height();
    for y in 0..h {
        for x in 0..w {
            let cell = grid.get_cell(x, y).expect("print_grid_line: cell must exist within bounds");
            print!("{:3}", cell.value.0 .0);
        }
        println!();
    }
}

fn print_grid_json(grid: &cellaria::Grid<cellaria::VecStorage>) {
    let w = grid.width();
    let h = grid.height();
    let mut cells: Vec<Vec<u8>> = Vec::with_capacity(h);
    for y in 0..h {
        let mut row = Vec::with_capacity(w);
        for x in 0..w {
            let cell = grid.get_cell(x, y).expect("print_grid_json: cell must exist within bounds");
            row.push(cell.value.0 .0);
        }
        cells.push(row);
    }
    let output = serde_json::json!({
        "width": w,
        "height": h,
        "cells": cells,
    });
    println!("{}", serde_json::to_string_pretty(&output).expect("print_grid_json: JSON serialization failed"));
}

fn main() {
    let args = Args::parse();
    let config_path = args.config.as_deref().unwrap_or("configs/collision.yaml");
    let max_ticks = args.ticks;

    let (mut grid, rule_index) = load_config(config_path).expect("Failed to load config");
    let total_rules: usize = rule_index.values().map(|v| v.len()).sum();

    if !args.json {
        println!(
            "Config: {} ({} rules, {}×{} grid)",
            config_path,
            total_rules,
            grid.width(),
            grid.height()
        );
        println!();
        println!("Tick 0:");
        print_grid_line(&grid);
        println!();
    }

    for tick in 1..=max_ticks {
        let accepted = run_tick(&mut grid, &rule_index);
        if !args.json {
            println!("Tick {} ({} matches):", tick, accepted.len());
            print_grid_line(&grid);
            println!();
            if accepted.is_empty() && tick > 1 {
                println!("System reached steady state. Stopping.");
                break;
            }
        } else if accepted.is_empty() && tick > 1 {
            break;
        }
    }

    if args.json {
        print_grid_json(&grid);
    }
}