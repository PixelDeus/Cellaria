//! `GpuEngine` — GPU-двигатель (feature `gpu`), параллельный CPU
//! `crate::Engine`, но НЕ реализующий `GridStorage` (см. doc-комментарий
//! модуля `crate::gpu` и план: GPU работает батчами буферов, а не по одной
//! клетке через трейт).
//!
//! Два пайплайна, выбираемые автоматически по [`GpuRuleTable::needs_arbitration`]
//! (см. её doc-комментарий и `shader.wgsl`):
//!
//! - **Simple** — self-write-only конфиги (например, Game of Life): каждый
//!   поток пишет только свою клетку, конфликтов записи в принципе нет,
//!   один compute-проход на тик.
//! - **Arbitrated** — есть хоть один сдвиг или запись в соседа: detect →
//!   `ROUNDS` раундов claim/resolve → apply, ВСЕ ВНУТРИ ОДНОГО compute
//!   pass / одной отправки в очередь, БЕЗ единого readback между ними —
//!   раунды синхронизируются между собой самим порядком выполнения на GPU
//!   (та же гарантия, что уже использовал прототип `arbitrate.rs`:
//!   несколько `dispatch_workgroups` подряд в одном pass'е видят запись
//!   предыдущего). `ROUNDS` — не догадка: алгоритм и его сходимость
//!   проверены отдельно в scratch-прототипе (`v2_arbitrate_check.rs`) на
//!   300 случайных сценариях конфликтов (до 8 клеток записи на матч) —
//!   худший случай сошёлся за 7 раундов; здесь заложен запас.
//!
//!   ГИБРИДНЫЙ GPU+CPU АРБИТРАЖ (см. также `chain_check.rs`/`hybrid_check.rs`
//!   scratch-прототипы): число раундов до сходимости РАСТЁТ ЛИНЕЙНО с
//!   длиной "цепочки" последовательно зависимых конфликтов (найдено
//!   экспериментально) — то есть НИКАКОЙ фиксированный `ROUNDS` не
//!   гарантирует сходимость universally, лишь практически достаточен для
//!   реалистичных (не специально адверсарных) конфликтов. `shader.wgsl`'s
//!   `count_pending` считает, сколько матчей осталось PENDING после
//!   `ROUNDS` раундов; `dispatch_tick` ПОСЛЕ КАЖДОГО тика, БЕЗ
//!   ИСКЛЮЧЕНИЙ, читает этот счётчик и, если он ненулевой, досчитывает
//!   оставшееся на CPU тем же алгоритмом, что и `arbitrator::arbitrate`
//!   (см. `cpu_fallback_resolve`), гарантируя точное совпадение с
//!   CPU-эталоном для ЛЮБОЙ длины цепочки (проверено `hybrid_check.rs`
//!   для цепочек от 10 до 1000, и `tests/gpu_v2_correctness.rs`'s
//!   `test_gpu_v2_hybrid_fallback_resolves_long_conflict_chain` — через
//!   настоящий `GpuEngine`, не только scratch-прототип).
//!
//!   ПОЧЕМУ ЭТА ПРОВЕРКА НЕ ЛЕНИВАЯ (важно, не техдолг): модель Cellaria
//!   построена на том, что каждый тик — атомарное, полностью определённое
//!   изменение состояния (вычисление только через правила, применённые к
//!   ОПРЕДЕЛЁННОМУ состоянию — не "почти определённому, если цепочка не
//!   слишком длинная"). Пробовал сделать проверку ленивой (отложенной до
//!   `read_grid` или до явного вызова пользователем) — измеримо быстрее
//!   (`examples/flagship_shifts.rs`: ближе к 40× вместо 13×), но
//!   АРХИТЕКТУРНО НЕВЕРНО: если тик N не досчитан, а `detect_pass` тика
//!   N+1 запускается поверх этого недосчитанного состояния, N+1
//!   вычисляется не по правилам над определённым состоянием, а над
//!   артефактом гонки — детерминизм модели ломается НЕ на финальном
//!   чтении, а в середине, и это может испортить весь последующий расчёт
//!   непредсказуемым образом. Перекладывать это на пользователя (мониторь
//!   `pending_count()` сам, если тебе важно) — это не архитектура, а
//!   признание, что движок иногда лжёт о завершённости тика. Поэтому
//!   проверка+добор — БЕЗУСЛОВНАЯ часть каждого `dispatch_tick`, а не
//!   опция и не то, что можно включить/выключить снаружи; цена (см.
//!   `run_tick`/`run_ticks`) — известная, измеренная, принятая: тик не
//!   считается завершённым, пока решётка не в полностью определённом
//!   состоянии.
//!
//!   Также пробовал (и отдельно отклонил) адаптивную остановку по раундам
//!   батчами С readback МЕЖДУ батчами (не после тика целиком, а несколько
//!   раз внутри одного тика) — на порядок хуже, чем даже readback раз в
//!   тик: каждый лишний `device.poll(Maintain::Wait)` внутри `run_ticks` —
//!   это полный CPU-GPU стоп-кран, ломающий конвейеризацию между отправкой
//!   команд и их выполнением на GPU. Урок: не всякая "умная" оптимизация
//!   оправдана — важно различать "экономит GPU-работу" (может того не
//!   стоить) и "необходима для корректности" (не опция вообще).

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use crate::types::{Cell, CellType, Rule, DEFAULT_CELL_VALUE};

use super::rule_table::{
    build_gpu_rule_table, GpuHeadSlot, GpuMatchLayout, GpuOffset, GpuPatternOffset, GpuRule, GpuRuleTable,
    GpuUnsupportedReason, MAX_MEMORY_WINDOW, MAX_WRITE_CELLS,
};

// `impl GpuEngine` разбит на построение (`engine_build.rs`: new/with_rounds/
// build_simple_pipeline/build_arbitrated_pipeline) и рантайм-исполнение
// (`engine_tick.rs`: run_tick/dispatch_tick/cpu_fallback_resolve/read_grid)
// — та же практика, что уже применена к `engine/mod.rs`: несколько
// `impl`-блоков одного типа в разных файлах, публичный API не меняется.
#[path = "engine_build.rs"]
mod engine_build;
#[path = "engine_tick.rs"]
mod engine_tick;

/// См. doc-комментарий модуля — запас (>2×) над худшим случаем, измеренным
/// в `v2_arbitrate_check.rs` (7 раундов на намеренно плотных случайных
/// конфликтах, до 8 ячеек записи на матч). Безусловный, БЕЗ readback (см.
/// doc-комментарий модуля про измеренно провальную попытку адаптивной
/// остановки).
///
/// Было 32 (>4× запас) — снижено ПОСЛЕ появления гибридного CPU-добора
/// (`cpu_fallback_resolve`): раньше недостаточный запас означал риск
/// СМОЛЧАТЬ неверный результат для конфликтов длиннее бюджета, теперь —
/// только чаще (но по-прежнему корректно) уходить в CPU-добор. `flagship_shifts`
/// с ROUNDS=32 тратит бюджет почти целиком впустую (типичный конфликт
/// движущихся частиц сходится за 1-2 раунда, но `clear_claims`/`claim_pass`/
/// `resolve_pass` всё равно диспатчатся ВСЕ 32 раза, даже если все матчи
/// давно ACCEPTED/REJECTED — ни один из трёх проходов не умеет остановиться
/// раньше срока без CPU-readback, а readback ВНУТРИ тика уже отдельно
/// измерен и отклонён, см. doc-комментарий модуля) — измерено на реальном
/// железе (`examples/flagship_shifts.rs`): ROUNDS=8 даёт N=400 ~110М
/// клеток/сек против ~95-104М у ROUNDS=32 (шумно, но стабильно лучше на
/// 200/400). Здесь взято 16, а не самое быстрое из промера (8) — сохраняет
/// более консервативный запас над 7-раундовым найденным худшим случаем, а
/// не только над тем, что удобно конкретно для `flagship_shifts`'s простых
/// 2-клеточных конфликтов.
const ROUNDS: u32 = 16;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuCell {
    value: u32,
    born_at: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    width: u32,
    height: u32,
    /// Поколение ДО инкремента текущего тика — см. doc-комментарий
    /// `shader.wgsl` про `params.generation`/`out.born_at`.
    generation: u32,
    default_cell_type: u32,
    /// `GpuRuleTable::margin` — реальный (не потолочный `MAX_MARGIN`) охват
    /// для этого набора правил; 0 для Simple-пайплайна (не используется).
    margin: u32,
    /// `GpuRuleTable::max_matches_per_cell` — реальный (не потолочный
    /// `MAX_MATCHES_PER_CELL`) максимум правил на голову; 0 для
    /// Simple-пайплайна (не используется).
    max_matches_per_cell: u32,
    _pad0: u32,
    _pad1: u32,
}

fn make_storage_buf<T: bytemuck::Pod>(device: &wgpu::Device, data: &[T]) -> wgpu::Buffer {
    // Нулевой размер запрещён у wgpu-буферов — правила без единого офсета
    // паттерна (в теории — правило с пустым `pattern`, `id`) допустимы на
    // CPU-стороне, так что резервируем минимум на один элемент, даже если
    // `data` пуст; шейдер такой буфер просто не читает (соответствующий
    // `pattern_len`/`count` будет 0).
    if data.is_empty() {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: std::mem::size_of::<T>().max(4) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        })
    } else {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(data),
            usage: wgpu::BufferUsages::STORAGE,
        })
    }
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// Simple-пайплайн: один self-write проход (`shader.wgsl::main`).
struct SimplePipeline {
    pipeline: wgpu::ComputePipeline,
    bg_a_to_b: wgpu::BindGroup,
    bg_b_to_a: wgpu::BindGroup,
}

/// Arbitrated-пайплайн: полный detect/claim/resolve/apply (см. doc-комментарий модуля).
struct ArbitratedPipeline {
    p_detect: wgpu::ComputePipeline,
    p_clear_locked: wgpu::ComputePipeline,
    p_clear_claims: wgpu::ComputePipeline,
    p_clear_counter: wgpu::ComputePipeline,
    p_count_pending: wgpu::ComputePipeline,
    p_claim: wgpu::ComputePipeline,
    p_resolve: wgpu::ComputePipeline,
    p_apply: wgpu::ComputePipeline,
    /// Дispatch'ится ОТДЕЛЬНЫМ submission'ом ПОСЛЕ `p_apply` И после (если
    /// понадобился) `GpuEngine::cpu_fallback_resolve` (см. `dispatch_tick`),
    /// поскольку ему нужен УЖЕ финальный `match_state`, который CPU-fallback
    /// дописывает только на CPU-стороне, между двумя submission'ами. Пайплайн
    /// создаётся ВСЕГДА (дёшево), но дispatch'ится (`GpuEngine::dispatch_tick`)
    /// только если `needs_starvation` — та же экономия, что и у CPU-side
    /// `ExtensionFlags`: нулевые накладные расходы (ни одного лишнего
    /// submission'а) для конфигов, которым это не нужно.
    p_update_starvation: wgpu::ComputePipeline,
    /// Persistent МЕЖДУ ТИКАМИ storage-буфер счётчиков голодания — см.
    /// `shader.wgsl`'s binding 12 doc-комментарий. Аллоцируется ЗДЕСЬ ОДИН
    /// РАЗ (не в `dispatch_tick`) даже для конфигов без `starvation_after`
    /// (минимальный размер, как `counters_buf`) — bind group layout требует
    /// присутствия ВСЕХ привязок независимо от того, реально ли они
    /// используются этим конкретным конфигом. Поле нигде явно не читается
    /// ПОСЛЕ конструктора (в отличие от `matches_buf`/`match_state_buf`/
    /// `locked_buf`, к которым `cpu_fallback_resolve` обращается по имени) —
    /// хранится здесь ТОЛЬКО ради времени жизни (RAII): bind group держит
    /// свою ссылку на GPU-стороне, но Rust-владение буфером обязано
    /// пережить саму структуру `ArbitratedPipeline`, иначе wgpu уничтожит
    /// буфер, пока bind group всё ещё на него ссылается.
    #[allow(dead_code)]
    starvation_counters_buf: wgpu::Buffer,
    /// `Rule::feedback` — ДВА раздельных пайплайна (не один, как у
    /// `p_update_starvation`), дispatch'ятся СТРОГО ПОСЛЕДОВАТЕЛЬНО внутри
    /// ОДНОГО compute pass'а (латч → перенос) — см. `update_feedback_latch_pass`/
    /// `update_feedback_relocate_pass`'s doc-комментарий в `shader.wgsl` про
    /// то, почему раздельность ОБЯЗАТЕЛЬНА (гонка между переносом счётчика в
    /// чужой слот и осиротевшим сбросом того же слота его собственным
    /// потоком). Как и `p_update_starvation`, создаются всегда, дispatch'ятся
    /// только при `needs_feedback`.
    p_update_feedback_latch: wgpu::ComputePipeline,
    p_update_feedback_relocate: wgpu::ComputePipeline,
    /// Persistent МЕЖДУ ТИКАМИ storage-буфер защёлок feedback — см.
    /// `shader.wgsl`'s binding 13 doc-комментарий. Та же логика хранения
    /// "только ради времени жизни", что и `starvation_counters_buf` выше.
    #[allow(dead_code)]
    feedback_counters_buf: wgpu::Buffer,
    /// `Rule::memory` — ТОЖЕ два раздельных пайплайна (push → relocate),
    /// та же причина (гонка перенос/осиротевший-сброс), что и у
    /// `p_update_feedback_latch`/`p_update_feedback_relocate` выше — см.
    /// `update_memory_push_pass`/`update_memory_relocate_pass`'s
    /// doc-комментарий в `shader.wgsl`.
    p_update_memory_push: wgpu::ComputePipeline,
    p_update_memory_relocate: wgpu::ComputePipeline,
    /// Persistent FIFO-буфер памяти (binding 14) — размер `n_matches *
    /// MAX_MEMORY_WINDOW`, см. её doc-комментарий в `shader.wgsl`.
    #[allow(dead_code)]
    memory_buffers_buf: wgpu::Buffer,
    /// Persistent счётчик заполненности (binding 15) — размер `n_matches`,
    /// та же индексация, что `starvation_counters_buf`/`feedback_counters_buf`.
    #[allow(dead_code)]
    memory_len_buf: wgpu::Buffer,
    bg_a_to_b: wgpu::BindGroup,
    bg_b_to_a: wgpu::BindGroup,
    /// Читается после [`ROUNDS`] раундов, чтобы узнать, сколько матчей
    /// остались PENDING — см. doc-комментарий модуля про гибридный
    /// GPU+CPU арбитраж.
    counters_buf: wgpu::Buffer,
    /// Переиспользуемый readback-буфер для `counters_buf` — создан ОДИН РАЗ
    /// (не каждый тик), см. её doc-комментарий в `build_arbitrated_pipeline`.
    pending_readback_buf: wgpu::Buffer,
    /// Заполняется `detect_pass`, читается ПОЛНОСТЬЮ только в редком
    /// случае (`pending_count > 0`) — см. `GpuEngine::cpu_fallback_resolve`.
    matches_buf: wgpu::Buffer,
    match_state_buf: wgpu::Buffer,
    locked_buf: wgpu::Buffer,
    /// Переиспользуемые readback-буферы для `matches_buf`/`match_state_buf`/
    /// `locked_buf` — тот же принцип, что и `pending_readback_buf` (создать
    /// один раз при известном размере, а не на каждый вызов). Путь редкий
    /// (только при `pending_count > 0`), но раз уже создаются один раз для
    /// самого частого случая (`pending_readback_buf`), нет причины оставлять
    /// этот путь единственным местом, где буфер всё ещё пересоздаётся.
    matches_readback_buf: wgpu::Buffer,
    state_readback_buf: wgpu::Buffer,
    locked_readback_buf: wgpu::Buffer,
}

// `ArbitratedPipeline` заметно крупнее `SimplePipeline` (больше wgpu-буферов
// и пайплайнов для арбитража) — clippy предлагает `Box` для выравнивания
// размеров вариантов, но `Pipeline` строится ОДИН раз в `GpuEngine::new` и
// живёт полем `GpuEngine`, не копируется и не пересобирается за тик —
// разница в пару КБ на структуру, живущую весь срок движка, не стоит
// косвенности `Box` в единственном месте, где по нему матчатся каждый тик.
#[allow(clippy::large_enum_variant)]
enum Pipeline {
    Simple(SimplePipeline),
    Arbitrated(ArbitratedPipeline),
}

/// GPU-двигатель (см. doc-комментарий модуля). `GpuEngine::new` возвращает
/// `Err`, если хотя бы одно правило набора вне поддерживаемого подмножества
/// — см. [`GpuUnsupportedReason`].
///
/// GPU не бесплатен на маленьких решётках — есть реальный, измеренный (не
/// предполагаемый) порог окупаемости, причём РАЗНЫЙ для двух пайплайнов
/// (см. `Pipeline`/`GpuRuleTable::needs_arbitration`):
///
/// - **Simple** (self-write-only, `examples/flagship_gol.rs`, N — сторона
///   решётки): GPU обгоняет CPU `Engine` уже при N≈50 (≈5×), дальше растёт
///   (N=200: ≈37×). Один compute pass почти без синхронизации — overhead
///   запуска минимален.
/// - **Arbitrated** (сдвиги/запись в соседа, `examples/flagship_shifts.rs`):
///   ГОРАЗДО дороже начать — при N=20 GPU МЕДЛЕННЕЕ CPU (≈0.1×): фиксовая
///   цена `ROUNDS` раундов claim/resolve на почти пустую задачу не
///   окупается. Точка перелома — между N=50 (≈0.6×, ещё проигрыш) и N=100
///   (≈5×, уже выигрыш); дальше растёт быстро (N=400: ≈40×).
///
/// Вывод: для малых решёток (десятки-низкие сотни клеток на сторону) с
/// реальным арбитражем (не self-write-only) CPU `Engine` — не запасной, а
/// ПРАВИЛЬНЫЙ выбор по чистой скорости, GPU не компенсирует свой fixed
/// overhead запуска. `GpuEngine` не выбирает бэкенд автоматически — выбор
/// между CPU/GPU остаётся за вызывающим кодом (нет универсального порога:
/// зависит от пайплайна и конкретного железа).
///
/// Важная оговорка (найдена при попытке переизмерить порог с batched
/// `run_ticks`, БЕЗ readback на каждом тике): точка перелома зависит не
/// ТОЛЬКО от N решётки, а от РЕАЛЬНОГО числа совпадений за тик. На
/// затухающей нагрузке (частицы + `OverflowAction::Discard`, как в
/// `flagship_shifts.rs`) популяция монотонно падает — за 20 тиков N=20
/// теряет ~99% (138→1 живая клетка), N=400 теряет ~80% (47873→9484). Долгий
/// прогон такого сценария не измеряет "устойчивый" throughput ни для CPU,
/// ни для GPU — оба меряют смесь плотного начала и разрежённого конца, в
/// разных пропорциях в зависимости от длины прогона. Число выше (таблица
/// из `flagship_gol`/`flagship_shifts`) — ориентир для конкретных сценариев
/// этих примеров, не универсальная формула "GPU выгоден начиная с N=X" — в
/// сценарии, который НЕ теряет плотность (например, отражение от границ
/// вместо `Discard`), порог может быть другим.
pub struct GpuEngine {
    device: wgpu::Device,
    queue: wgpu::Queue,
    params_buf: wgpu::Buffer,
    buf_a: wgpu::Buffer,
    buf_b: wgpu::Buffer,
    pipeline: Pipeline,
    width: u32,
    height: u32,
    n_cells: usize,
    /// `GpuRuleTable::margin` — 0 для Simple-пайплайна (не используется);
    /// нужен `dispatch_tick` для правильных workgroup-счётов у
    /// `clear_locked`/`clear_claims` (дополненная сетка, см. `shader.wgsl`).
    margin: u32,
    /// `GpuRuleTable::max_matches_per_cell` — 0 для Simple-пайплайна (не
    /// используется); нужен `dispatch_tick` для правильных
    /// workgroup-счётов у `detect_pass`/`claim_pass`/`resolve_pass`.
    max_matches_per_cell: u32,
    /// То же "поколение ДО инкремента", что описано у `Params::generation` —
    /// синхронизировано с ним 1-в-1 между тиками (см. doc-комментарий
    /// `shader.wgsl`: шейдер сам не увеличивает счётчик, это делает
    /// `run_tick`/`run_ticks` здесь, ровно как `Grid::advance_age` на CPU).
    generation: u32,
    /// `true`, если ПОСЛЕДНЕЕ актуальное состояние решётки лежит в `buf_a`
    /// (иначе — в `buf_b`); ping-pong между двумя буферами вместо
    /// аллокации нового буфера на каждый тик.
    current_is_a: bool,
    /// Переиспользуемый readback-буфер для `read_grid` — создан ОДИН РАЗ
    /// (размер решётки не меняется после `new`), а не на каждый вызов, тем
    /// же принципом, что и `ArbitratedPipeline::pending_readback_buf` (см.
    /// её doc-комментарий) — измеримо дорого пересоздавать буфер той же
    /// формы каждый раз, когда форма и так известна заранее.
    grid_readback_buf: wgpu::Buffer,
    /// `GpuRuleTable::needs_starvation` — `false` для Simple-пайплайна и для
    /// Arbitrated-конфигов без `starvation_after`. `dispatch_tick`
    /// пропускает `p_update_starvation`'s submission целиком, когда `false`
    /// — нулевые накладные расходы (ни одного лишнего submission'а/sync)
    /// для конфигов, которым это не нужно.
    needs_starvation: bool,
    /// `GpuRuleTable::needs_feedback` — то же "нулевые накладные расходы,
    /// если не просили", что и `needs_starvation` выше, но пропускает ДВА
    /// dispatch'а (латч + перенос) вместо одного — см.
    /// `ArbitratedPipeline::p_update_feedback_latch`'s doc-комментарий.
    needs_feedback: bool,
    /// `GpuRuleTable::needs_memory` — та же "нулевые накладные расходы,
    /// если не просили" экономия, но пропускает ДВА dispatch'а (push +
    /// relocate), см. `ArbitratedPipeline::p_update_memory_push`'s
    /// doc-комментарий.
    needs_memory: bool,
    /// Число claim/resolve-раундов на тик — см. doc-комментарий [`ROUNDS`]
    /// у модуля. По умолчанию (`GpuEngine::new`) равно `ROUNDS`; настраивается
    /// через `GpuEngine::with_rounds` (п.1, сессия 2026-08-09) для конфигов
    /// с заведомо короткими/длинными цепочками конфликтов, где
    /// экспериментально измеренный компромисс "16 — общий случай" не подходит.
    rounds: u32,
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
