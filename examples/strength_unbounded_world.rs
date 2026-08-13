//! Седьмая сила: `ChunkStorage` — решётка БЕЗ фиксированного размера.
//! Память и время пропорциональны тому, что реально тронуто (64x64-чанки
//! создаются лениво по требованию), а не номинальному размеру мира.
//! `VecStorage` (как и большинство простых движков) требует width*height
//! ячеек, выделенных заранее — миллиард на миллиард просто не влезет в
//! память. `ChunkStorage` кладёт клетку на любую координату мгновенно.

use std::time::Instant;

use cellaria::types::{Cell, CellType, CellValue};
use cellaria::{ChunkStorage, Grid, GridStorage};

fn main() {
    let storage = ChunkStorage::new();
    let mut grid = Grid::new(storage, Default::default());

    println!(
        "Границы решётки: {:?} (нет — мир не ограничен)\n",
        grid.storage.bounds()
    );

    let far_points: [(usize, usize); 4] = [
        (0, 0),
        (1_000_000, 1_000_000),
        (500_000_000, 12_345),
        (999_999_999, 999_999_999),
    ];

    for &(x, y) in &far_points {
        let t0 = Instant::now();
        grid.set_cell(
            x,
            y,
            Cell {
                value: CellValue(CellType(1)),
                born_at: 0,
            },
        );
        let placed = grid.get_cell(x, y).map(|c| c.value.0 .0);
        println!(
            "клетка ({:>12}, {:>12}): записана за {:>8?}, читается как {:?}",
            x,
            y,
            t0.elapsed(),
            placed
        );
    }

    println!(
        "\nВсего создано чанков 64x64: {} — память пропорциональна ТРОНУТОМУ,\n\
         а не номинальному размеру мира (тут номинально почти 10^18 клеток).",
        grid.storage.active_chunks().count()
    );
}
