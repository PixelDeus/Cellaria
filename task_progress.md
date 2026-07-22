# Cellaria Refactoring — Task Progress

## Stage 0 — Correctness (критические баги)
- [ ] 0.1: Fix `ChunkStorage::set` / `get_mut` counter sync в engine
- [ ] 0.2: Валидация правил в `load_config` (center, result_len, chain_length)
- [ ] 0.2: Валидация правил в `deserialize_packet` (center, result_len, chain_length)
- [ ] 0.3: Документация result < pattern в `apply_matches` (уже реализовано)

## Stage 1 — Reliability (нет паник в production)
- [x] 1.1: Убрать `unwrap()` из engine.rs (заменено на `expect`)
- [x] 1.2: `thiserror` + `CellariaError` уже добавлены
- [ ] 1.3: `load_config` → `Result<_, CellariaError>`
- [ ] 1.4: `MAX_BUFFER_SIZE = 1024` в RuleStore

## Stage 2 — Code Quality (стандарты Rust)
- [x] 2.1: Clippy lint `unwrap_used = "warn"` уже в Cargo.toml
- [x] 2.2: Pinned versions уже в Cargo.toml
- [x] 2.3: Explicit re-exports уже в lib.rs
- [ ] 2.4: `impl Default` для `RuleStore`

## Stage 3 — Инкапсуляция и API
- [ ] 3.1: `Grid<S>.storage` приватный + `iter_active()`
- [x] 3.2: `GridStorage::bounds()` уже есть
- [ ] 3.3: `BoundaryBuffer` → `HashMap<GridCoord, BoundaryBuffer>` в Grid
- [ ] 3.4: `RuleStore::decode_errors` приватный + `error_stats()`

## Stage 4 — Performance
- [x] 4.1: Spatial indexing (уже есть, улучшать не нужно)
- [x] 4.2: BinaryHeap в arbitrate уже реализован
- [ ] 4.3: `ChunkStorage::get_mut` — lazy init (не материализовать при первом доступе)
- [x] 4.4: Убраны `collect::<Vec<_>>()` в фазах движка

## Stage 5 — Extensibility
- [x] 5.1: `ShiftDirection` → `Direction` уже обобщён
- [ ] 5.2: Hook `on_match` для RuleMatch
- [ ] 5.3: Protocol RuleStore поддержка `min_age` и `shift`

## Stage 6 — CLI и DevEx
- [ ] 6.1: `clap` в main.rs
- [ ] 6.2: Флаг `--json`
- [ ] 6.3: Criterion benchmarks

## Stage 7 — Тесты и документация
- [ ] 7.1: Тесты граничных буферов
- [ ] 7.2: Тесты валидации правил
- [ ] 7.3: Doc-тесты публичного API
- [ ] 7.4: CHANGELOG.md