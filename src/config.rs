use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;

use crate::error::CellariaError;
use crate::grid::Grid;
use crate::storage::VecStorage;
use crate::types::{
    BoundaryBuffer, Cell, CamSearch, CellType, CellValue, ChangeValue, Direction, FeedbackSpec,
    MemorySpec, OverflowAction, RecordTrigger, RecordedValue, RecursionSpec, Rule, ShiftSpec,
};

// === Вспомогательный тип — входной шаблон конфига ===

/// YAML-формат записи сдвига.
#[derive(Debug, Deserialize)]
struct YamlShift {
    direction: String,
    steps: u16,
    /// См. `types::ShiftSpec::broadcast`.
    #[serde(default)]
    broadcast: bool,
    /// См. `types::ShiftSpec::keep_source` ("излучение" при `broadcast: true`).
    #[serde(default)]
    keep_source: bool,
}

/// YAML-формат группы сдвигов приоритета.
#[derive(Debug, Deserialize)]
struct YamlShiftGroup {
    group: Vec<YamlShift>,
}

/// Одно изменение: (смещение_x, смещение_y, значение).
/// Значение может быть числом (литерал) или строкой вида "$0" (ссылка на паттерн).
#[derive(Debug, Clone, Deserialize)]
struct YamlChange {
    dx: i32,
    dy: i32,
    value: serde_yaml::Value,
}

/// YAML-формат одной записи двумерного паттерна.
#[derive(Debug, Deserialize)]
struct YamlPatternEntry {
    offset: [i8; 2],
    #[serde(rename = "type")]
    cell_type: u8,
}

/// YAML-формат content-addressable поиска — см. `types::CamSearch`.
#[derive(Debug, Deserialize)]
struct YamlCam {
    radius: u8,
    target_type: u8,
}

/// YAML-формат обратной связи по результату — см. `types::FeedbackSpec`.
#[derive(Debug, Deserialize)]
struct YamlFeedback {
    timeout: u64,
    new_direction: String,
}

/// YAML-формат ограниченной рекурсии — см. `types::RecursionSpec`.
#[derive(Debug, Deserialize)]
struct YamlRecursion {
    max_depth: u8,
    direction: String,
}

/// YAML-формат памяти по последовательности — см. `types::MemorySpec`.
///
/// `trigger`: `"neighbor_type"` (требует `neighbor_direction`) или
/// `"rule_outcome"` (не должен задавать `neighbor_direction`).
/// `match_pattern`: список чисел (тип клетки, для `neighbor_type`) или
/// строк `"applied"`/`"missed"` (для `rule_outcome`) — какой вариант,
/// определяется значением `trigger`, см. `parse_recorded_value`.
#[derive(Debug, Deserialize)]
struct YamlMemory {
    window: usize,
    trigger: String,
    #[serde(default)]
    neighbor_direction: Option<String>,
    match_pattern: Vec<serde_yaml::Value>,
}

/// YAML-формат одного правила.
#[derive(Debug, Deserialize)]
struct YamlRule {
    /// Внутренняя область — последовательность чисел (n-кортеж).
    id: Vec<u8>,
    /// Двумерный паттерн (опционально, расширение одномерного id).
    #[serde(default)]
    pattern: Vec<YamlPatternEntry>,
    /// Приоритет.
    priority: u32,
    /// Сдвиги: каждая группа — Vec<ShiftSpec>.
    #[serde(default)]
    shifts: Vec<YamlShiftGroup>,
    /// Изменения ячеек.
    changes: Vec<YamlChange>,
    /// Если true — проверять только в активных ячейках.
    #[serde(default)]
    active_only: bool,
    /// Минимальный возраст ячейки-центра для активации правила.
    #[serde(default)]
    min_age: u64,
    /// Действие при overflow (выходе за границу решётки).
    #[serde(default)]
    overflow: OverflowAction,
    /// Content-addressable поиск с ограниченным радиусом.
    #[serde(default)]
    cam: Option<YamlCam>,
    /// Опциональный тай-брейк для арбитража при равном приоритете — см.
    /// `Rule::tie_break`.
    #[serde(default)]
    tie_break: u32,
    /// Защита от голодания при разном приоритете — см. `Rule::starvation_after`.
    #[serde(default)]
    starvation_after: Option<u32>,
    /// Обратная связь по результату — см. `Rule::feedback`.
    #[serde(default)]
    feedback: Option<YamlFeedback>,
    /// Ограниченная рекурсия в пределах одного тика — см. `Rule::recursion`.
    #[serde(default)]
    recursion: Option<YamlRecursion>,
    /// Память по последовательности прошлых наблюдений — см. `Rule::memory`.
    #[serde(default)]
    memory: Option<YamlMemory>,
}

/// YAML-формат начальной ячейки.
#[derive(Debug, Deserialize)]
struct YamlCell {
    coord: [usize; 2],
    #[serde(rename = "type")]
    cell_type: u8,
}

/// YAML-формат граничного буфера.
#[derive(Debug, Deserialize)]
struct YamlBoundary {
    cell: [usize; 2],
    channel: u32,
    direction: String,
    max_queue: Option<u8>,
}

/// YAML-формат секции grid.
#[derive(Debug, Deserialize)]
struct YamlGrid {
    width: usize,
    height: usize,
    default_cell_type: u8,
    initial_cells: Vec<YamlCell>,
    boundaries: Option<Vec<YamlBoundary>>,
}

/// YAML-формат конфигурации.
#[derive(Debug, Deserialize)]
struct YamlConfig {
    grid: YamlGrid,
    rules: Vec<YamlRule>,
}

// === Конвертеры ===

/// Преобразовать название направления в Direction.
fn parse_direction(s: &str) -> Result<Direction, CellariaError> {
    match s.to_lowercase().as_str() {
        "up" | "north" => Ok(Direction::Up),
        "down" | "south" => Ok(Direction::Down),
        "left" | "west" => Ok(Direction::Left),
        "right" | "east" => Ok(Direction::Right),
        other => Err(CellariaError::Config(format!(
            "Invalid direction: {}",
            other
        ))),
    }
}

/// Преобразовать serde_yaml::Value в ChangeValue.
fn parse_change_value(value: &serde_yaml::Value) -> Result<ChangeValue, CellariaError> {
    match value {
        serde_yaml::Value::Number(n) => {
            let v = n
                .as_u64()
                .ok_or_else(|| CellariaError::Config("Invalid number in changes".to_string()))?;
            if v > 255 {
                return Err(CellariaError::Config(format!(
                    "Change value {} exceeds 255",
                    v
                )));
            }
            Ok(ChangeValue::Literal(v as u8))
        }
        serde_yaml::Value::String(s) => {
            if let Some(rest) = s.strip_prefix('$') {
                let idx: usize = rest
                    .parse()
                    .map_err(|_| CellariaError::Config(format!("Invalid pattern ref: {}", s)))?;
                Ok(ChangeValue::Ref(idx))
            } else {
                Err(CellariaError::Config(format!(
                    "Invalid change value: {} (use number or $N)",
                    s
                )))
            }
        }
        other => Err(CellariaError::Config(format!(
            "Invalid change value type: {:?}",
            other
        ))),
    }
}

/// Преобразовать serde_yaml::Value в RecordedValue — число → тип клетки
/// (для `NeighborType`), строка `"applied"`/`"missed"` → исход арбитража
/// (для `RuleOutcome`). См. `types::RecordedValue`.
fn parse_recorded_value(value: &serde_yaml::Value) -> Result<RecordedValue, CellariaError> {
    match value {
        serde_yaml::Value::Number(n) => {
            let v = n.as_u64().ok_or_else(|| {
                CellariaError::Config("Invalid number in memory match_pattern".to_string())
            })?;
            if v > 255 {
                return Err(CellariaError::Config(format!(
                    "memory match_pattern cell type {} exceeds 255",
                    v
                )));
            }
            Ok(RecordedValue::Type(CellType::new(v as u8)))
        }
        serde_yaml::Value::String(s) => match s.to_lowercase().as_str() {
            "applied" => Ok(RecordedValue::Applied),
            "missed" => Ok(RecordedValue::Missed),
            other => Err(CellariaError::Config(format!(
                "Invalid memory match_pattern string: {} (expected \"applied\" or \"missed\")",
                other
            ))),
        },
        other => Err(CellariaError::Config(format!(
            "Invalid memory match_pattern entry: {:?}",
            other
        ))),
    }
}

/// Собрать `MemorySpec` из YAML-формата — разбирает `trigger`, проверяет
/// его согласованность с `neighbor_direction`, разбирает `match_pattern`
/// и проверяет, что его длина равна `window` (см. `types::MemorySpec`'s
/// doc-комментарий про гейт-семантику "буфер полон и совпадает целиком").
fn build_memory_spec(m: YamlMemory) -> Result<MemorySpec, CellariaError> {
    let record_trigger = match m.trigger.to_lowercase().as_str() {
        "neighbor_type" => {
            let dir_str = m.neighbor_direction.as_ref().ok_or_else(|| {
                CellariaError::RuleValidation(
                    "Rule with `memory` trigger `neighbor_type` requires `neighbor_direction`"
                        .to_string(),
                )
            })?;
            RecordTrigger::NeighborType(parse_direction(dir_str)?)
        }
        "rule_outcome" => {
            if m.neighbor_direction.is_some() {
                return Err(CellariaError::RuleValidation(
                    "Rule with `memory` trigger `rule_outcome` must not set `neighbor_direction`"
                        .to_string(),
                ));
            }
            RecordTrigger::RuleOutcome
        }
        other => {
            return Err(CellariaError::RuleValidation(format!(
                "Invalid memory trigger: {} (expected \"neighbor_type\" or \"rule_outcome\")",
                other
            )))
        }
    };
    let match_pattern: Vec<RecordedValue> = m
        .match_pattern
        .iter()
        .map(parse_recorded_value)
        .collect::<Result<_, _>>()?;
    if match_pattern.len() != m.window {
        return Err(CellariaError::RuleValidation(format!(
            "Rule with `memory` must have match_pattern.len() == window, found {} vs {}",
            match_pattern.len(),
            m.window
        )));
    }
    // Валидация: `match_pattern`'s варианты обязаны соответствовать
    // `record_trigger` — найдено при аудите: `NeighborType` кладёт в буфер
    // ТОЛЬКО `RecordedValue::Type(_)` (см. `engine/mod.rs`'s push-логику,
    // `RecordTrigger::NeighborType(dir) => ... RecordedValue::Type(cell_type)`),
    // `RuleOutcome` кладёт ТОЛЬКО `Applied`/`Missed` — никогда наоборот, ни
    // при каких обстоятельствах. Без этой проверки `trigger: neighbor_type`
    // с `match_pattern: [applied, missed]` (или наоборот) молча грузился бы
    // как валидный конфиг, но гейт НИКОГДА не открылся бы: буфер и паттерн
    // сравниваются поэлементно (`PartialEq` на `RecordedValue`), а разные
    // варианты enum никогда не равны — правило с `memory` тихо превращалось
    // бы в мёртвый код, без единой ошибки ни при загрузке, ни во время
    // выполнения.
    let shape_ok = match record_trigger {
        RecordTrigger::NeighborType(_) => match_pattern.iter().all(|v| matches!(v, RecordedValue::Type(_))),
        RecordTrigger::RuleOutcome => match_pattern.iter().all(|v| matches!(v, RecordedValue::Applied | RecordedValue::Missed)),
    };
    if !shape_ok {
        return Err(CellariaError::RuleValidation(format!(
            "Rule with `memory` trigger {:?} has a match_pattern entry of the wrong shape -- `neighbor_type` only ever records cell types, `rule_outcome` only ever records applied/missed, so a mismatched entry could never match and the gate would stay closed forever",
            record_trigger
        )));
    }
    Ok(MemorySpec { window: m.window, record_trigger, match_pattern })
}

/// Результат загрузки конфига: решётка + индекс правил по типу центра.
pub type ConfigResult = Result<(Grid<VecStorage>, RuleIndex), CellariaError>;

/// Индекс правил: отображение CellType → Vec<(Rule, правило_подходит_к_активным_ячейкам)>.
pub type RuleIndex = HashMap<CellType, Vec<Rule>>;

pub fn load_config(path: &str) -> ConfigResult {
    let content = fs::read_to_string(path)?;
    let yaml: YamlConfig = serde_yaml::from_str(&content)
        .map_err(|e| CellariaError::Config(format!("YAML parse error: {}", e)))?;
    let yg = yaml.grid;

    // Собираем начальные активные координаты
    let default_type = yg.default_cell_type;
    let mut initial_active: HashSet<(usize, usize)> = HashSet::new();

    // Ячейки с не-дефолтным типом — активны. Границы проверяются здесь явно
    // (не молча пропускаются) — раньше клетка вне [0,width)×[0,height)
    // попадала в `initial_active` (и тем самым в dirty-множество новой
    // решётки), но реальный `grid.set_cell` ниже её тихо пропускал
    // (`x < width && y < height`) — координата оставалась "активной" без
    // единой реальной ячейки за ней. Несогласованно с остальной
    // валидацией в этой функции (например, проверкой 0xFF в id ниже),
    // которая явно возвращает ошибку, а не молча теряет данные.
    for yc in &yg.initial_cells {
        let [x, y] = yc.coord;
        if x >= yg.width || y >= yg.height {
            return Err(CellariaError::Config(format!(
                "initial_cells: coordinate ({}, {}) is out of bounds for grid {}x{}",
                x, y, yg.width, yg.height
            )));
        }
        if yc.cell_type != default_type {
            initial_active.insert((x, y));
        }
    }

    // Граничные ячейки — всегда активны
    if let Some(boundaries) = &yg.boundaries {
        for b in boundaries {
            let [x, y] = b.cell;
            if x >= yg.width || y >= yg.height {
                return Err(CellariaError::Config(format!(
                    "boundaries: coordinate ({}, {}) is out of bounds for grid {}x{}",
                    x, y, yg.width, yg.height
                )));
            }
            initial_active.insert((x, y));
        }
    }

    // Создаём решётку
    let mut storage = VecStorage::new(yg.width, yg.height);

    // Заполняем default_cell_type для всех ячеек
    for cell in storage.cells.iter_mut() {
        cell.value = CellValue(CellType(yg.default_cell_type));
    }

    let mut grid = Grid::new(storage, initial_active);

    // Устанавливаем явно указанные ячейки — границы уже проверены выше.
    for yc in &yg.initial_cells {
        let [x, y] = yc.coord;
        grid.set_cell(
            x,
            y,
            Cell {
                value: CellValue(CellType(yc.cell_type)),
                born_at: 0,
            },
        );
    }

    // Устанавливаем граничные буферы с очередями
    if let Some(boundaries) = &yg.boundaries {
        for b in boundaries {
            let mut buf = BoundaryBuffer::new();
            // Создаём пустую очередь для указанного канала
            buf.queues.insert(b.channel, std::collections::VecDeque::new());
            buf.direction = b.direction.clone();
            buf.max_queue = b.max_queue;
            grid.set_boundary(b.cell[0], b.cell[1], buf);
        }
    }

    // Создаём правила
    let mut rules: Vec<Rule> = Vec::new();
    for yr in yaml.rules {
        // Валидация: id не может быть пустым, если не задан pattern
        if yr.id.is_empty() && yr.pattern.is_empty() {
            return Err(CellariaError::RuleValidation(
                "Rule id or pattern must not be empty".to_string(),
            ));
        }

        // Валидация: 0xFF зарезервирован для RuleStore
        if yr.id.contains(&0xFF) {
            return Err(CellariaError::RuleValidation(
                "Rule id contains 0xFF which is reserved for RuleStore protocol".to_string(),
            ));
        }

        // Валидация: cam и shifts взаимоисключающи — притяжение уже само
        // по себе единственный сдвиг правила (см. CamSearch).
        if yr.cam.is_some() && !yr.shifts.is_empty() {
            return Err(CellariaError::RuleValidation(
                "Rule with `cam` must not also have `shifts`".to_string(),
            ));
        }
        // Валидация: у cam-правила паттерн — только сама голова, без
        // дополнительных проверок соседей. Изолированный путь детекции CAM
        // (см. `matcher::detect_cam_matches`) не прогоняет полное
        // сопоставление паттерна — только это ограниченное подмножество.
        if yr.cam.is_some() && !yr.pattern.is_empty() {
            return Err(CellariaError::RuleValidation(
                "Rule with `cam` must not have an explicit `pattern` (only the head cell type is checked)".to_string(),
            ));
        }
        // Валидация: feedback осмыслен только для правила РОВНО с одним
        // сдвигом — `new_direction` заменяет его целиком (см.
        // `types::FeedbackSpec`'s doc-комментарий про ограничение).
        if yr.feedback.is_some() {
            let shift_count: usize = yr.shifts.iter().map(|g| g.group.len()).sum();
            if shift_count != 1 {
                return Err(CellariaError::RuleValidation(format!(
                    "Rule with `feedback` must have exactly one shift, found {shift_count}"
                )));
            }
        }
        // Валидация: recursion осмыслена только для правил БЕЗ сдвигов —
        // рекурсия расширяет `changes` вдоль направления, а не двигает
        // голову (см. `types::RecursionSpec`'s doc-комментарий).
        if yr.recursion.is_some() {
            let shift_count: usize = yr.shifts.iter().map(|g| g.group.len()).sum();
            if shift_count != 0 {
                return Err(CellariaError::RuleValidation(format!(
                    "Rule with `recursion` must have no shifts, found {shift_count}"
                )));
            }
        }
        // `cam` + `recursion` ВМЕСТЕ РАЗРЕШЕНЫ (реализовано, не запрещено):
        // `apply_cam_buffered` теперь после притяжения уровня 0 продолжает
        // каскад НЕЗАВИСИМЫХ магнитов вдоль `recursion.direction`
        // (`k = 1..=max_depth`, каждый со своим собственным диском поиска
        // радиуса `cam.radius`), а `conflict_analyzer::compute_rule_data`
        // строит статическую границу как union этих дисков (центр и радиус
        // каждого — чисто статические величины, известные на этапе
        // определения правила — только КОНКРЕТНАЯ найденная клетка внутри
        // диска рантайм-зависима, ровно как и у обычного одиночного CAM) —
        // см. doc-комментарии обеих функций и `paper/paper4.md` §8/§9.
        //
        // recursion + min_age > 0 РАЗРЕШЕНЫ (в отличие от recursion+memory
        // ниже): `applicator::pattern_matches_effective`
        // теперь проверяет `min_age` на каждом уровне каскада, а не только у
        // исходного матча — см. `read_age_effective`'s doc-комментарий про
        // то, почему клетка, записанная РАНЕЕ в этом же каскаде/тике,
        // корректно имеет эффективный возраст 0 (та же семантика, что и у
        // любой другой свежезаписанной клетки до конца тика).
        // Валидация: memory осмыслена только для правил с 0 или 1 сдвигом —
        // при БОЛЬШЕ ЧЕМ одном сдвиге неоднозначно, какую из нескольких
        // целей считать "новой позицией" того же маркера для переноса
        // записи буфера (см. `types::MemorySpec`'s doc-комментарий, тот же
        // выбор, что уже сделан для `feedback`).
        if yr.memory.is_some() {
            let shift_count: usize = yr.shifts.iter().map(|g| g.group.len()).sum();
            if shift_count > 1 {
                return Err(CellariaError::RuleValidation(format!(
                    "Rule with `memory` must have zero or one shift, found {shift_count}"
                )));
            }
        }
        // `memory` (`NeighborType`) + `recursion` ВМЕСТЕ РАЗРЕШЕНЫ (тот же
        // приём, что уже применён к `cam`+`recursion` и `recursion`+`min_age`
        // — найти обход, а не оставить блэнкет-запрет): `applicator`'s
        // "Фаза 3" каскада теперь ДОПОЛНИТЕЛЬНО проверяет (и обновляет)
        // собственный буфер уровня — ключ `(ox, oy, rule_idx)`, та же
        // самостоятельная позиция, что и у обычного top-level матча (см.
        // doc-комментарий цикла каскада в `applicator.rs`).
        //
        // `RuleOutcome` — ПО-ПРЕЖНЕМУ запрещён с `recursion`: у уровня
        // каскада НЕТ отдельного арбитража (весь каскад — часть уже
        // выигравшего top-level матча, применяется безусловно, если
        // pattern+gate совпали), так что "Applied vs Missed" для него
        // структурно не определено — не то же самое, что "сложнее
        // реализовать", а действительно бессмысленный вопрос для этой
        // конкретной комбинации.
        if let Some(mem) = &yr.memory {
            if yr.recursion.is_some() && mem.trigger.to_lowercase() == "rule_outcome" {
                return Err(CellariaError::RuleValidation(
                    "Rule with `memory` trigger `rule_outcome` must not also have `recursion` (cascade levels have no separate arbitration outcome to record — see applicator.rs's cascade loop doc-comment)".to_string(),
                ));
            }
        }

        // Преобразуем сдвиги
        let mut shifts: Vec<Vec<ShiftSpec>> = Vec::new();
        for group in yr.shifts {
            let mut group_shifts: Vec<ShiftSpec> = Vec::new();
            for yshift in group.group {
                let direction = parse_direction(&yshift.direction)?;
                group_shifts.push(ShiftSpec {
                    direction,
                    steps: yshift.steps,
                    broadcast: yshift.broadcast,
                    keep_source: yshift.keep_source,
                });
            }
            if !group_shifts.is_empty() {
                shifts.push(group_shifts);
            }
        }

        // Преобразуем изменения
        let mut changes: Vec<(i32, i32, ChangeValue)> = Vec::new();
        for yc in yr.changes {
            let cv = parse_change_value(&yc.value)?;
            changes.push((yc.dx, yc.dy, cv));
        }

        let rule_id: Vec<CellType> = yr.id.into_iter().map(CellType::new).collect();

        // Строим pattern: если задан явно — используем его, иначе из id
        let pattern: Vec<(i8, i8, CellType)> = if !yr.pattern.is_empty() {
            yr.pattern
                .into_iter()
                .map(|entry| {
                    let [dx, dy] = entry.offset;
                    (dx, dy, CellType::new(entry.cell_type))
                })
                .collect()
        } else {
            // Из id: (0,0, id[0]), (1,0, id[1]), ...
            rule_id
                .iter()
                .enumerate()
                .map(|(i, ct)| (i as i8, 0i8, *ct))
                .collect()
        };

        rules.push(Rule {
            id: rule_id,
            pattern,
            shifts,
            changes,
            active_only: yr.active_only,
            priority: yr.priority,
            min_age: yr.min_age,
            overflow: yr.overflow,
            cam: yr.cam.map(|c| CamSearch { radius: c.radius, target_type: CellType::new(c.target_type) }),
            tie_break: yr.tie_break,
            starvation_after: yr.starvation_after,
            feedback: match yr.feedback {
                Some(f) => Some(FeedbackSpec { timeout: f.timeout, new_direction: parse_direction(&f.new_direction)? }),
                None => None,
            },
            recursion: match yr.recursion {
                Some(r) => Some(RecursionSpec { max_depth: r.max_depth, direction: parse_direction(&r.direction)? }),
                None => None,
            },
            memory: yr.memory.map(build_memory_spec).transpose()?,
        });
    }

    // Строим индекс: ключ — первый элемент id или первый элемент pattern
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        // Определяем центральный тип для индексации
        let center_type = rule.id.first().copied().or_else(|| {
            rule.pattern
                .first()
                .map(|&(_, _, ct)| ct)
        });
        if let Some(ct) = center_type {
            rule_index.entry(ct).or_default().push(rule);
        }
    }

    // Сортируем по приоритету (убывание)
    for rules in rule_index.values_mut() {
        rules.sort_by_key(|b| std::cmp::Reverse(b.priority));
    }

    Ok((grid, rule_index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_direction() {
        assert_eq!(parse_direction("north").unwrap(), Direction::Up);
        assert_eq!(parse_direction("south").unwrap(), Direction::Down);
        assert_eq!(parse_direction("east").unwrap(), Direction::Right);
        assert_eq!(parse_direction("west").unwrap(), Direction::Left);
        assert!(parse_direction("invalid").is_err());
    }

    #[test]
    fn test_load_test_config() {
        let (grid, rule_index) =
            load_config("configs/test_config.yaml").expect("Should load test_config.yaml");
        assert_eq!(grid.width(), 8);
        assert_eq!(grid.height(), 8);
        // Initial cells: (1,1)=1, (2,1)=2, (5,3)=1, (6,3)=2
        assert_eq!(grid.get_cell(1, 1).unwrap().value, CellValue(CellType(1)));
        assert_eq!(grid.get_cell(2, 1).unwrap().value, CellValue(CellType(2)));
        // 3 rules
        let total_rules: usize = rule_index.values().map(|v| v.len()).sum();
        assert_eq!(total_rules, 3, "Should have 3 rules");
    }

    #[test]
    fn test_load_config_invalid_path() {
        let result = load_config("nonexistent.yaml");
        assert!(result.is_err(), "Should fail for nonexistent file");
    }

    #[test]
    fn test_parse_change_value_literal() {
        let v = serde_yaml::Value::Number(serde_yaml::Number::from(42));
        assert_eq!(parse_change_value(&v).unwrap(), ChangeValue::Literal(42));
    }

    #[test]
    fn test_parse_change_value_ref() {
        let v = serde_yaml::Value::String("$0".to_string());
        assert_eq!(parse_change_value(&v).unwrap(), ChangeValue::Ref(0));
        let v = serde_yaml::Value::String("$3".to_string());
        assert_eq!(parse_change_value(&v).unwrap(), ChangeValue::Ref(3));
    }

    #[test]
    fn test_parse_change_value_invalid_string() {
        let v = serde_yaml::Value::String("foo".to_string());
        assert!(parse_change_value(&v).is_err());
    }

    #[test]
    fn test_parse_change_value_overflow() {
        let v = serde_yaml::Value::Number(serde_yaml::Number::from(300u64));
        assert!(parse_change_value(&v).is_err());
    }

    #[test]
    fn test_parse_recorded_value_type() {
        let v = serde_yaml::Value::Number(serde_yaml::Number::from(7));
        assert_eq!(parse_recorded_value(&v).unwrap(), RecordedValue::Type(CellType::new(7)));
    }

    #[test]
    fn test_parse_recorded_value_outcome_strings() {
        assert_eq!(
            parse_recorded_value(&serde_yaml::Value::String("applied".to_string())).unwrap(),
            RecordedValue::Applied
        );
        assert_eq!(
            parse_recorded_value(&serde_yaml::Value::String("MISSED".to_string())).unwrap(),
            RecordedValue::Missed
        );
    }

    #[test]
    fn test_parse_recorded_value_invalid_string() {
        let v = serde_yaml::Value::String("foo".to_string());
        assert!(parse_recorded_value(&v).is_err());
    }

    #[test]
    fn test_build_memory_spec_requires_match_pattern_len_equals_window() {
        let m = YamlMemory {
            window: 2,
            trigger: "rule_outcome".to_string(),
            neighbor_direction: None,
            match_pattern: vec![serde_yaml::Value::String("applied".to_string())],
        };
        assert!(build_memory_spec(m).is_err(), "window=2 but match_pattern has 1 entry -- must be rejected");
    }

    #[test]
    fn test_build_memory_spec_neighbor_type_requires_direction() {
        let m = YamlMemory {
            window: 1,
            trigger: "neighbor_type".to_string(),
            neighbor_direction: None,
            match_pattern: vec![serde_yaml::Value::Number(serde_yaml::Number::from(1))],
        };
        assert!(build_memory_spec(m).is_err(), "neighbor_type trigger without neighbor_direction must be rejected");
    }

    #[test]
    fn test_build_memory_spec_rule_outcome_rejects_neighbor_direction() {
        let m = YamlMemory {
            window: 1,
            trigger: "rule_outcome".to_string(),
            neighbor_direction: Some("east".to_string()),
            match_pattern: vec![serde_yaml::Value::String("missed".to_string())],
        };
        assert!(build_memory_spec(m).is_err(), "rule_outcome trigger must not accept a neighbor_direction");
    }

    #[test]
    fn test_build_memory_spec_valid_neighbor_type() {
        let m = YamlMemory {
            window: 2,
            trigger: "neighbor_type".to_string(),
            neighbor_direction: Some("east".to_string()),
            match_pattern: vec![
                serde_yaml::Value::Number(serde_yaml::Number::from(3)),
                serde_yaml::Value::Number(serde_yaml::Number::from(4)),
            ],
        };
        let spec = build_memory_spec(m).unwrap();
        assert_eq!(spec.window, 2);
        assert_eq!(spec.record_trigger, RecordTrigger::NeighborType(Direction::Right));
        assert_eq!(spec.match_pattern, vec![RecordedValue::Type(CellType::new(3)), RecordedValue::Type(CellType::new(4))]);
    }

    #[test]
    fn test_build_memory_spec_valid_rule_outcome() {
        let m = YamlMemory {
            window: 2,
            trigger: "rule_outcome".to_string(),
            neighbor_direction: None,
            match_pattern: vec![
                serde_yaml::Value::String("missed".to_string()),
                serde_yaml::Value::String("applied".to_string()),
            ],
        };
        let spec = build_memory_spec(m).unwrap();
        assert_eq!(spec.match_pattern, vec![RecordedValue::Missed, RecordedValue::Applied]);
    }

    /// Регрессионный тест на реальный, найденный при аудите валидационный
    /// пробел: `neighbor_type` кладёт в буфер ТОЛЬКО `RecordedValue::Type(_)`
    /// (см. `engine/mod.rs`'s push-логику), никогда `Applied`/`Missed` — так
    /// что `match_pattern`, состоящий из "applied"/"missed" строк при
    /// `trigger: neighbor_type`, СТРУКТУРНО никогда не сможет совпасть
    /// (`PartialEq` на разных вариантах enum всегда `false`) — гейт был бы
    /// НАВСЕГДА закрыт, без единой ошибки ни при загрузке, ни в рантайме.
    /// Раньше `build_memory_spec` этого не проверяла вообще.
    #[test]
    fn test_build_memory_spec_rejects_rule_outcome_shaped_pattern_with_neighbor_type_trigger() {
        let m = YamlMemory {
            window: 2,
            trigger: "neighbor_type".to_string(),
            neighbor_direction: Some("east".to_string()),
            match_pattern: vec![
                serde_yaml::Value::String("applied".to_string()),
                serde_yaml::Value::String("missed".to_string()),
            ],
        };
        assert!(
            build_memory_spec(m).is_err(),
            "neighbor_type trigger with an applied/missed-shaped match_pattern must be rejected -- the gate could never open"
        );
    }

    /// Симметричный случай: `rule_outcome` с числовым (тип-клетки-shaped)
    /// `match_pattern` — тот же класс структурно-мёртвого гейта, зеркально.
    #[test]
    fn test_build_memory_spec_rejects_neighbor_type_shaped_pattern_with_rule_outcome_trigger() {
        let m = YamlMemory {
            window: 1,
            trigger: "rule_outcome".to_string(),
            neighbor_direction: None,
            match_pattern: vec![serde_yaml::Value::Number(serde_yaml::Number::from(3))],
        };
        assert!(
            build_memory_spec(m).is_err(),
            "rule_outcome trigger with a cell-type-shaped match_pattern must be rejected -- the gate could never open"
        );
    }
}