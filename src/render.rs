use crate::storage::GridStorage;
use crate::Grid;

/// Обе рендер-функции ниже перебирают ПОЛНЫЙ прямоугольник `[0,width) ×
/// [0,height)` — осмысленно для конечной `VecStorage`, но `ChunkStorage`
/// ("бесконечная" решётка, см. её doc-комментарий) возвращает
/// `width()`/`height() == usize::MAX` — без этой проверки цикл ниже
/// попытался бы пройти `usize::MAX × usize::MAX` итераций (зависание
/// навсегда, не паника — гораздо хуже для отладки). `VecStorage` физически
/// не может дойти сюда с такой шириной: `VecStorage::new` эагерно
/// аллоцирует `width * height` ячеек, что упало бы ещё на конструкции —
/// `usize::MAX` здесь однозначно означает ChunkStorage, а не совпадение.
fn assert_bounded(w: usize, h: usize, fn_name: &str) {
    assert!(
        w != usize::MAX && h != usize::MAX,
        "{fn_name}: решётка не ограничена (width/height == usize::MAX, похоже на ChunkStorage) -- \
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

/// Сериализовать решётку в JSON-строку.
pub fn render_grid_json<S: GridStorage>(grid: &Grid<S>) -> String {
    let w = grid.width();
    let h = grid.height();
    assert_bounded(w, h, "render_grid_json");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{ChunkStorage, VecStorage};
    use crate::types::{Cell, CellType, CellValue};
    use std::collections::HashSet;

    #[test]
    fn test_render_grid_works_on_finite_vec_storage() {
        let mut grid = Grid::new(VecStorage::new(2, 2), HashSet::new());
        grid.set_cell(0, 0, Cell { value: CellValue(CellType(7)), born_at: 0 });
        render_grid(&grid); // не должно паниковать
    }

    #[test]
    fn test_render_grid_json_works_on_finite_vec_storage() {
        let mut grid = Grid::new(VecStorage::new(2, 2), HashSet::new());
        grid.set_cell(1, 1, Cell { value: CellValue(CellType(3)), born_at: 0 });
        let json = render_grid_json(&grid);
        assert!(json.contains("\"width\": 2"));
        assert!(json.contains('3'));
    }

    /// `ChunkStorage` -- "бесконечная" решётка (`width()`/`height() ==
    /// usize::MAX`, см. её doc-комментарий) -- без явного отказа обе
    /// функции выше зависли бы навсегда, пытаясь перебрать
    /// `usize::MAX × usize::MAX` клеток. Обязаны отказать СРАЗУ, понятным
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
}