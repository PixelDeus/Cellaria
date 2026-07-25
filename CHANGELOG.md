# Changelog

## [Unreleased]

### Added
- **Новые конфиги**:
  - `configs/bubble_sort_desc.yaml` — пузырьковая сортировка по убыванию
  - `configs/ca_majority.yaml` — клеточный автомат: правило большинства
  - `configs/cf_ca_counterexample.yaml` — контрпример для CA-симуляции
  - `configs/game_of_life.yaml` — игра «Жизнь» (Conway's Game of Life)
- **Benches**: добавлены бенчмарки `bench_bubble_sort`, `bench_game_of_life`, `bench_sorting` + утилиты для загрузки конфигов и настройки `ChunkStorage`

### Changed
- **Benches**: `benches/cellaria_bench.rs` расширен с ~9 строк до ~600; переработан на `Criterion` + `Instant`; добавлены новые тесты производительности и хелперы для конфигураций
- **Конфиги** (`configs/`): обновлены поля (`active_only`, `overflow`, `min_age`) в существующих конфигах; изменены паттерны и изменения в конфигах cascade, collision, composition, conflict, io, ca_simulation