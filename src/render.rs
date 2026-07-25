use crate::storage::GridStorage;
use crate::Grid;

/// Вывести решётку в текстовом виде в stdout.
pub fn render_grid<S: GridStorage>(grid: &Grid<S>) {
    let w = grid.width();
    let h = grid.height();
    for y in 0..h {
        for x in 0..w {
            let cell = grid
                .get_cell(x, y)
                .expect("render_grid: ячейка должна существовать в пределах решётки");
            print!("{:3}", cell.value.0 .0);
        }
        println!();
    }
}

/// Сериализовать решётку в JSON-строку.
pub fn render_grid_json<S: GridStorage>(grid: &Grid<S>) -> String {
    let w = grid.width();
    let h = grid.height();
    let mut cells: Vec<Vec<u8>> = Vec::with_capacity(h);
    for y in 0..h {
        let mut row = Vec::with_capacity(w);
        for x in 0..w {
            let cell = grid
                .get_cell(x, y)
                .expect("render_grid_json: ячейка должна существовать в пределах решётки");
            row.push(cell.value.0 .0);
        }
        cells.push(row);
    }
    let output = serde_json::json!({
        "width": w,
        "height": h,
        "cells": cells,
    });
    serde_json::to_string_pretty(&output)
        .expect("render_grid_json: ошибка сериализации JSON")
}