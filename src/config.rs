use serde::Deserialize;
use std::collections::HashMap;
use std::fs;

use crate::error::CellariaError;
use crate::grid::Grid;
use crate::storage::GridStorage;
use crate::storage::VecStorage;
use crate::types::{
    BoundaryBuffer, Cell, CellType, CellValue, Direction, OverflowAction, Rule,
    ShiftSpec,
};

// === Вспомогательный тип — входной шаблон конфига ===

/// YAML-формат записи сдвига.
#[derive(Debug, Deserialize)]
struct YamlShift {
    direction: String,
    steps: u16,
}

/// YAML-формат группы сдвигов приоритета.
#[derive(Debug, Deserialize)]
struct YamlShiftGroup {
    group: Vec<YamlShift>,
}

/// YAML-формат одного правила.
#[derive(Debug, Deserialize)]
struct YamlRule {
    /// Внутренняя область — последовательность чисел (n-кортеж).
    id: Vec<u8>,
    /// Приоритет.
    priority: u32,
    /// Сдвиги: каждая группа — Vec<ShiftSpec>.
    #[serde(default)]
    shifts: Vec<YamlShiftGroup>,
    /// Изменения ячеек: (смещение_x, смещение_y, новое_значение).
    changes: Vec<[i32; 3]>,
    /// Если true — проверять только в активных ячейках.
    #[serde(default)]
    active_only: bool,
    /// Минимальный возраст ячейки-центра для активации правила.
    #[serde(default)]
    min_age: u64,
    /// Действие при overflow (выходе за границу решётки).
    #[serde(default)]
    overflow: OverflowAction,
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
    max_queue: u8,
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

/// Результат загрузки конфига: решётка + индекс правил по типу центра.
pub type ConfigResult = Result<(Grid<VecStorage>, RuleIndex), CellariaError>;

/// Индекс правил: отображение CellType → Vec<(Rule, правило_подходит_к_активным_ячейкам)>.
pub type RuleIndex = HashMap<CellType, Vec<Rule>>;

pub fn load_config(path: &str) -> ConfigResult {
    let content = fs::read_to_string(path)?;
    let yaml: YamlConfig = serde_yaml::from_str(&content)
        .map_err(|e| CellariaError::Config(format!("YAML parse error: {}", e)))?;
    let yg = yaml.grid;

    // Создаём решётку
    let mut storage = VecStorage::new(yg.width, yg.height);

    // Заполняем default_cell_type для всех ячеек
    for cell in storage.cells.iter_mut() {
        cell.value = CellValue(CellType(yg.default_cell_type));
    }

    // Устанавливаем явно указанные ячейки
    for yc in &yg.initial_cells {
        let [x, y] = yc.coord;
        if x < yg.width && y < yg.height {
            storage.set(
                x,
                y,
                Cell {
                    value: CellValue(CellType(yc.cell_type)),
                    age: 0,
                },
            );
        }
    }

    let mut grid = Grid::new(storage);

    // Устанавливаем граничные буферы с очередями
    if let Some(boundaries) = &yg.boundaries {
        for b in boundaries {
            let mut buf = BoundaryBuffer::new();
            // Создаём пустую очередь для указанного канала
            buf.queues.insert(b.channel, Vec::new());
            buf.direction = b.direction.clone();
            grid.set_boundary(b.cell[0], b.cell[1], buf);
        }
    }

    // Создаём правила
    let mut rules: Vec<Rule> = Vec::new();
    for yr in yaml.rules {
        // Валидация: id не может быть пустым
        if yr.id.is_empty() {
            return Err(CellariaError::RuleValidation(
                "Rule id must not be empty".to_string(),
            ));
        }

        // Валидация: 0xFF зарезервирован для RuleStore
        if yr.id.contains(&0xFF) {
            return Err(CellariaError::RuleValidation(
                "Rule id contains 0xFF which is reserved for RuleStore protocol".to_string(),
            ));
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
                });
            }
            if !group_shifts.is_empty() {
                shifts.push(group_shifts);
            }
        }

        // Преобразуем изменения
        let changes: Vec<(i32, i32, u8)> = yr
            .changes
            .iter()
            .map(|c| (c[0], c[1], c[2] as u8))
            .collect();

        let rule_id: Vec<CellType> = yr.id.into_iter().map(CellType::new).collect();

        // Строим pattern из rule.id (для совместимости)
        let pattern: Vec<Vec<u8>> = vec![rule_id.iter().map(|ct| ct.0).collect()];

        rules.push(Rule {
            id: rule_id,
            pattern,
            shifts,
            changes,
            active_only: yr.active_only,
            priority: yr.priority,
            min_age: yr.min_age,
            overflow: Default::default(),
        });
    }

    // Строим индекс
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        if let Some(center) = rule.id.first() {
            rule_index.entry(*center).or_default().push(rule);
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
            load_config("test_config.yaml").expect("Should load test_config.yaml");
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
}