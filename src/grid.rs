use std::collections::HashMap;

use crate::storage::GridStorage;
use crate::types::{BoundaryBuffer, Cell};

/// Решётка ячеек с произвольным хранилищем.
///
/// Параметризована типом хранилища `S: GridStorage`.
/// Предоставляет унифицированный интерфейс для работы с ячейками.
///
/// Граничные буферы ввода-вывода вынесены в отдельную HashMap,
/// а не в каждую ячейку (пункт 3.3 рефакторинга).
pub struct Grid<S: GridStorage> {
    /// Внутреннее хранилище.
    pub storage: S,
    /// Граничные буферы: координата → буфер.
    pub boundaries: HashMap<(usize, usize), BoundaryBuffer>,
}

impl<S: GridStorage + Default> Default for Grid<S> {
    fn default() -> Self {
        Self {
            storage: S::default(),
            boundaries: HashMap::new(),
        }
    }
}

impl<S: GridStorage> Grid<S> {
    /// Создать решётку с указанным хранилищем.
    pub fn new(storage: S) -> Self {
        Self {
            storage,
            boundaries: HashMap::new(),
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
    /// в [`ChunkStorage`](crate::storage::ChunkStorage).
    pub fn set_cell(&mut self, x: usize, y: usize, cell: Cell) {
        self.storage.set(x, y, cell);
    }

    /// Итератор по активным (не-дефолтным) ячейкам.
    pub fn iter_active(&self) -> Box<dyn Iterator<Item = (usize, usize)> + '_> {
        self.storage.active_cells()
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
    pub fn set_boundary(&mut self, x: usize, y: usize, buf: BoundaryBuffer) {
        self.boundaries.insert((x, y), buf);
    }

    /// Удалить граничный буфер по координатам.
    pub fn remove_boundary(&mut self, x: usize, y: usize) {
        self.boundaries.remove(&(x, y));
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