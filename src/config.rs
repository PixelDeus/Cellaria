use serde::Deserialize;
use std::collections::HashMap;
use std::fs;

use crate::error::CellariaError;
use crate::grid::Grid;
use crate::storage::{GridStorage, VecStorage};
use crate::types::{
    BoundaryBuffer, CellType, CellValue, Direction, OverflowAction, Rule, RuleId, ShiftSpec,
};

// === YAML-структуры ===

/// YAML-формат паттерна (offset + type).
#[derive(Debug, Deserialize)]
struct YamlPatternEntry {
    offset: [i8; 2],
    #[serde(rename = "type")]
    cell_type: u8,
}

/// YAML-формат правила.
#[derive(Debug, Deserialize)]
struct YamlRule {
    id: u32,
    priority: u8,
    /// Минимальный возраст ячейки-центра для активации (опционально, по умолчанию 0).
    #[serde(default)]
    min_age: u64,
    pattern: Vec<YamlPatternEntry>,
    result: Vec<u8>,
    shift: Option<YamlShift>,
}

/// YAML-формат сдвига.
#[derive(Debug, Deserialize)]
pub(crate) struct YamlShift {
    pub direction: String,
    pub chain_length: u8,
    pub fill_value: u8,
    pub overflow: String,
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
    #[allow(dead_code)]
    direction: String,
    max_queue: u8,
}

/// YAML-формат grid-секции.
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

/// Преобразовать YAML-сдвиг в [`ShiftSpec`].
pub(crate) fn convert_shift(yaml: &YamlShift) -> Result<ShiftSpec, CellariaError> {
    let direction = match yaml.direction.to_lowercase().as_str() {
        "north" => Direction::NORTH,
        "south" => Direction::SOUTH,
        "east" => Direction::EAST,
        "west" => Direction::WEST,
        other => {
            return Err(CellariaError::Config(format!(
                "Invalid direction: {}",
                other
            )))
        }
    };

    let overflow_action = if let Some(channel) = yaml.overflow.strip_prefix("channel:") {
        let id: u32 = channel
            .parse()
            .map_err(|e| CellariaError::Config(format!("Invalid channel id: {}", e)))?;
        OverflowAction::OutputToChannel(id)
    } else if let Some(value) = yaml.overflow.strip_prefix("write:") {
        let v: u8 = value
            .parse()
            .map_err(|e| CellariaError::Config(format!("Invalid write value: {}", e)))?;
        OverflowAction::WriteValue(CellValue(CellType(v)))
    } else if yaml.overflow == "discard" {
        OverflowAction::Discard
    } else {
        return Err(CellariaError::Config(format!(
            "Invalid overflow action: {}",
            yaml.overflow
        )));
    };

    Ok(ShiftSpec {
        direction,
        chain_length: yaml.chain_length,
        fill_value: CellValue(CellType(yaml.fill_value)),
        overflow_action,
    })
}

/// Результат загрузки конфига: решётка + индекс правил по типу центра.
pub type ConfigResult = Result<(Grid<VecStorage>, HashMap<CellType, Vec<Rule>>), CellariaError>;

pub fn load_config(
    path: &str,
) -> ConfigResult {
    let content = fs::read_to_string(path)?;
    let yaml: YamlConfig = serde_yaml::from_str(&content)
        .map_err(|e| CellariaError::Config(format!("YAML parse error: {}", e)))?;
    let yg = yaml.grid;

    // Создаём решётку
    let mut storage = VecStorage {
        cells: vec![crate::types::Cell::default(); yg.width * yg.height],
        width: yg.width,
        height: yg.height,
    };

    // Заполняем начальные ячейки
    // Сначала устанавливаем default_cell_type для всех ячеек
    for cell in storage.cells.iter_mut() {
        cell.value = CellValue(CellType(yg.default_cell_type));
    }

    // Затем устанавливаем явно указанные ячейки
    for yc in &yg.initial_cells {
        let [x, y] = yc.coord;
        if x < yg.width && y < yg.height {
            storage.set(
                x,
                y,
                crate::types::Cell {
                    value: CellValue(CellType(yc.cell_type)),
                    age: 0,
                },
            );
        }
    }

    // Создаём решётку
    let mut grid = Grid::new(storage);

    // Устанавливаем граничные буферы
    if let Some(boundaries) = &yg.boundaries {
        for b in boundaries {
            grid.set_boundary(
                b.cell[0],
                b.cell[1],
                BoundaryBuffer {
                    channel: b.channel,
                    input_queue: std::collections::VecDeque::new(),
                    output_queue: std::collections::VecDeque::new(),
                    pending_output: None,
                    max_queue_depth: b.max_queue,
                },
            );
        }
    }

    // Создаём правила
    let mut rules: Vec<Rule> = Vec::new();
    for yr in yaml.rules {
        // Проверка: 255 (терминатор протокола RuleStore) запрещён
        for e in &yr.pattern {
            if e.cell_type == 0xFF {
                return Err(CellariaError::RuleValidation(format!(
                    "Rule {}: type 255 (0xFF) is reserved for RuleStore protocol, cannot be used in pattern",
                    yr.id
                )));
            }
        }
        for &v in &yr.result {
            if v == 0xFF {
                return Err(CellariaError::RuleValidation(format!(
                    "Rule {}: result value 255 (0xFF) is reserved for RuleStore protocol",
                    yr.id
                )));
            }
        }
        if let Some(ref yaml_shift) = yr.shift {
            if yaml_shift.fill_value == 0xFF {
                return Err(CellariaError::RuleValidation(format!(
                    "Rule {}: shift fill_value 255 (0xFF) is reserved for RuleStore protocol",
                    yr.id
                )));
            }
        }

        // Валидация: паттерн должен содержать центр (0, 0)
        if !yr.pattern.iter().any(|e| e.offset == [0, 0]) {
            return Err(CellariaError::RuleValidation(format!(
                "Rule {}: pattern must contain center (0, 0)",
                yr.id
            )));
        }

        // Валидация: количество результатов должно совпадать с количеством записей в паттерне
        if yr.result.len() != yr.pattern.len() {
            return Err(CellariaError::RuleValidation(format!(
                "Rule {}: result length {} != pattern length {}",
                yr.id,
                yr.result.len(),
                yr.pattern.len()
            )));
        }

        let pattern: Vec<(i8, i8, CellType)> = yr
            .pattern
            .iter()
            .map(|e| (e.offset[0], e.offset[1], CellType(e.cell_type)))
            .collect();
        let result_cells: Vec<CellValue> =
            yr.result.iter().map(|&v| CellValue(CellType(v))).collect();
        let shift = match yr.shift {
            Some(ref yaml_shift) => {
                let s = convert_shift(yaml_shift)?;
                // Валидация: chain_length > 0
                if s.chain_length == 0 {
                    return Err(CellariaError::RuleValidation(format!(
                        "Rule {}: shift chain_length must be > 0",
                        yr.id
                    )));
                }
                Some(s)
            }
            None => None,
        };

        rules.push(Rule {
            id: RuleId(yr.id),
            priority: yr.priority,
            min_age: yr.min_age,
            pattern,
            result_cells,
            shift,
        });
    }

    // Строим индекс
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    for rule in rules {
        if let Some(&(_, _, center_type)) =
            rule.pattern.iter().find(|&&(dx, dy, _)| dx == 0 && dy == 0)
        {
            rule_index.entry(center_type).or_default().push(rule);
        }
    }

    // Сортируем по приоритету
    for rules in rule_index.values_mut() {
        rules.sort_by_key(|b| std::cmp::Reverse(b.priority));
    }

    Ok((grid, rule_index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_shift_overflow_discard() {
        let yaml = YamlShift {
            direction: "east".to_string(),
            chain_length: 2,
            fill_value: 0,
            overflow: "discard".to_string(),
        };
        let shift = convert_shift(&yaml).unwrap();
        assert!(matches!(shift.overflow_action, OverflowAction::Discard));
    }

    #[test]
    fn test_convert_shift_overflow_write() {
        let yaml = YamlShift {
            direction: "east".to_string(),
            chain_length: 2,
            fill_value: 0,
            overflow: "write:7".to_string(),
        };
        let shift = convert_shift(&yaml).unwrap();
        assert!(matches!(
            shift.overflow_action,
            OverflowAction::WriteValue(CellValue(CellType(7)))
        ));
    }

    #[test]
    fn test_convert_shift_overflow_channel() {
        let yaml = YamlShift {
            direction: "east".to_string(),
            chain_length: 2,
            fill_value: 0,
            overflow: "channel:3".to_string(),
        };
        let shift = convert_shift(&yaml).unwrap();
        assert!(matches!(
            shift.overflow_action,
            OverflowAction::OutputToChannel(3)
        ));
    }

    #[test]
    fn test_convert_shift_invalid_overflow() {
        let yaml = YamlShift {
            direction: "east".to_string(),
            chain_length: 2,
            fill_value: 0,
            overflow: "invalid".to_string(),
        };
        let result = convert_shift(&yaml);
        assert!(result.is_err(), "Invalid overflow should error");
    }

    #[test]
    fn test_load_test_config() {
        let (grid, rule_index) =
            load_config("test_config.yaml").expect("Should load test_config.yaml");
        // Grid: 8x8, default type 0
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
    fn test_load_config_rejects_no_center() {
        let yaml_content = r#"
grid:
  width: 2
  height: 2
  default_cell_type: 0
  initial_cells: []
rules:
  - id: 1
    priority: 10
    pattern:
      - offset: [1, 0]
        type: 1
    result: [2]
"#;
        let path = "test_no_center.yaml";
        std::fs::write(path, yaml_content).unwrap();
        let result = load_config(path);
        std::fs::remove_file(path).unwrap();
        assert!(result.is_err(), "Should reject rule without center");
        if let Err(err) = result {
            assert!(
                err.to_string().contains("center"),
                "Error should mention center"
            );
        }
    }

    #[test]
    fn test_load_config_rejects_result_length_mismatch() {
        let yaml_content = r#"
grid:
  width: 2
  height: 2
  default_cell_type: 0
  initial_cells: []
rules:
  - id: 1
    priority: 10
    pattern:
      - offset: [0, 0]
        type: 1
      - offset: [1, 0]
        type: 2
    result: [3]
"#;
        let path = "test_len_mismatch.yaml";
        std::fs::write(path, yaml_content).unwrap();
        let result = load_config(path);
        std::fs::remove_file(path).unwrap();
        assert!(result.is_err(), "Should reject mismatched result length");
    }

    #[test]
    fn test_load_config_rejects_zero_chain_length() {
        let yaml_content = r#"
grid:
  width: 4
  height: 1
  default_cell_type: 0
  initial_cells:
    - coord: [0, 0]
      type: 1
    - coord: [1, 0]
      type: 2
rules:
  - id: 1
    priority: 10
    pattern:
      - offset: [0, 0]
        type: 1
      - offset: [1, 0]
        type: 2
    result: [3, 4]
    shift:
      direction: east
      chain_length: 0
      fill_value: 0
      overflow: discard
"#;
        let path = "test_zero_chain.yaml";
        std::fs::write(path, yaml_content).unwrap();
        let result = load_config(path);
        std::fs::remove_file(path).unwrap();
        assert!(result.is_err(), "Should reject zero chain_length");
    }
}