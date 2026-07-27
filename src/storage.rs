use crate::types::Cell;
use std::collections::HashMap;

/// Абстракция хранилища ячеек решётки.
///
/// Позволяет реализовать как конечную решётку фиксированного размера
/// ([`VecStorage`]), так и бесконечную, разбитую на чанки ([`ChunkStorage`]).
pub trait GridStorage: Sync {
    /// Получить ссылку на ячейку по координатам.
    fn get(&self, x: usize, y: usize) -> Option<&Cell>;
    /// Установить значение ячейки по координатам.
    ///
    /// Единственный способ гарантировать корректную синхронизацию
    /// `non_default_count` в [`ChunkStorage`].
    fn set(&mut self, x: usize, y: usize, cell: Cell);
    /// Ширина решётки. Для бесконечной — [`usize::MAX`].
    fn width(&self) -> usize;
    /// Высота решётки. Для бесконечной — [`usize::MAX`].
    fn height(&self) -> usize;
    /// Итератор по координатам активных (не-дефолтных) ячеек.
    fn active_cells(&self) -> Box<dyn Iterator<Item = (usize, usize)> + '_>;
    /// Границы решётки, если есть (для конечных решёток).
    fn bounds(&self) -> Option<(usize, usize)> {
        None
    }
}

/// Конечная решётка фиксированного размера.
///
/// Хранит все ячейки в плоском векторе `cells` размером `width × height`.
/// Индексация: `index = y * width + x`.
#[derive(Clone)]
pub struct VecStorage {
    /// Плоский вектор всех ячеек решётки.
    pub cells: Vec<Cell>,
    /// Ширина решётки.
    pub width: usize,
    /// Высота решётки.
    pub height: usize,
}

impl Default for VecStorage {
    fn default() -> Self {
        Self::new(1, 1)
    }
}

impl VecStorage {
    /// Создать новую решётку заданного размера со значениями по умолчанию.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            cells: vec![Cell::default(); width * height],
            width,
            height,
        }
    }
}

impl GridStorage for VecStorage {
    fn get(&self, x: usize, y: usize) -> Option<&Cell> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.cells.get(y * self.width + x)
    }

    fn set(&mut self, x: usize, y: usize, cell: Cell) {
        assert!(
            x < self.width && y < self.height,
            "VecStorage::set: out of bounds: ({}, {}) in grid {}x{}",
            x,
            y,
            self.width,
            self.height
        );
        self.cells[y * self.width + x] = cell;
    }

    fn width(&self) -> usize {
        self.width
    }
    fn height(&self) -> usize {
        self.height
    }

    fn active_cells(&self) -> Box<dyn Iterator<Item = (usize, usize)> + '_> {
        let default = Cell::default();
        Box::new(
            self.cells
                .iter()
                .enumerate()
                .filter_map(move |(idx, cell)| {
                    if cell.value != default.value
                        || cell.born_at != default.born_at
                    {
                        Some((idx % self.width, idx / self.width))
                    } else {
                        None
                    }
                }),
        )
    }

    fn bounds(&self) -> Option<(usize, usize)> {
        Some((self.width, self.height))
    }
}

/// Размер чанка в одну сторону (64×64 ячеек).
const CHUNK_SIZE: usize = 64;

/// Чанк — квадратный блок ячеек фиксированного размера.
///
/// Хранит `Option<Cell>`: `None` для дефолтных ячеек (оптимизация памяти).
/// `non_default_count` отслеживает количество не-дефолтных ячеек для быстрой
/// проверки, нужно ли хранить чанк.
#[derive(Clone)]
struct Chunk {
    cells: Vec<Option<Cell>>,
    non_default_count: usize,
}

impl Chunk {
    /// Создать новый пустой чанк (все ячейки `None`).
    fn new() -> Self {
        Self {
            cells: vec![None; CHUNK_SIZE * CHUNK_SIZE],
            non_default_count: 0,
        }
    }

    /// Преобразовать локальные координаты в индекс внутри чанка.
    fn index(lx: usize, ly: usize) -> usize {
        ly * CHUNK_SIZE + lx
    }

    /// Есть ли в чанке хоть одна не-дефолтная ячейка?
    fn has_non_default(&self) -> bool {
        self.non_default_count > 0
    }

    /// Проверить, является ли ячейка дефолтной.
    fn is_default(cell: &Option<Cell>) -> bool {
        match cell {
            None => true,
            Some(c) => {
                let default = Cell::default();
                c.value == default.value && c.born_at == default.born_at
            }
        }
    }
}

/// Бесконечная решётка, разбитая на чанки 64×64.
///
/// Чанки создаются лениво при первом обращении через [`set`](GridStorage::set).
/// Пустые чанки не хранятся в `HashMap`.
/// Не-дефолтные ячейки хранятся как `Some(Cell)`, дефолтные — как `None`.
#[derive(Clone)]
pub struct ChunkStorage {
    chunks: HashMap<(usize, usize), Chunk>,
    default_cell: Cell,
}

impl Default for ChunkStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl ChunkStorage {
    /// Создать пустое хранилище.
    pub fn new() -> Self {
        Self {
            chunks: HashMap::new(),
            default_cell: Cell::default(),
        }
    }

    /// Итератор по координатам чанков, содержащих не-дефолтные ячейки.
    pub fn active_chunks(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.chunks
            .iter()
            .filter(|(_, chunk)| chunk.has_non_default())
            .map(|(&coords, _)| coords)
    }

    /// Преобразовать глобальные координаты в координаты чанка.
    fn chunk_coords(x: usize, y: usize) -> (usize, usize) {
        (x / CHUNK_SIZE, y / CHUNK_SIZE)
    }

    /// Преобразовать глобальные координаты в локальные внутри чанка.
    fn local_coords(x: usize, y: usize) -> (usize, usize) {
        (x % CHUNK_SIZE, y % CHUNK_SIZE)
    }

    /// Получить или создать чанк по координатам.
    fn ensure_chunk(&mut self, cx: usize, cy: usize) -> &mut Chunk {
        self.chunks.entry((cx, cy)).or_insert_with(Chunk::new)
    }

    /// Заменить содержимое ячейки, проверяя, нужно ли обновить счётчик.
    ///
    /// Возвращает старое значение ячейки (опционально).
    fn replace_cell(&mut self, x: usize, y: usize, cell: Cell) -> Option<Cell> {
        let (cx, cy) = Self::chunk_coords(x, y);
        let (lx, ly) = Self::local_coords(x, y);
        let idx = Chunk::index(lx, ly);

        let chunk = self.ensure_chunk(cx, cy);

        let was_default = Chunk::is_default(&chunk.cells[idx]);

        let default = Cell::default();
        let is_default = cell.value == default.value && cell.born_at == default.born_at;

        let old = chunk.cells[idx].take();

        match (was_default, is_default) {
            (true, false) => chunk.non_default_count += 1,
            (false, true) => chunk.non_default_count -= 1,
            _ => {}
        }

        chunk.cells[idx] = if is_default { None } else { Some(cell) };

        // Если чанк стал пустым — удаляем из HashMap (предотвращает утечку памяти)
        if chunk.non_default_count == 0 {
            self.chunks.remove(&(cx, cy));
        }

        old
    }
}

impl GridStorage for ChunkStorage {
    fn get(&self, x: usize, y: usize) -> Option<&Cell> {
        let (cx, cy) = Self::chunk_coords(x, y);
        let (lx, ly) = Self::local_coords(x, y);

        if let Some(chunk) = self.chunks.get(&(cx, cy)) {
            let idx = Chunk::index(lx, ly);
            if let Some(cell) = &chunk.cells[idx] {
                return Some(cell);
            }
        }

        Some(&self.default_cell)
    }

    fn set(&mut self, x: usize, y: usize, cell: Cell) {
        self.replace_cell(x, y, cell);
    }

    fn width(&self) -> usize {
        usize::MAX
    }
    fn height(&self) -> usize {
        usize::MAX
    }

    fn active_cells(&self) -> Box<dyn Iterator<Item = (usize, usize)> + '_> {
        let default = &self.default_cell;
        Box::new(self.chunks.iter().flat_map(move |(&(cx, cy), chunk)| {
            chunk
                .cells
                .iter()
                .enumerate()
                .filter_map(move |(idx, cell)| {
                    cell.as_ref().and_then(|c| {
                        if c.value != default.value || c.born_at != default.born_at {
                            let lx = idx % CHUNK_SIZE;
                            let ly = idx / CHUNK_SIZE;
                            Some((cx * CHUNK_SIZE + lx, cy * CHUNK_SIZE + ly))
                        } else {
                            None
                        }
                    })
                })
        }))
    }
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;

