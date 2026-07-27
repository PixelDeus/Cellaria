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
/// Кэш `active_coords_vec` содержит координаты всех не-дефолтных ячеек.
/// Ячейка считается не-дефолтной, если её значение ≠ 0 или born_at != 0,
/// или к ней привязан граничный буфер (BoundaryBuffer).
///
/// Возраст ячейки вычисляется как `generation - cell.born_at`.
/// `advance_age` — O(1): просто инкремент `generation`.
///
/// Для итерации используется `active_coords_vec` — cache-friendly линейный
/// Vec. Рядом с ним хранится `active_index: HashMap<coord, usize>` —
/// позиция координаты в этом Vec. Вставка — push + запись индекса, O(1).
/// Удаление — `swap_remove` по известному индексу с обновлением индекса
/// переставленного элемента, тоже O(1) амортизированно. Порядок элементов
/// в `active_coords_vec` при этом не сохраняется, но нигде в кодовой базе
/// на порядок итерации не полагаются.
///
/// (До этой оптимизации кэш был парой `HashSet` + `Vec`, и каждое удаление
/// пересобирало весь Vec из HashSet — O(A) на одно удаление. Поскольку сдвиг
/// головки всегда очищает исходную позицию, это давало O(A) на каждый сдвиг
/// вместо O(1).)
#[derive(Clone)]
pub struct Grid<S: GridStorage> {
    /// Внутреннее хранилище.
    pub storage: S,
    /// Граничные буферы: координата → буфер.
    pub boundaries: HashMap<(usize, usize), BoundaryBuffer>,
    /// Cache-friendly линейный Vec активных (не-дефолтных) координат.
    /// Порядок элементов не гарантирован (swap_remove при удалении).
    active_coords_vec: Vec<(usize, usize)>,
    /// Индекс координаты в `active_coords_vec` — для O(1) contains/remove.
    active_index: HashMap<(usize, usize), usize>,
    /// Глобальный счётчик поколений. Инкрементится каждый tick.
    /// Используется для вычисления возраста ячейки: age = generation - cell.born_at.
    generation: u64,
}

impl<S: GridStorage + Default> Default for Grid<S> {
    fn default() -> Self {
        Self {
            storage: S::default(),
            boundaries: HashMap::new(),
            active_coords_vec: Vec::new(),
            active_index: HashMap::new(),
            generation: 0,
        }
    }
}

impl<S: GridStorage> Grid<S> {
    /// Создать решётку с указанным хранилищем и предварительно заполненным
    /// кэшем активных ячеек.
    pub fn new(storage: S, active_coords: HashSet<(usize, usize)>) -> Self {
        let active_coords_vec: Vec<(usize, usize)> = active_coords.iter().copied().collect();
        let active_index: HashMap<(usize, usize), usize> = active_coords_vec
            .iter()
            .enumerate()
            .map(|(i, &coord)| (coord, i))
            .collect();
        Self {
            storage,
            boundaries: HashMap::new(),
            active_coords_vec,
            active_index,
            generation: 0,
        }
    }

    /// Вставить координату в кэш активных, если её там ещё нет. O(1).
    fn active_insert(&mut self, coord: (usize, usize)) {
        if self.active_index.contains_key(&coord) {
            return;
        }
        let idx = self.active_coords_vec.len();
        self.active_coords_vec.push(coord);
        self.active_index.insert(coord, idx);
    }

    /// Убрать координату из кэша активных через swap_remove. O(1) амортизированно.
    fn active_remove(&mut self, coord: (usize, usize)) {
        if let Some(idx) = self.active_index.remove(&coord) {
            self.active_coords_vec.swap_remove(idx);
            // swap_remove переместил последний элемент на место idx (если это
            // не был сам удаляемый элемент) — обновляем его индекс.
            if let Some(&moved) = self.active_coords_vec.get(idx) {
                self.active_index.insert(moved, idx);
            }
        }
    }

    /// Проверить, находится ли координата в кэше активных. O(1).
    fn active_contains(&self, coord: &(usize, usize)) -> bool {
        self.active_index.contains_key(coord)
    }

    /// Получить ссылку на Vec активных координат для итерации.
    /// Используется в detect_matches для быстрого линейного прохода.
    pub fn active_coords(&self) -> &Vec<(usize, usize)> {
        &self.active_coords_vec
    }

    /// Перестроить `active_index` из `active_coords_vec`.
    /// Публичный хук на случай внешней мутации Vec (если она станет доступна).
    pub fn rebuild_active_coords(&mut self) {
        self.active_index.clear();
        for (i, &coord) in self.active_coords_vec.iter().enumerate() {
            self.active_index.insert(coord, i);
        }
    }

    /// Текущее поколение (глобальный счётчик).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Вычислить возраст ячейки по координатам.
    /// Возвращает 0 для дефолтных/отсутствующих ячеек.
    pub fn get_age(&self, x: usize, y: usize) -> u64 {
        self.storage
            .get(x, y)
            .map(|c| {
                if c.is_default() {
                    0
                } else {
                    self.generation - c.born_at
                }
            })
            .unwrap_or(0)
    }

    /// Увеличить возраст всех активных ячеек на 1 — O(1).
    /// Просто инкрементирует глобальный счётчик поколений.
    pub fn advance_age(&mut self) {
        self.generation += 1;
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
        let was_in_active = self.active_contains(&(x, y));
        let was_default = self.storage.get(x, y).map_or(true, |c| c.is_default());
        let is_default = cell.is_default() && !self.boundaries.contains_key(&(x, y));
        self.storage.set(x, y, cell);
        match (was_default, is_default) {
            (true, false) => {
                // Вставка (active_insert сама проверяет, нет ли координаты уже
                // в кэше — например, она могла попасть туда через set_boundary,
                // пока хранимое значение оставалось дефолтным).
                self.active_insert((x, y));
            }
            (false, true) => {
                self.active_remove((x, y));
            }
            _ => {
                // Если ячейка была в active_coords из-за границы (не из-за storage),
                // а теперь граница удалена и значение дефолтное — убираем из кэша.
                if is_default && was_in_active {
                    self.active_remove((x, y));
                }
            }
        }
    }

    /// Итератор по активным (не-дефолтным) ячейкам.
    /// Использует кэш `active_coords_vec` — cache-friendly линейный проход.
    pub fn iter_active(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.active_coords_vec.iter().copied()
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
        self.active_insert((x, y));
        self.boundaries.insert((x, y), buf);
    }

    /// Удалить граничный буфер по координатам.
    /// Запись в active_coords не удаляется — если ячейка станет дефолтной,
    /// следующий же set_cell уберёт её из кэша.
    pub fn remove_boundary(&mut self, x: usize, y: usize) {
        self.boundaries.remove(&(x, y));
        // Если ячейка стала дефолтной — убираем из active_coords
        if self.storage.get(x, y).map_or(true, |c| c.is_default()) {
            self.active_remove((x, y));
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

    #[test]
    fn test_rebuild_active_coords() {
        let mut grid = make_grid(10, 10);

        // Добавляем ячейки
        grid.set_cell(0, 0, Cell::new(1));
        grid.set_cell(1, 1, Cell::new(2));
        grid.set_cell(2, 2, Cell::new(3));
        assert_eq!(grid.iter_active().count(), 3);

        // Удаляем одну — Vec перестраивается сразу
        grid.set_cell(1, 1, Cell::default());
        assert_eq!(grid.iter_active().count(), 2);
        assert!(grid.iter_active().any(|(x, y)| x == 0 && y == 0));
        assert!(grid.iter_active().any(|(x, y)| x == 2 && y == 2));
    }

    #[test]
    fn test_active_coords_contains_after_set() {
        let mut grid = make_grid(10, 10);

        // set_cell добавляет в active_coords
        grid.set_cell(7, 7, Cell::new(5));
        assert!(grid.active_contains(&(7, 7)));

        // set_cell с дефолтом убирает
        grid.set_cell(7, 7, Cell::default());
        assert!(!grid.active_contains(&(7, 7)));
    }
}