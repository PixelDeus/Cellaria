use std::collections::HashMap;
use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::fast_hash::{FxHashMap, FxHashSet};
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
///
/// `dirty_coords` — координаты, изменённые (через `set_cell`) с момента
/// последнего `take_dirty()`. Используется `engine::run_tick` для
/// инкрементального detect_matches: вместо пересканирования ВСЕХ активных
/// ячеек каждый тик, следующий скан ограничивается окрестностью только
/// РЕАЛЬНО изменившихся с прошлого тика клеток. `set_cell` — единственная
/// точка мутации значения ячейки в кодовой базе (см. его doc-комментарий),
/// поэтому хук здесь ловит абсолютно любое изменение независимо от вызывающего
/// кода (apply_matches, apply_input, ручная загрузка конфига, тесты).
/// `Serialize`/`Deserialize` — для [`crate::engine::EngineSnapshot`]
/// (сохранение/восстановление состояния симуляции). Явный `bound` нужен
/// потому, что derive иначе требовал бы `S: GridStorage` САМ реализовывал
/// `Serialize`/`Deserialize` только когда это фактически нужно (то есть при
/// сериализации конкретного `Grid<S>`), а не как часть трейта `GridStorage`
/// вообще — `GridStorage` не требует этого от всех реализаций, только от
/// тех, что реально сериализуются.
#[derive(Clone, Serialize, Deserialize)]
#[serde(bound(serialize = "S: Serialize", deserialize = "S: Deserialize<'de>"))]
pub struct Grid<S: GridStorage> {
    /// Внутреннее хранилище. Приватное — `Grid::set_cell` (единственная
    /// точка записи, синхронизирующая `active_coords`/`dirty_coords`)
    /// обязана быть НЕ просто конвенцией, а закрытой на уровне типов: было
    /// найдено, что несколько мест (`reset_age_for_regions`,
    /// `Engine::reset_age`, а также один бенчмарк) писали через
    /// `storage.set(...)` напрямую, в обход этой синхронизации. Только
    /// чтение (`Grid::storage()`, ниже) остаётся
    /// открытым — read-only ссылка не может обойти `set_cell` физически,
    /// раз у неё нет `&mut`.
    storage: S,
    /// Граничные буферы: координата → буфер.
    pub boundaries: HashMap<(usize, usize), BoundaryBuffer>,
    /// Cache-friendly линейный Vec активных (не-дефолтных) координат.
    /// Порядок элементов не гарантирован (swap_remove при удалении).
    active_coords_vec: Vec<(usize, usize)>,
    /// Индекс координаты в `active_coords_vec` — для O(1) contains/remove.
    /// `FxHashMap`, а не стандартный `HashMap` (SipHash): чисто внутренняя
    /// структура на горячем пути (задевается КАЖДЫМ `set_cell` — единственной
    /// точкой мутации клетки в кодовой базе), координаты не из недоверенного
    /// источника (см. `fast_hash`).
    active_index: FxHashMap<(usize, usize), usize>,
    /// Глобальный счётчик поколений. Инкрементится каждый tick.
    /// Используется для вычисления возраста ячейки: age = generation - cell.born_at.
    generation: u64,
    /// Координаты, изменённые с момента последнего `take_dirty()`. FxHashSet
    /// по той же причине, что и `active_index` — задевается каждым `set_cell`.
    dirty_coords: FxHashSet<(usize, usize)>,
}

impl<S: GridStorage + Default> Default for Grid<S> {
    fn default() -> Self {
        Self {
            storage: S::default(),
            boundaries: HashMap::new(),
            active_coords_vec: Vec::new(),
            active_index: FxHashMap::default(),
            generation: 0,
            dirty_coords: FxHashSet::default(),
        }
    }
}

impl<S: GridStorage> Grid<S> {
    /// Решётка с готовым набором активных клеток — для пустого набора см.
    /// [`Grid::from_storage`].
    pub fn new(storage: S, active_coords: HashSet<(usize, usize)>) -> Self {
        let active_coords_vec: Vec<(usize, usize)> = active_coords.iter().copied().collect();
        let active_index: FxHashMap<(usize, usize), usize> = active_coords_vec
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
            // Начальные активные ячейки ещё ни разу не сканировались —
            // считаем их "грязными", чтобы первый detect_matches увидел их
            // (эквивалентно полному скану на первом тике). Публичная
            // сигнатура принимает стандартный `HashSet` (стабильность API),
            // внутреннее поле — `FxHashSet` (см. её doc-комментарий) — здесь
            // одноразовая конвертация при конструировании, не на горячем пути.
            dirty_coords: active_coords.into_iter().collect(),
        }
    }

    /// [`Grid::new`] с пустым начальным набором активных клеток — найдено
    /// эмпирически при поиске эргономических улучшений: 103 из 104
    /// вызовов `Grid::new` в этой кодовой базе передают вторым аргументом
    /// именно `HashSet::new()`/`Default::default()`, потому что клетки
    /// затем выставляются через `set_cell` (который сам поддерживает
    /// индекс активных клеток — предзаполнять его вручную незачем).
    /// Непустой `active_coords` нужен ТОЛЬКО когда клетки уже стоят
    /// НАПРЯМУЮ в `storage` в обход `set_cell` — единственный такой
    /// случай во всей кодовой базе, `config.rs`'s YAML-загрузчик,
    /// продолжает использовать полный `Grid::new`.
    pub fn from_storage(storage: S) -> Self {
        Self::new(storage, HashSet::new())
    }

    fn active_insert(&mut self, coord: (usize, usize)) {
        if self.active_index.contains_key(&coord) {
            return;
        }
        let idx = self.active_coords_vec.len();
        self.active_coords_vec.push(coord);
        self.active_index.insert(coord, idx);
    }

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

    fn active_contains(&self, coord: &(usize, usize)) -> bool {
        self.active_index.contains_key(coord)
    }

    /// Используется в `detect_matches` для быстрого линейного прохода.
    pub fn active_coords(&self) -> &Vec<(usize, usize)> {
        &self.active_coords_vec
    }

    /// Используется `engine::run_tick` для построения инкрементального
    /// кандидатного множества для `detect_matches`.
    pub(crate) fn take_dirty(&mut self) -> FxHashSet<(usize, usize)> {
        std::mem::take(&mut self.dirty_coords)
    }

    /// Посмотреть на текущее "грязное" множество, не извлекая (не очищая)
    /// его — в отличие от `take_dirty`, безопасно вызывать многократно.
    pub(crate) fn peek_dirty(&self) -> FxHashSet<(usize, usize)> {
        self.dirty_coords.clone()
    }

    /// Явно пометить координату "грязной" без изменения значения ячейки.
    ///
    /// Нужен для случая, который `set_cell`-хук не ловит: правило может
    /// совпасть и быть принятым арбитражем, но ничего не записать (пустые
    /// `shifts`/`changes` — вырожденный, но валидный случай). Такое правило
    /// продолжает совпадать на той же клетке бесконечно (её значение не
    /// менялось и не изменится), и это должно быть видно detect_matches на
    /// каждом следующем тике — иначе после первого тика клетка выпадает из
    /// dirty-множества навсегда, и инкрементальный скан её больше не найдёт.
    pub(crate) fn mark_dirty(&mut self, x: usize, y: usize) {
        self.dirty_coords.insert((x, y));
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
            .map(|c| if c.is_default() { 0 } else { self.generation - c.born_at })
            .unwrap_or(0)
    }

    /// Увеличить возраст всех активных ячеек на 1 — O(1).
    /// Просто инкрементирует глобальный счётчик поколений.
    pub fn advance_age(&mut self) {
        self.generation += 1;
    }

    /// Read-only доступ к хранилищу — для типо-специфичных методов, которых
    /// нет в трейте `GridStorage` (например `ChunkStorage::active_chunks`).
    /// Только `&`, не `&mut` — намеренно: единственный способ писать в
    /// хранилище остаётся [`Grid::set_cell`] (см. doc-комментарий поля
    /// `storage`).
    pub fn storage(&self) -> &S {
        &self.storage
    }

    /// Ширина решётки.
    pub fn width(&self) -> usize {
        self.storage.width()
    }

    /// Высота решётки.
    pub fn height(&self) -> usize {
        self.storage.height()
    }

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
    ///
    /// Паникует, если `x`/`y` превышают `u32::MAX` — `RuleMatch::x`/`y`
    /// (`matcher.rs`) хранят координату как `u32` ради памяти на hot path,
    /// без этой проверки клетка за пределами `u32::MAX` на `ChunkStorage`
    /// молча ПОРТИТ данные другой, far-меньшей клетки вместо того, чтобы
    /// ошибиться явно (найдено целенаправленной адверсариальной проверкой)
    /// — громкая паника здесь строго лучше тихого усечения `cx as u32`
    /// дальше по пайплайну.
    pub fn set_cell(&mut self, x: usize, y: usize, cell: Cell) {
        assert!(
            x <= u32::MAX as usize && y <= u32::MAX as usize,
            "Grid::set_cell: координата ({x}, {y}) превышает u32::MAX -- \
             RuleMatch хранит координаты как u32, дальнейшая работа с такой клеткой \
             тихо усекла бы координату и попортила данные другой клетки вместо явной ошибки"
        );
        // Решение "вставить/убрать из active_coords" принимается по
        // ФАКТИЧЕСКОМУ членству в кэше (`was_in_active`), а не по
        // пересчитанному `is_default()` предыдущей ячейки — раньше здесь
        // читался `is_default()` СОХРАНЁННОЙ ячейки, что ломалось в связке с
        // `reset_age_for_regions`: та обновляет `born_at` в обход `set_cell`
        // (см. её doc-комментарий), и клетка, честно очищенная ДО этого
        // (born_at сброшен в 0, active_remove сработал корректно), после
        // такого обновления возраста хранит born_at≠0 при value=0 — то есть
        // `is_default()` для НЕЁ САМОЙ теперь лжёт "не дефолтная", хотя
        // active_coords её уже не содержит. Следующий set_cell на эту же
        // координату читал это лживое "was_default=false" и решал, что
        // clone insert не нужен — координата навсегда выпадала из
        // active_coords, даже когда в неё реально писали новое значение.
        // `active_contains` не подвержен этой порче: он не пересчитывается
        // из содержимого ячейки, а хранит фактическое состояние кэша.
        let was_in_active = self.active_contains(&(x, y));
        let is_default = cell.is_default() && !self.boundaries.contains_key(&(x, y));
        self.storage.set(x, y, cell);
        self.dirty_coords.insert((x, y));
        match (was_in_active, is_default) {
            (false, false) => {
                self.active_insert((x, y));
            }
            (true, true) => {
                self.active_remove((x, y));
            }
            _ => {}
        }
    }

    /// Итератор по активным (не-дефолтным) ячейкам.
    /// Использует кэш `active_coords_vec` — cache-friendly линейный проход.
    pub fn iter_active(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.active_coords_vec.iter().copied()
    }

    pub fn get_boundary(&self, x: usize, y: usize) -> Option<&BoundaryBuffer> {
        self.boundaries.get(&(x, y))
    }

    pub fn get_boundary_mut(&mut self, x: usize, y: usize) -> Option<&mut BoundaryBuffer> {
        self.boundaries.get_mut(&(x, y))
    }

    /// Граничная ячейка всегда добавляется в `active_coords`.
    pub fn set_boundary(&mut self, x: usize, y: usize, buf: BoundaryBuffer) {
        self.active_insert((x, y));
        self.boundaries.insert((x, y), buf);
    }

    /// Запись в `active_coords` не удаляется — если ячейка станет дефолтной,
    /// следующий же `set_cell` уберёт её из кэша.
    pub fn remove_boundary(&mut self, x: usize, y: usize) {
        self.boundaries.remove(&(x, y));
        if self.storage.get(x, y).is_none_or(|c| c.is_default()) {
            self.active_remove((x, y));
        }
    }

    pub fn iter_boundaries(&self) -> impl Iterator<Item = (&(usize, usize), &BoundaryBuffer)> {
        self.boundaries.iter()
    }

    pub fn iter_boundaries_mut(&mut self) -> impl Iterator<Item = (&(usize, usize), &mut BoundaryBuffer)> {
        self.boundaries.iter_mut()
    }

    /// Канал не привязан к координате в текущей реализации — логика каналов
    /// вынесена в `RuleStore`, здесь всегда `None`.
    pub fn get_channel(&self, _x: usize, _y: usize) -> Option<u32> {
        None
    }

    pub fn boundary_coords(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.boundaries.keys().copied()
    }
}

/// Синоним для решётки с конечным хранилищем ([`VecStorage`](crate::storage::VecStorage)).
pub type SimpleGrid = Grid<crate::storage::VecStorage>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{ChunkStorage, VecStorage};

    fn make_grid(w: usize, h: usize) -> Grid<VecStorage> {
        Grid::from_storage(VecStorage::new(w, h))
    }

    #[test]
    fn test_from_storage_matches_new_with_empty_active_coords() {
        let via_from_storage = Grid::from_storage(VecStorage::new(3, 3));
        let via_new = Grid::new(VecStorage::new(3, 3), HashSet::new());
        assert_eq!(via_from_storage.iter_active().count(), 0);
        assert_eq!(via_from_storage.iter_active().count(), via_new.iter_active().count());
        assert_eq!(via_from_storage.width(), via_new.width());
        assert_eq!(via_from_storage.height(), via_new.height());
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

    /// Регрессия для бага, найденного целенаправленной адверсариальной
    /// проверкой (не автоматикой): `RuleMatch::x`/`y` — `u32`, построены
    /// через `cx as u32` в `matcher.rs` без проверки диапазона. На
    /// `ChunkStorage` (заявленно "почти безграничной") клетка на
    /// `x = 2³²` (легальная `usize`-координата) раньше МОЛЧА усекалась до
    /// `x = 0` при матчинге — конструктивно воспроизведено: правило,
    /// сработавшее на клетке `x = 2³²`, применяло результат к координатам
    /// клетки `x = 0` вместо своей собственной, портя ЧУЖИЕ данные без
    /// единой ошибки. Теперь `Grid::set_cell` паникует СРАЗУ при попытке
    /// создать клетку за пределами `u32::MAX` — громко и на границе входа,
    /// а не тихо где-то в середине пайплайна арбитража.
    #[test]
    #[should_panic(expected = "превышает u32::MAX")]
    fn test_set_cell_panics_instead_of_silently_truncating_coordinate_beyond_u32_max() {
        let mut grid = Grid::from_storage(ChunkStorage::new());
        grid.set_cell(u32::MAX as usize + 1, 5, Cell::new(1));
    }

    /// Симметричный контроль: `x == u32::MAX` (граница, не за ней) обязана
    /// остаться легальной -- проверка `<=`, не `<`, у `Grid::set_cell`, не
    /// должна случайно отсечь последнюю валидную координату.
    #[test]
    fn test_set_cell_at_exactly_u32_max_does_not_panic() {
        let mut grid = Grid::from_storage(ChunkStorage::new());
        grid.set_cell(u32::MAX as usize, 5, Cell::new(1));
        assert_eq!(grid.get_cell(u32::MAX as usize, 5).map(|c| c.value.0 .0), Some(1));
    }
}
