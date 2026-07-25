use std::collections::HashMap;
use std::collections::HashSet;

use crate::storage::GridStorage;
use crate::types::{BoundaryBuffer, Cell};

/// Решётка ячеек с произвольным хранилищем.
///
/// Параметризована типом хранилища `S: GridStorage`.
/// Предоставляет унифицированный интерфейс для работы с ячейками.
///
/// Граничные буферы ввода-вывода вынесены в отдельную HashMap,
/// а не в каждую ячейку (пункт 3.3 рефакторинга).
///
/// Кэш `active_coords` содержит координаты всех не-дефолтных ячеек.
/// Ячейка считается не-дефолтной, если её значение ≠ 0 или возраст ≠ 0,
/// или к ней привязан граничный буфер (BoundaryBuffer).
#[derive(Clone)]
pub struct Grid<S: GridStorage> {
    /// Внутреннее хранилище.
    pub storage: S,
    /// Граничные буферы: координата → буфер.
    pub boundaries: HashMap<(usize, usize), BoundaryBuffer>,
    /// Кэш координат активных (не-дефолтных) ячеек.
    /// Поддерживается в актуальном состоянии методами set_cell/set_boundary.
    active_coords: HashSet<(usize, usize)>,
}

impl<S: GridStorage + Default> Default for Grid<S> {
    fn default() -> Self {
        Self {
            storage: S::default(),
            boundaries: HashMap::new(),
            active_coords: HashSet::new(),
        }
    }
}

impl<S: GridStorage> Grid<S> {
    /// Создать решётку с указанным хранилищем и предварительно заполненным
    /// кэшем активных ячеек.
    pub fn new(storage: S, active_coords: HashSet<(usize, usize)>) -> Self {
        Self {
            storage,
            boundaries: HashMap::new(),
            active_coords,
        }
    }

    /// Ширина решётки.
    pub fn width(&self) -> usize {
        self.storage.width()
    }

    /// Высота решётки.
    pub fn height(&self) -> usize {
        self.storage.height()
    }

    /// Получить ссылку на ячейку по координатам.
    pub fn get_cell(&self, x: usize, y: usize) -> Option<&Cell> {
        self.storage.get(x, y)
    }

    /// Установить значение ячейки по координатам.
    ///
    /// Единственный способ корректно синхронизировать счётчик `non_default_count`
    /// в [`ChunkStorage`](crate::storage::ChunkStorage) и кэш `active_coords`.
    ///
    /// Граничная ячейка (с привязанным BoundaryBuffer) никогда не считается дефолтной,
    /// даже если её значение 0 и возраст 0.
    pub fn set_cell(&mut self, x: usize, y: usize, cell: Cell) {
        let was_in_active = self.active_coords.contains(&(x, y));
        let was_default = self.storage.get(x, y).map_or(true, |c| c.is_default());
        let is_default = cell.is_default() && !self.boundaries.contains_key(&(x, y));
        self.storage.set(x, y, cell);
        match (was_default, is_default) {
            (true, false) => {
                self.active_coords.insert((x, y));
            }
            (false, true) => {
                self.active_coords.remove(&(x, y));
            }
            _ => {
                // Если ячейка была в active_coords из-за границы (не из-за storage),
                // а теперь граница удалена и значение дефолтное — убираем из кэша.
                if is_default && was_in_active {
                    self.active_coords.remove(&(x, y));
                }
            }
        }
    }

    /// Итератор по активным (не-дефолтным) ячейкам.
    /// Использует кэш `active_coords` — O(1) вместо O(N) по всей решётке.
    pub fn iter_active(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.active_coords.iter().copied()
    }

    /// Получить граничный буфер по координатам.
    pub fn get_boundary(&self, x: usize, y: usize) -> Option<&BoundaryBuffer> {
        self.boundaries.get(&(x, y))
    }

    /// Получить мутабельную ссылку на граничный буфер по координатам.
    pub fn get_boundary_mut(&mut self, x: usize, y: usize) -> Option<&mut BoundaryBuffer> {
        self.boundaries.get_mut(&(x, y))
    }

    /// Установить граничный буфер для координат.
    /// Граничная ячейка всегда добавляется в active_coords.
    pub fn set_boundary(&mut self, x: usize, y: usize, buf: BoundaryBuffer) {
        self.active_coords.insert((x, y));
        self.boundaries.insert((x, y), buf);
    }

    /// Удалить граничный буфер по координатам.
    /// Запись в active_coords не удаляется — если ячейка станет дефолтной,
    /// следующий же set_cell уберёт её из кэша.
    pub fn remove_boundary(&mut self, x: usize, y: usize) {
        self.boundaries.remove(&(x, y));
        // Если ячейка стала дефолтной — убираем из active_coords
        if self.storage.get(x, y).map_or(true, |c| c.is_default()) {
            self.active_coords.remove(&(x, y));
        }
    }

    /// Итератор по всем граничным буферам (координата + буфер).
    pub fn iter_boundaries(&self) -> impl Iterator<Item = (&(usize, usize), &BoundaryBuffer)> {
        self.boundaries.iter()
    }

    /// Итератор по всем граничным буферам (mut).
    pub fn iter_boundaries_mut(
        &mut self,
    ) -> impl Iterator<Item = (&(usize, usize), &mut BoundaryBuffer)> {
        self.boundaries.iter_mut()
    }

    /// Получить канал для координаты, если это граничная ячейка.
    /// В текущей реализации канал определяется из конфигурации.
    /// Для упрощения возвращаем `None` — логика каналов вынесена в RuleStore.
    pub fn get_channel(&self, _x: usize, _y: usize) -> Option<u32> {
        None
    }

    /// Все координаты граничных ячеек.
    pub fn boundary_coords(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.boundaries.keys().copied()
    }
}

/// Синоним для решётки с конечным хранилищем ([`VecStorage`](crate::storage::VecStorage)).
pub type SimpleGrid = Grid<crate::storage::VecStorage>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::VecStorage;

    fn make_grid(w: usize, h: usize) -> Grid<VecStorage> {
        let storage = VecStorage::new(w, h);
        Grid::new(storage, HashSet::new())
    }

    #[test]
    fn test_active_coords_cache() {
        let mut grid = make_grid(10, 10);

        // Пустая решётка — нет активных
        assert_eq!(grid.iter_active().count(), 0);

        // Устанавливаем не-дефолтную ячейку
        grid.set_cell(0, 0, Cell::new(1));
        assert_eq!(grid.iter_active().count(), 1);
        assert!(grid.iter_active().any(|(x, y)| x == 0 && y == 0));

        // Сбрасываем в дефолт — активных нет
        grid.set_cell(0, 0, Cell::default());
        assert_eq!(grid.iter_active().count(), 0);

        // Две активных ячейки
        grid.set_cell(0, 0, Cell::new(1));
        grid.set_cell(1, 1, Cell::new(2));
        assert_eq!(grid.iter_active().count(), 2);
    }

    #[test]
    fn test_boundary_cell_is_active() {
        let mut grid = make_grid(10, 10);

        // Устанавливаем границу на ячейку (5, 5) с дефолтным значением
        let mut buf = BoundaryBuffer::new();
        buf.direction = "input".to_string();
        let mut q = std::collections::VecDeque::new();
        q.push_back(Cell::new(42));
        buf.queues.insert(0, q);
        grid.set_boundary(5, 5, buf);

        // Ячейка активна, даже если её значение 0
        assert!(grid.iter_active().any(|(x, y)| x == 5 && y == 5));

        // set_cell с дефолтным значением не убирает её из active_coords
        grid.set_cell(5, 5, Cell::default());
        assert!(grid.iter_active().any(|(x, y)| x == 5 && y == 5));
    }

    #[test]
    fn test_iter_active_filters_removed_boundary() {
        let mut grid = make_grid(10, 10);

        let mut buf = BoundaryBuffer::new();
        buf.direction = "input".to_string();
        grid.set_boundary(3, 3, buf);

        // Активна из-за границы
        assert!(grid.iter_active().any(|(x, y)| x == 3 && y == 3));

        // Удаляем границу. Ячейка была дефолтной (значение 0, возраст 0),
        // поэтому remove_boundary убирает её из active_coords.
        grid.remove_boundary(3, 3);
        assert!(!grid.iter_active().any(|(x, y)| x == 3 && y == 3));
    }

    #[test]
    fn test_default_grid() {
        // Проверяем, что Default::default() создаёт пустой кэш
        let grid: Grid<VecStorage> = Grid::default();
        assert_eq!(grid.iter_active().count(), 0);
    }

}
