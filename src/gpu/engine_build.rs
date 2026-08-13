//! Построение `GpuEngine`: `new`/`with_rounds` + сборка Simple/Arbitrated
//! wgpu-пайплайнов (`build_simple_pipeline`/`build_arbitrated_pipeline`).

use super::*;

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
        Self::with_rounds(width, height, initial_cells, rule_index, ROUNDS)
    }

    /// Как [`GpuEngine::new`], но с явным числом claim/resolve-раундов на
    /// тик вместо дефолтного `ROUNDS` — см. doc-комментарий поля `rounds`.
    pub fn with_rounds(
        width: usize,
        height: usize,
        initial_cells: &[(usize, usize, Cell)],
        rule_index: &HashMap<CellType, Vec<Rule>>,
        rounds: u32,
    ) -> Result<Self, GpuUnsupportedReason> {
        let table = build_gpu_rule_table(rule_index)?;
        Ok(pollster::block_on(Self::init(
            width,
            height,
            initial_cells,
            &table,
            rounds,
        )))
    }

    async fn init(
        width: usize,
        height: usize,
        initial_cells: &[(usize, usize, Cell)],
        table: &GpuRuleTable,
        rounds: u32,
    ) -> Self {
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
            .request_device(
                &wgpu::DeviceDescriptor {
                    required_limits: adapter.limits(),
                    ..Default::default()
                },
                None,
            )
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
                &device,
                &shader,
                &params_buf,
                &buf_a,
                &buf_b,
                &head_slots_buf,
                &rules_buf,
                &offsets_buf,
                &head_offsets_buf,
                width,
                height,
                n_cells,
                margin,
                max_matches_per_cell,
            ))
        } else {
            Pipeline::Simple(Self::build_simple_pipeline(
                &device,
                &shader,
                &params_buf,
                &buf_a,
                &buf_b,
                &head_slots_buf,
                &rules_buf,
                &offsets_buf,
                &head_offsets_buf,
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
            needs_starvation: table.needs_starvation,
            needs_feedback: table.needs_feedback,
            needs_memory: table.needs_memory,
            rounds,
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
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
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
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: current.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: next.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: head_slots_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: rules_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: offsets_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: head_offsets_buf.as_entire_binding(),
                    },
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

        SimplePipeline {
            bg_a_to_b: mk_bg(buf_a, buf_b),
            bg_b_to_a: mk_bg(buf_b, buf_a),
            pipeline,
        }
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
            // COPY_DST добавлен для `GpuEngine::cpu_fallback_resolve`,
            // которая теперь дописывает финальный ACCEPTED/REJECTED для
            // матчей, доигранных на CPU, обратно сюда (`queue.write_buffer`)
            // — иначе `update_starvation_pass` видел бы их навсегда
            // застрявшими в PENDING(0).
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let starvation_counters_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (n_matches * 4).max(4) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let feedback_counters_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (n_matches * 4).max(4) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let memory_buffers_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (n_matches * MAX_MEMORY_WINDOW * 4).max(4) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let memory_len_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (n_matches * 4).max(4) as u64,
            usage: wgpu::BufferUsages::STORAGE,
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
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
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
                storage_entry(12, false),
                storage_entry(13, false),
                storage_entry(14, false),
                storage_entry(15, false),
            ],
        });

        let mk_bg = |current: &wgpu::Buffer, next: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: current.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: next.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: head_slots_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: rules_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: offsets_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: head_offsets_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: matches_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: match_state_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: claims_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: locked_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 11,
                        resource: counters_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 12,
                        resource: starvation_counters_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 13,
                        resource: feedback_counters_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 14,
                        resource: memory_buffers_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 15,
                        resource: memory_len_buf.as_entire_binding(),
                    },
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
            p_update_starvation: mk_pipeline("update_starvation_pass"),
            p_update_feedback_latch: mk_pipeline("update_feedback_latch_pass"),
            p_update_feedback_relocate: mk_pipeline("update_feedback_relocate_pass"),
            p_update_memory_push: mk_pipeline("update_memory_push_pass"),
            p_update_memory_relocate: mk_pipeline("update_memory_relocate_pass"),
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
            starvation_counters_buf,
            feedback_counters_buf,
            memory_buffers_buf,
            memory_len_buf,
        }
    }
}
