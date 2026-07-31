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

// MAX_WRITE_CELLS в rule_table.rs = 1 + MAX_SHIFTS*(1+MAX_CHANGES) = 1+2*5 = 11.
const MAX_WRITE_CELLS: u32 = 11u;
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
        matches[m].priority = rules[rule_idx].priority;
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
        let sx0 = i32(x) + rules[rule_idx].shift_dx0;
        let sy0 = i32(y) + rules[rule_idx].shift_dy0;
        let sx1 = i32(x) + rules[rule_idx].shift_dx1;
        let sy1 = i32(y) + rules[rule_idx].shift_dy1;

        n = push_write_cell(m, n, i32(x), i32(y), params.default_cell_type);
        if (shift_count >= 1u) { n = push_write_cell(m, n, sx0, sy0, me.value); }
        if (shift_count >= 2u) { n = push_write_cell(m, n, sx1, sy1, me.value); }

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
    matches[m].priority = rules[rule_idx].priority;
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
