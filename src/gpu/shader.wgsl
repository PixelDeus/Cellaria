// Два пайплайна тика, выбираемые `GpuEngine` по `GpuRuleTable::needs_arbitration`
// (см. её doc-комментарий в rule_table.rs):
//
// 1. `main` — self-write-only путь (например, Game of Life): каждый поток
//    (= одна клетка) читает и пишет ТОЛЬКО свою собственную ячейку.
//    Конфликтов записи между потоками в принципе нет — арбитраж не нужен,
//    один проход на тик.
//
// 2. `detect_pass` → [`clear_claims`, `claim_pass`, `resolve_pass`]×ROUNDS →
//    `count_pending` → `apply_pass` — общий путь (сдвиги и/или запись в
//    соседа): у клетки может быть НЕСКОЛЬКО совпавших правил, каждое
//    пишущее в НЕСКОЛЬКО ячеек (source-clear сдвига + цель(и) сдвига +
//    changes относительно каждой цели — зеркалит
//    `applicator::apply_rule_buffered`), и РАЗНЫЕ клетки-источники могут
//    целиться в ОДНУ и ту же ячейку. Арбитраж — многораундовый lock-free
//    claim/resolve через `atomicCompareExchangeWeak` (не `atomicMax`:
//    тай-брейк — составной 13-польный ключ priority→age→tie_break→id0..id7→x→y→rule_idx,
//    как `arbitrator::arbitrate`, а не одно число) — алгоритм и его
//    сходимость (гарантированная: топ-1 незанятый матч по ключу выигрывает
//    ВСЕ свои ячейки каждый раунд, значит pending-множество строго
//    уменьшается) проверены отдельно в scratch-прототипе
//    (`v2_arbitrate_check.rs`) на 300 случайных сценариях конфликтов,
//    включая сверку ПОБЕДИТЕЛЯ каждой ячейки, не только accept/reject.
//
//    ВАЖНО: число раундов до сходимости РАСТЁТ ЛИНЕЙНО с длиной "цепочки"
//    последовательно зависимых конфликтов (найдено экспериментально,
//    scratch-прототип `chain_check.rs`: цепочка из n матчей требует ~n/2
//    раундов) — то есть НИКАКОЙ фиксированный `ROUNDS` не гарантирует
//    сходимость universally. Поэтому `count_pending` в конце считает,
//    сколько матчей НЕ сошлось за `ROUNDS`, и `GpuEngine::dispatch_tick`
//    досчитывает их на CPU (гибридная схема, см. её doc-комментарий и
//    scratch-прототип `hybrid_check.rs`, подтвердивший точное совпадение
//    с полным CPU-эталоном для цепочек длиной от 10 до 1000).
//
// Структуры и раскладка полей 1-в-1 соответствуют `rule_table.rs`
// (`GpuRule`/`GpuPatternOffset`/`GpuHeadSlot`/`GpuOffset`) —
// `bytemuck::cast_slice` льёт CPU-структуры в буферы напрямую, без
// сериализации. Поля везде плоские (`id_b0..id_b7`, а не `[u32;8]`) —
// избегает индексации МАССИВА ВНУТРИ ЗНАЧЕНИЯ (не указателя) динамическим
// индексом: naga отказывается это компилировать ("may only be indexed by a
// constant"). Массивы `GpuMatch::cells`/`values` ниже — ИСКЛЮЧЕНИЕ (не
// избежать, матч пишет переменное число ячеек), поэтому к ним ВЕЗДЕ
// обращаются через storage-буфер напрямую (`matches[m].cells[k]`), НИКОГДА
// не через промежуточную локальную копию всей структуры.
//
// `params.generation` — счётчик поколений ДО инкремента этого тика (то же
// значение, что видел бы `grid.generation()` в CPU `detect_matches` перед
// `advance_age()`) — им же считается `age`. Записываемым клеткам ставится
// `generation + 1` (как `engine::reset_age_for_regions` на CPU — born_at
// уже ПОСЛЕ advance_age()) — вызывающая сторона обязана держать свой
// счётчик поколений синхронно с этим же сдвигом на 1 между тиками.

struct Params {
    width: u32,
    height: u32,
    generation: u32,
    default_cell_type: u32,
    // Реальный (не потолочный) охват для этого набора правил — см.
    // `rule_table::GpuRuleTable::margin`. Не используется путём `main`
    // (Simple-пайплайн вообще не строит дополненную сетку).
    margin: u32,
    // Реальный (не потолочный `MAX_MATCHES_PER_CELL`) максимум правил на
    // одну голову для ЭТОГО набора правил — см.
    // `rule_table::GpuRuleTable::max_matches_per_cell`. Только для
    // Arbitrated-пайплайна (см. `detect_pass`/`claim_pass`/`resolve_pass`) —
    // задаёт, сколько потоков-кандидатов реально нужно на клетку, вместо
    // всегда потолочных 8.
    max_matches_per_cell: u32,
    pad0: u32,
    pad1: u32,
};

struct Cell {
    value: u32,
    born_at: u32,
};

struct HeadSlot {
    rules_start: u32,
    rules_count: u32,
    offsets_start: u32,
    offsets_count: u32,
};

struct GpuRule {
    pattern_start: u32,
    pattern_len: u32,
    priority: u32,
    min_age: u32,
    active_only: u32,
    id_len: u32,
    id_b0: u32,
    id_b1: u32,
    id_b2: u32,
    id_b3: u32,
    id_b4: u32,
    id_b5: u32,
    id_b6: u32,
    id_b7: u32,
    rule_idx: u32,
    shift_count: u32,
    shift_dx0: i32,
    shift_dy0: i32,
    shift_dx1: i32,
    shift_dy1: i32,
    // `ShiftSpec::broadcast` соответствующего сдвига (1/0) — см.
    // `rule_table::MAX_BROADCAST_REACH`'s doc-комментарий. Осмыслено только
    // при `shift_count >= 1`/`>= 2` соответственно.
    shift_broadcast0: u32,
    shift_broadcast1: u32,
    change_count: u32,
    change_dx0: i32,
    change_dy0: i32,
    change_val0: u32,
    change_dx1: i32,
    change_dy1: i32,
    change_val1: u32,
    change_dx2: i32,
    change_dy2: i32,
    change_val2: u32,
    change_dx3: i32,
    change_dy3: i32,
    change_val3: u32,
    // 0, если правило НЕ использует CAM — см. `rule_table::GpuRule::cam_radius`.
    cam_radius: u32,
    cam_target_type: u32,
    // См. `rule_table::GpuRule::tie_break` — прямое значение, вращение
    // делается при записи в GpuMatch (см. TIE_BREAK_MODULUS ниже).
    tie_break: u32,
    // 0, если правило НЕ использует recursion — см.
    // `rule_table::MAX_RECURSION_DEPTH`'s doc-комментарий.
    recursion_max_depth: u32,
    recursion_dx: i32,
    recursion_dy: i32,
    // См. `rule_table::GpuRule::has_starvation`'s doc-комментарий про то,
    // почему это ОТДЕЛЬНЫЙ флаг, а не "0 в starvation_threshold = выключено"
    // (порог 0 — реальное, отличное от "не установлено" значение).
    has_starvation: u32,
    starvation_threshold: u32,
    // См. `rule_table::GpuRule::has_feedback`'s doc-комментарий.
    has_feedback: u32,
    feedback_timeout: u32,
    feedback_alt_dx: i32,
    feedback_alt_dy: i32,
    // См. `rule_table::GpuRule::has_memory`'s doc-комментарий.
    has_memory: u32,
    memory_window: u32,
    memory_trigger: u32, // 0 = NeighborType, 1 = RuleOutcome
    memory_dx: i32,
    memory_dy: i32,
    memory_has_shift: u32,
    // Плоские поля вместо массива (см. `rule_table::GpuRule`'s
    // doc-комментарий про ограничение naga "may only be indexed by a
    // constant" для значений, загруженных динамическим индексом) —
    // значимы только первые `memory_window` из них.
    memory_pattern0: u32,
    memory_pattern1: u32,
    memory_pattern2: u32,
    memory_pattern3: u32,
};

// ДОЛЖНО совпадать с CPU `arbitrator::TIE_BREAK_MODULUS` — иначе побитовое
// совпадение с CPU-эталоном сломается на любом правиле с tie_break != 0
// (см. её подробный doc-комментарий в arbitrator.rs про выбор степени
// двойки и рецепт M/2-расстановки для честного чередования).
const TIE_BREAK_MODULUS: u32 = 16u;

struct PatternOffset {
    dx: i32,
    dy: i32,
    expected: u32,
    pad0: u32,
};

struct Offset {
    dx: i32,
    dy: i32,
};

// MAX_WRITE_CELLS в rule_table.rs = max(путь сдвигов = 1 + MAX_SHIFTS*(MAX_BROADCAST_REACH+MAX_CHANGES) = 17,
// путь recursion = (MAX_RECURSION_DEPTH+1)*MAX_CHANGES = 5*4 = 20) = 20.
// Обычный (не-broadcast) сдвиг пишет только 1 ячейку (конечную точку);
// broadcast — до MAX_BROADCAST_REACH ячеек пути (см. её doc-комментарий про
// то, почему это отдельный, более узкий потолок, чем MAX_SHIFT_REACH обычных
// сдвигов); recursion (взаимоисключим со сдвигами) — до MAX_RECURSION_DEPTH+1
// уровней, каждый до MAX_CHANGES ячеек (см. её doc-комментарий) — этот
// массив общий для ВСЕХ конфигов сразу, шейдер компилируется один раз.
const MAX_WRITE_CELLS: u32 = 20u;
// ДОЛЖНО совпадать с CPU `rule_table::MAX_MEMORY_WINDOW` — потолок
// `MemorySpec::window`, размер per-match среза в `memory_buffers` ниже.
const MAX_MEMORY_WINDOW: u32 = 4u;
// `params.max_matches_per_cell` (не константа!) — см. её doc-комментарий в
// struct Params выше; статический потолок — rule_table::MAX_MATCHES_PER_CELL,
// используется только для валидации на CPU-стороне, сюда не попадает.
// `params.margin` (не константа!) — РЕАЛЬНЫЙ охват для ЭТОГО набора правил
// (см. `rule_table::GpuRuleTable::margin`), не статический потолок
// `MAX_MARGIN` — тот применяется только на этапе валидации в rule_table.rs
// и сюда уже не попадает. `claims`/`locked` живут в ДОПОЛНЕННОЙ координатной
// сетке шириной/высотой решётки + `2×margin` по каждой оси (см.
// `padded_idx`) — этого достаточно, чтобы ЛЮБАЯ клетка записи
// (source-clear/цель сдвига/change относительно неё), даже уходящая за
// видимый край, имела свой уникальный слот вместо того, чтобы либо
// обрезаться (расходясь с CPU), либо давить в общий "мусорный" индекс
// (ложно "конфликтуя" с несвязанными матчами). "Фантомные" ячейки записи
// возникают из-за того, что CPU-арбитраж (`arbitrator::get_match_affected_cells`)
// учитывает конфликты по НЕклэмпнутым относительным координатам при
// `OverflowAction::Discard` — см. подробный doc-комментарий у
// `rule_table::MAX_MARGIN`.

struct GpuMatch {
    priority: u32,
    age: u32,
    // Уже ПОВЁРНУТОЕ значение — (rules[i].tie_break + params.generation) %
    // TIE_BREAK_MODULUS, посчитано ОДИН раз при записи матча (detect_pass/
    // main_tiled), не при каждом сравнении в match_is_better (тот вызывается
    // много раз за раунды claim/resolve).
    tie_break: u32,
    id0: u32, id1: u32, id2: u32, id3: u32, id4: u32, id5: u32, id6: u32, id7: u32,
    x: u32,
    y: u32,
    rule_idx: u32,
    cell_count: u32,
    cells: array<u32, MAX_WRITE_CELLS>,
    values: array<u32, MAX_WRITE_CELLS>,
    // 1, если правило ДЕЙСТВИТЕЛЬНО совпало (прошло все проверки pattern/
    // min_age/gate) на этом тике, 0 — если отвергнуто ДО этой точки (тип не
    // тот, паттерн не совпал, и т.д.). НЕ то же самое, что `cell_count > 0`
    // (совпавшее правило может законно писать 0 ячеек — например, cam без
    // цели в радиусе — и это НЕ равнозначно "не совпало" для целей
    // `starvation_after`: правило детектировалось, просто нечего было
    // писать). Используется ТОЛЬКО `update_starvation_pass` ниже, чтобы
    // отличить "не участвовало в этом тике вовсе" (сброс счётчика — тот же
    // класс осиротевшей записи, что был найден и исправлен для
    // CPU-side `starvation_counters`, см. `engine/mod.rs`) от "участвовало,
    // но проиграло" (рост счётчика).
    //
    // Для `Rule::memory`: `matched` означает "гейт открыт" (финальный
    // статус кандидата — гейт-закрытые матчи трактуются как НЕ совпавшие
    // для арбитража/starvation/feedback, см. `structural` ниже про их
    // отличие).
    matched: u32,
    // 1, если правило СТРУКТУРНО совпало (pattern/min_age/active_only все
    // прошли), НЕЗАВИСИМО от гейта `Rule::memory`. Нужен ТОЛЬКО для
    // `update_memory_push_pass`/`update_memory_relocate_pass` ниже — буфер
    // обязан продолжать наблюдать, даже когда гейт закрыт (зеркалит CPU
    // `memory_targets`, взятый из ПОЛНОГО, ещё не гейтованного списка
    // матчей, см. `engine/mod.rs`'s doc-комментарий). Для правил без
    // `memory` это поле ни на что не влияет.
    structural: u32,
};

struct Counters {
    changed: atomic<u32>,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> current: array<Cell>;
@group(0) @binding(2) var<storage, read_write> next: array<Cell>;
@group(0) @binding(3) var<storage, read> head_slots: array<HeadSlot>;
@group(0) @binding(4) var<storage, read> rules: array<GpuRule>;
@group(0) @binding(5) var<storage, read> pattern_offsets: array<PatternOffset>;
@group(0) @binding(6) var<storage, read> head_offsets: array<Offset>;
@group(0) @binding(7) var<storage, read_write> matches: array<GpuMatch>;
@group(0) @binding(8) var<storage, read_write> match_state: array<atomic<u32>>;
@group(0) @binding(9) var<storage, read_write> claims: array<atomic<u32>>;
@group(0) @binding(10) var<storage, read_write> locked: array<atomic<u32>>;
@group(0) @binding(11) var<storage, read_write> counters: Counters;
// Persistent МЕЖДУ ТИКАМИ (в отличие от ВСЕХ буферов выше, которые
// `clear_locked`/`clear_claims`/detect_pass пересоздают/перезаписывают
// каждый тик заново) — единственная причина, по которой `starvation_after`
// вообще портируем на GPU (см. `rule_table::GpuUnsupportedReason`'s
// doc-комментарий про то, почему старое обоснование отказа было неверным).
// Тот же размер/индексация, что и `matches`/`match_state`
// (`width*height*max_matches_per_cell`) — АЛЛОЦИРУЕТСЯ ОДИН РАЗ в
// `GpuEngine::init`, никогда не очищается между тиками, обновляется ТОЛЬКО
// `update_starvation_pass` (после того, как финальный ACCEPTED/REJECTED
// каждого матча уже известен — GPU-раундами ИЛИ CPU-fallback'ом, см. её
// doc-комментарий).
@group(0) @binding(12) var<storage, read_write> starvation_counters: array<atomic<u32>>;
// Persistent, та же индексация, что и `starvation_counters` выше — ЗАЩЁЛКА
// (см. `rule_table::GpuRule::has_feedback`'s doc-комментарий и CPU
// `Engine::feedback_counters`): растёт на КАЖДЫЙ тик, где матч
// детектируется (независимо от исхода арбитража, в отличие от
// `starvation_counters`), никогда не сбрасывается победой — только
// осиротевшей записью (`update_feedback_pass`) или ПЕРЕНОСОМ на новую
// позицию, когда матч физически двигается (см. `apply_pass`'s релокацию
// ниже) — маркер `feedback` двигается сдвигом каждый тик, а не
// переоценивает одну и ту же клетку, как `starvation_after`.
@group(0) @binding(13) var<storage, read_write> feedback_counters: array<atomic<u32>>;
// Persistent — `Rule::memory`'s FIFO-буфер (см. `rule_table::GpuRule::has_memory`'s
// doc-комментарий и CPU `Engine::memory_buffers`). Размер `n_matches *
// MAX_MEMORY_WINDOW` — за матчем `m` закреплён СРЕЗ `[m*MAX_MEMORY_WINDOW ..
// (m+1)*MAX_MEMORY_WINDOW)`, из которого реально используются только первые
// `rules[rule_idx].memory_window` слотов. Индексация всегда ОДНИМ плоским
// индексом (`m * MAX_MEMORY_WINDOW + i`) — top-level storage-массив, НЕ
// поле-массив внутри значения, загруженного динамическим индексом (см.
// `rule_table::GpuRule`'s doc-комментарий про ограничение naga), так что
// динамическое `i` здесь абсолютно безопасно.
@group(0) @binding(14) var<storage, read_write> memory_buffers: array<atomic<u32>>;
// Persistent, индексация как `starvation_counters`/`feedback_counters`
// (один слот на матч) — число РЕАЛЬНО заполненных элементов
// `memory_buffers[m]`'s среза (0..=`memory_window`) — отличает "буфер ещё
// не полон" (гейт закрыт по построению) от "полон, но значения не
// совпадают" (гейт закрыт по содержимому).
@group(0) @binding(15) var<storage, read_write> memory_len: array<atomic<u32>>;

fn idx(x: u32, y: u32) -> u32 {
    return y * params.width + x;
}

fn padded_width() -> u32 {
    return params.width + 2u * params.margin;
}

fn padded_height() -> u32 {
    return params.height + 2u * params.margin;
}

// Индекс в ДОПОЛНЕННОЙ координатной сетке (см. doc-комментарий у
// `params.margin`) — валиден для x/y в диапазоне `-margin..width+margin`
// (по построению `rule_table::GpuRuleTable::margin`, никогда не выходит
// дальше). Используется ТОЛЬКО для `claims`/`locked` (арбитраж) — `current`/
// `next` остаются в обычной, неполненной сетке (`idx`).
fn padded_idx(x: i32, y: i32) -> u32 {
    return u32(y + i32(params.margin)) * padded_width() + u32(x + i32(params.margin));
}

// Побайтовый лексикографический тай-брейк по id_b0..id_b7, как
// `arbitrator::RuleIdKey` (см. её doc-комментарий про принятое упрощение с
// паддингом нулями вместо честного учёта длины) — возвращает true, если
// `a` строго "больше" `b`.
fn id_greater(
    a0: u32, a1: u32, a2: u32, a3: u32, a4: u32, a5: u32, a6: u32, a7: u32,
    b0: u32, b1: u32, b2: u32, b3: u32, b4: u32, b5: u32, b6: u32, b7: u32,
) -> bool {
    if (a0 != b0) { return a0 > b0; }
    if (a1 != b1) { return a1 > b1; }
    if (a2 != b2) { return a2 > b2; }
    if (a3 != b3) { return a3 > b3; }
    if (a4 != b4) { return a4 > b4; }
    if (a5 != b5) { return a5 > b5; }
    if (a6 != b6) { return a6 > b6; }
    return a7 > b7;
}

// true, если union-офсеты головы `head_value` уходят за границу решётки
// у клетки (x, y) — тогда голова целиком не матчится здесь ни одним
// правилом (см. doc-комментарий модуля / `GpuHeadSlot::offsets_start`).
fn union_gate_blocks(slot: HeadSlot, x: u32, y: u32) -> bool {
    for (var u: u32 = 0u; u < slot.offsets_count; u = u + 1u) {
        let o = head_offsets[slot.offsets_start + u];
        let nx = i32(x) + o.dx;
        let ny = i32(y) + o.dy;
        if (nx < 0 || ny < 0 || u32(nx) >= params.width || u32(ny) >= params.height) {
            return true;
        }
    }
    return false;
}

// true, если паттерн правила `rule_idx` совпадает у клетки (x, y).
fn pattern_matches(rule_idx: u32, x: u32, y: u32) -> bool {
    let pattern_start = rules[rule_idx].pattern_start;
    let pattern_len = rules[rule_idx].pattern_len;
    for (var p: u32 = 0u; p < pattern_len; p = p + 1u) {
        let off = pattern_offsets[pattern_start + p];
        let nx = i32(x) + off.dx;
        let ny = i32(y) + off.dy;
        var neighbor_value = params.default_cell_type;
        if (nx >= 0 && ny >= 0 && u32(nx) < params.width && u32(ny) < params.height) {
            neighbor_value = current[idx(u32(nx), u32(ny))].value;
        }
        if (neighbor_value != off.expected) {
            return false;
        }
    }
    return true;
}

// ============================================================================
// Путь 1: self-write-only (needs_arbitration == false) — один проход,
// без арбитража. См. doc-комментарий модуля.
// ============================================================================

fn last_self_change_value(rule_idx: u32) -> u32 {
    let n = rules[rule_idx].change_count;
    if (n >= 4u) { return rules[rule_idx].change_val3; }
    if (n == 3u) { return rules[rule_idx].change_val2; }
    if (n == 2u) { return rules[rule_idx].change_val1; }
    return rules[rule_idx].change_val0;
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) {
        return;
    }
    let i = idx(gid.x, gid.y);
    let me = current[i];

    if (me.value >= 256u) {
        next[i] = me;
        return;
    }
    let slot = head_slots[me.value];
    if (slot.rules_count == 0u) {
        next[i] = me;
        return;
    }
    if (union_gate_blocks(slot, gid.x, gid.y)) {
        next[i] = me;
        return;
    }

    let age = params.generation - me.born_at;

    var best_found = false;
    var best_priority = 0u;
    var best_rule_idx = 0u;
    var best_new_value = me.value;
    var best_id0 = 0u; var best_id1 = 0u; var best_id2 = 0u; var best_id3 = 0u;
    var best_id4 = 0u; var best_id5 = 0u; var best_id6 = 0u; var best_id7 = 0u;

    for (var r: u32 = 0u; r < slot.rules_count; r = r + 1u) {
        let rule_idx = slot.rules_start + r;
        if (age < rules[rule_idx].min_age) { continue; }
        if (rules[rule_idx].active_only == 1u && me.value == params.default_cell_type && age == 0u) { continue; }
        if (!pattern_matches(rule_idx, gid.x, gid.y)) { continue; }

        var better = false;
        if (!best_found) {
            better = true;
        } else if (rules[rule_idx].priority != best_priority) {
            better = rules[rule_idx].priority > best_priority;
        } else {
            let idg = id_greater(
                rules[rule_idx].id_b0, rules[rule_idx].id_b1, rules[rule_idx].id_b2, rules[rule_idx].id_b3,
                rules[rule_idx].id_b4, rules[rule_idx].id_b5, rules[rule_idx].id_b6, rules[rule_idx].id_b7,
                best_id0, best_id1, best_id2, best_id3, best_id4, best_id5, best_id6, best_id7,
            );
            let ide = rules[rule_idx].id_b0 == best_id0 && rules[rule_idx].id_b1 == best_id1
                && rules[rule_idx].id_b2 == best_id2 && rules[rule_idx].id_b3 == best_id3
                && rules[rule_idx].id_b4 == best_id4 && rules[rule_idx].id_b5 == best_id5
                && rules[rule_idx].id_b6 == best_id6 && rules[rule_idx].id_b7 == best_id7;
            if (!ide) {
                better = idg;
            } else {
                better = rules[rule_idx].rule_idx > best_rule_idx;
            }
        }

        if (better) {
            best_found = true;
            best_priority = rules[rule_idx].priority;
            best_rule_idx = rules[rule_idx].rule_idx;
            best_new_value = last_self_change_value(rule_idx);
            best_id0 = rules[rule_idx].id_b0; best_id1 = rules[rule_idx].id_b1;
            best_id2 = rules[rule_idx].id_b2; best_id3 = rules[rule_idx].id_b3;
            best_id4 = rules[rule_idx].id_b4; best_id5 = rules[rule_idx].id_b5;
            best_id6 = rules[rule_idx].id_b6; best_id7 = rules[rule_idx].id_b7;
        }
    }

    if (best_found) {
        var out: Cell;
        out.value = best_new_value;
        out.born_at = params.generation + 1u;
        next[i] = out;
    } else {
        next[i] = me;
    }
}

// `main_tiled` — тот же self-write путь, что `main` выше, но с соседями
// клетки, подгруженными ОДИН РАЗ на всю workgroup в shared-память
// (`var<workgroup>`), а не читаемыми из глобальной памяти заново на каждое
// сравнение офсета паттерна каждым потоком независимо (соседние потоки
// workgroup'а читают СИЛЬНО пересекающиеся области `current[]` — например,
// у Game of Life с его 8-соседним паттерном). Halo радиуса 1 — это НЕ
// потолок в духе `MAX_SHIFT_REACH`, а жёсткая граница ЭТОГО конкретного
// кернела: `GpuEngine` выбирает `main_tiled` вместо `main` ТОЛЬКО когда
// `GpuRuleTable::pattern_reach <= 1` (проверено на этапе построения
// пайплайна, см. `build_simple_pipeline`) — при большем охвате паттерна
// используется обычный `main`, полностью общий, без tiling. Экономит
// повторные глобальные чтения, но не меняет, ЧТО вычисляется — семантика
// идентична `main`/`pattern_matches` побитово (проверено property-тестами
// и `examples/flagship_gol.rs`'s собственной сверкой с CPU-эталоном).
const TILE_DIM: u32 = 18u; // workgroup_size(16,16,1) + halo 1 с каждой стороны
var<workgroup> tile_values: array<u32, 324>; // TILE_DIM * TILE_DIM

// Кооперативная загрузка: 256 потоков workgroup'а заполняют 324 ячейки
// тайла (18×18) — большинство потоков грузят ровно одну ячейку, часть (те,
// чей линейный индекс < 324-256=68) грузят вторую. Подстановка
// `default_cell_type` для ячеек за пределами решётки — ТА ЖЕ логика
// границы, что и в `pattern_matches` (не клэмпинг, замена значением
// "как будто там default").
fn load_tile(local_x: u32, local_y: u32, base_x: i32, base_y: i32) {
    let tid = local_y * 16u + local_x;
    var i = tid;
    loop {
        if (i >= 324u) { break; }
        let ty = i / TILE_DIM;
        let tx = i % TILE_DIM;
        let gx = base_x + i32(tx) - 1;
        let gy = base_y + i32(ty) - 1;
        var v = params.default_cell_type;
        if (gx >= 0 && gy >= 0 && u32(gx) < params.width && u32(gy) < params.height) {
            v = current[idx(u32(gx), u32(gy))].value;
        }
        tile_values[i] = v;
        i = i + 256u;
    }
}

// Как `pattern_matches`, но читает соседей из `tile_values` вместо
// `current[]` — безопасно ТОЛЬКО при `pattern_reach <= 1` (см. её
// doc-комментарий выше): `local_x`/`local_y` ∈ [0,15], офсет ∈ [-1,1],
// значит индекс тайла всегда ∈ [0,17] — в границах 18×18 без доп. проверки.
fn pattern_matches_tiled(rule_idx: u32, local_x: u32, local_y: u32) -> bool {
    let pattern_start = rules[rule_idx].pattern_start;
    let pattern_len = rules[rule_idx].pattern_len;
    for (var p: u32 = 0u; p < pattern_len; p = p + 1u) {
        let off = pattern_offsets[pattern_start + p];
        let tx = u32(i32(local_x) + 1 + off.dx);
        let ty = u32(i32(local_y) + 1 + off.dy);
        let neighbor_value = tile_values[ty * TILE_DIM + tx];
        if (neighbor_value != off.expected) {
            return false;
        }
    }
    return true;
}

@compute @workgroup_size(16, 16, 1)
fn main_tiled(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    // Загрузка тайла + barrier — БЕЗУСЛОВНО, ДО любого early-return: WGSL
    // требует единообразного control flow для `workgroupBarrier()` (его
    // обязаны достичь ВСЕ потоки workgroup'а, включая те, чей `gid` уже вне
    // решётки на краю сетки) — именно поэтому проверка границ идёт ПОСЛЕ.
    let base_x = i32(gid.x) - i32(lid.x);
    let base_y = i32(gid.y) - i32(lid.y);
    load_tile(lid.x, lid.y, base_x, base_y);
    workgroupBarrier();

    if (gid.x >= params.width || gid.y >= params.height) {
        return;
    }
    let i = idx(gid.x, gid.y);
    let me = current[i];

    if (me.value >= 256u) {
        next[i] = me;
        return;
    }
    let slot = head_slots[me.value];
    if (slot.rules_count == 0u) {
        next[i] = me;
        return;
    }
    if (union_gate_blocks(slot, gid.x, gid.y)) {
        next[i] = me;
        return;
    }

    let age = params.generation - me.born_at;

    var best_found = false;
    var best_priority = 0u;
    var best_rule_idx = 0u;
    var best_new_value = me.value;
    var best_id0 = 0u; var best_id1 = 0u; var best_id2 = 0u; var best_id3 = 0u;
    var best_id4 = 0u; var best_id5 = 0u; var best_id6 = 0u; var best_id7 = 0u;

    for (var r: u32 = 0u; r < slot.rules_count; r = r + 1u) {
        let rule_idx = slot.rules_start + r;
        if (age < rules[rule_idx].min_age) { continue; }
        if (rules[rule_idx].active_only == 1u && me.value == params.default_cell_type && age == 0u) { continue; }
        if (!pattern_matches_tiled(rule_idx, lid.x, lid.y)) { continue; }

        var better = false;
        if (!best_found) {
            better = true;
        } else if (rules[rule_idx].priority != best_priority) {
            better = rules[rule_idx].priority > best_priority;
        } else {
            let idg = id_greater(
                rules[rule_idx].id_b0, rules[rule_idx].id_b1, rules[rule_idx].id_b2, rules[rule_idx].id_b3,
                rules[rule_idx].id_b4, rules[rule_idx].id_b5, rules[rule_idx].id_b6, rules[rule_idx].id_b7,
                best_id0, best_id1, best_id2, best_id3, best_id4, best_id5, best_id6, best_id7,
            );
            let ide = rules[rule_idx].id_b0 == best_id0 && rules[rule_idx].id_b1 == best_id1
                && rules[rule_idx].id_b2 == best_id2 && rules[rule_idx].id_b3 == best_id3
                && rules[rule_idx].id_b4 == best_id4 && rules[rule_idx].id_b5 == best_id5
                && rules[rule_idx].id_b6 == best_id6 && rules[rule_idx].id_b7 == best_id7;
            if (!ide) {
                better = idg;
            } else {
                better = rules[rule_idx].rule_idx > best_rule_idx;
            }
        }

        if (better) {
            best_found = true;
            best_priority = rules[rule_idx].priority;
            best_rule_idx = rules[rule_idx].rule_idx;
            best_new_value = last_self_change_value(rule_idx);
            best_id0 = rules[rule_idx].id_b0; best_id1 = rules[rule_idx].id_b1;
            best_id2 = rules[rule_idx].id_b2; best_id3 = rules[rule_idx].id_b3;
            best_id4 = rules[rule_idx].id_b4; best_id5 = rules[rule_idx].id_b5;
            best_id6 = rules[rule_idx].id_b6; best_id7 = rules[rule_idx].id_b7;
        }
    }

    if (best_found) {
        var out: Cell;
        out.value = best_new_value;
        out.born_at = params.generation + 1u;
        next[i] = out;
    } else {
        next[i] = me;
    }
}

// ============================================================================
// Путь 2: общий (needs_arbitration == true) — detect → claim/resolve×N → apply.
// ============================================================================

// Записать одну ячейку-цель в матч `m` по текущему слоту `n` и вернуть
// новый `n`. `value` — что реально запишется (для source-clear —
// `default_cell_type`, для цели сдвига — перенесённое значение головки,
// для change — его литерал).
//
// БЕЗ проверки границ ВИДИМОЙ решётки: регистрируется в ДОПОЛНЕННОЙ сетке
// (`padded_idx`) безусловно — CPU-арбитраж (`arbitrator::get_match_affected_cells`)
// при `OverflowAction::Discard` тоже не отбрасывает уходящие за край
// относительные ячейки для целей КОНФЛИКТА (см. doc-комментарий `params.margin`),
// хотя реально записывает только те, что физически попадают в решётку —
// это разделение уже сделано на уровне `apply_pass` (дальше в файле):
// он проходит только по РЕАЛЬНЫМ клеткам решётки, так что "фантомная"
// (за пределами `0..width`/`0..height`) запись просто никогда не
// материализуется, даже если выиграла арбитраж.
fn push_write_cell(m: u32, n: u32, x: i32, y: i32, value: u32) -> u32 {
    if (n >= MAX_WRITE_CELLS) {
        return n; // недостижимо при валидных MAX_SHIFTS/MAX_CHANGES из rule_table.rs
    }
    matches[m].cells[n] = padded_idx(x, y);
    matches[m].values[n] = value;
    return n + 1u;
}

// ============================================================================
// `Rule::recursion` — каскад НЕЗАВИСИМЫХ (не-cam) уровней ВНУТРИ ОДНОГО
// потока (см. `rule_table::MAX_RECURSION_DEPTH`'s doc-комментарий про то,
// почему это чисто локальное вычисление, безопасное на GPU, в отличие от
// `feedback`/`memory`/`starvation_after`). Каждый уровень `k=1..=max_depth`
// заново проверяет ТОТ ЖЕ pattern, сдвинутый на `k×direction`, читая уже
// НАКОПЛЕННЫЕ ЭТИМ ЖЕ матчем ячейки записи (`matches[m].cells[0..n]`,
// `matches[m].values[0..n]` — уже единственный источник истины для "что
// этот матч уже написал", ничего ДОПОЛНИТЕЛЬНОГО заводить не нужно) —
// зеркалит CPU `applicator::read_cell_effective`/`read_age_effective`
// 1-в-1, только "write_buffer" здесь — это уже посчитанный префикс
// `cells[0..n]` ТЕКУЩЕГО матча, а не общий per-тик буфер (которого у GPU
// нет и не может быть без межпоточной синхронизации — см. doc-комментарий
// модуля про Simple/Arbitrated пайплайны).
// ============================================================================

// Значение клетки (x,y), учитывая уже накопленные ЭТИМ матчем записи
// `cells[0..n]`/`values[0..n]` — зеркалит `read_cell_effective`.
fn read_cell_effective_local(m: u32, n: u32, x: i32, y: i32) -> u32 {
    if (x < 0 || y < 0) {
        return params.default_cell_type;
    }
    let key = padded_idx(x, y);
    for (var i: u32 = 0u; i < n; i = i + 1u) {
        if (matches[m].cells[i] == key) {
            return matches[m].values[i];
        }
    }
    if (u32(x) >= params.width || u32(y) >= params.height) {
        return params.default_cell_type;
    }
    return current[idx(u32(x), u32(y))].value;
}

// Эффективный возраст клетки (x,y) — зеркалит `read_age_effective`. Клетка,
// уже записанная ЭТИМ каскадом (найдена в `cells[0..n]`), имеет эффективный
// `born_at == params.generation` по построению (см. `apply_pass`'s `out.born_at
// = params.generation + 1u` — REAL born_at материализуется только там, но
// СЕМАНТИЧЕСКИ, для целей "сколько тиков клетка стабильна", "записана в этом
// же каскаде этого же тика" ⟺ возраст 0, ровно как у CPU `write_buffer`).
fn read_age_effective_local(m: u32, n: u32, x: i32, y: i32) -> u32 {
    if (x < 0 || y < 0) {
        return 0u;
    }
    let key = padded_idx(x, y);
    for (var i: u32 = 0u; i < n; i = i + 1u) {
        if (matches[m].cells[i] == key) {
            return 0u;
        }
    }
    if (u32(x) >= params.width || u32(y) >= params.height) {
        return 0u;
    }
    let cell = current[idx(u32(x), u32(y))];
    if (cell.value == params.default_cell_type) {
        return 0u;
    }
    return params.generation - cell.born_at;
}

// Проверить pattern правила в позиции (ox,oy), читая ЭФФЕКТИВНОЕ (с учётом
// уже накопленных ЭТИМ каскадом записей `cells[0..n]`) состояние, включая
// `min_age` гейт на самой (ox,oy) — зеркалит `pattern_matches_effective`
// 1-в-1 (та же роль (ox,oy) как единый "якорь" для min_age, что и (cx,cy)
// у обычного матчера, см. её doc-комментарий).
fn pattern_matches_effective_local(m: u32, n: u32, rule_idx: u32, ox: i32, oy: i32) -> bool {
    let min_age = rules[rule_idx].min_age;
    if (min_age > 0u && read_age_effective_local(m, n, ox, oy) < min_age) {
        return false;
    }
    let pattern_start = rules[rule_idx].pattern_start;
    let pattern_len = rules[rule_idx].pattern_len;
    for (var p: u32 = 0u; p < pattern_len; p = p + 1u) {
        let off = pattern_offsets[pattern_start + p];
        let nx = ox + off.dx;
        let ny = oy + off.dy;
        if (read_cell_effective_local(m, n, nx, ny) != off.expected) {
            return false;
        }
    }
    return true;
}

// Ближайшая клетка типа `target_ct` в Chebyshev-радиусе `radius` вокруг (cx,cy)
// — зеркалит CPU `matcher::search_nearest` 1-в-1, включая тай-брейк
// (минимальное расстояние, при равенстве — лексикографически меньшая
// (y, x)): без этого GPU и CPU расходились бы, когда несколько целей
// равноудалены. Читает `current[]` (состояние ДО тика, как `pattern_matches`)
// — НЕ тайловую версию: CAM всегда идёт через Arbitrated-пайплайн
// (`needs_arbitration` принудительно true для cam-правил, см.
// `rule_table.rs`), `main_tiled` сюда не относится. Возвращает true и
// пишет найденную позицию в `out_x`/`out_y`, если что-то нашлось.
fn cam_search(cx: u32, cy: u32, radius: u32, target_ct: u32, out_x: ptr<function, u32>, out_y: ptr<function, u32>) -> bool {
    var found = false;
    var best_dist: i32 = 0x7fffffff;
    var best_y: i32 = 0x7fffffff;
    var best_x: i32 = 0x7fffffff;
    let r = i32(radius);
    for (var dy: i32 = -r; dy <= r; dy = dy + 1) {
        let ny = i32(cy) + dy;
        if (ny < 0 || u32(ny) >= params.height) { continue; }
        for (var dx: i32 = -r; dx <= r; dx = dx + 1) {
            let nx = i32(cx) + dx;
            if (nx < 0 || u32(nx) >= params.width) { continue; }
            if (u32(nx) == cx && u32(ny) == cy) { continue; }
            if (current[idx(u32(nx), u32(ny))].value != target_ct) { continue; }
            let dist = max(abs(dx), abs(dy));
            let better = !found || dist < best_dist || (dist == best_dist && (ny < best_y || (ny == best_y && nx < best_x)));
            if (better) {
                found = true;
                best_dist = dist;
                best_y = ny;
                best_x = nx;
            }
        }
    }
    if (found) {
        *out_x = u32(best_x);
        *out_y = u32(best_y);
    }
    return found;
}

// 256 — измеренный на реальном железе оптимум среди {64, 128, 256, 512} для
// ВСЕХ 1D-проходов ниже (detect/clear_locked/clear_claims/clear_counter/
// claim/resolve/count_pending) на `examples/flagship_shifts.rs` (N=400:
// 64 → ~95М кл/с, 128 → ~97М, 256 → ~110-114М, 512 → ~95М) — не догадка,
// свип. Держать в паре с `GpuEngine::dispatch_tick`'s `wg_matches`/
// `wg_padded_cells` (`div_ceil(256)`) — НЕ трогать `wg_grid_x`/`wg_grid_y`
// заодно: те дispatch'ат `main`/`apply_pass`, у которых ФИКСИРОВАННЫЙ
// `@workgroup_size(16, 16, 1)` (см. ниже), не участвовавший в этом свипе.
@compute @workgroup_size(256, 1, 1)
fn detect_pass(@builtin(global_invocation_id) gid: vec3<u32>) {
    let m = gid.x;
    let total = params.width * params.height * params.max_matches_per_cell;
    if (m >= total) { return; }

    let cell_idx = m / params.max_matches_per_cell;
    let slot_in_cell = m % params.max_matches_per_cell;
    let x = cell_idx % params.width;
    let y = cell_idx / params.width;

    let me = current[cell_idx];
    matches[m].cell_count = 0u;
    matches[m].matched = 0u;
    // Инициализация здесь ОБЯЗАТЕЛЬНА (не только у `matched` выше) — любой
    // ранний `return` ДО строки `matches[m].structural = 1u;` ниже (тип не
    // тот, паттерн не совпал, min_age и т.д.) обязан оставить `structural`
    // равным 0 на ЭТОМ тике, иначе `update_memory_push_pass` читал бы
    // УСТАРЕВШЕЕ значение с предыдущего тика (буфер продолжал бы расти,
    // хотя структурного совпадения на самом деле уже нет).
    matches[m].structural = 0u;

    if (me.value >= 256u) {
        atomicStore(&match_state[m], 2u); // REJECTED
        return;
    }
    let slot = head_slots[me.value];
    if (slot_in_cell >= slot.rules_count) {
        atomicStore(&match_state[m], 2u);
        return;
    }
    if (union_gate_blocks(slot, x, y)) {
        atomicStore(&match_state[m], 2u);
        return;
    }

    let rule_idx = slot.rules_start + slot_in_cell;
    let age = params.generation - me.born_at;
    if (age < rules[rule_idx].min_age) {
        atomicStore(&match_state[m], 2u);
        return;
    }
    if (rules[rule_idx].active_only == 1u && me.value == params.default_cell_type && age == 0u) {
        atomicStore(&match_state[m], 2u);
        return;
    }
    if (!pattern_matches(rule_idx, x, y)) {
        atomicStore(&match_state[m], 2u);
        return;
    }

    // С этой точки правило СТРУКТУРНО совпало (тип/min_age/active_only/
    // pattern все прошли) — независимо от того, найдётся ли реально что
    // записать (cam без цели в радиусе, например). См. `GpuMatch::matched`'s
    // doc-комментарий про то, почему это НЕ то же самое, что `cell_count > 0`.
    matches[m].matched = 1u;
    matches[m].structural = 1u;

    // `Rule::memory` (см. `rule_table::GpuRule::has_memory`'s doc-комментарий
    // и CPU `engine/mod.rs`'s гейт-фильтр): ЧИСТО runtime-фильтр кандидатов
    // ДО арбитража — если гейт закрыт, этот матч трактуется РОВНО как если
    // бы паттерн не совпал вовсе (не участвует в arbитраже/starvation/
    // feedback), но `structural` ОСТАЁТСЯ 1u — буфер обязан продолжать
    // наблюдать (см. `update_memory_push_pass` ниже), даже пока гейт
    // закрыт, иначе искомая последовательность никогда бы не накопилась.
    // Гарантированно исключает CAM (`MemoryCamUnsupported` в
    // `rule_table.rs`), так что этот блок безопасно стоит ДО CAM-ветки —
    // ни один CAM-матч сюда никогда не попадёт.
    if (rules[rule_idx].has_memory == 1u) {
        let win = rules[rule_idx].memory_window;
        var gate_open = atomicLoad(&memory_len[m]) == win;
        if (gate_open && win >= 1u) { gate_open = atomicLoad(&memory_buffers[m * MAX_MEMORY_WINDOW + 0u]) == rules[rule_idx].memory_pattern0; }
        if (gate_open && win >= 2u) { gate_open = atomicLoad(&memory_buffers[m * MAX_MEMORY_WINDOW + 1u]) == rules[rule_idx].memory_pattern1; }
        if (gate_open && win >= 3u) { gate_open = atomicLoad(&memory_buffers[m * MAX_MEMORY_WINDOW + 2u]) == rules[rule_idx].memory_pattern2; }
        if (gate_open && win >= 4u) { gate_open = atomicLoad(&memory_buffers[m * MAX_MEMORY_WINDOW + 3u]) == rules[rule_idx].memory_pattern3; }
        if (!gate_open) {
            matches[m].matched = 0u;
            atomicStore(&match_state[m], 2u); // REJECTED — как если бы паттерн не совпал
            return;
        }
    }

    // `Rule::starvation_after` (см. `rule_table::GpuRule::has_starvation`'s
    // doc-комментарий): эффективный priority — из persistent-счётчика ДО
    // этого тика (обновляется `update_starvation_pass` ПОСЛЕ арбитража/
    // гибридного добора этого же тика, см. её doc-комментарий) — та же
    // семантика, что и CPU `arbitrator::resolve_sort_fields`.
    var effective_priority = rules[rule_idx].priority;
    if (rules[rule_idx].has_starvation == 1u && atomicLoad(&starvation_counters[m]) >= rules[rule_idx].starvation_threshold) {
        effective_priority = 0xFFFFFFFFu;
    }

    // CAM (`Rule::cam`, см. её doc-комментарий в `types.rs`) — отдельная
    // ветка, ДО обычной shift/changes-логики: у cam-правила `shift_count`/
    // `change_count` всегда 0 (см. валидацию в `config.rs`/`rule_table.rs`),
    // а запись — динамическая (найденная позиция, не фиксированный офсет).
    // Ровно 2 ячейки записи (найденная — очищается, сама клетка — получает
    // `cam_target_type`), зеркалит CPU `applicator::apply_cam_buffered`.
    // Не нашли ничего в радиусе — матч REJECTED (нечего писать), как
    // существующий случай "n==0" ниже для обычных правил.
    if (rules[rule_idx].cam_radius > 0u) {
        var fx: u32 = 0u;
        var fy: u32 = 0u;
        if (!cam_search(x, y, rules[rule_idx].cam_radius, rules[rule_idx].cam_target_type, &fx, &fy)) {
            atomicStore(&match_state[m], 2u); // REJECTED — ничего в радиусе
            return;
        }
        var n: u32 = 0u;
        n = push_write_cell(m, n, i32(fx), i32(fy), params.default_cell_type);
        n = push_write_cell(m, n, i32(x), i32(y), rules[rule_idx].cam_target_type);

        matches[m].cell_count = n;
        matches[m].priority = effective_priority;
        matches[m].age = age;
        matches[m].tie_break = (rules[rule_idx].tie_break + params.generation) % TIE_BREAK_MODULUS;
        matches[m].id0 = rules[rule_idx].id_b0; matches[m].id1 = rules[rule_idx].id_b1;
        matches[m].id2 = rules[rule_idx].id_b2; matches[m].id3 = rules[rule_idx].id_b3;
        matches[m].id4 = rules[rule_idx].id_b4; matches[m].id5 = rules[rule_idx].id_b5;
        matches[m].id6 = rules[rule_idx].id_b6; matches[m].id7 = rules[rule_idx].id_b7;
        matches[m].x = x;
        matches[m].y = y;
        matches[m].rule_idx = rules[rule_idx].rule_idx;
        atomicStore(&match_state[m], 0u); // PENDING
        return;
    }

    // Совпало — считаем ячейки записи (зеркалит `applicator::apply_rule_buffered`
    // и `arbitrator::get_match_affected_cells` ВМЕСТЕ: без сдвигов — changes
    // относительно (0,0); со сдвигами — source-clear ВСЕГДА + для КАЖДОЙ
    // цели сдвига, БЕЗУСЛОВНО (не только если она в границах видимой
    // решётки — см. doc-комментарий `params.margin`/`push_write_cell`: CPU-арбитраж
    // учитывает эти ячейки для конфликтов, даже когда `apply_shift_buffered`
    // реально ничего туда не пишет из-за overflow), сама цель (несёт
    // head_value) и changes относительно неё, тоже безусловно).
    var n: u32 = 0u;
    let shift_count = rules[rule_idx].shift_count;
    let change_count = rules[rule_idx].change_count;

    if (shift_count == 0u) {
        if (change_count >= 1u) { n = push_write_cell(m, n, i32(x) + rules[rule_idx].change_dx0, i32(y) + rules[rule_idx].change_dy0, rules[rule_idx].change_val0); }
        if (change_count >= 2u) { n = push_write_cell(m, n, i32(x) + rules[rule_idx].change_dx1, i32(y) + rules[rule_idx].change_dy1, rules[rule_idx].change_val1); }
        if (change_count >= 3u) { n = push_write_cell(m, n, i32(x) + rules[rule_idx].change_dx2, i32(y) + rules[rule_idx].change_dy2, rules[rule_idx].change_val2); }
        if (change_count >= 4u) { n = push_write_cell(m, n, i32(x) + rules[rule_idx].change_dx3, i32(y) + rules[rule_idx].change_dy3, rules[rule_idx].change_val3); }

        // `Rule::recursion` (взаимоисключим со сдвигами по валидации
        // `config.rs`, значит `shift_count == 0u` здесь гарантирован для
        // ЛЮБОГО recursion-правила) — см. блок функций выше и
        // `applicator.rs`'s "Фаза 3" doc-комментарий, который этот цикл
        // зеркалит 1-в-1: каждый уровень k=1..=max_depth заново проверяет
        // pattern на (x,y)+k×direction, эффективно (с учётом уже
        // накопленных этим же каскадом `cells[0..n]`), и останавливается на
        // первом несовпадении.
        let rmax = rules[rule_idx].recursion_max_depth;
        if (rmax > 0u) {
            let rdx = rules[rule_idx].recursion_dx;
            let rdy = rules[rule_idx].recursion_dy;
            for (var k: u32 = 1u; k <= rmax; k = k + 1u) {
                let ox = i32(x) + rdx * i32(k);
                let oy = i32(y) + rdy * i32(k);
                if (!pattern_matches_effective_local(m, n, rule_idx, ox, oy)) {
                    break;
                }
                if (change_count >= 1u) { n = push_write_cell(m, n, ox + rules[rule_idx].change_dx0, oy + rules[rule_idx].change_dy0, rules[rule_idx].change_val0); }
                if (change_count >= 2u) { n = push_write_cell(m, n, ox + rules[rule_idx].change_dx1, oy + rules[rule_idx].change_dy1, rules[rule_idx].change_val1); }
                if (change_count >= 3u) { n = push_write_cell(m, n, ox + rules[rule_idx].change_dx2, oy + rules[rule_idx].change_dy2, rules[rule_idx].change_val2); }
                if (change_count >= 4u) { n = push_write_cell(m, n, ox + rules[rule_idx].change_dx3, oy + rules[rule_idx].change_dy3, rules[rule_idx].change_val3); }
            }
        }
    } else {
        // ДВЕ отдельные фазы, СНАЧАЛА все сдвиги целиком, ПОТОМ все changes
        // целиком (не по одному сдвигу за раз с его "собственными" changes
        // сразу следом) — зеркалит `apply_rule_buffered`'s "Фаза 1: сдвиги...
        // Фаза 2: изменения — ПЕРЕЗАПИСЫВАЮТ сдвиги при конфликте": ЛЮБОЙ
        // change побеждает ЛЮБОЙ сдвиг при совпадении ячейки, независимо от
        // того, какой конкретно сдвиг/change его произвёл — при интерливинге
        // "сдвиг1+его changes, затем сдвиг2+его changes" (как было раньше)
        // цель сдвига2 могла оказаться ПОСЛЕ change'а сдвига1 на той же
        // ячейке и неверно победить его (найдено `tests/gpu_v2_correctness.rs`'s
        // property-тестом на правиле с 2 сдвигами, чьи changes пересекались).
        // `Rule::feedback` (см. её doc-комментарий у `feedback_counters`
        // binding'а выше и CPU `applicator::apply_rule_buffered`'s
        // "Фаза 1"): если persistent-счётчик ДЛЯ ЭТОГО матча уже достиг
        // `feedback_timeout`, реальный применённый сдвиг — `feedback_alt_dx/dy`
        // (`new_direction`), а не декларированный `shift_dx0/dy0`.
        // Гарантированно применимо только к ЭТОМУ (единственному, см.
        // `TooManyShifts`-защиту в `rule_table.rs`) сдвигу — `shift_dx1/dy1`
        // (второй сдвиг) сюда не относится, `feedback` его в принципе не
        // может иметь.
        //
        // ВАЖНО (найдено адверсариальным тестом, не сразу очевидно):
        // сравниваем с `counter + 1`, НЕ с сырым `atomicLoad`. На CPU
        // (`applicator.rs:342-343`) защёлка инкрементируется в
        // `run_tick_with_cache` ДО вызова apply (после арбитража, но перед
        // применением сдвигов), так что `feedback_override` там читает УЖЕ
        // учитывающее ТЕКУЩИЙ тик значение — детекция этого тика тоже
        // засчитывается в проверку timeout. Здесь же `feedback_counters[m]`
        // хранит значение с КОНЦА предыдущего тика (сама защёлка растёт
        // позже, в `update_feedback_latch_pass`, уже ПОСЛЕ того, как этот
        // сдвиг посчитан) — без `+1` решение отставало бы от CPU ровно на
        // один тик (счётчик всегда "не досчитывает" текущую детекцию).
        // Насыщение (не переполнение через край u32) — та же защита, что и
        // у `saturating_add` на CPU.
        var effective_shift_dx0 = rules[rule_idx].shift_dx0;
        var effective_shift_dy0 = rules[rule_idx].shift_dy0;
        if (rules[rule_idx].has_feedback == 1u) {
            let fc = atomicLoad(&feedback_counters[m]);
            let fc_this_tick = select(fc + 1u, 0xFFFFFFFFu, fc == 0xFFFFFFFFu);
            if (fc_this_tick >= rules[rule_idx].feedback_timeout) {
                effective_shift_dx0 = rules[rule_idx].feedback_alt_dx;
                effective_shift_dy0 = rules[rule_idx].feedback_alt_dy;
            }
        }
        let sx0 = i32(x) + effective_shift_dx0;
        let sy0 = i32(y) + effective_shift_dy0;
        let sx1 = i32(x) + rules[rule_idx].shift_dx1;
        let sy1 = i32(y) + rules[rule_idx].shift_dy1;

        n = push_write_cell(m, n, i32(x), i32(y), params.default_cell_type);

        // Путь сдвига: обычный сдвиг пишет РОВНО конечную точку (телепорт);
        // broadcast (`ShiftSpec::broadcast`, см. её doc-комментарий в
        // `types.rs` и `applicator::apply_shift_buffered`'s `for k in
        // 1..=steps`) пишет head_cell в КАЖДУЮ клетку пути от source+1 до
        // конечной точки включительно — путь монотонен (фиксированное
        // направление = знак dx/dy, фиксированный шаг), поэтому `steps`/unit
        // вектор восстанавливаются прямо из дельты сдвига, без отдельного
        // поля. `changes` ниже по-прежнему применяются ТОЛЬКО относительно
        // конечной точки (sx0,sy0)/(sx1,sy1) — зеркалит CPU, где
        // `apply_changes_at` вызывается по `shift_targets` (финальным целям),
        // не по промежуточным клеткам пути (см. `applicator::apply_rule_buffered`).
        if (shift_count >= 1u) {
            if (rules[rule_idx].shift_broadcast0 == 1u) {
                let steps0 = max(abs(rules[rule_idx].shift_dx0), abs(rules[rule_idx].shift_dy0));
                var ux0: i32 = 0;
                if (rules[rule_idx].shift_dx0 > 0) { ux0 = 1; } else if (rules[rule_idx].shift_dx0 < 0) { ux0 = -1; }
                var uy0: i32 = 0;
                if (rules[rule_idx].shift_dy0 > 0) { uy0 = 1; } else if (rules[rule_idx].shift_dy0 < 0) { uy0 = -1; }
                for (var k: i32 = 1; k <= steps0; k = k + 1) {
                    n = push_write_cell(m, n, i32(x) + ux0 * k, i32(y) + uy0 * k, me.value);
                }
            } else {
                n = push_write_cell(m, n, sx0, sy0, me.value);
            }
        }
        if (shift_count >= 2u) {
            if (rules[rule_idx].shift_broadcast1 == 1u) {
                let steps1 = max(abs(rules[rule_idx].shift_dx1), abs(rules[rule_idx].shift_dy1));
                var ux1: i32 = 0;
                if (rules[rule_idx].shift_dx1 > 0) { ux1 = 1; } else if (rules[rule_idx].shift_dx1 < 0) { ux1 = -1; }
                var uy1: i32 = 0;
                if (rules[rule_idx].shift_dy1 > 0) { uy1 = 1; } else if (rules[rule_idx].shift_dy1 < 0) { uy1 = -1; }
                for (var k: i32 = 1; k <= steps1; k = k + 1) {
                    n = push_write_cell(m, n, i32(x) + ux1 * k, i32(y) + uy1 * k, me.value);
                }
            } else {
                n = push_write_cell(m, n, sx1, sy1, me.value);
            }
        }

        if (shift_count >= 1u) {
            if (change_count >= 1u) { n = push_write_cell(m, n, sx0 + rules[rule_idx].change_dx0, sy0 + rules[rule_idx].change_dy0, rules[rule_idx].change_val0); }
            if (change_count >= 2u) { n = push_write_cell(m, n, sx0 + rules[rule_idx].change_dx1, sy0 + rules[rule_idx].change_dy1, rules[rule_idx].change_val1); }
            if (change_count >= 3u) { n = push_write_cell(m, n, sx0 + rules[rule_idx].change_dx2, sy0 + rules[rule_idx].change_dy2, rules[rule_idx].change_val2); }
            if (change_count >= 4u) { n = push_write_cell(m, n, sx0 + rules[rule_idx].change_dx3, sy0 + rules[rule_idx].change_dy3, rules[rule_idx].change_val3); }
        }
        if (shift_count >= 2u) {
            if (change_count >= 1u) { n = push_write_cell(m, n, sx1 + rules[rule_idx].change_dx0, sy1 + rules[rule_idx].change_dy0, rules[rule_idx].change_val0); }
            if (change_count >= 2u) { n = push_write_cell(m, n, sx1 + rules[rule_idx].change_dx1, sy1 + rules[rule_idx].change_dy1, rules[rule_idx].change_val1); }
            if (change_count >= 3u) { n = push_write_cell(m, n, sx1 + rules[rule_idx].change_dx2, sy1 + rules[rule_idx].change_dy2, rules[rule_idx].change_val2); }
            if (change_count >= 4u) { n = push_write_cell(m, n, sx1 + rules[rule_idx].change_dx3, sy1 + rules[rule_idx].change_dy3, rules[rule_idx].change_val3); }
        }
    }

    matches[m].cell_count = n;
    matches[m].priority = effective_priority;
    matches[m].age = age;
    matches[m].tie_break = (rules[rule_idx].tie_break + params.generation) % TIE_BREAK_MODULUS;
    matches[m].id0 = rules[rule_idx].id_b0; matches[m].id1 = rules[rule_idx].id_b1;
    matches[m].id2 = rules[rule_idx].id_b2; matches[m].id3 = rules[rule_idx].id_b3;
    matches[m].id4 = rules[rule_idx].id_b4; matches[m].id5 = rules[rule_idx].id_b5;
    matches[m].id6 = rules[rule_idx].id_b6; matches[m].id7 = rules[rule_idx].id_b7;
    matches[m].x = x;
    matches[m].y = y;
    matches[m].rule_idx = rules[rule_idx].rule_idx;

    if (n == 0u) {
        // Ничего реально не пишет (все цели вне границ) — CPU-арбитраж
        // всегда принимает такой матч (пустой affected-список никогда не
        // конфликтует), но раз писать всё равно нечего, результат
        // неотличим от REJECTED — не участвует в claim/resolve вовсе.
        atomicStore(&match_state[m], 2u);
    } else {
        atomicStore(&match_state[m], 0u); // PENDING
    }
}

// Сброс `locked` в начале КАЖДОГО тика (в отличие от `claims`, которые
// `clear_claims` уже бережно сбрасывает КАЖДЫЙ раунд, `locked` живёт ВЕСЬ
// тик — раунды внутри тика опираются на то, что она монотонно растёт — и
// поэтому должна явно обнуляться между тиками отдельным проходом).
@compute @workgroup_size(256, 1, 1)
fn clear_locked(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= padded_width() * padded_height()) { return; }
    atomicStore(&locked[i], 0u);
}

const FREE: u32 = 0xFFFFFFFFu;

// Тай-брейк между PENDING-матчами `a` и `b` — та же логика, что и в
// self-write пути выше, но между двумя ИНДЕКСАМИ матчей, оба читаются
// напрямую из storage-буфера (см. doc-комментарий модуля про запрет
// локальных копий структур с массивами).
fn match_is_better(a: u32, b: u32) -> bool {
    if (matches[a].priority != matches[b].priority) { return matches[a].priority > matches[b].priority; }
    if (matches[a].age != matches[b].age) { return matches[a].age > matches[b].age; }
    if (matches[a].tie_break != matches[b].tie_break) { return matches[a].tie_break > matches[b].tie_break; }
    if (matches[a].id0 != matches[b].id0) { return matches[a].id0 > matches[b].id0; }
    if (matches[a].id1 != matches[b].id1) { return matches[a].id1 > matches[b].id1; }
    if (matches[a].id2 != matches[b].id2) { return matches[a].id2 > matches[b].id2; }
    if (matches[a].id3 != matches[b].id3) { return matches[a].id3 > matches[b].id3; }
    if (matches[a].id4 != matches[b].id4) { return matches[a].id4 > matches[b].id4; }
    if (matches[a].id5 != matches[b].id5) { return matches[a].id5 > matches[b].id5; }
    if (matches[a].id6 != matches[b].id6) { return matches[a].id6 > matches[b].id6; }
    if (matches[a].id7 != matches[b].id7) { return matches[a].id7 > matches[b].id7; }
    if (matches[a].x != matches[b].x) { return matches[a].x > matches[b].x; }
    if (matches[a].y != matches[b].y) { return matches[a].y > matches[b].y; }
    return matches[a].rule_idx > matches[b].rule_idx;
}

@compute @workgroup_size(256, 1, 1)
fn clear_counter(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x != 0u) { return; }
    atomicStore(&counters.changed, 0u);
}

@compute @workgroup_size(256, 1, 1)
fn clear_claims(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= padded_width() * padded_height()) { return; }
    // НЕ трогаем claims уже НАВСЕГДА занятых ячеек (locked==1) — иначе к
    // концу сходимости там оказался бы FREE вместо индекса реального
    // победителя (см. scratch-прототип `v2_arbitrate_check.rs`, где это
    // было найдено и исправлено).
    if (atomicLoad(&locked[i]) == 0u) {
        atomicStore(&claims[i], FREE);
    }
}

@compute @workgroup_size(256, 1, 1)
fn claim_pass(@builtin(global_invocation_id) gid: vec3<u32>) {
    let m = gid.x;
    let total = params.width * params.height * params.max_matches_per_cell;
    if (m >= total) { return; }
    if (atomicLoad(&match_state[m]) != 0u) { return; } // не PENDING

    let cell_count = matches[m].cell_count;

    for (var k: u32 = 0u; k < cell_count; k = k + 1u) {
        if (atomicLoad(&locked[matches[m].cells[k]]) == 1u) {
            atomicStore(&match_state[m], 2u); // REJECTED — навсегда занято другим
            return;
        }
    }

    for (var k: u32 = 0u; k < cell_count; k = k + 1u) {
        let c = matches[m].cells[k];
        loop {
            let old = atomicLoad(&claims[c]);
            if (old != FREE && !match_is_better(m, old)) {
                break;
            }
            let res = atomicCompareExchangeWeak(&claims[c], old, m);
            if (res.exchanged) {
                break;
            }
        }
    }
}

@compute @workgroup_size(256, 1, 1)
fn resolve_pass(@builtin(global_invocation_id) gid: vec3<u32>) {
    let m = gid.x;
    let total = params.width * params.height * params.max_matches_per_cell;
    if (m >= total) { return; }
    if (atomicLoad(&match_state[m]) != 0u) { return; } // не PENDING

    let cell_count = matches[m].cell_count;
    var won_all = true;
    for (var k: u32 = 0u; k < cell_count; k = k + 1u) {
        if (atomicLoad(&claims[matches[m].cells[k]]) != m) {
            won_all = false;
            break;
        }
    }
    if (won_all) {
        atomicStore(&match_state[m], 1u); // ACCEPTED
        for (var k: u32 = 0u; k < cell_count; k = k + 1u) {
            atomicStore(&locked[matches[m].cells[k]], 1u);
        }
    }
}

// После ROUNDS раундов claim/resolve считает, сколько матчей ОСТАЛОСЬ
// PENDING (не сошлись за отведённый бюджет — см. doc-комментарий модуля и
// `GpuEngine::dispatch_tick` про гибридный GPU+CPU арбитраж: длинные
// цепочки конфликтов требуют O(длина цепочки) раундов, что не укладывается
// в ЛЮБОЙ фиксированный бюджет — GPU досчитывает то, что успевает, CPU
// гарантированно дорешает остаток). Единственный писатель `counters.changed`
// на пути арбитража (claim_pass/resolve_pass её больше не трогают — ранняя
// версия, детектировавшая сходимость по ходу раундов, отменена как
// измеримо более медленная, см. doc-комментарий модуля) — вызывающая
// сторона обязана сделать `clear_counter` НЕПОСРЕДСТВЕННО перед этим проходом.
@compute @workgroup_size(256, 1, 1)
fn count_pending(@builtin(global_invocation_id) gid: vec3<u32>) {
    let m = gid.x;
    let total = params.width * params.height * params.max_matches_per_cell;
    if (m >= total) { return; }
    if (atomicLoad(&match_state[m]) == 0u) { // PENDING
        atomicAdd(&counters.changed, 1u);
    }
}

@compute @workgroup_size(16, 16, 1)
fn apply_pass(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) {
        return;
    }
    let i = idx(gid.x, gid.y);
    // `next`/`current` живут в обычной (неполненной) сетке — `i`; арбитраж
    // (`claims`/`locked`) живёт в ДОПОЛНЕННОЙ (см. `params.margin`/`padded_idx`).
    // `apply_pass` дispatch'ится ТОЛЬКО по видимым клеткам решётки
    // (`gid.x < width && gid.y < height`), так что "фантомные" (за
    // пределами видимой части) ячейки записи, даже выигравшие арбитраж,
    // сюда никогда не попадают — ровно то же самое, что CPU: реальная
    // запись происходит только внутри границ решётки, а фантомные ячейки
    // участвовали только в конфликтах.
    let pi = padded_idx(i32(gid.x), i32(gid.y));

    if (atomicLoad(&locked[pi]) == 0u) {
        next[i] = current[i];
        return;
    }

    let winner = atomicLoad(&claims[pi]);
    let cell_count = matches[winner].cell_count;
    var value = current[i].value;
    // БЕЗ break: если несколько записей матча целятся в одну и ту же
    // ячейку (например, 2+ `changes` на одном смещении, или change,
    // совпавший со своей же shift-целью), побеждает ПОСЛЕДНЯЯ по порядку
    // построения `cells[]` (см. `detect_pass`) — та же семантика, что
    // `write_buffer.insert` на CPU (`applicator::apply_changes_at`:
    // "changes побеждают сдвиги", и между собой changes применяются по
    // порядку списка, позже — выигрывает).
    for (var k: u32 = 0u; k < cell_count; k = k + 1u) {
        if (matches[winner].cells[k] == pi) {
            value = matches[winner].values[k];
        }
    }
    var out: Cell;
    out.value = value;
    out.born_at = params.generation + 1u;
    next[i] = out;
}

// Обновить `starvation_counters` — ЕДИНСТВЕННЫЙ потребитель финального
// `match_state` (ACCEPTED/REJECTED), поэтому дispatch'ится (см.
// `GpuEngine::dispatch_tick`) ПОСЛЕ `apply_pass` И ПОСЛЕ (если понадобился)
// `GpuEngine::cpu_fallback_resolve`, который теперь дописывает финальный
// исход НЕсошедшихся за GPU-раунды матчей обратно в `match_state_buf` —
// без этого матчи, доигранные на CPU, навсегда остались бы PENDING(0) с
// точки зрения этого прохода. Зеркалит CPU-side обновление
// `Engine::starvation_counters` в `run_tick_with_cache` (win → сброс,
// loss → рост, "осиротела" → сброс — см. её doc-комментарий про
// найденный и исправленный баг с незачищенными записями) 1-в-1, только
// каждый (cell, rule-слот) — свой собственный, персистентный элемент
// GPU-буфера вместо HashMap-записи.
//
// ВАЖНО (найдено при повторном аудите после добавления `memory`, не сразу
// очевидно): ЗДЕСЬ ОБЯЗАНА проверяться `structural`, а НЕ `matched`, для
// решения "осиротела ли запись". Для `Rule::memory`-having правил `matched`
// означает "гейт открыт" — а гейт может быть ЗАКРЫТ на этом тике, при этом
// правило ВСЁ ЕЩЁ структурно совпадает (просто временно исключено из
// арбитража). CPU (`engine/mod.rs`) считает `starving_keys` ПОСЛЕ
// гейт-фильтра — гейт-закрытый матч просто НЕ ПОПАДАЕТ в этот список, а
// НЕ появляется там с намерением сбросить счётчик: `starvation_counters`
// для этого ключа этим тиком вообще НЕ ТРОГАЕТСЯ (замораживается на своём
// текущем значении), а НЕ обнуляется. `structural` — единственный флаг,
// который для НЕ-memory правил ВСЕГДА равен `matched` (см.
// `GpuMatch::structural`'s doc-комментарий), так что переход на него
// НИКАК не меняет поведение для `starvation_after` без `memory` — только
// корректно чинит комбинацию `starvation_after` + `memory`, где раньше
// закрытие гейта ошибочно обнуляло накопленный счётчик, не давая правилу
// когда-либо выиграть через голодание (см.
// `test_gpu_v2_starvation_plus_memory_gate_freezes_counter_not_resets_matches_cpu`
// в `tests/gpu_v2_correctness.rs`).
@compute @workgroup_size(256, 1, 1)
fn update_starvation_pass(@builtin(global_invocation_id) gid: vec3<u32>) {
    let m = gid.x;
    let total = params.width * params.height * params.max_matches_per_cell;
    if (m >= total) { return; }

    let cell_idx = m / params.max_matches_per_cell;
    let slot_in_cell = m % params.max_matches_per_cell;
    let me = current[cell_idx];
    if (me.value >= 256u) { return; }
    let slot = head_slots[me.value];
    if (slot_in_cell >= slot.rules_count) { return; }
    let rule_idx = slot.rules_start + slot_in_cell;
    if (rules[rule_idx].has_starvation == 0u) { return; }

    if (matches[m].structural == 0u) {
        // Осиротевшая запись (см. doc-комментарий binding'а выше): матч не
        // был кандидатом вовсе на этом тике (структурно не совпал) — сброс,
        // не рост, та же причина, что и у CPU-side фикса.
        atomicStore(&starvation_counters[m], 0u);
        return;
    }
    if (matches[m].matched == 0u) {
        // Структурно совпало, но `Rule::memory`'s гейт закрыт этим тиком —
        // ЭТО НЕ то же самое, что осиротела: правило просто исключено из
        // арбитража на ЭТОТ тик, но остаётся тем же матчем — CPU НЕ трогает
        // счётчик в этом случае (см. doc-комментарий выше), значит и здесь
        // счётчик должен остаться КАК ЕСТЬ (ни сброс, ни рост) — `return`
        // ОБЯЗАТЕЛЕН, иначе управление проваливается в ветку ниже
        // (`match_state[m]` в этом случае всегда REJECTED, форсированное
        // гейтом, что без `return` ошибочно засчиталось бы как "проиграл"
        // и НЕВЕРНО нарастило бы счётчик).
        return;
    }
    if (atomicLoad(&match_state[m]) == 1u) { // ACCEPTED — выиграл
        atomicStore(&starvation_counters[m], 0u);
    } else {
        // REJECTED (проиграл арбитраж, но БЫЛ кандидатом) — растим,
        // с насыщением (та же `saturating_add`, что и на CPU).
        let cur = atomicLoad(&starvation_counters[m]);
        if (cur < 0xFFFFFFFFu) {
            atomicStore(&starvation_counters[m], cur + 1u);
        }
    }
}

// `Rule::feedback` — ДВА раздельных прохода (не один, как у
// `update_starvation_pass`), дispatch'ятся СТРОГО ПОСЛЕДОВАТЕЛЬНО (см.
// `GpuEngine::dispatch_tick`): между ДВУМЯ `dispatch_workgroups` внутри
// ОДНОГО compute pass'а WebGPU гарантирует видимость записей предыдущего
// (та же гарантия, на которой держится весь раундовый claim/resolve —
// см. doc-комментарий модуля `engine.rs`). Раздельность ОБЯЗАТЕЛЬНА, не
// стилистический выбор: перенос счётчика (Фаза 2) пишет в СЛОТ ДРУГОЙ
// клетки (`new_m`, новая позиция маркера), которая параллельно СВОИМ
// СОБСТВЕННЫМ потоком (Фаза 1, обрабатывающая `new_m` как "клетку САМУ ПО
// СЕБЕ", по pre-tick состоянию) почти наверняка пишет туда же (осиротевший
// сброс на 0, поскольку pre-tick эта позиция ещё не была feedback-матчем)
// — если бы обе фазы были ОДНИМ проходом, это была бы гонка (какой поток
// пишет последним — не определено), тихо стирающая перенесённый счётчик.
// Порядок Фаза1→Фаза2 гарантирует, что перенос (Фаза 2, работает с уже
// АКТУАЛЬНЫМ после Фазы 1 значением слота-источника) всегда происходит
// СТРОГО ПОСЛЕ осиротевшего сброса slot'а-назначения (Фаза 1), так что
// финальная запись в `new_m` — всегда от Фазы 2, детерминированно.

// Фаза 1: обычное обновление защёлки — растёт при обнаружении, сбрасывается
// только при осиротении. НИКАКИХ чужих слотов не трогает.
//
// ВАЖНО (тот же класс бага, что уже найден и исправлен в
// `update_starvation_pass` — см. её doc-комментарий выше для полного
// объяснения): проверять нужно `structural`, а НЕ `matched`. Для
// `Rule::feedback`+`Rule::memory`-having правил `matched==0u` может
// означать ПРОСТО "гейт памяти закрыт этим тиком" (правило по-прежнему
// структурно совпадает) — CPU в этом случае НЕ трогает `feedback_counters`
// вовсе (его инкремент-only цикл в `engine/mod.rs` строится по
// `feedback_keys`, которые ТОЖЕ гейт-фильтрованы, см. её doc-комментарий:
// "Считаются ПОСЛЕ гейт-фильтра памяти"), а НЕ сбрасывает защёлку — сброс
// оправдан ТОЛЬКО когда правило по-настоящему перестало структурно
// совпадать (`structural == 0u`). См.
// `test_gpu_v2_feedback_plus_memory_gate_freezes_latch_not_resets_matches_cpu`
// в `tests/gpu_v2_correctness.rs`.
@compute @workgroup_size(256, 1, 1)
fn update_feedback_latch_pass(@builtin(global_invocation_id) gid: vec3<u32>) {
    let m = gid.x;
    let total = params.width * params.height * params.max_matches_per_cell;
    if (m >= total) { return; }

    let cell_idx = m / params.max_matches_per_cell;
    let slot_in_cell = m % params.max_matches_per_cell;
    let me = current[cell_idx];
    if (me.value >= 256u) { return; }
    let slot = head_slots[me.value];
    if (slot_in_cell >= slot.rules_count) { return; }
    let rule_idx = slot.rules_start + slot_in_cell;
    if (rules[rule_idx].has_feedback == 0u) { return; }

    if (matches[m].structural == 0u) {
        atomicStore(&feedback_counters[m], 0u);
        return;
    }
    if (matches[m].matched == 0u) {
        // Структурно совпало, но гейт памяти закрыт этим тиком — защёлку
        // НЕ трогаем (ни сброс, ни рост), см. doc-комментарий выше.
        return;
    }
    let cur = atomicLoad(&feedback_counters[m]);
    if (cur < 0xFFFFFFFFu) {
        atomicStore(&feedback_counters[m], cur + 1u);
    }
}

// Фаза 2: перенос — ТОЛЬКО для матчей, выигравших арбитраж этого тика
// (значит, реально применивших свой сдвиг). `matches[m].cells[0]` —
// source-clear (старая позиция, == сама m), `cells[1]` — цель сдвига
// (новая позиция) — гарантированно эта раскладка: `feedback` всегда имеет
// РОВНО один, не-broadcast сдвиг (см. `rule_table.rs`'s защитную
// проверку), значит `push_write_cell`'s порядок в `detect_pass` всегда
// [source-clear, единственная цель, ...changes].
@compute @workgroup_size(256, 1, 1)
fn update_feedback_relocate_pass(@builtin(global_invocation_id) gid: vec3<u32>) {
    let m = gid.x;
    let total = params.width * params.height * params.max_matches_per_cell;
    if (m >= total) { return; }

    let cell_idx = m / params.max_matches_per_cell;
    let slot_in_cell = m % params.max_matches_per_cell;
    let me = current[cell_idx];
    if (me.value >= 256u) { return; }
    let slot = head_slots[me.value];
    if (slot_in_cell >= slot.rules_count) { return; }
    let rule_idx = slot.rules_start + slot_in_cell;
    if (rules[rule_idx].has_feedback == 0u) { return; }
    if (matches[m].matched == 0u || atomicLoad(&match_state[m]) != 1u) { return; }

    let new_key = matches[m].cells[1];
    let py = i32(new_key / padded_width());
    let px = i32(new_key % padded_width());
    let nx = px - i32(params.margin);
    let ny = py - i32(params.margin);
    if (nx < 0 || ny < 0 || u32(nx) >= params.width || u32(ny) >= params.height) {
        return; // цель за пределами видимой решётки (overflow Discard) — переносить некуда
    }
    let new_cell_idx = u32(ny) * params.width + u32(nx);
    let new_m = new_cell_idx * params.max_matches_per_cell + slot_in_cell;
    if (new_m == m) {
        return; // сдвиг "на месте" (вырожденный случай) — уже верное значение
    }
    let moved = atomicLoad(&feedback_counters[m]);
    atomicStore(&feedback_counters[new_m], moved);
    atomicStore(&feedback_counters[m], 0u);
}

// `Rule::memory` — ДВА раздельных прохода (та же причина, что у
// `update_feedback_latch_pass`/`update_feedback_relocate_pass`: перенос
// пишет в СЛОТ ДРУГОЙ клетки, что было бы гонкой с той клетки собственным
// потоком-осиротевшим-сбросом внутри ОДНОГО прохода — см. их подробный
// doc-комментарий выше, дословно применим и здесь). Диспетчеризуются
// СТРОГО ПОСЛЕДОВАТЕЛЬНО (push → relocate), см. `GpuEngine::dispatch_tick`.

// Фаза 1: запись нового наблюдения в буфер (FIFO — если полон, сдвиг влево
// на 1 и запись в конец; если не полон, запись в первую свободную позицию),
// ЛИБО осиротевший сброс, если структурного совпадения на этом тике не
// было (см. `GpuMatch::structural`'s doc-комментарий — зеркалит CPU
// `memory_targets`, взятый из ПОЛНОГО списка матчей, ДО применения гейта).
// НИКАКИХ чужих слотов не трогает.
@compute @workgroup_size(256, 1, 1)
fn update_memory_push_pass(@builtin(global_invocation_id) gid: vec3<u32>) {
    let m = gid.x;
    let total = params.width * params.height * params.max_matches_per_cell;
    if (m >= total) { return; }

    let cell_idx = m / params.max_matches_per_cell;
    let slot_in_cell = m % params.max_matches_per_cell;
    let me = current[cell_idx];
    if (me.value >= 256u) { return; }
    let slot = head_slots[me.value];
    if (slot_in_cell >= slot.rules_count) { return; }
    let rule_idx = slot.rules_start + slot_in_cell;
    if (rules[rule_idx].has_memory == 0u) { return; }

    if (matches[m].structural == 0u) {
        atomicStore(&memory_len[m], 0u);
        return;
    }

    // Значение для записи — см. `types::RecordTrigger`'s doc-комментарий:
    // `NeighborType` читает ТЕКУЩЕЕ (pre-tick, ещё не изменённое `apply_pass`
    // этого же тика — `current[]` не трогается до самого конца тика) состояние
    // соседа; `RuleOutcome` читает финальный исход арбитража ЭТОГО матча
    // (уже точно известен — этот проход идёт ПОСЛЕ apply/CPU-fallback).
    // Кодировка ОБЩАЯ с `rule_table::encode_recorded_value` — держать
    // синхронно: 0..=255 = Type(код типа), 256 = Applied, 257 = Missed.
    var value: u32;
    if (rules[rule_idx].memory_trigger == 0u) {
        let x = cell_idx % params.width;
        let y = cell_idx / params.width;
        let nx = i32(x) + rules[rule_idx].memory_dx;
        let ny = i32(y) + rules[rule_idx].memory_dy;
        if (nx < 0 || ny < 0 || u32(nx) >= params.width || u32(ny) >= params.height) {
            value = params.default_cell_type;
        } else {
            value = current[idx(u32(nx), u32(ny))].value;
        }
    } else {
        value = select(257u, 256u, atomicLoad(&match_state[m]) == 1u); // 256=Applied, 257=Missed
    }

    let win = rules[rule_idx].memory_window;
    let len = atomicLoad(&memory_len[m]);
    if (len < win) {
        atomicStore(&memory_buffers[m * MAX_MEMORY_WINDOW + len], value);
        atomicStore(&memory_len[m], len + 1u);
    } else {
        // Буфер уже полон — сдвиг влево на 1 (индекс 0 теряется, самое
        // старое значение), новое значение — в конец. `win` ≤
        // MAX_MEMORY_WINDOW (проверено в `build_gpu_rule_table`), цикл
        // ограничен реальным `win`, не потолком.
        for (var i = 0u; i + 1u < win; i = i + 1u) {
            let next = atomicLoad(&memory_buffers[m * MAX_MEMORY_WINDOW + i + 1u]);
            atomicStore(&memory_buffers[m * MAX_MEMORY_WINDOW + i], next);
        }
        atomicStore(&memory_buffers[m * MAX_MEMORY_WINDOW + win - 1u], value);
    }
}

// Фаза 2: перенос — ТОЛЬКО для матчей, выигравших арбитраж этого тика
// (гарантированно означает, что гейт БЫЛ открыт — гейт-закрытые матчи
// принудительно REJECTED в `detect_pass`, никогда не доходят до ACCEPTED)
// И физически имеющих сдвиг (`memory_has_shift` — правило без сдвига
// никогда не двигается, буфер живёт на фиксированной позиции). Та же
// раскладка `cells[1]` = новая позиция, что и у `update_feedback_relocate_pass`
// — гарантированно (см. `MemoryBroadcastUnsupported`'s защиту в
// `rule_table.rs`): `memory` с сдвигом всегда РОВНО один, не-broadcast.
@compute @workgroup_size(256, 1, 1)
fn update_memory_relocate_pass(@builtin(global_invocation_id) gid: vec3<u32>) {
    let m = gid.x;
    let total = params.width * params.height * params.max_matches_per_cell;
    if (m >= total) { return; }

    let cell_idx = m / params.max_matches_per_cell;
    let slot_in_cell = m % params.max_matches_per_cell;
    let me = current[cell_idx];
    if (me.value >= 256u) { return; }
    let slot = head_slots[me.value];
    if (slot_in_cell >= slot.rules_count) { return; }
    let rule_idx = slot.rules_start + slot_in_cell;
    if (rules[rule_idx].has_memory == 0u || rules[rule_idx].memory_has_shift == 0u) { return; }
    if (atomicLoad(&match_state[m]) != 1u) { return; }

    let new_key = matches[m].cells[1];
    let py = i32(new_key / padded_width());
    let px = i32(new_key % padded_width());
    let nx = px - i32(params.margin);
    let ny = py - i32(params.margin);
    if (nx < 0 || ny < 0 || u32(nx) >= params.width || u32(ny) >= params.height) {
        return;
    }
    let new_cell_idx = u32(ny) * params.width + u32(nx);
    let new_m = new_cell_idx * params.max_matches_per_cell + slot_in_cell;
    if (new_m == m) {
        return;
    }
    let win = rules[rule_idx].memory_window;
    for (var i = 0u; i < win; i = i + 1u) {
        let v = atomicLoad(&memory_buffers[m * MAX_MEMORY_WINDOW + i]);
        atomicStore(&memory_buffers[new_m * MAX_MEMORY_WINDOW + i], v);
    }
    atomicStore(&memory_len[new_m], atomicLoad(&memory_len[m]));
    atomicStore(&memory_len[m], 0u);
}
