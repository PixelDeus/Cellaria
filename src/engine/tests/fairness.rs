use super::super::*;
use super::common::*;
use crate::types::{Cell, CellType, ChangeValue, Direction, Rule, ShiftSpec};

const TIMER: u8 = 60;
const FIRED: u8 = 61;

/// Способ 1 — `min_age` буквально И ЕСТЬ таймер по определению: правило
/// срабатывает, только когда возраст клетки достиг порога. Клетка стоит
/// TIMER ровно `THRESHOLD` тиков, затем "выстреливает" в FIRED — без
/// единого дополнительного механизма.
#[test]
fn test_timer_via_min_age_is_already_expressible() {
    const THRESHOLD: u64 = 5;
    let mut grid = make_grid(1, 1);
    grid.set_cell(0, 0, Cell::new(TIMER));
    let rule = Rule {
        id: vec![CellType(TIMER)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(0, 0, ChangeValue::Literal(FIRED))],
        active_only: false,
        priority: 0,
        min_age: THRESHOLD,
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
    let ri = make_rule_index(vec![rule]);
    let mut engine = Engine::new(grid, ri);

    for tick in 0..THRESHOLD {
        engine.run_tick();
        assert_eq!(
            engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
            Some(TIMER),
            "must still be TIMER before threshold (tick={tick})"
        );
    }
    engine.run_tick(); // tick == THRESHOLD: min_age condition finally satisfied
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(FIRED),
        "must fire exactly at the threshold"
    );
}

/// Способ 2 — счётная цепочка self-change правил (типы TIMER..TIMER+N-1,
/// каждый тик +1), для случаев, когда сам счёт должен быть ВИДИМ/читаем
/// другими правилами по пути (min_age скрыт внутри клетки, недоступен
/// чтению соседями) — тоже уже существующий примитив, не новый.
#[test]
fn test_timer_via_self_change_counting_chain_is_already_expressible() {
    const N: u8 = 5;
    let mut grid = make_grid(1, 1);
    grid.set_cell(0, 0, Cell::new(TIMER));
    let mut rules = Vec::new();
    for k in 0..N {
        let (from, to) = (TIMER + k, if k + 1 == N { FIRED } else { TIMER + k + 1 });
        rules.push(Rule {
            id: vec![CellType(from)],
            pattern: vec![],
            shifts: vec![],
            changes: vec![(0, 0, ChangeValue::Literal(to))],
            active_only: false,
            priority: 0,
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
        });
    }
    let ri = make_rule_index(rules);
    let mut engine = Engine::new(grid, ri);

    for k in 0..N {
        engine.run_tick();
        let expected = if k + 1 == N { FIRED } else { TIMER + k + 1 };
        assert_eq!(
            engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
            Some(expected),
            "counting chain step {k}"
        );
    }
}

// ──────────────────────────────────────────────────────────────
// Модульный tie-break в арбитраже (block F, п.3): два правила с ОДИНАКОВЫМ
// priority, матчащие одну и ту же клетку (тип 1, никогда не меняется — ни
// одно из правил не пишет в неё саму, только в соседнюю (1,0)) КАЖДЫЙ тик,
// так что age у обоих матчей тоже всегда совпадает — приоритет и возраст
// специально уравнены, чтобы изолировать именно tie_break как решающий
// фактор (иначе он бы никогда не дошёл до сравнения).
// ──────────────────────────────────────────────────────────────

fn make_tie_break_rules(tie_break_a: u32, tie_break_b: u32) -> HashMap<CellType, Vec<Rule>> {
    let rule_a = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(1, 0, ChangeValue::Literal(100))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: tie_break_a,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let rule_b = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(1, 0, ChangeValue::Literal(200))],
        active_only: false,
        priority: 10,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: tie_break_b,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    make_rule_index(vec![rule_a, rule_b])
}

/// tie_break=0 у ОБОИХ правил (значение по умолчанию) не должно менять
/// старое поведение: арбитраж по-прежнему падает на лексикографический
/// порядок id/rule_idx, который НЕ зависит от поколения — победитель обязан
/// быть одним и тем же на каждом тике.
#[test]
fn test_tie_break_default_zero_preserves_old_rule_id_order() {
    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let mut engine = Engine::new(grid, make_tie_break_rules(0, 0));

    let mut winners = Vec::new();
    for _ in 0..10 {
        engine.run_tick();
        winners.push(engine.grid().get_cell(1, 0).map(|c| c.value.0 .0));
    }
    assert!(
        winners.iter().all(|&w| w == winners[0]),
        "с tie_break=0 у обоих правил победитель не должен зависеть от поколения: {winners:?}"
    );
}

/// Два правила с tie_break, расставленными РОВНО на M/2 друг от друга
/// (см. doc-комментарий `arbitrator::TIE_BREAK_MODULUS`), должны чередовать
/// победу СТРОГО поровну за один полный период M поколений — прямая
/// проверка формулы `(tie_break + generation) % M`, а не просто "иногда
/// меняется".
#[test]
fn test_tie_break_rotates_fairly_when_spaced_half_modulus_apart() {
    use crate::engine::arbitrator::TIE_BREAK_MODULUS;

    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let mut engine = Engine::new(grid, make_tie_break_rules(0, TIE_BREAK_MODULUS / 2));

    let (mut a_wins, mut b_wins) = (0u32, 0u32);
    for gen in 0..TIE_BREAK_MODULUS {
        engine.run_tick();
        match engine.grid().get_cell(1, 0).map(|c| c.value.0 .0) {
            Some(100) => a_wins += 1,
            Some(200) => b_wins += 1,
            other => panic!("неожиданное значение (1,0) на поколении {gen}: {other:?}"),
        }
    }
    assert_eq!(
        a_wins,
        TIE_BREAK_MODULUS / 2,
        "правило A должно выигрывать ровно половину периода"
    );
    assert_eq!(
        b_wins,
        TIE_BREAK_MODULUS / 2,
        "правило B должно выигрывать ровно половину периода"
    );
}

// ──────────────────────────────────────────────────────────────
// Опциональный temporal arbitration против голодания по РАЗНОМУ приоритету
// (block F, п.5) — в отличие от tie_break (решает только РАВНЫЙ приоритет),
// здесь HIGH (priority=20) и LOW (priority=5) конкурируют за одну и ту же
// клетку каждый тик; без starvation_after HIGH обязан побеждать НАВСЕГДА
// (priority — первое и решающее поле ключа сортировки).
// ──────────────────────────────────────────────────────────────

#[test]
fn test_without_starvation_guard_low_priority_never_wins() {
    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let mut engine = Engine::new(grid, make_starvation_rules(None));

    for tick in 0..30 {
        engine.run_tick();
        assert_eq!(
            engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
            Some(100),
            "без starvation_after HIGH должен побеждать абсолютно всегда (тик {tick}) — иначе голодание не доказано"
        );
    }
}

/// `starvation_after = Some(K)` на LOW: проигрывает K тиков подряд, потом
/// гарантированно побеждает РОВНО на (K+1)-м тике (эффективный priority в
/// этот тик становится u32::MAX), после чего счётчик сбрасывается и цикл
/// повторяется — строго периодический паттерн побед на тиках K+1, 2(K+1),
/// 3(K+1), ... — прямая проверка формулы, а не просто "хоть раз победил".
#[test]
fn test_starvation_guard_guarantees_periodic_progress() {
    const K: u32 = 3;
    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let mut engine = Engine::new(grid, make_starvation_rules(Some(K)));

    let mut low_wins_at = Vec::new();
    const TOTAL_TICKS: u32 = 20;
    for tick in 1..=TOTAL_TICKS {
        engine.run_tick();
        match engine.grid().get_cell(1, 0).map(|c| c.value.0 .0) {
            Some(200) => low_wins_at.push(tick),
            Some(100) => {}
            other => panic!("неожиданное значение (1,0) на тике {tick}: {other:?}"),
        }
    }

    let expected: Vec<u32> = (1..=TOTAL_TICKS).filter(|t| t % (K + 1) == 0).collect();
    assert_eq!(
        low_wins_at, expected,
        "LOW должен побеждать РОВНО каждый (K+1)-й тик, не чаще и не реже"
    );
}

/// Регрессионный тест на реальный, найденный при аудите GPU-портирования
/// `starvation_after` баг: в отличие от `feedback_counters`/`memory_buffers`
/// (оба явно чистятся от осиротевших записей — см. `ExtensionFlags::extension_rule_indices`'s
/// doc-комментарий, который дословно говорит "для правил с `feedback` ИЛИ
/// `memory`", НЕ упоминая `starvation_after`), `starvation_counters`
/// обновляется ТОЛЬКО в двух местах: рост при проигрыше (если матч —
/// кандидат ЭТОГО тика) и удаление при выигрыше. Если матч (x,y,rule_idx)
/// просто ПЕРЕСТАЁТ быть кандидатом (клетка сменила тип из-за чего-то
/// постороннего) с НЕНУЛЕВЫМ, но ещё не достигшим порога счётчиком, запись
/// не растёт (не кандидат — не в `starving_keys`) и не удаляется (не
/// выигрыш) — просто ЗАСТЫВАЕТ в `HashMap` навсегда. Если та же позиция
/// ПОЗЖЕ снова станет кандидатом для ТОГО ЖЕ rule_idx, счётчик ошибочно
/// ПРОДОЛЖИТ с замороженного значения, а не с нуля — голодающее правило
/// побеждает раньше, чем должно бы по своей ЖЕ гарантии "K проигрышей
/// подряд, отсчитываемых с нуля".
///
/// Раскладка: WATCHER (тип 1) на x=0, конкурируют HIGH (priority 20, без
/// starvation) и LOW (priority 5, starvation_after=Some(3)) — оба пишут
/// РАЗНЫЕ литералы в СОСЕДА (x=1), сама клетка x=0 остаётся типом 1 у ОБОИХ
/// (идемпотентно), пока её кто-то НЕ тронет напрямую — здесь тик 3
/// принудительно подменяет x=0 на посторонний DECOY (без единого
/// подходящего правила) на РОВНО один тик, потом возвращает обратно. К
/// этому моменту LOW успел проиграть ровно 2 раза (счётчик=2 < K=3, ещё НЕ
/// выиграл бы сам по себе). Правильное поведение (счётчик сброшен на 0 при
/// исчезновении матча) требует ЕЩЁ 3 полных проигрыша ПОСЛЕ возвращения,
/// прежде чем LOW снова выиграет; баг (счётчик заморожен на 2) даёт LOW
/// выиграть уже через 1 проигрыш после возвращения.
#[test]
fn test_starvation_counter_resets_after_match_disappears_and_reappears() {
    const K: u32 = 3;
    const DECOY: u8 = 250;
    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let mut engine = Engine::new(grid, make_starvation_rules(Some(K)));

    // Тики 1-2: LOW проигрывает дважды (счётчик 0->1->2), матч жив всё время
    // (x=0 остаётся типом 1 у обоих правил).
    for tick in 1..=2 {
        engine.run_tick();
        assert_eq!(
            engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
            Some(100),
            "тик {tick}: HIGH должен побеждать (счётчик LOW ещё не достиг порога)"
        );
    }

    // Тик 3: подменяем x=0 на DECOY напрямую (посторонняя клетка, ни у кого
    // нет для неё правил) — матч (0,0,rule_idx LOW) на этот тик просто не
    // существует, счётчик НЕ должен ни расти, ни выигрывать. Возвращаем x=0
    // обратно в тип 1 сразу после — на СЛЕДУЮЩЕМ тике матч снова существует.
    engine.grid_mut().set_cell(0, 0, Cell::new(DECOY));
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(0, 0).map(|c| c.value.0 .0),
        Some(DECOY),
        "тик 3: клетка временно постороннего типа, ничьё правило её не трогает"
    );
    engine.grid_mut().set_cell(0, 0, Cell::new(1));

    // Тики 4-6 (3 полных проигрыша после возвращения): если счётчик
    // КОРРЕКТНО сброшен на 0 при исчезновении, HIGH обязан побеждать все три
    // раза — LOW выигрывает только на тике 7 (4-й проигрыш подряд с нуля).
    // Баг дал бы LOW выиграть уже на тике 4 (замороженный счётчик 2 + 1
    // проигрыш = 3 >= K).
    for tick in 4..=6 {
        engine.run_tick();
        assert_eq!(
            engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
            Some(100),
            "тик {tick}: HIGH должен побеждать -- если LOW победил здесь, счётчик голодания НЕ был сброшен при исчезновении матча (реальный баг, не гипотеза)"
        );
    }
    engine.run_tick(); // тик 7: 4-й проигрыш подряд с нуля -> LOW обязан выиграть
    assert_eq!(
        engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
        Some(200),
        "тик 7: LOW обязан выиграть -- ровно K=3 проигрыша подряд С НУЛЯ после возвращения матча"
    );
}

/// Регрессия: `rule_idx` -- позиция в списке правил головы, не стабильный id
/// (см. `Engine::last_rebuilt_rule_index`'s doc-комментарий). Если прямая
/// правка `rule_index` заменяет правило на другое, занимающее ТУ ЖЕ позицию
/// у той же головы, новое правило не должно наследовать `starvation_counters`
/// старого -- иначе оно может выиграть арбитраж на первом же тике своего
/// существования, ничего в реальности не "выстрадав".
///
/// Порог у НОВОГО правила намеренно 1 (не то же K=5, что у старого) --
/// проверяем не просто "счётчик не тот", а конкретно наблюдаемое поведение:
/// если унаследованный счётчик (3) >= нового порога (1), баг дал бы победу
/// LOW немедленно на первом тике после замены. С фиксом -- только на втором.
#[test]
fn test_rebuild_rule_cache_clears_stale_starvation_counter_on_rule_idx_reuse() {
    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let mut engine = Engine::new(grid, make_starvation_rules(Some(5)));

    // Тики 1-3: LOW (rule_idx 1 для головы CellType(1)) проигрывает трижды,
    // счётчик 0->1->2->3, порог 5 ещё не достигнут -- HIGH побеждает все три раза.
    for tick in 1..=3 {
        engine.run_tick();
        assert_eq!(
            engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
            Some(100),
            "тик {tick}: HIGH должен побеждать (старый LOW ещё не достиг K=5)"
        );
    }
    assert_eq!(
        engine.state.snapshot().starvation_counters().get(&(0, 0, 1)),
        Some(&3),
        "счётчик старого LOW должен быть 3 перед заменой правила"
    );

    // Прямая замена rule_idx=1 у головы 1 на ДРУГОЕ правило с НИЗКИМ порогом
    // (K=1) -- тот же паттерн `strength_live_rules.rs`, что уже используется
    // в других тестах самомодификации/прямой правки: мутировать `rule_index`
    // и вызвать `rebuild_rule_cache()`.
    let new_low = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(1, 0, ChangeValue::Literal(201))],
        active_only: false,
        priority: 5,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: Some(1),
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut low_head_rules = engine.rule_index().get(&CellType(1)).unwrap().clone();
    low_head_rules[1] = new_low;
    engine.set_rules_for_head(CellType(1), low_head_rules);

    assert_eq!(
        engine.state.snapshot().starvation_counters().get(&(0, 0, 1)),
        None,
        "rebuild_rule_cache должен был очистить унаследованный счётчик старого правила на переиспользованном rule_idx"
    );

    // Тик 4 (первый после замены): без фикса счётчик 3 >= нового порога 1,
    // NEW LOW выиграл бы немедленно (201). С фиксом счётчик 0 < 1 -- HIGH
    // побеждает, это первый "настоящий" проигрыш нового правила.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
        Some(100),
        "тик 4: HIGH должен победить -- новое правило не должно унаследовать счётчик 3 от старого"
    );

    // Тик 5: ровно один проигрыш нового правила с нуля >= его порога 1 --
    // теперь оно обязано выиграть.
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
        Some(201),
        "тик 5: новое LOW обязано выиграть -- ровно 1 проигрыш подряд С НУЛЯ после замены правила"
    );
}

/// Регрессия: `tie_break`-победа НЕ должна сбрасывать `starvation_counters`
/// так же, как решительная (priority/age) победа -- иначе правило,
/// побеждающее только жребием, может НИКОГДА не накопить `starvation_after`,
/// даже суммарно проигрывая чаще, чем выигрывая (реальный найденный класс
/// бага, не гипотеза -- см. `arbitrator::TieBreakDecidedWins`).
///
/// LOW (priority=5, tie_break=0, starvation_after=10) конкурирует с PARTNER
/// (priority=5, tie_break=8, без starvation_after) за одну и ту же клетку
/// -- РАВНЫЙ priority у обоих, так что победитель решается исключительно
/// `tie_break_rotated = (tie_break + generation) % 16` (`TIE_BREAK_MODULUS`).
/// При этой паре tie_break-значений победитель чередуется БЛОКАМИ по 8
/// тиков (арифметика по модулю 16, см. комментарий внутри теста) -- PARTNER
/// побеждает generation 0-7 и 16-23, LOW побеждает generation 8-15.
///
/// Старая (баговая) семантика сбрасывала бы счётчик LOW в 0 на КАЖДОЙ из 8
/// побед generation 8-15 -- следующий блок проигрышей (16-23) успевает
/// накопить не больше 8 ПОДРЯД, порог 10 никогда не достигается, счётчик
/// вечно колеблется 0<->8, LOW голодает НАВСЕГДА, несмотря на
/// `starvation_after`. Новая семантика не трогает счётчик на tie_break-
/// победах -- накопленные 8 проигрышей из первого блока переживают блок
/// побед LOW, и всего 2 дополнительных проигрыша во втором блоке (generation
/// 16, 17) добивают счётчик до 10, форсируя гарантированную победу на
/// generation 18 -- СРЕДИ блока, который иначе (без гарантии) выиграл бы
/// PARTNER.
#[test]
fn test_starvation_after_ignores_tie_break_decided_wins() {
    const K: u32 = 10;
    let mut grid = make_grid(2, 1);
    grid.set_cell(0, 0, Cell::new(1));
    let rule_low = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(1, 0, ChangeValue::Literal(200))],
        active_only: false,
        priority: 5,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 0,
        starvation_after: Some(K),
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let rule_partner = Rule {
        id: vec![CellType(1)],
        pattern: vec![],
        shifts: vec![],
        changes: vec![(1, 0, ChangeValue::Literal(100))],
        active_only: false,
        priority: 5,
        min_age: 0,
        overflow: Default::default(),
        cam: None,
        tie_break: 8,
        starvation_after: None,
        feedback: None,
        recursion: None,
        memory: None,
        max_activations: None,
        cross_layer_reads: Vec::new(),
    };
    let mut engine = Engine::new(grid, make_rule_index(vec![rule_low, rule_partner]));

    // Generation 0-7 (тики 1-8): PARTNER побеждает все 8 раз -- LOW теряет
    // 8 раз подряд, счётчик 0->8.
    for tick in 1..=8 {
        engine.run_tick();
        assert_eq!(
            engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
            Some(100),
            "тик {tick}: PARTNER должен побеждать (generation 0-7)"
        );
    }
    assert_eq!(
        engine.state.snapshot().starvation_counters().get(&(0, 0, 0)),
        Some(&8),
        "счётчик LOW должен быть 8 после первого блока проигрышей"
    );

    // Generation 8-15 (тики 9-16): LOW побеждает все 8 раз через tie_break
    // (priority РАВНЫ -- не forced-победа, `starvation_counters` ещё не
    // достиг K=10). Счётчик должен остаться 8 -- НЕ сброситься.
    for tick in 9..=16 {
        engine.run_tick();
        assert_eq!(
            engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
            Some(200),
            "тик {tick}: LOW должен побеждать через tie_break (generation 8-15)"
        );
    }
    assert_eq!(
        engine.state.snapshot().starvation_counters().get(&(0, 0, 0)),
        Some(&8),
        "счётчик LOW НЕ должен был сброситься -- эти 8 побед решены tie_break, не priority/age"
    );

    // Generation 16-17 (тики 17-18): PARTNER снова побеждает -- 2
    // дополнительных проигрыша добивают счётчик LOW до 8+2=10=K.
    engine.run_tick(); // generation 16 -> тик 17
    assert_eq!(
        engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
        Some(100),
        "тик 17: PARTNER побеждает (generation 16, счётчик LOW ещё 8<10)"
    );
    engine.run_tick(); // generation 17 -> тик 18, счётчик после тика: 8+1+1=10
    assert_eq!(
        engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
        Some(100),
        "тик 18: PARTNER побеждает (generation 17, счётчик LOW ещё 9<10)"
    );
    assert_eq!(
        engine.state.snapshot().starvation_counters().get(&(0, 0, 0)),
        Some(&10),
        "счётчик LOW должен достичь K=10 после этого проигрыша"
    );

    // Тик 19 (generation 18): счётчик 10>=10 -- LOW ОБЯЗАН выиграть форсированно,
    // хотя generation 18 -- часть блока (16-23), который по чистому tie_break
    // отдал бы победу PARTNER (см. арифметику в doc-комментарии теста).
    engine.run_tick();
    assert_eq!(
        engine.grid().get_cell(1, 0).map(|c| c.value.0 .0),
        Some(200),
        "тик 19: LOW обязан выиграть форсированно -- без фикса счётчик никогда бы не накопился до K=10 (вечно колебался бы 0<->8)"
    );
    assert_eq!(
        engine.state.snapshot().starvation_counters().get(&(0, 0, 0)),
        None,
        "форсированная победа -- решительная (priority override), счётчик должен сброситься"
    );
}

/// Регрессия: `compose_with` раньше гонял свой собственный цикл тиков через
/// "сырые" `Engine::arbitrate`/`Engine::apply_matches` (см. `raw_phases.rs`'s
/// doc-комментарий) вместо настоящего `Engine::run_tick` -- тот же класс
/// бага, что был у `LayeredEngine`, только для этой
/// проверки не закрытый до сих пор.
///
/// `max_activations` даёт прямое, наблюдаемое через САМ ВЕРДИКТ различие:
/// правило с `keep_source` копирует маркер вправо (не убывая у источника,
/// см. `max_activations::test_max_activations_bounds_keep_source_growth`),
/// без работающего бюджета срабатываний растёт, пока не заполнит решётку
/// (20 клеток, `initial_count=1` -> `final_count=20` -> `Divergent`, т.к.
/// 20 > 1*2). С РАБОТАЮЩИМ бюджетом (`Some(1)`, через `Engine::state`,
/// сырые методы для него всегда no-op) рост останавливается на 2 маркерах
/// (1 исходный + 1 копия) -> `final_count=2`, НЕ строго больше `1*2=2` ->
/// `Bounded(2)`. Если `compose_with` регрессирует обратно на сырые методы,
/// этот тест увидит `Divergent` вместо `Bounded(2)` и упадёт.
#[test]
fn test_compose_with_respects_max_activations_not_raw_stateless_methods() {
    const MARKER: u8 = 72;
    let rule = Rule {
        id: vec![CellType(MARKER)],
        pattern: vec![(0, 0, CellType(MARKER)), (1, 0, CellType(0))],
        shifts: vec![vec![ShiftSpec { direction: Direction::Right, steps: 1, broadcast: false, keep_source: true }]],
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
        max_activations: Some(1),
        cross_layer_reads: Vec::new(),
    };
    let mut grid = make_grid(20, 1);
    grid.set_cell(0, 0, Cell::new(MARKER));
    let mut engine = Engine::new(grid, make_rule_index(vec![rule]));

    let verdict = engine.compose_with();
    assert_eq!(
        verdict,
        CompositionVerdict::Bounded(2),
        "max_activations=1 обязан остановить рост на 2 маркерах (1 исходный + 1 копия) через настоящий тик-пайплайн -- \
         Divergent здесь означал бы, что compose_with снова использует сырые detect/arbitrate/apply, для которых max_activations -- no-op"
    );
}
