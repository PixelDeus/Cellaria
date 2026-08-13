use crate::layered::LayeredEngine;
use crate::storage::GridStorage;
use crate::Grid;

/// Обе рендер-функции ниже перебирают ПОЛНЫЙ прямоугольник `[0,width) ×
/// [0,height)` — осмысленно для конечной `VecStorage`, но `ChunkStorage`
/// ("бесконечная" решётка, см. её doc-комментарий) возвращает
/// `width()`/`height() == u32::MAX` (было `usize::MAX` — снижено при
/// фиксе молчаливого усечения координат, см. `Grid::set_cell`'s
/// doc-комментарий) — без этой проверки цикл ниже
/// попытался бы пройти `u32::MAX × u32::MAX` итераций (зависание
/// навсегда, не паника — гораздо хуже для отладки). `VecStorage` физически
/// не может дойти сюда с такой шириной: `VecStorage::new` эагерно
/// аллоцирует `width * height` ячеек, что упало бы ещё на конструкции —
/// `u32::MAX` здесь однозначно означает ChunkStorage, а не совпадение.
fn assert_bounded(w: usize, h: usize, fn_name: &str) {
    assert!(
        w != u32::MAX as usize && h != u32::MAX as usize,
        "{fn_name}: решётка не ограничена (width/height == u32::MAX, похоже на ChunkStorage) -- \
         рендер всей номинально бесконечной решётки не имеет смысла; переберите активные клетки \
         вручную (`grid.iter_active()`) и постройте свою собственную ограниченную область"
    );
}

/// Вывести решётку в текстовом виде в stdout.
pub fn render_grid<S: GridStorage>(grid: &Grid<S>) {
    let w = grid.width();
    let h = grid.height();
    assert_bounded(w, h, "render_grid");
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

/// Прочитать `[0,w) × [0,h)` в плоский 2D-массив типов клеток — общая
/// часть `render_grid_json`/`render_layered_grid_json`. `w`/`h` уже
/// проверены `assert_bounded` вызывающей стороной.
fn cells_grid<S: GridStorage>(grid: &Grid<S>, w: usize, h: usize, fn_name: &str) -> Vec<Vec<u8>> {
    let mut cells: Vec<Vec<u8>> = Vec::with_capacity(h);
    for y in 0..h {
        let mut row = Vec::with_capacity(w);
        for x in 0..w {
            let cell = grid
                .get_cell(x, y)
                .unwrap_or_else(|| panic!("{fn_name}: ячейка должна существовать в пределах решётки"));
            row.push(cell.value.0 .0);
        }
        cells.push(row);
    }
    cells
}

/// Сериализовать решётку в JSON-строку.
pub fn render_grid_json<S: GridStorage>(grid: &Grid<S>) -> String {
    let w = grid.width();
    let h = grid.height();
    assert_bounded(w, h, "render_grid_json");
    let cells = cells_grid(grid, w, h, "render_grid_json");
    let output = serde_json::json!({
        "width": w,
        "height": h,
        "cells": cells,
    });
    serde_json::to_string_pretty(&output).expect("render_grid_json: ошибка сериализации JSON")
}

/// Сериализовать ВЕСЬ стек `LayeredEngine` в JSON-строку — по одному
/// `cells`-массиву на слой, в том же формате, что и `render_grid_json`'s
/// поле. `LayeredEngine::new` уже гарантирует (`assert!`), что все слои
/// одного размера, так что ширина/высота проверяются один раз, по слою 0,
/// а не заново на каждом слое.
///
/// ```
/// use std::collections::HashMap;
/// use cellaria::render::render_layered_grid_json;
/// use cellaria::types::{Cell, CellType, CellValue, Rule};
/// use cellaria::{Grid, LayeredEngine, VecStorage};
///
/// let grid0 = Grid::new(VecStorage::new(2, 2), Default::default());
/// let grid1 = Grid::new(VecStorage::new(2, 2), Default::default());
/// let layered = LayeredEngine::new(vec![grid0, grid1], HashMap::<CellType, Vec<Rule>>::new());
///
/// let json = render_layered_grid_json(&layered);
/// assert!(json.contains("\"layer_count\": 2"));
/// ```
pub fn render_layered_grid_json<S: GridStorage>(layered: &LayeredEngine<S>) -> String {
    let layer_count = layered.layer_count();
    assert!(
        layer_count > 0,
        "render_layered_grid_json: LayeredEngine has zero layers -- nothing to render"
    );

    let first_grid = layered.layer(0).grid();
    let w = first_grid.width();
    let h = first_grid.height();
    assert_bounded(w, h, "render_layered_grid_json");

    let layers: Vec<Vec<Vec<u8>>> = (0..layer_count)
        .map(|i| cells_grid(layered.layer(i).grid(), w, h, "render_layered_grid_json"))
        .collect();

    let output = serde_json::json!({
        "width": w,
        "height": h,
        "layer_count": layer_count,
        "layers": layers,
    });
    serde_json::to_string_pretty(&output).expect("render_layered_grid_json: ошибка сериализации JSON")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{ChunkStorage, VecStorage};
    use crate::types::{Cell, CellType, CellValue};
    use std::collections::HashSet;

    #[test]
    fn test_render_grid_works_on_finite_vec_storage() {
        let mut grid = Grid::new(VecStorage::new(2, 2), HashSet::new());
        grid.set_cell(
            0,
            0,
            Cell {
                value: CellValue(CellType(7)),
                born_at: 0,
            },
        );
        render_grid(&grid); // не должно паниковать
    }

    #[test]
    fn test_render_grid_json_works_on_finite_vec_storage() {
        let mut grid = Grid::new(VecStorage::new(2, 2), HashSet::new());
        grid.set_cell(
            1,
            1,
            Cell {
                value: CellValue(CellType(3)),
                born_at: 0,
            },
        );
        let json = render_grid_json(&grid);
        assert!(json.contains("\"width\": 2"));
        assert!(json.contains('3'));
    }

    /// `ChunkStorage` -- "бесконечная" решётка (`width()`/`height() ==
    /// u32::MAX`, см. её doc-комментарий) -- без явного отказа обе
    /// функции выше зависли бы навсегда, пытаясь перебрать
    /// `u32::MAX × u32::MAX` клеток. Обязаны отказать СРАЗУ, понятным
    /// сообщением, не зависать.
    #[test]
    #[should_panic(expected = "решётка не ограничена")]
    fn test_render_grid_refuses_unbounded_chunk_storage() {
        let grid = Grid::new(ChunkStorage::new(), HashSet::new());
        render_grid(&grid);
    }

    #[test]
    #[should_panic(expected = "решётка не ограничена")]
    fn test_render_grid_json_refuses_unbounded_chunk_storage() {
        let grid = Grid::new(ChunkStorage::new(), HashSet::new());
        render_grid_json(&grid);
    }

    fn make_layered_engine(w: usize, h: usize, layer_count: usize) -> LayeredEngine<VecStorage> {
        let grids: Vec<Grid<VecStorage>> = (0..layer_count)
            .map(|_| Grid::new(VecStorage::new(w, h), HashSet::new()))
            .collect();
        LayeredEngine::new(grids, std::collections::HashMap::new())
    }

    #[test]
    fn test_render_layered_grid_json_includes_every_layer() {
        let mut layered = make_layered_engine(2, 2, 3);
        layered.layer_mut(0).grid_mut().set_cell(
            0,
            0,
            Cell {
                value: CellValue(CellType(7)),
                born_at: 0,
            },
        );
        layered.layer_mut(2).grid_mut().set_cell(
            1,
            1,
            Cell {
                value: CellValue(CellType(9)),
                born_at: 0,
            },
        );

        let json = render_layered_grid_json(&layered);
        assert!(json.contains("\"width\": 2"));
        assert!(json.contains("\"height\": 2"));
        assert!(json.contains("\"layer_count\": 3"));
        assert!(json.contains('7'), "must include layer 0's marker: {json}");
        assert!(json.contains('9'), "must include layer 2's marker: {json}");

        let parsed: serde_json::Value = serde_json::from_str(&json).expect("must be valid JSON");
        let layers = parsed["layers"].as_array().expect("layers must be an array");
        assert_eq!(layers.len(), 3, "one cells-array per layer");
    }

    #[test]
    #[should_panic(expected = "решётка не ограничена")]
    fn test_render_layered_grid_json_refuses_unbounded_chunk_storage() {
        let layered = LayeredEngine::new(
            vec![Grid::new(ChunkStorage::new(), HashSet::new())],
            std::collections::HashMap::new(),
        );
        render_layered_grid_json(&layered);
    }
}
