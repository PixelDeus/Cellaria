# Changelog

## [0.2.0] — Unreleased

### Added
- **Conflict analyzer**: новый модуль `src/conflict_analyzer.rs` для статического обнаружения конфликтов между правилами (пересечение affected regions, несовместимость типов, разные min_age)
- **7 новых конфигов**:
  - `configs/composition.yaml` — композиция TM-головки + cleaner
  - `configs/oscillation.yaml` — контрпример для не-необходимости счётчикового потенциала
  - `configs/self_replication.yaml` — самовоспроизведение: цепочка `10` растёт слева
  - `configs/ca_simulation.yaml` — клеточный автомат: маркер обновляет ячейки по XOR
  - `configs/multi_head_tm.yaml` — две TM-головки на одной ленте
  - `configs/sorting.yaml` — пузырьковая сортировка (один проход)
  - `configs/propagation.yaml` — волна: граница `1...1`/`0...0` смещается вправо
- **Paper**: главы `paper/paper2.md` (termination & completeness) и `paper/paper3.md` (data model & comparisons), HTML/PDF сборка
- **Match engine**: интеграция статистического конфликт-анализа в фазу арбитража; опциональный коллбэк `on_match`

### Changed
- **Engine**: при непустом конфликт-графе разрешение коллизий через `resolve_conflicts` вместо прямого применения
- **Grid**: доработки для поддержки анализа конфликтов
- **RuleStore**: расширен API для запроса правил по маске
- **Types**: добавлены новые типы для конфликт-анализа
- **Benches**: обновлены под новую архитектуру движка

### Fixed
- Предыдущий CHANGELOG содержал описание изменений, которые не были закоммичены. История исправлена.

### Removed
- Устаревшие упоминания несуществующих конфигов в документации

## [0.1.0] — Initial release (committed to GitHub)