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
    let (grid, rule_index) = load_config("configs/test_config.yaml").expect("Should load test_config.yaml");
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

/// Пишет YAML во временный файл с уникальным именем (тесты этого
/// модуля идут параллельно -- фиксированное имя ловило бы гонку между
/// тестами, пишущими и читающими один и тот же путь).
fn write_temp_config(name: &str, content: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("cellaria_test_{name}_{:?}.yaml", std::thread::current().id()));
    fs::write(&path, content).expect("failed to write temp config");
    path
}

#[test]
fn test_load_layered_config_merges_rules_from_all_files_into_one_shared_rule_index() {
    // Слой 0: правило на голове 1 (файл A). Слой 1: правило на голове
    // 2 (файл B) -- РАЗНЫЕ CellType, чтобы не задеть проверку коллизий
    // ниже (см. test_load_layered_config_rejects_same_cell_type_from_two_files) --
    // сама эта проверка тестирует, что слияние в ОДИН общий
    // `rule_index` физически произошло (оба видны с ЛЮБОГО слоя, не
    // только со "своего"), не приоритетную сортировку конкретно --
    // после запрета на коллизии между файлами каждый CellType всегда
    // приходит из РОВНО одного файла, уже отсортированного
    // `load_config_str`, так что кросс-файловый ресорт для валидного
    // конфига больше не наблюдаемый сценарий (сам шаг пересортировки в
    // коде остался как защитная мера, не как фикс конкретного бага).
    let path_a = write_temp_config(
            "layered_merge_a",
            "grid:\n  width: 2\n  height: 2\n  default_cell_type: 0\n  initial_cells:\n    - {coord: [0, 0], type: 1}\nrules:\n  - id: [1]\n    priority: 1\n    changes:\n      - {dx: 0, dy: 0, value: 50}\n",
        );
    let path_b = write_temp_config(
            "layered_merge_b",
            "grid:\n  width: 2\n  height: 2\n  default_cell_type: 0\n  initial_cells: []\nrules:\n  - id: [2]\n    priority: 5\n    changes:\n      - {dx: 0, dy: 0, value: 90}\n",
        );

    let engine = load_layered_config(&[path_a.to_str().unwrap(), path_b.to_str().unwrap()])
        .expect("load_layered_config must succeed for two same-size layers with non-colliding CellTypes");
    assert_eq!(engine.layer_count(), 2);
    // ОБА CellType видны в ОБЩЕМ rule_index, доступном с ЛЮБОГО слоя --
    // не "слой 0 знает только про 1, слой 1 только про 2".
    for layer in 0..2 {
        assert!(
            engine.layer(layer).rule_index().contains_key(&CellType(1)),
            "layer {layer} must see head 1 from file A -- rule_index is shared, not per-layer"
        );
        assert!(
            engine.layer(layer).rule_index().contains_key(&CellType(2)),
            "layer {layer} must see head 2 from file B -- rule_index is shared, not per-layer"
        );
    }

    let _ = fs::remove_file(&path_a);
    let _ = fs::remove_file(&path_b);
}

/// Реальный риск, который эта проверка закрывает: два независимо
/// написанных домена (файла) случайно занимают один и тот же
/// `CellType` для РАЗНОГО смысла -- поскольку все слои делят ОДИН
/// `rule_index`, клетка этого типа на любом слое матчила бы правила
/// ОБОИХ файлов сразу, тихо, без единой ошибки.
#[test]
fn test_load_layered_config_rejects_same_cell_type_from_two_files() {
    let path_a = write_temp_config(
            "layered_collision_a",
            "grid:\n  width: 2\n  height: 2\n  default_cell_type: 0\n  initial_cells: []\nrules:\n  - id: [1]\n    priority: 1\n    changes:\n      - {dx: 0, dy: 0, value: 50}\n",
        );
    let path_b = write_temp_config(
            "layered_collision_b",
            "grid:\n  width: 2\n  height: 2\n  default_cell_type: 0\n  initial_cells: []\nrules:\n  - id: [1]\n    priority: 5\n    changes:\n      - {dx: 0, dy: 0, value: 90}\n",
        );

    let result = load_layered_config(&[path_a.to_str().unwrap(), path_b.to_str().unwrap()]);
    let Err(err) = result else {
        panic!("expected an error when two files both define rules for CellType 1");
    };
    let msg = format!("{err}");
    assert!(
        msg.contains("CellType 1"),
        "error must name the colliding CellType: {msg}"
    );
    assert!(
        msg.contains("layered_collision_a") && msg.contains("layered_collision_b"),
        "error must name BOTH colliding files: {msg}"
    );

    let _ = fs::remove_file(&path_a);
    let _ = fs::remove_file(&path_b);
}

#[test]
fn test_load_layered_config_rejects_mismatched_layer_dimensions() {
    let path_a = write_temp_config(
        "layered_dims_a",
        "grid:\n  width: 2\n  height: 2\n  default_cell_type: 0\n  initial_cells: []\nrules: []\n",
    );
    let path_b = write_temp_config(
        "layered_dims_b",
        "grid:\n  width: 3\n  height: 3\n  default_cell_type: 0\n  initial_cells: []\nrules: []\n",
    );

    let result = load_layered_config(&[path_a.to_str().unwrap(), path_b.to_str().unwrap()]);
    let Err(err) = result else {
        panic!("expected an error for mismatched layer dimensions (2x2 vs 3x3)");
    };
    let msg = format!("{err}");
    assert!(
        msg.contains("2x2") && msg.contains("3x3"),
        "error must mention both conflicting sizes: {msg}"
    );

    let _ = fs::remove_file(&path_a);
    let _ = fs::remove_file(&path_b);
}

#[test]
fn test_load_layered_config_rejects_empty_paths() {
    let result = load_layered_config(&[]);
    assert!(result.is_err(), "at least one layer path must be required");
}

const MINIMAL_CONFIG_BODY: &str =
    "\ngrid:\n  width: 2\n  height: 2\n  default_cell_type: 0\n  initial_cells: []\nrules: []\n";

#[test]
fn test_load_config_missing_version_is_accepted_as_v1() {
    // Конфиги, написанные ДО версионирования, не должны сломаться --
    // отсутствие `version` эквивалентно версии 1.
    let result = load_config_str(MINIMAL_CONFIG_BODY);
    assert!(
        result.is_ok(),
        "config without a version field must load: {:?}",
        result.err()
    );
}

#[test]
fn test_load_config_explicit_supported_version_is_accepted() {
    let content = format!("version: {SUPPORTED_CONFIG_VERSION}{MINIMAL_CONFIG_BODY}");
    assert!(load_config_str(&content).is_ok());
}

#[test]
fn test_load_config_rejects_unsupported_version() {
    let content = format!("version: 999{MINIMAL_CONFIG_BODY}");
    let Err(err) = load_config_str(&content) else {
        panic!("expected an error for unsupported config version 999");
    };
    let msg = format!("{err}");
    assert!(msg.contains("999"), "error must mention the unsupported version: {msg}");
    assert!(
        msg.contains(&SUPPORTED_CONFIG_VERSION.to_string()),
        "error must mention the supported version: {msg}"
    );
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
fn test_parse_change_value_add_flat() {
    let yaml = serde_yaml::from_str::<serde_yaml::Value>("add: [\"$0\", 1]").unwrap();
    let m = match yaml {
        serde_yaml::Value::Mapping(m) => serde_yaml::Value::Mapping(m),
        _ => unreachable!(),
    };
    assert_eq!(
        parse_change_value(&m).unwrap(),
        ChangeValue::Add(Box::new(ChangeValue::Ref(0)), Box::new(ChangeValue::Literal(1)))
    );
}

#[test]
fn test_parse_change_value_sub_flat() {
    let yaml = serde_yaml::from_str::<serde_yaml::Value>("sub: [10, \"$2\"]").unwrap();
    assert_eq!(
        parse_change_value(&yaml).unwrap(),
        ChangeValue::Sub(Box::new(ChangeValue::Literal(10)), Box::new(ChangeValue::Ref(2)))
    );
}

#[test]
fn test_parse_change_value_add_nested() {
    // add: [ $0, {add: [1, 2]} ] -- вложенный операнд.
    let yaml = serde_yaml::from_str::<serde_yaml::Value>("add:\n  - \"$0\"\n  - add: [1, 2]\n").unwrap();
    let expected = ChangeValue::Add(
        Box::new(ChangeValue::Ref(0)),
        Box::new(ChangeValue::Add(
            Box::new(ChangeValue::Literal(1)),
            Box::new(ChangeValue::Literal(2)),
        )),
    );
    assert_eq!(parse_change_value(&yaml).unwrap(), expected);
}

#[test]
fn test_parse_change_value_add_rejects_wrong_operand_count() {
    let yaml = serde_yaml::from_str::<serde_yaml::Value>("add: [1, 2, 3]").unwrap();
    assert!(parse_change_value(&yaml).is_err());
    let yaml = serde_yaml::from_str::<serde_yaml::Value>("add: [1]").unwrap();
    assert!(parse_change_value(&yaml).is_err());
}

#[test]
fn test_parse_change_value_rejects_unknown_operation() {
    let yaml = serde_yaml::from_str::<serde_yaml::Value>("mul: [1, 2]").unwrap();
    assert!(parse_change_value(&yaml).is_err());
}

#[test]
fn test_load_config_with_add_change_end_to_end() {
    let content = format!(
            "version: {SUPPORTED_CONFIG_VERSION}\ngrid:\n  width: 2\n  height: 1\n  default_cell_type: 0\n  initial_cells:\n    - {{coord: [0, 0], type: 1}}\nrules:\n  - id: [1]\n    priority: 1\n    changes:\n      - {{dx: 0, dy: 0, value: {{add: [\"$0\", 5]}}}}\n"
        );
    let (grid, rule_index) = load_config_str(&content).expect("config with an add-change must load");
    let rules = rule_index.get(&CellType(1)).expect("head 1 must have a rule");
    assert_eq!(
        rules[0].changes[0].2,
        ChangeValue::Add(Box::new(ChangeValue::Ref(0)), Box::new(ChangeValue::Literal(5)))
    );
    let _ = grid;
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
    assert!(
        build_memory_spec(m).is_err(),
        "window=2 but match_pattern has 1 entry -- must be rejected"
    );
}

#[test]
fn test_build_memory_spec_neighbor_type_requires_direction() {
    let m = YamlMemory {
        window: 1,
        trigger: "neighbor_type".to_string(),
        neighbor_direction: None,
        match_pattern: vec![serde_yaml::Value::Number(serde_yaml::Number::from(1))],
    };
    assert!(
        build_memory_spec(m).is_err(),
        "neighbor_type trigger without neighbor_direction must be rejected"
    );
}

#[test]
fn test_build_memory_spec_rule_outcome_rejects_neighbor_direction() {
    let m = YamlMemory {
        window: 1,
        trigger: "rule_outcome".to_string(),
        neighbor_direction: Some("east".to_string()),
        match_pattern: vec![serde_yaml::Value::String("missed".to_string())],
    };
    assert!(
        build_memory_spec(m).is_err(),
        "rule_outcome trigger must not accept a neighbor_direction"
    );
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
    assert_eq!(
        spec.match_pattern,
        vec![
            RecordedValue::Type(CellType::new(3)),
            RecordedValue::Type(CellType::new(4))
        ]
    );
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

/// Адверсариальная проверка (не автоматика -- целенаправленная попытка
/// сломать `load_config` на потенциально недоверенном YAML-файле):
/// классический "billion laughs" -- вложенные YAML-якоря/алиасы,
/// каждый уровень умножает предыдущий в 10 раз (10 уровней -> 10^10
/// при наивном разворачивании). Конструктивно подтверждено: `serde_yaml`
/// 0.9.x УЖЕ защищён встроенным лимитом повторений ("repetition limit
/// exceeded") -- ошибка приходит за миллисекунды, не зависание/OOM.
/// Тест ЗАКРЕПЛЯЕТ эту гарантию: если будущее обновление `serde_yaml`
/// когда-нибудь ослабит или уберёт защиту, тест поймает это как
/// зависший/долгий прогон, а не как молчаливую деградацию.
#[test]
fn test_load_config_rejects_billion_laughs_yaml_bomb_quickly() {
    let yaml = "\
grid:\n  width: 5\n  height: 5\n\
a0: &a0 \"lol\"\n\
a1: &a1 [*a0,*a0,*a0,*a0,*a0,*a0,*a0,*a0,*a0,*a0]\n\
a2: &a2 [*a1,*a1,*a1,*a1,*a1,*a1,*a1,*a1,*a1,*a1]\n\
a3: &a3 [*a2,*a2,*a2,*a2,*a2,*a2,*a2,*a2,*a2,*a2]\n\
a4: &a4 [*a3,*a3,*a3,*a3,*a3,*a3,*a3,*a3,*a3,*a3]\n\
a5: &a5 [*a4,*a4,*a4,*a4,*a4,*a4,*a4,*a4,*a4,*a4]\n\
a6: &a6 [*a5,*a5,*a5,*a5,*a5,*a5,*a5,*a5,*a5,*a5]\n\
a7: &a7 [*a6,*a6,*a6,*a6,*a6,*a6,*a6,*a6,*a6,*a6]\n\
a8: &a8 [*a7,*a7,*a7,*a7,*a7,*a7,*a7,*a7,*a7,*a7]\n\
a9: &a9 [*a8,*a8,*a8,*a8,*a8,*a8,*a8,*a8,*a8,*a8]\n\
initial_cells: []\nrules: []\n";
    let start = std::time::Instant::now();
    let parsed = serde_yaml::from_str::<serde_yaml::Value>(yaml);
    assert!(
        parsed.is_err(),
        "serde_yaml must reject a 10-level, 10x-branching alias bomb via its repetition limit, not silently expand it"
    );
    assert!(
        start.elapsed().as_secs() < 5,
        "rejection must be fast (repetition-limit check), not a slow partial expansion before giving up"
    );
}
