use super::super::*;
use super::common::*;
use crate::types::{Cell, CellType, ChangeValue, Direction, Rule, ShiftSpec};
use crate::BoundaryBuffer;
use crate::VecStorage;

/// `Engine::enable_input_recording`/`Engine::replay` — отладочный сценарий
/// "нашли расхождение на позднем тике, хотим продолжить с более раннего
/// снимка, не пересчитывая всё руками": движок с ГРАНИЧНЫМ ВВОДОМ (не
/// только с изменением решётки изнутри — именно `push_input` и есть то,
/// что `input_log`/`replay` обязаны воспроизвести, в отличие от
/// `EngineSnapshot`, который сам по себе видит только СОСТОЯНИЕ, а не
/// историю того, как решётка до него дошла).
///
/// Проверка: движок A работает НЕПРЕРЫВНО (push_input вперемешку с
/// run_tick, как в реальном использовании) до тика 10. Отдельно — снимок и
/// `input_log`, снятые НА тике 5 (до того, как все входные события
/// случились). `Engine::replay(снимок, log, 10)` обязан дать РОВНО то же
/// состояние решётки на тике 10, что и непрерывный прогон A — не
/// приблизительно похожее, а побитово идентичное, включая эффект
/// граничного ввода, случившегося ПОСЛЕ снимка.
#[test]
fn test_input_recording_and_replay_reproduces_continuous_run() {
    const INPUT_CHANNEL: u32 = 0;
    const MARKER: u8 = 5;

    // Правило: маркер ДВИЖЕТСЯ вправо на 1 клетку каждый тик (обычный
    // сдвиг, источник очищается). Намеренно НЕ "клетка появилась и
    // осталась навсегда" (та версия НЕ различает "вошёл на тике 0" от
    // "вошёл на тике 1" уже через пару тиков — эффект насыщается и
    // ошибка на 1 тик перестаёт быть видна) -- позиция движущегося
    // маркера на позднем тике НАПРЯМУЮ кодирует, сколько тиков он уже
    // движется, то есть КОГДА именно он появился, что и делает тест
    // чувствительным к точному тайминга push_input относительно run_tick.
    fn make_index() -> HashMap<CellType, Vec<Rule>> {
        let mut idx = HashMap::new();
        idx.insert(
            CellType(MARKER),
            vec![Rule {
                id: vec![CellType(MARKER)],
                pattern: vec![],
                shifts: vec![vec![ShiftSpec::new(Direction::Right, 1)]],
                changes: vec![],
                active_only: false,
                priority: 10,
                min_age: 0,
                overflow: Default::default(),
                cam: None,
                tie_break: 0,
                starvation_after: None,
                feedback: None,
                recursion: None,
                memory: None,
                max_activations: None,
                cross_layer_reads: Vec::new(),
            }],
        );
        idx
    }

    fn make_engine_with_input_boundary() -> Engine<VecStorage> {
        let mut grid = make_grid(20, 1);
        let mut input_buf = BoundaryBuffer::new();
        input_buf.direction = "input".to_string();
        grid.set_boundary(0, 0, input_buf);
        Engine::new(grid, make_index())
    }

    // Движок A: непрерывный прогон "как в реальности" -- push_input
    // вперемешку с run_tick на РАЗНЫХ тиках (не все сразу в начале).
    // `apply_input()` -- ОТДЕЛЬНЫЙ шаг от `run_tick()` (см. её
    // doc-комментарий: перенос значения из очереди на решётку — это
    // `apply_input`, не часть `run_tick`), вызывается КАЖДЫЙ тик
    // безусловно — канонический паттерн, см. `examples/strength_live_io.rs`.
    let mut engine_a = make_engine_with_input_boundary();
    engine_a.enable_input_recording();
    engine_a.push_input(INPUT_CHANNEL, MARKER); // тик 0: заходит перед run_tick #1
    for _ in 1..=3 {
        engine_a.apply_input();
        engine_a.run_tick();
    }
    engine_a.push_input(INPUT_CHANNEL, MARKER); // тик 3: второй маркер входит позже
    for _ in 4..=5 {
        engine_a.apply_input();
        engine_a.run_tick();
    }

    // Снимок И журнал СЕЙЧАС (тик 5) -- журнал уже содержит оба события
    // (0 и 3), это НЕ "снимок без истории", а полноценная точка возврата.
    let snapshot_at_5 = engine_a.snapshot();
    let log_at_5: Vec<InputEvent> = engine_a.input_log().unwrap().to_vec();
    // Проверка самого механизма записи (не только сквозного результата
    // реплея): `tick` каждого события обязан быть поколением НА МОМЕНТ
    // вызова push_input (0 и 3), не номером тика, на котором его заметили,
    // и не порядковым номером вызова.
    assert_eq!(
        log_at_5,
        vec![
            InputEvent {
                tick: 0,
                channel: INPUT_CHANNEL,
                value: MARKER
            },
            InputEvent {
                tick: 3,
                channel: INPUT_CHANNEL,
                value: MARKER
            },
        ],
        "input_log должен точно отразить (tick, channel, value) обоих вызовов push_input"
    );

    // Движок A продолжает жить ДАЛЬШЕ, с ЕЩЁ одним вводом уже ПОСЛЕ снимка.
    engine_a.push_input(INPUT_CHANNEL, MARKER); // тик 5
    for _ in 6..=10 {
        engine_a.apply_input();
        engine_a.run_tick();
    }

    // Реплей должен знать и про событие ПОСЛЕ снимка (тик 5) -- добавляем
    // его в копию журнала, снятого на тике 5, ровно как это сделал бы
    // человек, продолжающий писать в тот же лог-файл.
    let mut log_for_replay = log_at_5;
    log_for_replay.push(InputEvent {
        tick: 5,
        channel: INPUT_CHANNEL,
        value: MARKER,
    });

    let replayed = Engine::replay(snapshot_at_5, &log_for_replay, 10);

    for x in 0..20 {
        assert_eq!(
            engine_a.grid().get_cell(x, 0),
            replayed.grid().get_cell(x, 0),
            "x={x}: реплей от снимка тика 5 + журнал обязан совпасть с непрерывным прогоном на тике 10"
        );
    }
    assert_eq!(
        engine_a.grid().generation(),
        replayed.grid().generation(),
        "поколение должно совпасть"
    );
}

/// `Engine::snapshot()`/`Engine::from_snapshot()` — реальный serde-раунд-трип
/// (сериализация в текст и обратно, не просто "поля совпали в памяти") на
/// движке с накопленным `starvation_counters` (проверяет, что персистентное
/// состояние расширений переживает сохранение, не только `grid`/`rule_index`).
///
/// `Engine::run_tick_profiled()` не должен менять НАБЛЮДАЕМОЕ поведение —
/// два одинаково построенных движка, один прогнанный через `run_tick()`,
/// другой через `run_tick_profiled()`, обязаны дать побитово идентичный
/// результат. Инструментирование само по себе не должно быть источником
/// расхождения (макрос `mark_phase!` добавляет только чтение времени и
/// запись в отдельную структуру, но это ровно тот класс правки, которую
/// стоит перепроверить явно, не полагаясь на "не должно было ничего
/// сломать").
#[test]
fn test_run_tick_profiled_matches_run_tick_behavior() {
    let mut plain = Engine::new(make_grid(3, 1), make_starvation_rules(Some(3)));
    plain.grid_mut().set_cell(0, 0, Cell::new(1));
    let mut profiled = Engine::new(make_grid(3, 1), make_starvation_rules(Some(3)));
    profiled.grid_mut().set_cell(0, 0, Cell::new(1));

    for tick in 1..=8 {
        plain.run_tick();
        profiled.run_tick_profiled();
        for x in 0..3 {
            assert_eq!(
                plain.grid().get_cell(x, 0),
                profiled.grid().get_cell(x, 0),
                "тик {tick}: run_tick_profiled разошёлся с run_tick при x={x}"
            );
        }
    }
}

/// Разбивка по фазам реально что-то измеряет — не все три поля остаются
/// нулевыми на тике с реальными совпадениями и конкуренцией в арбитраже
/// (два правила на одну голову — `arbitrate`-фаза должна что-то делать, не
/// вырождаться в no-op). Не проверяет КОНКРЕТНЫЕ значения (таймингы
/// недетерминированы по своей природе) — только что механизм в принципе
/// считает, а не всегда возвращает `Duration::ZERO` из-за какой-нибудь
/// перепутанной ветки `if let`.
#[test]
fn test_run_tick_profiled_reports_nonzero_phase_timings() {
    let mut engine = Engine::new(make_grid(3, 1), make_starvation_rules(Some(3)));
    engine.grid_mut().set_cell(0, 0, Cell::new(1));

    let (_, _, timings) = engine.run_tick_profiled();
    assert!(
        timings.detect > std::time::Duration::ZERO,
        "detect должен занять измеримое время на тике с реальными совпадениями"
    );
    assert!(
        timings.arbitrate > std::time::Duration::ZERO,
        "arbitrate должен занять измеримое время при конкуренции двух правил"
    );
    assert!(
        timings.apply > std::time::Duration::ZERO,
        "apply должен занять измеримое время, когда есть принятые совпадения"
    );
}

/// `Engine::enable_tick_logging`/`tick_log` (п.5, сессия 2026-08-09) —
/// счётчики отражают реальную конкуренцию правил, а не всегда нулевые
/// значения из-за перепутанной ветки: HIGH и LOW конкурируют за одну и ту
/// же клетку каждый тик (ровно как в `test_without_starvation_guard_low_priority_never_wins`),
/// так что на КАЖДОМ тике ожидается ровно один принятый и один отклонённый
/// кандидат, и ровно один кандидат "под наблюдением" starvation (только у
/// LOW есть `starvation_after`, у HIGH — нет). Также проверяет реальный
/// serde_json-раунд-трип (не просто "поля выглядят разумно в памяти") —
/// пятый пункт списка сессии явно назван "структурированное JSON-логирование".
#[test]
fn test_tick_logging_records_accepted_rejected_and_starvation_counts() {
    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let mut engine = Engine::new(grid, make_starvation_rules(Some(3)));
    engine.enable_tick_logging();

    for _ in 0..5 {
        engine.run_tick();
    }

    let log = engine
        .tick_log()
        .expect("tick_log должен быть Some после enable_tick_logging");
    assert_eq!(log.len(), 5, "по одной записи на каждый вызов run_tick");
    for (i, entry) in log.iter().enumerate() {
        assert_eq!(entry.tick, i as u64, "tick должен быть generation ДО этого тика");
        assert_eq!(
            entry.accepted, 1,
            "ровно один победитель на клетку в каждом тике: {:?}",
            entry
        );
        assert_eq!(
            entry.rejected, 1,
            "ровно один проигравший (тот же матч, что и без защиты): {:?}",
            entry
        );
        assert_eq!(
            entry.starvation_events, 1,
            "только LOW использует starvation_after: {:?}",
            entry
        );
        assert_eq!(
            entry.feedback_events, 0,
            "ни одно правило этого набора не использует feedback: {:?}",
            entry
        );
    }

    let json = serde_json::to_string(log).expect("TickLogEntry обязан сериализоваться в JSON без нестроковых ключей");
    let restored: Vec<TickLogEntry> = serde_json::from_str(&json).expect("обратная десериализация обязана пройти");
    assert_eq!(restored, log, "серде-раунд-трип обязан вернуть побитово тот же лог");
}

/// `Engine::snapshot()`/`Engine::from_snapshot()` — реальный serde-раунд-трип
/// (сериализация в текст и обратно, не просто "поля совпали в памяти") на
/// движке с накопленным `starvation_counters` (проверяет, что персистентное
/// состояние расширений переживает сохранение, не только `grid`/`rule_index`).
///
/// `serde_yaml`, НЕ `serde_json` — намеренно: JSON требует строковые ключи
/// объектов, а `rule_index` (ключ `CellType`), `grid.boundaries` (ключ
/// `(usize,usize)`) и все четыре карты `RuleStateStore` (ключи
/// `(u32,u32,usize)`/`(CellType,usize)`) — все с НЕ-строковыми ключами.
/// `serde_json::to_string` падает на этом с "key must be a string" —
/// найдено этим же тестом при первой попытке. YAML такого ограничения не
/// имеет. См. doc-комментарий `EngineSnapshot` — то же самое верно для
/// ЛЮБОГО формата, который выберет пользователь.
///
/// Самая сильная проверка из возможных: ОБА движка (оригинал, продолживший
/// работу, и восстановленный из снимка) прогоняются ДАЛЬШЕ на одинаковое
/// число тиков и сверяются побитово каждый тик — не "поля после восстановления
/// выглядят разумно", а "восстановленный движок ведёт себя ИДЕНТИЧНО тому, каким
/// был бы оригинал, не будь снимка вообще".
#[test]
fn test_engine_snapshot_yaml_roundtrip_matches_original_execution() {
    const K: u32 = 5;
    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let mut engine = Engine::new(grid, make_starvation_rules(Some(K)));

    // Копим состояние: 3 проигрыша LOW (starvation_counters ещё не 0) —
    // именно то персистентное состояние, которое просто пересборка кэшей
    // из rule_index (как делает `Engine::new`) не восстановила бы.
    for _ in 1..=3 {
        engine.run_tick();
    }
    assert_eq!(
        engine.state.snapshot().starvation_counters().get(&(0, 0, 1)),
        Some(&3),
        "счётчик LOW должен быть 3 перед снимком"
    );

    let snapshot = engine.snapshot();
    let yaml = serde_yaml::to_string(&snapshot).expect("snapshot must serialize to YAML");
    let restored_snapshot: EngineSnapshot<VecStorage> =
        serde_yaml::from_str(&yaml).expect("snapshot must deserialize back from YAML");
    let mut restored = Engine::from_snapshot(restored_snapshot);

    assert_eq!(
        restored.state.snapshot().starvation_counters().get(&(0, 0, 1)),
        Some(&3),
        "восстановленный движок должен видеть тот же счётчик голодания, что был на момент снимка"
    );
    assert_eq!(
        restored.grid().get_cell(0, 0),
        engine.grid().get_cell(0, 0),
        "содержимое решётки должно совпасть после восстановления"
    );

    // Тики 4-5: если бы счётчик НЕ восстановился (например, тихо обнулился),
    // LOW выиграл бы только на тике 7 (K=5 проигрышей С НУЛЯ), а не на тике 6
    // (K=5 проигрышей, ПРОДОЛЖАЯ уже накопленные 3) — тест ловит именно эту
    // разницу, не просто "оба движка не падают".
    for tick in 4..=7 {
        engine.run_tick();
        restored.run_tick();
        assert_eq!(
            engine.grid().get_cell(1, 0),
            restored.grid().get_cell(1, 0),
            "тик {tick}: оригинал и восстановленный из снимка движок обязаны совпасть побитово"
        );
    }
    // Явная проверка ожидаемого исхода (не только "оба совпали друг с
    // другом", а "оба сделали то, что математически обязаны были") — K=5,
    // 3 накоплено до снимка, ровно 2 новых проигрыша (тики 4-5) добивают до
    // 5, форсированная победа на тике 6, счётчик сбрасывается, тик 7 снова
    // HIGH.
    assert_eq!(
        engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
        Some(100),
        "тик 7: HIGH снова побеждает после форсированной победы LOW на тике 6"
    );
}

/// РЕАЛЬНЫЙ БАГ, найден 2026-08-11 при подготовке модели к 1.0:
/// `ChangeValue` раньше был `#[serde(untagged)]` — `Literal(u8)` и
/// `Ref(usize)` оба сериализуются как ГОЛОЕ число, неразличимо. При
/// десериализации `Ref(3)` тихо возвращался как `Literal(3)`. Это молча
/// ломало ЛЮБОЕ правило с `Ref` в `changes` после
/// `Engine::snapshot()`/`from_snapshot()` (и `replay()`, который тоже идёт
/// через снимок) — не только изолированный round-trip самого типа
/// (следующая половина этого теста), но и ЦЕЛЫЙ движок: правило "скопируй
/// значение из паттерна" превращалось в "запиши литерал, равный индексу
/// ссылки", без единой ошибки. Ни один существующий property/snapshot-тест
/// этого не ловил — все они гоняли `changes: vec![]` (пустой), ни разу не
/// строили правило с `Ref` в `changes` для проверки именно через снимок.
#[test]
fn test_change_value_ref_survives_serde_roundtrip_and_snapshot_restore() {
    // Изолированная проверка самого типа -- не через Engine, просто
    // serde_yaml round-trip.
    let original = ChangeValue::Ref(3);
    let yaml = serde_yaml::to_string(&original).expect("ChangeValue must serialize");
    let restored: ChangeValue = serde_yaml::from_str(&yaml).expect("ChangeValue must deserialize");
    assert_eq!(
        restored, original,
        "ChangeValue::Ref must survive a raw serde_yaml round-trip without collapsing into Literal"
    );

    // Сквозная проверка через реальный Engine::snapshot/from_snapshot:
    // правило "скопируй значение из pattern[0] в (1,0)" — паттерн матчит
    // (0,0)=1 и запоминает его значение (совпадающее с самим собой, но
    // важно НЕ значение шаблона, а то, что реально стоит в решётке — здесь
    // это то же самое 1, различие проявится через сам факт, что после
    // восстановления правило продолжает копировать, а не начинает писать
    // литерал "0" -- индекс ссылки).
    let rule = Rule {
        id: vec![CellType(1)],
        pattern: vec![(0, 0, CellType(1))],
        shifts: vec![],
        changes: vec![(1, 0, ChangeValue::Ref(0))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(1), vec![rule]);

    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let engine = Engine::new(grid, rule_index);

    let snapshot = engine.snapshot();
    let yaml = serde_yaml::to_string(&snapshot).expect("snapshot must serialize");
    let restored_snapshot: EngineSnapshot<VecStorage> = serde_yaml::from_str(&yaml).expect("snapshot must deserialize");
    let mut restored = Engine::from_snapshot(restored_snapshot);

    restored.run_tick();
    assert_eq!(
        restored.grid().get_cell(1, 0).map(|c| c.value.0 .0),
        Some(1),
        "after snapshot/restore, ChangeValue::Ref(0) must still COPY pattern[0]'s value (1) -- \
         if it silently collapsed to Literal, this cell would read Some(0) (the ref index) instead"
    );
}

/// `ChangeValue::Add`/`Sub` — конец-в-конец через реальный `Engine`, не
/// изолированный юнит-тест резолвера: правило матчит (0,0)=1, помнит его
/// значение через `pattern`, и пишет `pattern[0] + 10` в саму же клетку.
///
/// **Честное ограничение, а не забытый случай:** это ОДНО применение, не
/// накопление тик за тиком одним и тем же правилом. `CellType` — это
/// ОДНОВРЕМЕННО и "значение" (то, что арифметика читает/пишет), и КЛЮЧ
/// матчинга (`Rule::id`/`pattern` матчат ТОЧНЫЙ тип) — после первого тика
/// клетка становится типом 11, а это правило зарегистрировано под головой
/// `CellType(1)` и на тип 11 больше не сработает. "Счётчик, растущий
/// каждый тик ОДНИМ правилом" в текущей модели не выражается напрямую —
/// нужно было бы либо отдельное правило на каждое возможное значение
/// счётчика, либо более широкий (диапазонный/wildcard) матчинг, которого
/// у `pattern`/`id` пока нет. Не баг арифметики -- следствие того, что
/// значение и тип у клетки — одно и то же поле.
#[test]
fn test_change_value_add_applies_once_from_pattern_ref() {
    let rule = Rule {
        id: vec![CellType(1)],
        pattern: vec![(0, 0, CellType(1))],
        shifts: vec![],
        changes: vec![(
            0,
            0,
            ChangeValue::Add(Box::new(ChangeValue::Ref(0)), Box::new(ChangeValue::Literal(10))),
        )],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(1), vec![rule]);
    let mut grid = make_grid(1, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let mut engine = Engine::new(grid, rule_index);
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(11),
        "1 + 10 = 11 -- Add must read the pre-tick pattern value, not overwrite blindly"
    );
    // Второй тик: клетка теперь тип 11, это правило (голова 1) больше не
    // матчит -- значение остаётся неизменным, ИМЕННО демонстрируя
    // ограничение из doc-комментария выше, не гипотетическое.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(11),
        "after the value became a different CellType, the same rule (registered under head 1) must not fire again"
    );
}

/// `ChangeValue::Add`/`Sub` — переполнение `u8` заворачивается
/// (`wrapping_add`/`wrapping_sub`), не паникует и не насыщается на 255/0 --
/// см. `ChangeValue`'s doc-комментарий про то, почему wrapping выбран
/// намеренно, не случайно.
#[test]
fn test_change_value_add_wraps_on_u8_overflow() {
    let rule = Rule {
        id: vec![CellType(250)],
        pattern: vec![(0, 0, CellType(250))],
        shifts: vec![],
        changes: vec![(
            0,
            0,
            ChangeValue::Add(Box::new(ChangeValue::Ref(0)), Box::new(ChangeValue::Literal(10))),
        )],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(250), vec![rule]);
    let mut grid = make_grid(1, 1);
    grid.set_cell(0, 0, Cell::new(250));
    let mut engine = Engine::new(grid, rule_index);
    engine.run_tick();
    // 250 + 10 = 260, wrapping u8 -> 260 - 256 = 4.
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(4),
        "250 + 10 must wrap to 4 (260 mod 256), not panic or saturate at 255"
    );
}

/// `ChangeValue::Sub`, вложенная композиция (`Sub(Add(Ref, Literal),
/// Literal)`) — произвольная глубина рекурсии, не только одноуровневая пара.
#[test]
fn test_change_value_nested_add_sub() {
    let rule = Rule {
        id: vec![CellType(5)],
        pattern: vec![(0, 0, CellType(5))],
        shifts: vec![],
        // (pattern[0] + 20) - 3 = (5 + 20) - 3 = 22.
        changes: vec![(
            0,
            0,
            ChangeValue::Sub(
                Box::new(ChangeValue::Add(
                    Box::new(ChangeValue::Ref(0)),
                    Box::new(ChangeValue::Literal(20)),
                )),
                Box::new(ChangeValue::Literal(3)),
            ),
        )],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut rule_index: HashMap<CellType, Vec<Rule>> = HashMap::new();
    rule_index.insert(CellType(5), vec![rule]);
    let mut grid = make_grid(1, 1);
    grid.set_cell(0, 0, Cell::new(5));
    let mut engine = Engine::new(grid, rule_index);
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(22),
        "nested Sub(Add(Ref,Literal),Literal) must resolve recursively: (5+20)-3=22"
    );
}
