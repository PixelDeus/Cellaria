//! # Cellaria
//!
//! **Вычислительная модель на принципе локальной редукции.**
//!
//! Состояние двумерной решётки изменяется только замещением локально-связанной
//! группы ячеек по фиксированному правилу. Нет процессора, нет шины, нет общей памяти.
//! Правила — это данные. Жадный арбитраж с приоритетом и возрастом.
//! Цепочечный сдвиг — единственный способ перемещения данных.
//! Граничный ввод-вывод через каналы.
//!
//! ## Архитектура
//!
//! Исходный код разделён на модули по функциональному признаку:
//!
//! - [`types`](mod@types) — базовые типы данных (ячейка, правило, сдвиг и т.д.).
//! - [`storage`](mod@storage) — абстракция хранилища решётки и две реализации
//!   ([`VecStorage`], [`ChunkStorage`]).
//! - [`grid`](mod@grid) — обёртка [`Grid`] над хранилищем с унифицированным API.
//! - [`engine`](mod@engine) — пять фаз тика симуляции
//!   (ввод, обнаружение, арбитраж, применение, сброс).
//! - [`config`](mod@config) — загрузка правил и начального состояния из YAML.
//! - [`rule_store`](mod@rule_store) — самомодифицирующееся хранилище правил.
//! - [`error`](mod@error) — единый тип ошибки [`CellariaError`].
//!
//! ## Хранение
//!
//! Два варианта хранилища через трейт [`GridStorage`](storage::GridStorage):
//! - [`VecStorage`](storage::VecStorage) — конечная решётка фиксированного размера
//! - [`ChunkStorage`](storage::ChunkStorage) — бесконечная решётка, разбитая на чанки 64×64
//!
//! ## Конфигурация
//!
//! Правила и начальное состояние загружаются из YAML-файлов через
//! [`load_config`](config::load_config). Примеры конфигов находятся в каталоге `configs/`.
//!
//! ## Примеры
//!
//! ```rust
//! use cellaria::config::load_config;
//! use cellaria::engine::run_tick;
//!
//! // Загрузка конфига с правилами
//! let (mut grid, rule_index) = load_config("configs/collision.yaml")
//!     .expect("YAML parsing failed: check configs/collision.yaml");
//! let initial = grid.get_cell(0, 0)
//!     .expect("Cell (0,0) not found")
//!     .value.0 .0;
//!
//! // Один тик симуляции
//! let (accepted, _outputs) = run_tick(&mut grid, &rule_index);
//!
//! // После тика состояние могло измениться
//! println!("Начальное: {}, совпадений: {}", initial, accepted.len());
//! ```
//!
//! ```rust
//! use cellaria::Grid;
//! use cellaria::VecStorage;
//! use cellaria::types::{Cell, CellType, CellValue};
//!
//! // Создание решётки 3×3 с VecStorage
//! let storage = VecStorage::new(3, 3);
//! use std::collections::HashSet;
//! let mut grid = Grid::new(storage, HashSet::new());
//!
//! // Установка ячейки
//! grid.set_cell(1, 1, Cell {
//!     value: CellValue(CellType(5)),
//!     born_at: 0,
//! });
//!
//! // Чтение ячейки
//! let cell = grid.get_cell(1, 1).unwrap();
//! assert_eq!(cell.value.0 .0, 5);
//!
//! // Итерация по активным ячейкам
//! let active: Vec<_> = grid.iter_active().collect();
//! assert_eq!(active.len(), 1);
//! ```
//!
//! ```rust
//! use cellaria::rule_store::RuleStore;
//!
//! // RuleStore — самомодифицирующееся хранилище правил
//! let mut store = RuleStore::new();
//! let stats = store.error_stats();
//! assert_eq!(stats, 0);
//! ```

pub mod config;
pub mod conflict_analyzer;
pub mod engine;
pub mod error;
mod fast_hash;
mod grid;
pub mod render;
pub mod tm_translator;
pub mod rule_store;
mod storage;
pub mod types;

// Явные реэкспорты публичного API
pub use config::load_config;
pub use conflict_analyzer::ConflictGraph;
pub use engine::{run_tick, Engine, TerminationVerdict, CompositionVerdict};
pub use error::CellariaError;
pub use grid::{Grid, SimpleGrid};
pub use render::{render_grid, render_grid_json};
pub use rule_store::RuleStore;
pub use storage::{ChunkStorage, GridStorage, VecStorage};
pub use types::{
    AffectedRegion, BoundaryBuffer, Cell, CellType, CellValue, ChangeValue, Direction,
    OverflowAction, Rule, RuleId, RuleMatch, ShiftSpec,
};
