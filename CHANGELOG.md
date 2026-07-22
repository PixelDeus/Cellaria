# Changelog

## [0.2.0] - 2026-07-22

### Added
- **Error handling**: единый тип `CellariaError` с `thiserror`, `load_config` возвращает `Result<_, CellariaError>`
- **Validation**: проверка наличия центра `(0, 0)` в паттерне, соответствие длин `result_cells` и `pattern`, `chain_length > 0` при наличии shift
- **RuleStore**: константа `MAX_BUFFER_SIZE = 1024` с автоочисткой буфера при переполнении; поддержка `min_age` и `shift` в пакете `AddRule`
- **Grid API**: `iter_active()` для итерации активных ячеек, `bounds()` для получения размеров хранилища
- **BoundaryBuffer**: вынесен в `HashMap<GridCoord, BoundaryBuffer>` в Grid, убран `Option` из каждой ячейки
- **Direction**: новая структура `Direction(i8, i8)` вместо enum `ShiftDirection` с константами NORTH/SOUTH/EAST/WEST
- **Engine hook**: опциональный `on_match: Option<Box<dyn Fn(&RuleMatch)>>`
- **CLI**: парсинг через `clap` с флагами `--ticks` и `--json`
- **Benchmarks**: бенчмарки для `detect_matches`, `arbitrate`, `apply_matches`, `run_tick`
- **RuleStore API**: `error_stats()` для получения статистики ошибок декодирования

### Changed
- **Инкапсуляция**: `Grid<S>.storage` сделан приватным, доступ через публичные методы
- **RuleStore**: `decode_errors` — приватное поле
- **Imports**: явные re-exports в `lib.rs` вместо wildcard
- **ChunkStorage**: `get_mut` не материализует ячейку при первом обращении (lazy init on write)
- **Производительность**: убраны промежуточные `collect::<Vec<_>>()` в фазах движка

### Fixed
- **ChunkStorage::set**: при изменении ячейки через `get_mut` + ручное присвоение счётчик `non_default_count` не синхронизировался. Исправлено: убрана прямая мутация `cells[idx]`, все изменения через `set()`
- **RuleStore**: очистка буфера канала при превышении `MAX_BUFFER_SIZE`
- **All unwrap()**: заменены на `expect` с контекстом или `?`

### Removed
- `ShiftDirection` enum (заменён на `Direction(i8, i8)`)
- `boundary` поле из `Cell` (вынесено в `HashMap` в `Grid`)
- Wildcard re-exports в `lib.rs`

## [0.1.0] - Initial release