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
//!   [`ROUNDS`] раундов claim/resolve → apply, ВСЕ ВНУТРИ ОДНОГО compute
//!   pass / одной отправки в очередь, БЕЗ единого readback между ними —
//!   раунды синхронизируются между собой самим порядком выполнения на GPU
//!   (та же гарантия, что уже использовал прототип `arbitrate.rs`:
//!   несколько `dispatch_workgroups` подряд в одном pass'е видят запись
//!   предыдущего). [`ROUNDS`] — не догадка: алгоритм и его сходимость
//!   проверены отдельно в scratch-прототипе (`v2_arbitrate_check.rs`) на
//!   300 случайных сценариях конфликтов (до 8 клеток записи на матч) —
//!   худший случай сошёлся за 7 раундов; здесь заложен запас.
//!
//!   ГИБРИДНЫЙ GPU+CPU АРБИТРАЖ (см. также `chain_check.rs`/`hybrid_check.rs`
//!   scratch-прототипы): число раундов до сходимости РАСТЁТ ЛИНЕЙНО с
//!   длиной "цепочки" последовательно зависимых конфликтов (найдено
//!   экспериментально) — то есть НИКАКОЙ фиксированный [`ROUNDS`] не
//!   гарантирует сходимость universally, лишь практически достаточен для
//!   реалистичных (не специально адверсарных) конфликтов. `shader.wgsl`'s
//!   `count_pending` считает, сколько матчей осталось PENDING после
//!   [`ROUNDS`] раундов; `dispatch_tick` ПОСЛЕ КАЖДОГО тика, БЕЗ
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
    GpuUnsupportedReason, MAX_WRITE_CELLS,
};

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
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None },
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

enum Pipeline {
    Simple(SimplePipeline),
    Arbitrated(ArbitratedPipeline),
}

/// GPU-двигатель (см. doc-комментарий модуля). `GpuEngine::new` возвращает
/// `Err`, если хотя бы одно правило набора вне поддерживаемого подмножества
/// — см. [`GpuUnsupportedReason`].
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
}

impl GpuEngine {
    /// Построить GPU-двигатель. `initial_cells` — только НЕ-дефолтные ячейки
    /// (аналог `Grid::active_coords` при загрузке конфига) — остальные
    /// клетки решётки инициализируются `Cell::default()`.
    pub fn new(
        width: usize,
        height: usize,
        initial_cells: &[(usize, usize, Cell)],
        rule_index: &HashMap<CellType, Vec<Rule>>,
    ) -> Result<Self, GpuUnsupportedReason> {
        let table = build_gpu_rule_table(rule_index)?;
        Ok(pollster::block_on(Self::init(width, height, initial_cells, &table)))
    }

    async fn init(width: usize, height: usize, initial_cells: &[(usize, usize, Cell)], table: &GpuRuleTable) -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .expect("GpuEngine::new: не найден совместимый GPU-адаптер (нужен Vulkan/Metal/DX12-драйвер)");
        // `wgpu::Limits::default()` — консервативные WebGPU-лимиты (например,
        // всего 8 storage-буферов на стадию) — недостаточно для
        // arbitrated-пайплайна (11 storage-биндингов, см. `build_arbitrated_pipeline`).
        // `adapter.limits()` — то, что РЕАЛЬНО поддерживает железо (на
        // десктопных GPU практически всегда куда больше).
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor { required_limits: adapter.limits(), ..Default::default() }, None)
            .await
            .expect("GpuEngine::new: request_device не удался");

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cellaria"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let n_cells = width * height;
        let mut cells = vec![GpuCell { value: 0, born_at: 0 }; n_cells];
        for &(x, y, cell) in initial_cells {
            cells[y * width + x] = GpuCell {
                value: cell.value.0 .0 as u32,
                born_at: cell.born_at.min(u32::MAX as u64) as u32,
            };
        }

        let margin = table.margin.max(0) as u32;
        let max_matches_per_cell = table.max_matches_per_cell;
        let params = Params {
            width: width as u32,
            height: height as u32,
            generation: 0,
            default_cell_type: DEFAULT_CELL_VALUE as u32,
            margin,
            max_matches_per_cell,
            _pad0: 0,
            _pad1: 0,
        };
        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let buf_a = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&cells),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        });
        let buf_b = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (n_cells * std::mem::size_of::<GpuCell>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let head_slots_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&table.head_slots as &[GpuHeadSlot]),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let rules_buf = make_storage_buf::<GpuRule>(&device, &table.rules);
        let offsets_buf = make_storage_buf::<GpuPatternOffset>(&device, &table.pattern_offsets);
        let head_offsets_buf = make_storage_buf::<GpuOffset>(&device, &table.head_offsets);

        let pipeline = if table.needs_arbitration {
            Pipeline::Arbitrated(Self::build_arbitrated_pipeline(
                &device, &shader, &params_buf, &buf_a, &buf_b,
                &head_slots_buf, &rules_buf, &offsets_buf, &head_offsets_buf,
                width, height, n_cells, margin, max_matches_per_cell,
            ))
        } else {
            Pipeline::Simple(Self::build_simple_pipeline(
                &device, &shader, &params_buf, &buf_a, &buf_b,
                &head_slots_buf, &rules_buf, &offsets_buf, &head_offsets_buf,
                table.pattern_reach,
            ))
        };

        let grid_readback_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (n_cells * std::mem::size_of::<GpuCell>()) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Self {
            device,
            queue,
            params_buf,
            buf_a,
            buf_b,
            pipeline,
            width: width as u32,
            height: height as u32,
            n_cells,
            margin,
            max_matches_per_cell,
            generation: 0,
            current_is_a: true,
            grid_readback_buf,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_simple_pipeline(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        params_buf: &wgpu::Buffer,
        buf_a: &wgpu::Buffer,
        buf_b: &wgpu::Buffer,
        head_slots_buf: &wgpu::Buffer,
        rules_buf: &wgpu::Buffer,
        offsets_buf: &wgpu::Buffer,
        head_offsets_buf: &wgpu::Buffer,
        pattern_reach: i32,
    ) -> SimplePipeline {
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                storage_entry(1, true),
                storage_entry(2, false),
                storage_entry(3, true),
                storage_entry(4, true),
                storage_entry(5, true),
                storage_entry(6, true),
            ],
        });

        let mk_bg = |current: &wgpu::Buffer, next: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: params_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: current.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: next.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: head_slots_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: rules_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: offsets_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 6, resource: head_offsets_buf.as_entire_binding() },
                ],
            })
        };

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        // `main_tiled` кэширует соседей клетки в shared-памяти workgroup'а
        // (см. её doc-комментарий в `shader.wgsl`) — безопасно ТОЛЬКО когда
        // halo радиуса 1 гарантированно покрывает ЛЮБОЙ офсет паттерна
        // ЭТОГО конкретного набора правил (`GpuRuleTable::pattern_reach`);
        // иначе используется обычный `main` (полностью общий случай).
        let entry_point = if pattern_reach <= 1 { "main_tiled" } else { "main" };
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            module: shader,
            entry_point,
            compilation_options: Default::default(),
            cache: None,
        });

        SimplePipeline { bg_a_to_b: mk_bg(buf_a, buf_b), bg_b_to_a: mk_bg(buf_b, buf_a), pipeline }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_arbitrated_pipeline(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        params_buf: &wgpu::Buffer,
        buf_a: &wgpu::Buffer,
        buf_b: &wgpu::Buffer,
        head_slots_buf: &wgpu::Buffer,
        rules_buf: &wgpu::Buffer,
        offsets_buf: &wgpu::Buffer,
        head_offsets_buf: &wgpu::Buffer,
        width: usize,
        height: usize,
        n_cells: usize,
        margin: u32,
        max_matches_per_cell: u32,
    ) -> ArbitratedPipeline {
        // Реальный (не потолочный MAX_MATCHES_PER_CELL) максимум правил на
        // голову у ЭТОГО конкретного набора правил — см. doc-комментарий
        // `GpuRuleTable::max_matches_per_cell`: типичный конфиг (1 правило
        // на голову) даёт здесь буфер матчей/потоков detect-claim-resolve
        // в 8 раз меньше, чем потолочный расчёт.
        let n_matches = n_cells * max_matches_per_cell as usize;
        // COPY_SRC на всех трёх (matches/match_state/locked) — читаются
        // ПОЛНОСТЬЮ, но только в редком случае (`pending_count > 0`, см.
        // `GpuEngine::cpu_fallback_resolve`), не каждый тик.
        let matches_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (n_matches * std::mem::size_of::<GpuMatchLayout>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let match_state_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (n_matches * 4).max(4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        // `claims`/`locked` живут в ДОПОЛНЕННОЙ координатной сетке (см.
        // `shader.wgsl::padded_idx`) — шире видимой решётки на РЕАЛЬНЫЙ (не
        // потолочный `MAX_MARGIN`) охват этого набора правил по каждую
        // сторону, под "фантомные" (реально никогда не записываемые, но
        // участвующие в конфликте — см. doc-комментарий `rule_table::MAX_MARGIN`)
        // ячейки записи у самого края.
        let margin = margin as usize;
        let padded_cells = (width + 2 * margin) * (height + 2 * margin);
        let claims_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (padded_cells * 4).max(4) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let locked_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (padded_cells * 4).max(4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let counters_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        // Переиспользуемый readback-буфер для `pending_count` — эта проверка
        // происходит БЕЗУСЛОВНО каждый тик (см. doc-комментарий модуля), так
        // что `device.create_buffer` заново на каждый тик — фиксированные
        // накладные расходы, которые платятся ВСЕГДА, даже на решётках,
        // слишком маленьких, чтобы round-loop вообще что-то значил (см.
        // плоские N=20/50 числа в `flagship_shifts.rs` независимо от ROUNDS).
        let pending_readback_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        // Переиспользуемые readback-буферы для `cpu_fallback_resolve` — тот
        // же принцип, что и `pending_readback_buf` выше, только для редкого
        // пути (`pending_count > 0`): раз размер известен заранее (не
        // меняется между тиками), нет причины пересоздавать буфер той же
        // формы при каждом срабатывании гибридного добора.
        let matches_readback_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (n_matches * std::mem::size_of::<GpuMatchLayout>()) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let state_readback_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (n_matches * 4).max(4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let locked_readback_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (padded_cells * 4).max(4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                storage_entry(1, true),
                storage_entry(2, false),
                storage_entry(3, true),
                storage_entry(4, true),
                storage_entry(5, true),
                storage_entry(6, true),
                storage_entry(7, false),
                storage_entry(8, false),
                storage_entry(9, false),
                storage_entry(10, false),
                storage_entry(11, false),
            ],
        });

        let mk_bg = |current: &wgpu::Buffer, next: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: params_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: current.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: next.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: head_slots_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: rules_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: offsets_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 6, resource: head_offsets_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 7, resource: matches_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 8, resource: match_state_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 9, resource: claims_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 10, resource: locked_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 11, resource: counters_buf.as_entire_binding() },
                ],
            })
        };

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let mk_pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: shader,
                entry_point: entry,
                compilation_options: Default::default(),
                cache: None,
            })
        };

        ArbitratedPipeline {
            p_detect: mk_pipeline("detect_pass"),
            p_clear_locked: mk_pipeline("clear_locked"),
            p_clear_claims: mk_pipeline("clear_claims"),
            p_clear_counter: mk_pipeline("clear_counter"),
            p_count_pending: mk_pipeline("count_pending"),
            p_claim: mk_pipeline("claim_pass"),
            p_resolve: mk_pipeline("resolve_pass"),
            p_apply: mk_pipeline("apply_pass"),
            bg_a_to_b: mk_bg(buf_a, buf_b),
            bg_b_to_a: mk_bg(buf_b, buf_a),
            counters_buf,
            pending_readback_buf,
            matches_buf,
            match_state_buf,
            locked_buf,
            matches_readback_buf,
            state_readback_buf,
            locked_readback_buf,
        }
    }

    /// Один тик. Возвращается ТОЛЬКО когда решётка полностью в
    /// определённом состоянии — см. doc-комментарий модуля: Simple — один
    /// dispatch, без readback (арбитраж вообще не нужен); Arbitrated —
    /// [`ROUNDS`] раундов в одном submission БЕЗ readback, ПЛЮС
    /// безусловная проверка+добор (маленький readback, редко — полный
    /// CPU-добор) ПОСЛЕ каждого тика. Это не опция и не то, что можно
    /// отключить: следующий тик обязан видеть определённое состояние.
    pub fn run_tick(&mut self) {
        self.dispatch_tick();
    }

    /// N тиков подряд. Каждый тик — атомарная единица (см. `run_tick`), но
    /// в отличие от единого readback "в самом конце" пропускная
    /// способность здесь ограничена тем, что КАЖДЫЙ тик всё равно платит
    /// за свою собственную проверку+добор (см. doc-комментарий модуля) —
    /// более раннее "ленивое" решение (читать только при `read_grid`)
    /// было быстрее, но нарушало детерминизм модели между тиками внутри
    /// пакета, поэтому отклонено.
    pub fn run_ticks(&mut self, n: u32) {
        for _ in 0..n {
            self.dispatch_tick();
        }
    }

    fn dispatch_tick(&mut self) {
        let params = Params {
            width: self.width,
            height: self.height,
            generation: self.generation,
            default_cell_type: DEFAULT_CELL_VALUE as u32,
            margin: self.margin,
            max_matches_per_cell: self.max_matches_per_cell,
            _pad0: 0,
            _pad1: 0,
        };
        self.queue.write_buffer(&self.params_buf, 0, bytemuck::bytes_of(&params));

        let wg_grid_x = self.width.div_ceil(16);
        let wg_grid_y = self.height.div_ceil(16);

        match &self.pipeline {
            Pipeline::Simple(p) => {
                let bg = if self.current_is_a { &p.bg_a_to_b } else { &p.bg_b_to_a };
                let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
                    pass.set_pipeline(&p.pipeline);
                    pass.set_bind_group(0, bg, &[]);
                    pass.dispatch_workgroups(wg_grid_x, wg_grid_y, 1);
                }
                self.queue.submit(Some(encoder.finish()));
            }
            Pipeline::Arbitrated(p) => {
                let bg = if self.current_is_a { &p.bg_a_to_b } else { &p.bg_b_to_a };
                // `self.max_matches_per_cell` — реальный (не потолочный)
                // максимум правил на голову для ЭТОГО конфига, см.
                // doc-комментарий `GpuRuleTable::max_matches_per_cell`.
                let n_matches = self.n_cells * self.max_matches_per_cell as usize;
                let wg_matches = (n_matches as u32).div_ceil(256);
                // `clear_locked`/`clear_claims` покрывают ДОПОЛНЕННУЮ сетку
                // (см. `shader.wgsl::padded_idx`/`params.margin`), а не
                // только видимую решётку — фантомные ячейки записи у края
                // тоже нуждаются в своём locked/claims-состоянии между
                // раундами. `self.margin` — реальный охват ЭТОГО набора
                // правил (см. `GpuRuleTable::margin`), не потолочный `MAX_MARGIN`.
                let padded_cells = (self.width + 2 * self.margin) * (self.height + 2 * self.margin);
                let wg_padded_cells = padded_cells.div_ceil(256);

                // detect + ROUNDS раундов + apply — ВСЁ В ОДНОМ submission,
                // БЕЗ единого readback внутри (см. doc-комментарий модуля:
                // батчевый readback-based ранний выход между раундами был
                // реально измерен и убран — на этой модели исполнения
                // (без async-конвейеризации между тиками) каждый
                // `device.poll(Maintain::Wait)` внутри тика — это полный
                // CPU-GPU стоп-кран, который стоит на порядки дороже, чем
                // несколько "впустую" прогнанных раундов claim/resolve он
                // якобы экономит).
                let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
                    pass.set_bind_group(0, bg, &[]);

                    pass.set_pipeline(&p.p_clear_locked);
                    pass.dispatch_workgroups(wg_padded_cells, 1, 1);
                    pass.set_pipeline(&p.p_detect);
                    pass.dispatch_workgroups(wg_matches, 1, 1);

                    for _ in 0..ROUNDS {
                        pass.set_pipeline(&p.p_clear_claims);
                        pass.dispatch_workgroups(wg_padded_cells, 1, 1);
                        pass.set_pipeline(&p.p_claim);
                        pass.dispatch_workgroups(wg_matches, 1, 1);
                        pass.set_pipeline(&p.p_resolve);
                        pass.dispatch_workgroups(wg_matches, 1, 1);
                    }

                    pass.set_pipeline(&p.p_clear_counter);
                    pass.dispatch_workgroups(1, 1, 1);
                    pass.set_pipeline(&p.p_count_pending);
                    pass.dispatch_workgroups(wg_matches, 1, 1);

                    pass.set_pipeline(&p.p_apply);
                    pass.dispatch_workgroups(wg_grid_x, wg_grid_y, 1);
                }
                self.queue.submit(Some(encoder.finish()));

                // ОБЯЗАТЕЛЬНАЯ проверка+добор ПОСЛЕ КАЖДОГО тика, БЕЗ
                // исключений — см. doc-комментарий модуля. Раньше здесь
                // стоял "ленивый" вариант (проверка отложена до
                // `read_grid`, ради пропускной способности в `run_ticks`)
                // — отменено: если тик N недосчитан, а `detect_pass` тика
                // N+1 запускается поверх этого недосчитанного состояния,
                // N+1 вычисляется НЕ по правилам над определённым
                // состоянием решётки, а над артефактом гонки, которого в
                // модели существовать не должно (аксиома 2 — вычисление
                // только через правила, применённые к ОПРЕДЕЛЁННОМУ
                // состоянию). Единственный корректный момент для добора —
                // ДО того, как следующий тик увидит эту решётку, то есть
                // прямо здесь, а не "когда-нибудь, когда кто-то прочитает
                // решётку". Цена (см. doc-комментарий модуля) — известная,
                // измеренная, принятая: тик не считается завершённым,
                // пока решётка не в полностью определённом состоянии, и
                // это не опция.
                let pending_count = Self::read_u32(&self.device, &self.queue, &p.counters_buf, &p.pending_readback_buf);
                if pending_count > 0 {
                    // `target_buf` — буфер, в который шейдер только что
                    // писал как в "next" (см. `bg` выше) — `current_is_a`
                    // здесь ещё ДО переворота в конце функции.
                    let target_buf = if self.current_is_a { &self.buf_b } else { &self.buf_a };
                    Self::cpu_fallback_resolve(
                        &self.device, &self.queue,
                        &p.matches_buf, &p.match_state_buf, &p.locked_buf,
                        &p.matches_readback_buf, &p.state_readback_buf, &p.locked_readback_buf,
                        target_buf, self.width, self.height, self.margin, self.generation + 1,
                    );
                }
            }
        }

        self.generation += 1;
        self.current_is_a = !self.current_is_a;
    }

    /// Блокирующий readback ОДНОГО u32 — часть проверки "остались ли
    /// PENDING-матчи" после раундов арбитража, см. `dispatch_tick`. Эта
    /// проверка происходит БЕЗУСЛОВНО каждый тик, поэтому `readback` —
    /// ПЕРЕИСПОЛЬЗУЕМЫЙ буфер (`ArbitratedPipeline::pending_readback_buf`),
    /// а не создаваемый заново на каждый вызов: `device.create_buffer` на
    /// каждый тик — фиксированные накладные расходы, которые платятся
    /// ВСЕГДА, включая решётки, слишком маленькие, чтобы round-loop вообще
    /// был узким местом (см. плоские числа при N=20/50 в
    /// `flagship_shifts.rs`, не зависящие от ROUNDS). `unmap()` в конце
    /// ОБЯЗАТЕЛЕН — иначе следующий тик не сможет использовать этот же
    /// буфер как цель `copy_buffer_to_buffer` (wgpu запрещает GPU-операции
    /// над замапленным буфером).
    fn read_u32(device: &wgpu::Device, queue: &wgpu::Queue, src: &wgpu::Buffer, readback: &wgpu::Buffer) -> u32 {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_buffer_to_buffer(src, 0, readback, 0, 4);
        queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            tx.send(r).expect("read_u32: readback receiver dropped before map_async completed");
        });
        device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .expect("read_u32: map_async callback never fired")
            .expect("read_u32: GPU buffer mapping failed");

        let value = {
            let mapped = slice.get_mapped_range();
            bytemuck::cast_slice::<u8, u32>(&mapped)[0]
        };
        readback.unmap();
        value
    }

    /// Редкий путь (см. doc-комментарий `dispatch_tick` и `shader.wgsl` про
    /// `count_pending`): CPU досчитывает матчи, которые GPU не успел
    /// разрешить за [`ROUNDS`] раундов. Жадный сортировочный алгоритм,
    /// зеркалящий `arbitrator::arbitrate` (тот же тай-брейк: priority →
    /// age → id → x → y → rule_idx), с уже-занятыми GPU ячейками
    /// (`locked`, читается ОДИН РАЗ здесь) как стартовым "used"-множеством
    /// — корректно ПОТОМУ, что GPU-принятые матчи по построению алгоритма
    /// (см. `shader.wgsl::claim_pass`/`resolve_pass`) уже ранжированы
    /// ВЫШЕ любого оставшегося PENDING на любой общей ячейке (иначе они
    /// сами остались бы PENDING) — значит для PENDING-матчей их не нужно
    /// заново сравнивать по тай-брейку, только считать "навсегда занято".
    /// Точное совпадение результата с ПОЛНЫМ CPU-эталоном для цепочек
    /// длиной от 10 до 1000 проверено в scratch-прототипе `hybrid_check.rs`.
    /// `target_born_at` — итоговое значение `born_at` для КАЖДОЙ ячейки,
    /// добираемой этим вызовом (тот же принцип, что и `params.generation + 1u`
    /// в `shader.wgsl`: "поколение ПОСЛЕ инкремента тика" — вызывающая
    /// сторона обязана передать уже готовое значение, эта функция сама
    /// ничего не прибавляет).
    #[allow(clippy::too_many_arguments)]
    fn cpu_fallback_resolve(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        matches_buf: &wgpu::Buffer,
        match_state_buf: &wgpu::Buffer,
        locked_buf: &wgpu::Buffer,
        rb_matches: &wgpu::Buffer,
        rb_state: &wgpu::Buffer,
        rb_locked: &wgpu::Buffer,
        target_buf: &wgpu::Buffer,
        width: u32,
        height: u32,
        margin: u32,
        target_born_at: u32,
    ) {
        // Полный readback matches/match_state/locked — оправдан ТОЛЬКО в
        // этом редком пути; дешёвый `read_u32` перед вызовом — это то, что
        // решает, входить ли сюда вообще (см. `dispatch_tick`). `rb_matches`/
        // `rb_state`/`rb_locked` — ПЕРЕИСПОЛЬЗУЕМЫЕ буферы
        // (`ArbitratedPipeline::matches_readback_buf` и т.д.), не создаются
        // здесь заново — тот же принцип, что и `read_u32`'s `readback`.
        let matches_bytes = matches_buf.size();
        let state_bytes = match_state_buf.size();
        let locked_bytes = locked_buf.size();
        let n_matches = (state_bytes / 4) as usize;

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_buffer_to_buffer(matches_buf, 0, rb_matches, 0, matches_bytes);
        encoder.copy_buffer_to_buffer(match_state_buf, 0, rb_state, 0, state_bytes);
        encoder.copy_buffer_to_buffer(locked_buf, 0, rb_locked, 0, locked_bytes);
        queue.submit(Some(encoder.finish()));

        let s_matches = rb_matches.slice(..);
        let s_state = rb_state.slice(..);
        let s_locked = rb_locked.slice(..);
        let (tx1, rx1) = std::sync::mpsc::channel();
        let (tx2, rx2) = std::sync::mpsc::channel();
        let (tx3, rx3) = std::sync::mpsc::channel();
        s_matches.map_async(wgpu::MapMode::Read, move |r| { tx1.send(r).expect("cpu_fallback_resolve: matches receiver dropped"); });
        s_state.map_async(wgpu::MapMode::Read, move |r| { tx2.send(r).expect("cpu_fallback_resolve: state receiver dropped"); });
        s_locked.map_async(wgpu::MapMode::Read, move |r| { tx3.send(r).expect("cpu_fallback_resolve: locked receiver dropped"); });
        device.poll(wgpu::Maintain::Wait);
        rx1.recv().expect("cpu_fallback_resolve: matches map_async never fired").expect("cpu_fallback_resolve: matches mapping failed");
        rx2.recv().expect("cpu_fallback_resolve: state map_async never fired").expect("cpu_fallback_resolve: state mapping failed");
        rx3.recv().expect("cpu_fallback_resolve: locked map_async never fired").expect("cpu_fallback_resolve: locked mapping failed");

        let matches_mapped = s_matches.get_mapped_range();
        let matches: &[GpuMatchLayout] = bytemuck::cast_slice(&matches_mapped);
        let state_mapped = s_state.get_mapped_range();
        let states: &[u32] = bytemuck::cast_slice(&state_mapped);
        let locked_mapped = s_locked.get_mapped_range();
        let mut locked: Vec<u32> = bytemuck::cast_slice(&locked_mapped).to_vec();

        // Тот же тай-брейк, что и `arbitrator::arbitrate`/`shader.wgsl::match_is_better`
        // (priority → age → tie_break → id → x → y → rule_idx, по убыванию).
        // `tie_break` в `matches[].tie_break` уже ПОВЁРНУТ по generation —
        // сделано один раз в шейдере при записи матча, здесь просто читаем.
        let mut pending: Vec<usize> = (0..n_matches).filter(|&m| states[m] == 0).collect();
        pending.sort_by(|&a, &b| {
            let ma = &matches[a];
            let mb = &matches[b];
            (mb.priority, mb.age, mb.tie_break, mb.id, mb.x, mb.y, mb.rule_idx)
                .cmp(&(ma.priority, ma.age, ma.tie_break, ma.id, ma.x, ma.y, ma.rule_idx))
        });

        let padded_width = width + 2 * margin;
        let mut writes: Vec<(u64, GpuCell)> = Vec::new();

        for &m in &pending {
            let mat = &matches[m];
            let cell_count = (mat.cell_count as usize).min(MAX_WRITE_CELLS);
            let cells = &mat.cells[..cell_count];
            if cells.iter().any(|&c| locked[c as usize] == 1) {
                continue; // конфликт с уже принятым (GPU или CPU-fallback) матчем
            }
            for &c in cells {
                locked[c as usize] = 1;
            }
            for k in 0..cell_count {
                let c = mat.cells[k] as i64;
                let py = c / padded_width as i64;
                let px = c % padded_width as i64;
                let x = px - margin as i64;
                let y = py - margin as i64;
                if x >= 0 && y >= 0 && (x as u32) < width && (y as u32) < height {
                    let real_idx = y as u64 * width as u64 + x as u64;
                    let byte_offset = real_idx * std::mem::size_of::<GpuCell>() as u64;
                    writes.push((byte_offset, GpuCell { value: mat.values[k], born_at: target_born_at }));
                }
            }
        }

        // `matches`/`states`/`locked_mapped` (и их `get_mapped_range()`
        // view'ы `matches_mapped`/`state_mapped`) больше не используются
        // после этой точки — `unmap()` нужен ДО следующего срабатывания
        // гибридного добора, иначе wgpu откажется использовать эти же
        // буферы как цель `copy_buffer_to_buffer` (см. `read_u32`'s
        // аналогичный комментарий).
        drop(matches_mapped);
        drop(state_mapped);
        drop(locked_mapped);
        rb_matches.unmap();
        rb_state.unmap();
        rb_locked.unmap();

        // БЕЗ break на дубликатах внутри одного матча (см. `apply_pass`):
        // `writes` уже в порядке построения `cells[]` в `detect_pass`, и
        // `queue.write_buffer` для того же смещения, вызванный ПОЗЖЕ,
        // естественно перезаписывает более ранний — тот же порядок, что и
        // цикл "без break" в шейдере.
        for (offset, cell) in writes {
            queue.write_buffer(target_buf, offset, bytemuck::bytes_of(&cell));
        }
    }

    /// Блокирующий readback решётки целиком, в порядке `y * width + x`
    /// (см. `shader.wgsl::idx`) — вызывающий код восстанавливает `(x, y)`
    /// сам делением/остатком на `width()`.
    ///
    /// Решётка к этому моменту уже гарантированно в полностью определённом
    /// состоянии — `dispatch_tick` не возвращает управление, пока не
    /// досчитает (на GPU и, если нужно, на CPU) КАЖДЫЙ тик целиком, см.
    /// её doc-комментарий и doc-комментарий модуля.
    pub fn read_grid(&self) -> Vec<Cell> {
        let source = if self.current_is_a { &self.buf_a } else { &self.buf_b };
        let size = (self.n_cells * std::mem::size_of::<GpuCell>()) as u64;
        let readback = &self.grid_readback_buf;
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_buffer_to_buffer(source, 0, readback, 0, size);
        self.queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            tx.send(r).expect("read_grid: readback receiver dropped before map_async completed");
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .expect("read_grid: map_async callback never fired")
            .expect("read_grid: GPU buffer mapping failed");

        let result: Vec<Cell> = {
            let mapped = slice.get_mapped_range();
            let raw: &[GpuCell] = bytemuck::cast_slice(&mapped);
            raw.iter()
                .map(|c| Cell {
                    value: crate::types::CellValue(CellType(c.value as u8)),
                    born_at: c.born_at as u64,
                })
                .collect()
        };
        readback.unmap();
        result
    }

    pub fn width(&self) -> usize {
        self.width as usize
    }

    pub fn height(&self) -> usize {
        self.height as usize
    }
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
