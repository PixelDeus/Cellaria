//! Рантайм-исполнение: `run_tick`/`run_ticks`/`dispatch_tick`,
//! `cpu_fallback_resolve` (гибридный добор), `read_grid`.

use super::*;

impl GpuEngine {
    /// Один тик. Возвращается ТОЛЬКО когда решётка полностью в
    /// определённом состоянии — см. doc-комментарий модуля: Simple — один
    /// dispatch, без readback (арбитраж вообще не нужен); Arbitrated —
    /// `ROUNDS` раундов в одном submission БЕЗ readback, ПЛЮС
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
        self.queue
            .write_buffer(&self.params_buf, 0, bytemuck::bytes_of(&params));

        let wg_grid_x = self.width.div_ceil(16);
        let wg_grid_y = self.height.div_ceil(16);

        match &self.pipeline {
            Pipeline::Simple(p) => {
                let bg = if self.current_is_a { &p.bg_a_to_b } else { &p.bg_b_to_a };
                let mut encoder = self
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: None,
                        timestamp_writes: None,
                    });
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
                let mut encoder = self
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: None,
                        timestamp_writes: None,
                    });
                    pass.set_bind_group(0, bg, &[]);

                    pass.set_pipeline(&p.p_clear_locked);
                    pass.dispatch_workgroups(wg_padded_cells, 1, 1);
                    pass.set_pipeline(&p.p_detect);
                    pass.dispatch_workgroups(wg_matches, 1, 1);

                    for _ in 0..self.rounds {
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
                        &self.device,
                        &self.queue,
                        &p.matches_buf,
                        &p.match_state_buf,
                        &p.locked_buf,
                        &p.matches_readback_buf,
                        &p.state_readback_buf,
                        &p.locked_readback_buf,
                        target_buf,
                        self.width,
                        self.height,
                        self.margin,
                        self.generation + 1,
                    );
                }

                // `update_starvation_pass` — см. её doc-комментарий в
                // `shader.wgsl` про то, почему ОБЯЗАНА идти ПОСЛЕ (не внутри)
                // предыдущего submission'а: ей нужен УЖЕ финальный
                // `match_state`, который `cpu_fallback_resolve` выше могла
                // только что дописать (редкий путь). Пропускается целиком,
                // если конфиг не использует `starvation_after` вовсе — то же
                // "нулевые накладные расходы, если не просили" (см.
                // `GpuRuleTable::needs_starvation`'s doc-комментарий), что
                // уже применено к CPU-side `ExtensionFlags`.
                if self.needs_starvation {
                    let mut encoder2 = self
                        .device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                    {
                        let mut pass2 = encoder2.begin_compute_pass(&wgpu::ComputePassDescriptor {
                            label: None,
                            timestamp_writes: None,
                        });
                        pass2.set_bind_group(0, bg, &[]);
                        pass2.set_pipeline(&p.p_update_starvation);
                        pass2.dispatch_workgroups(wg_matches, 1, 1);
                    }
                    self.queue.submit(Some(encoder2.finish()));
                }

                // `update_feedback_{latch,relocate}_pass` — та же причина
                // (нужен финальный `match_state`) идти ПОСЛЕ CPU-fallback,
                // что и у `update_starvation_pass` выше; независима от неё
                // (разные буферы), порядок между ними друг на друга не
                // влияет. Латч и перенос — ДВА dispatch'а В ОДНОМ pass'е
                // (не отдельные submission'ы) специально: ordering между
                // dispatch_workgroups внутри одного compute pass'а —
                // ГАРАНТИЯ WebGPU, на которой уже держится весь
                // раундовый claim/resolve выше, и она же убирает гонку
                // между переносом и осиротевшим сбросом (см. doc-комментарий
                // `update_feedback_relocate_pass` в `shader.wgsl`).
                if self.needs_feedback {
                    let mut encoder3 = self
                        .device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                    {
                        let mut pass3 = encoder3.begin_compute_pass(&wgpu::ComputePassDescriptor {
                            label: None,
                            timestamp_writes: None,
                        });
                        pass3.set_bind_group(0, bg, &[]);
                        pass3.set_pipeline(&p.p_update_feedback_latch);
                        pass3.dispatch_workgroups(wg_matches, 1, 1);
                        pass3.set_pipeline(&p.p_update_feedback_relocate);
                        pass3.dispatch_workgroups(wg_matches, 1, 1);
                    }
                    self.queue.submit(Some(encoder3.finish()));
                }

                // `update_memory_{push,relocate}_pass` — та же причина
                // (финальный `match_state`) идти после CPU-fallback, и та
                // же "два dispatch'а в одном pass'е" ordering-гарантия
                // (push → relocate), что и у `update_feedback_*` выше —
                // независима от неё (разные буферы), порядок между блоками
                // друг на друга не влияет.
                if self.needs_memory {
                    let mut encoder4 = self
                        .device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                    {
                        let mut pass4 = encoder4.begin_compute_pass(&wgpu::ComputePassDescriptor {
                            label: None,
                            timestamp_writes: None,
                        });
                        pass4.set_bind_group(0, bg, &[]);
                        pass4.set_pipeline(&p.p_update_memory_push);
                        pass4.dispatch_workgroups(wg_matches, 1, 1);
                        pass4.set_pipeline(&p.p_update_memory_relocate);
                        pass4.dispatch_workgroups(wg_matches, 1, 1);
                    }
                    self.queue.submit(Some(encoder4.finish()));
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
            tx.send(r)
                .expect("read_u32: readback receiver dropped before map_async completed");
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
        s_matches.map_async(wgpu::MapMode::Read, move |r| {
            tx1.send(r).expect("cpu_fallback_resolve: matches receiver dropped");
        });
        s_state.map_async(wgpu::MapMode::Read, move |r| {
            tx2.send(r).expect("cpu_fallback_resolve: state receiver dropped");
        });
        s_locked.map_async(wgpu::MapMode::Read, move |r| {
            tx3.send(r).expect("cpu_fallback_resolve: locked receiver dropped");
        });
        device.poll(wgpu::Maintain::Wait);
        rx1.recv()
            .expect("cpu_fallback_resolve: matches map_async never fired")
            .expect("cpu_fallback_resolve: matches mapping failed");
        rx2.recv()
            .expect("cpu_fallback_resolve: state map_async never fired")
            .expect("cpu_fallback_resolve: state mapping failed");
        rx3.recv()
            .expect("cpu_fallback_resolve: locked map_async never fired")
            .expect("cpu_fallback_resolve: locked mapping failed");

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
            (mb.priority, mb.age, mb.tie_break, mb.id, mb.x, mb.y, mb.rule_idx).cmp(&(
                ma.priority,
                ma.age,
                ma.tie_break,
                ma.id,
                ma.x,
                ma.y,
                ma.rule_idx,
            ))
        });

        let padded_width = width + 2 * margin;
        let mut writes: Vec<(u64, GpuCell)> = Vec::new();
        // Финальный ACCEPTED(1)/REJECTED(2) для КАЖДОГО матча, доигранного
        // здесь — раньше эта функция писала только клетки решётки
        // (`writes`), оставляя `match_state_buf` навсегда PENDING(0) для
        // этих матчей. Теперь пишет и то, и другое: `update_starvation_pass`
        // (см. `shader.wgsl`) — первый и единственный потребитель, которому
        // это действительно нужно (см. её doc-комментарий про то, почему
        // `starvation_after` вообще стал портируемым).
        let mut state_writes: Vec<(u64, u32)> = Vec::new();

        for &m in &pending {
            let mat = &matches[m];
            let cell_count = (mat.cell_count as usize).min(MAX_WRITE_CELLS);
            let cells = &mat.cells[..cell_count];
            let state_offset = (m * 4) as u64;
            if cells.iter().any(|&c| locked[c as usize] == 1) {
                state_writes.push((state_offset, 2)); // REJECTED — конфликт с уже принятым матчем
                continue;
            }
            for &c in cells {
                locked[c as usize] = 1;
            }
            state_writes.push((state_offset, 1)); // ACCEPTED
                                                  // ИЗВЕСТНОЕ, задокументированное ограничение (не забытый баг):
                                                  // `mat.keep_age_mask` (см. её doc-комментарий у `GpuMatchLayout` и
                                                  // `apply_pass`'s использование в `shader.wgsl`) здесь НЕ
                                                  // учитывается — каждая клетка получает `target_born_at`
                                                  // безусловно, даже если это клетка-источник `keep_source`-сдвига,
                                                  // которой возраст сбрасываться не должен. В отличие от
                                                  // `apply_pass` (обычный, сходящийся за раунды путь), у этой
                                                  // функции нет дешёвого доступа к ТЕКУЩЕМУ `born_at` целевой
                                                  // клетки без дополнительного readback решётки — а это редкий
                                                  // путь (длинные несходящиеся конфликтные цепочки), для которого
                                                  // цена лишнего readback ради этого узкого случая не оправдана.
                                                  // Практическое следствие: `keep_source`-правило, чей матч попал
                                                  // именно в CPU-добор (не в обычный сходящийся путь), в этом
                                                  // единственном тике получит сброшенный возраст источника —
                                                  // расхождение с CPU, того же класса, что и `starvation_after`+
                                                  // `tie_break` на GPU (см. специф. §13.4) — не исправлено по той
                                                  // же причине: редкий случай, явно задокументирован, а не тихо
                                                  // сломан.
            for k in 0..cell_count {
                let c = mat.cells[k] as i64;
                let py = c / padded_width as i64;
                let px = c % padded_width as i64;
                let x = px - margin as i64;
                let y = py - margin as i64;
                if x >= 0 && y >= 0 && (x as u32) < width && (y as u32) < height {
                    let real_idx = y as u64 * width as u64 + x as u64;
                    let byte_offset = real_idx * std::mem::size_of::<GpuCell>() as u64;
                    writes.push((
                        byte_offset,
                        GpuCell {
                            value: mat.values[k],
                            born_at: target_born_at,
                        },
                    ));
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
        for (offset, state) in state_writes {
            queue.write_buffer(match_state_buf, offset, bytemuck::bytes_of(&state));
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
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_buffer_to_buffer(source, 0, readback, 0, size);
        self.queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            tx.send(r)
                .expect("read_grid: readback receiver dropped before map_async completed");
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
