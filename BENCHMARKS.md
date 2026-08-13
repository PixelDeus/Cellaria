# Cellaria Benchmark Suite — Руководство

## Быстрый запуск

```bash
# ========== Кастомные бенчмарки (через cargo bench) ==========

# Полный прогон (все фазы, ~30-40 секунд)
cargo bench --bench cellaria_bench

# Быстрый прогон (~5-10 секунд)
cargo bench --bench cellaria_bench -- --quick

# Компактный вывод (одна строка на тест)
cargo bench --bench cellaria_bench -- --compact

# Тихий режим (только сводка)
cargo bench --bench cellaria_bench -- --quiet

# Вывод в JSON
cargo bench --bench cellaria_bench -- --json

# Сохранить в файл
cargo bench --bench cellaria_bench -- --json --output bench_results.json

# Сравнить с эталоном
cargo bench --bench cellaria_bench -- --check bench_baseline.json

# ========== Criterion-бенчмарки (через cargo bench) ==========
# ВНИМАНИЕ: флаг именно --criterion, не --bench -- cargo bench САМА всегда
# дописывает --bench в конец argv (стандартное поведение для bench-таргетов),
# так что --bench не может использоваться для выбора ветки Criterion — она
# присутствует в любом запуске независимо от намерения пользователя. См.
# doc-комментарий в начале benches/cellaria_bench.rs.

# Полный прогон Criterion (может быть долгим — 100 семплов)
cargo bench --bench cellaria_bench -- --criterion

# Быстрый прогон Criterion (10 семплов, 2 сек замера, отдельный baseline
# от полного прогона -- см. --criterion --quick's doc-комментарий в коде)
cargo bench --bench cellaria_bench -- --criterion --quick

# Фильтрация по имени группы Criterion СЕЙЧАС НЕ ПОДДЕРЖИВАЕТСЯ через
# --criterion (см. doc-комментарий в cellaria_bench.rs: Criterion больше не
# читает argv сама, чтобы не конфликтовать с --criterion/--quick/--bench от
# Cargo) -- для фильтрации нужен отдельный вызов скомпилированного
# бинарника бенчмарка напрямую, не через `cargo bench`.

# ========== Комбинации ==========

# Быстрый компактный прогон
cargo bench --bench cellaria_bench -- --quick --compact

# Сохранить baseline
cargo bench --bench cellaria_bench -- --json --output bench_baseline.json

# Проверить регрессию
cargo bench --bench cellaria_bench -- --check bench_baseline.json --compact
```

## Режимы вывода

| Флаг | Описание | Когда использовать |
|------|----------|-------------------|
| *(без флагов)* | Полный: таблицы с разделителями, заголовки фаз, цвета | Детальный анализ |
| `--compact` | Компактный: одна строка на тест, цвета | Быстрая проверка |
| `--quiet` | Тихий: только сводка в конце | CI/CD |
| `--json` | JSON-вывод | Машинная обработка |
| `--quick` | Быстрый: меньше итераций в бенчмарках | Разработка |
| `--quick --compact` | Максимально быстрый прогон | Частая верификация |

## Пример вывода

### Full mode (`cargo bench --bench cellaria_bench`)

```
═══ ФАЗА 1: Максимальный throughput ═══
─── 1A: Без сдвига (N×N, чередование, apply) ───
┌──────────┬──────────────┬──────────────────┬────────┐
│ Параметр │ Значение     │ Дополнительно    │ Статус │
├──────────┼──────────────┼──────────────────┼────────┤
│     N=10 │         45 тиков │ tps: 450        │ PASS   │
│    N=500 │    124 750 тиков │ tps: 725 784    │ PEAK   │
└──────────┴──────────────┴──────────────────┴────────┘

═══ СВОДКА ═══
  Всего тестов:    42
  ✓ Пройдено:      41
  ★ Пиковых:       1
  ✓ Время:         35.042s
```

### Compact mode (`cargo bench --bench cellaria_bench -- --compact`)

```
  1A               N=10       45 тиков          tps: 450         PASS
  1A               N=50       1 225 тиков       tps: 12 249      PASS
  1A               N=500      124 750 тиков     tps: 725 784     PEAK
  ...
═══ СВОДКА ═══  Всего: 42  ✓41  ★1  35.042s
```

### Quiet mode (`cargo bench --bench cellaria_bench -- --quiet`)

```
═══ СВОДКА ═══  Всего: 42  ✓41  ★1  35.042s
```

## Структура бенчмарков

### Фаза 1: Максимальный throughput
- **1A**: Без сдвига (N×N, чередование, apply)
- **1B**: Со сдвигом (TM-лента длины N)
- **1C**: Конфликт (M правил на одной ячейке)
- **1D**: Пустой тик (0 активных ячеек)
- **1E**: Одна ячейка, одно правило
- **1F**: Длинная цепочка сдвигов (N)

### Фаза 2: Разложение по фазам
- detect_matches, arbitrate, apply_matches, advance_age, reset_age
- Полный run_tick, overhead

### Фаза 3: Память
- VecStorage: оценочный размер N×N
- ChunkStorage: оценочный размер N×N (32×32 chunks)

### Фаза 4: Сложность правил
- **4A**: Размер паттерна
- **4B**: Правил на один head-тип

### Фаза 5: Профилирование find_rule
- Среднее время поиска правила (10000 итераций)

## Criterion-бенчмарки

Доступны через `cargo bench --bench cellaria_bench -- --criterion` (не
`--bench` — см. предупреждение выше):

| Группа | Описание |
|--------|----------|
| `tm` | TM-симуляция (100 шагов) |
| `tag` | Tag system (20 шагов) |
| `conflict_free` | Conflict-free (32 ячейки) |
| `worst_case` | Worst-case arbitration |
| `storage` | Vec vs Chunk storage |
| `grid_growth` | Grid growth N×N |
| `rule_count` | Rule count overhead |
| `replication` | Replication chain |
| `throughput_no_shift` | Throughput без сдвига |
| `throughput_with_shift` | Throughput со сдвигом |
| `throughput_conflict` | Throughput с конфликтами |
| `empty_tick` | Пустой тик |
| `single_cell` | Одна ячейка |
| `long_shift_chain` | Длинная цепочка сдвигов |
| `pattern_size` | Размер паттерна |
| `rules_per_head` | Правил на head-тип |
| `find_rule` | Поиск правила |

## Baseline

Для отслеживания регрессий:

```bash
# Сохранить текущие результаты как эталон
cargo bench --bench cellaria_bench -- --json --output bench_baseline.json

# Позже проверить, не упала ли производительность
cargo bench --bench cellaria_bench -- --check bench_baseline.json
```

Baseline проверяет, что значения не превышают 150% от эталона.

## CI/CD

Для CI/CD рекомендуется:

```bash
# Быстрая проверка (только сводка)
cargo bench --bench cellaria_bench -- --quiet --json --output bench_results.json

# Или с проверкой baseline
cargo bench --bench cellaria_bench -- --quiet --check bench_baseline.json --json
```

## Результаты измерений (ИСТОРИЧЕСКИЙ СНИМОК, устарел)

**Эта секция — снимок с ранней стадии проекта** (после первого раунда
оптимизаций: spatial hashing, Rayon, neighbourhood cache, упаковка
паттерна в u64, RuleDataCache), маленькие решётки (10×10 .. 500×500). С
тех пор добавлены dirty-tracking (тик масштабируется по АКТИВНОЙ области,
не номинальному размеру решётки), GPU-бэкенд, spatial band-split
арбитража на больших наборах матчей — числа ниже НЕ отражают текущее
поведение движка и не сопоставимы с ним напрямую. За актуальными,
датированными измерениями городского масштаба (плотность/разрежённость/
GPU, вплоть до 1M клеток) — смотрите `CHANGELOG.md`, записи "Замер
производительности"/"Замер плотной производительности". Секция ниже
оставлена как есть (не переизмерена) — историческая ценность в том, что
показывает НАПРАВЛЕНИЕ первых оптимизаций, не текущие абсолютные цифры.

### Criterion-бенчмарки (абсолютные значения)

| Группа | Параметр | Время | Изменение |
|--------|----------|-------|-----------|
| `storage/vec_10x10` | 10×10 | 418.24 ns | **-15%** |
| `storage/vec_100x100` | 100×100 | 32.080 µs | **-10%** |
| `storage/vec_500x500` | 500×500 | 1.9948 ms | **-27%** |
| `storage/chunk_10x10` | 10×10 | 119.26 µs | **-13%** |
| `storage/chunk_100x100` | 100×100 | 12.504 ms | **-22%** |
| `storage/chunk_500x500` | 500×500 | 309.45 ms | **-27%** |
| `grid_growth/N_10` | 10×10 | 428.89 ns | **-24%** |
| `grid_growth/N_100` | 100×100 | 31.610 µs | **-22%** |
| `grid_growth/N_500` | 500×500 | 1.8364 ms | **-12%** |
| `rule_count/K_1` | 1 rule | 444.87 ns | **-6%** |
| `rule_count/K_10` | 10 rules | 12.022 µs | **-22%** |
| `rule_count/K_50` | 50 rules | 191.51 µs | **-21%** |
| `rule_count/K_100` | 100 rules | 726.26 µs | **-14%** |
| `replication/len_1` | length 1 | 448.52 ns | **-19%** |
| `replication/len_10` | length 10 | 3.2065 µs | **-18%** |
| `replication/len_50` | length 50 | 14.540 µs | **-6%** |
| `replication/len_100` | length 100 | 31.023 µs | **-15%** |
| `pattern_size/size_1` | 1 cell | 1.2592 µs | **-28%** |
| `pattern_size/size_2` | 2 cells | 1.4354 µs | **-19%** |
| `pattern_size/size_4` | 4 cells | 2.1431 µs | **-5%** |
| `pattern_size/size_9` | 9 cells | 2.8389 µs | **-13%** |
| `throughput_with_shift/N_500` | 500×500 | 103.64 ms | **-4%** |
| `rules_per_head/K_1` | 1 rule/head | 1.3499 µs | *—* (noise) |
| `rules_per_head/K_10` | 10 rules/head | 3.5052 µs | **-9%** |
| `rules_per_head/K_50` | 50 rules/head | 13.761 µs | *—* (noise) |
| `rules_per_head/K_100` | 100 rules/head | 24.114 µs | **-11%** |
| `find_rule/K_1` | 1 rule | 12.450 ns | *—* (noise) |
| `find_rule/K_10` | 10 rules | 12.223 ns | *—* (noise) |
| `find_rule/K_50` | 50 rules | 12.036 ns | *—* (noise) |
| `find_rule/K_100` | 100 rules | 11.915 ns | *—* (noise) |
| `find_rule/K_200` | 200 rules | 12.144 ns | *—* (noise) |

**Примечание:** `find_rule` показывает ~12 ns независимо от количества правил — HashMap даёт O(1).

### Фаза 1: Максимальный throughput

| # | Тест | N/M | Тиков | tps | Статус |
|---|------|-----|-------|-----|--------|
| 1A | Без сдвига | 10 | 45 | 450 | PASS |
| 1A | Без сдвига | 50 | 1 225 | 12 249 | PASS |
| 1A | Без сдвига | 100 | 4 950 | 49 495 | PASS |
| 1A | Без сдвига | 200 | 19 900 | 198 964 | PASS |
| 1A | Без сдвига | 500 | 124 750 | 725 784 | **PEAK** |
| 1B | Со сдвигом | 10 | 11 | 110 | PASS |
| 1B | Со сдвигом | 100 | 101 | 1 009 | PASS |
| 1B | Со сдвигом | 1 000 | 280 | 1 704 | PASS |
| 1B | Со сдвигом | 5 000 | 1 381 | 294 | PASS |
| 1C | Конфликт | 10 | 1 | 10 | PASS |
| 1C | Конфликт | 50 | 1 | 10 | PASS |
| 1C | Конфликт | 100 | 1 | 10 | PASS |
| 1C | Конфликт | 200 | 1 | 10 | PASS |
| 1D | Пустой тик | — | 28 263 004 | 28.26 млн/с | PASS |
| 1E | Одна ячейка | — | 1 | ~1 | PASS |
| 1F | Цепочка сдвигов | 10 | 11 | 110 | PASS |
| 1F | Цепочка сдвигов | 100 | 101 | 1 010 | PASS |
| 1F | Цепочка сдвигов | 500 | 425 | 4 155 | PASS |

### Фаза 2: Разложение по фазам

| N | Фаза | ns | % |
|---|------|----|----|
| 10 | detect_matches | 700 | 70.0% |
| 10 | arbitrate | 200 | 20.0% |
| 10 | apply_matches | 100 | 10.0% |
| 10 | **Сумма фаз** | 1 000 | |
| 10 | **Полный run_tick** | 800 | |
| 10 | Overhead | 200 | 25.0% |
| 100 | detect_matches | 25 700 | **98.8%** |
| 100 | **Сумма фаз** | 26 000 | |
| 100 | **Полный run_tick** | 25 700 | |
| 500 | detect_matches | 2 524 700 | **100.0%** |
| 500 | **Сумма фаз** | 2 525 000 | |
| 500 | **Полный run_tick** | 1 906 200 | |
| 500 | Overhead | 618 800 | 32.5% |

### Фаза 3: Память

| N | VecStorage | ChunkStorage |
|---|-----------|-------------|
| 10 | 0.00 MB | 4 KB |
| 100 | 0.16 MB | 64 KB |
| 500 | 4.00 MB | 1 024 KB |
| 1 000 | 16.00 MB | 4 096 KB |
| 5 000 | — | ~96 MB |

### Фаза 4: Сложность правил

| # | Параметр | Время (µs) |
|---|----------|-----------|
| 4A | size=1 | 5 |
| 4A | size=2 | 2 |
| 4A | size=4 | 2 |
| 4A | size=9 | 5 |
| 4B | K=1 | 1 |
| 4B | K=10 | 1 |
| 4B | K=50 | 5 |
| 4B | K=100 | 8 |
| 4B | K=200 | 15 |

### Фаза 5: Профилирование find_rule

| K | ns/поиск | Найдено |
|---|---------|---------|
| 1 | 12 | 10 000 |
| 10 | 12 | 10 000 |
| 50 | 12 | 10 000 |
| 100 | 12 | 10 000 |
| 200 | 12 | 10 000 |
| 500 | 12 | 10 000 |

## Анализ

### Влияние применённых оптимизаций

1. **Spatial hashing / inverted index** — `detect_matches` теперь фильтрует ячейки по типу: если для типа нет правил — пропускает. На смешанных сетках (типы 0 и 1) это снижает нагрузку в 2×.
2. **Neighbourhood cache** — для каждой ячейки соседи (паттерн) вычисляются один раз и кэшируются. При нескольких правилах на один head-тип повторный вызов `grid.get_cell` не происходит. Ускорение `pattern_size` на **13–28%**.
3. **Rayon (parallel iter)** — `detect_matches` разбивает активные ячейки на 4–8 потоков. Ускорение `grid_growth` на **12–24%**, `rule_count` на **6–22%**.
4. **Упаковка паттерна в u64** — сравнение паттерна одной инструкцией вместо цикла. Дало 5–28% на `pattern_size`.
5. **RuleDataCache (предвычисление affected regions)** — `RuleData` (affected cells, bbox, pattern cells, total shift) вычисляется один раз и кэшируется в `HashMap<RuleId, RuleData>`. Используется в `arbitrate` (вместо `get_match_affected_cells` с повторным вызовом `compute_affected_cells`) и в `apply_matches` (вместо inline-вычисления bbox). Ускорение `throughput_with_shift/N_500` на **4%**, остальные тесты в пределах шума (кэш строится один раз на тик, микротесты меряют отдельные операции).

### Оставшиеся узкие места

- **Bottleneck #1: `detect_matches` (98–100% времени)** — даже с параллелизацией доминирует.
  - Сканирует все активные ячейки
  - Для каждой ячейки: `find_rule` (~12 ns) + сопоставление паттерна
  - Сложность: O(A × R_h) → после группировки правил ~O(A)
- **Bottleneck #2: Сдвиги** — сдвиг затрагивает O(N) ячеек последовательно (Rayon не помогает из-за цепочечной зависимости). Для N=5 000 throughput падает до 294 тиков/сек.
- **Bottleneck #3: `VecStorage`** — на больших сетках (500×500) ~2 ms vs ChunkStorage ~309 ms (ChunkStorage медленнее из-за chunk-оверхеда на маленьких сетках, но на разреженных больших сетках ChunkStorage выигрывает по памяти).

### Теоретические пределы

- **Максимум**: 28 млн пустых тиков/сек (35 ns/tick)
- **Практический предел**: ~725 000 тиков/сек (N=500, detect_matches)
- **Узкое место**: сканирование активных ячеек O(A) с константой ~100 ns/ячейка (было ~120 ns/ячейка до оптимизаций)
