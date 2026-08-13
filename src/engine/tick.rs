//! Полный стейтфул-пайплайн тика: `run_tick`/`run_tick_profiled`/
//! `run_tick_with_cross_layer_filter` + поглощение самомодификации
//! (`absorb_self_modifications`). Сама механика тика — `pipeline.rs`.

use super::*;

impl<S: GridStorage> Engine<S> {
    /// Выполнить один тик симуляции.
    ///
    /// В отличие от свободной функции `run_tick`, использует уже
    /// закэшированные `self.rule_cache`/`self.group_cache` вместо пересборки
    /// на каждый вызов — `Engine` хранит их именно для переиспользования
    /// между тиками (как уже делают `Engine::arbitrate`/`Engine::apply_matches`/
    /// `Engine::detect_matches`). Валиден, пока `rule_index` не менялся
    /// снаружи после `Engine::new` — если правила были изменены напрямую
    /// (в обход RuleStore), нужно вызвать [`Engine::rebuild_rule_cache`]
    /// перед следующим тиком.
    pub fn run_tick(&mut self) -> (Vec<RuleMatch>, Vec<(u32, Cell)>) {
        let conflict_ctx = ConflictContext {
            partners: &self.conflict_partners,
            max_radius: self.max_affected_radius,
        };
        let tick = self.grid.generation();
        let mut counts = self.tick_log.is_some().then(TickEventCounts::default);
        let result = run_tick_with_cache(
            &mut self.grid,
            &self.rule_index,
            &self.rule_cache,
            &self.group_cache,
            &self.search_radius_cache,
            &self.extension_flags,
            Some(&conflict_ctx),
            &mut self.state,
            None,
            counts.as_mut(),
            &mut self.write_buffer,
            &mut self.pattern_buffer,
            None,
        );
        if let (Some(log), Some(c)) = (self.tick_log.as_mut(), counts) {
            log.push(TickLogEntry {
                tick,
                accepted: c.accepted,
                rejected: c.rejected,
                starvation_events: c.starvation_events,
                feedback_events: c.feedback_events,
            });
        }
        self.absorb_self_modifications();
        result
    }

    /// Как [`Engine::run_tick`], но с дополнительным гейтом на найденные
    /// матчи ПЕРЕД тем, как они пойдут в memory/max_activations-гейты,
    /// starvation/feedback-учёт и сам арбитраж — единственный потребитель
    /// сейчас: `LayeredEngine` (см. её doc-комментарий), фильтрующий по
    /// `Rule::cross_layer_reads`. `pub(crate)`, не публичный API — сигнатура
    /// с `&dyn Fn` внутри пакета, наружу выходит только `LayeredEngine`,
    /// уже не как "движок с колбэком".
    pub(crate) fn run_tick_with_cross_layer_filter(
        &mut self,
        filter: &dyn Fn(&RuleMatch) -> bool,
    ) -> (Vec<RuleMatch>, Vec<(u32, Cell)>) {
        let conflict_ctx = ConflictContext {
            partners: &self.conflict_partners,
            max_radius: self.max_affected_radius,
        };
        let tick = self.grid.generation();
        let mut counts = self.tick_log.is_some().then(TickEventCounts::default);
        let result = run_tick_with_cache(
            &mut self.grid,
            &self.rule_index,
            &self.rule_cache,
            &self.group_cache,
            &self.search_radius_cache,
            &self.extension_flags,
            Some(&conflict_ctx),
            &mut self.state,
            None,
            counts.as_mut(),
            &mut self.write_buffer,
            &mut self.pattern_buffer,
            Some(filter),
        );
        if let (Some(log), Some(c)) = (self.tick_log.as_mut(), counts) {
            log.push(TickLogEntry {
                tick,
                accepted: c.accepted,
                rejected: c.rejected,
                starvation_events: c.starvation_events,
                feedback_events: c.feedback_events,
            });
        }
        self.absorb_self_modifications();
        result
    }

    /// Как [`Engine::run_tick`], но дополнительно возвращает разбивку этого
    /// тика по фазам ([`TickPhaseTimings`]) — для профилирования "где узкое
    /// место у конкретного конфига", без общего времени всего тика как
    /// единственного числа. `Instant::now()` вызывается ТОЛЬКО этим путём
    /// (`run_tick` использует ветку `None`, см. `mark_phase!` в
    /// `run_tick_with_cache`) — нулевая цена для кода, который об этом не
    /// просил.
    pub fn run_tick_profiled(&mut self) -> (Vec<RuleMatch>, Vec<(u32, Cell)>, TickPhaseTimings) {
        let conflict_ctx = ConflictContext {
            partners: &self.conflict_partners,
            max_radius: self.max_affected_radius,
        };
        let mut timings = TickPhaseTimings::default();
        let result = run_tick_with_cache(
            &mut self.grid,
            &self.rule_index,
            &self.rule_cache,
            &self.group_cache,
            &self.search_radius_cache,
            &self.extension_flags,
            Some(&conflict_ctx),
            &mut self.state,
            Some(&mut timings),
            None,
            &mut self.write_buffer,
            &mut self.pattern_buffer,
            None,
        );
        self.absorb_self_modifications();
        (result.0, result.1, timings)
    }

    /// Если самомодификация включена — дренировать канал 0 выходных
    /// граничных буферов, применить готовые операции к `rule_index` и,
    /// только если что-то реально изменилось, перестроить кэши. Отдельный
    /// метод, а не код прямо в `run_tick`, только чтобы не загромождать его —
    /// логически это часть одного тика.
    fn absorb_self_modifications(&mut self) {
        let Some(mut rule_store) = self.self_mod.take() else {
            return;
        };
        let ops = rule_store.drain_rule_channel(&mut self.grid);
        let mut changed = false;
        for op in ops {
            if self.guard_self_modification && !self.composition_allows(&op, &mut rule_store) {
                self.rejected_self_modifications += 1;
                continue;
            }
            if rule_store.apply(op) {
                changed = true;
            }
        }
        if changed {
            let self_mod_index = rule_store.get_index().clone();
            let current: FxHashSet<CellType> = self_mod_index.keys().copied().collect();
            // Каждая голова, задействованная либо изначально (rule_index на
            // момент Engine::new), либо самомодификацией (сейчас или раньше),
            // пересобирается заново как "оригинал (если был) ++ то, что
            // сейчас реально числится за RuleStore" — а не просто
            // ЗАМЕНЯЕТСЯ содержимым get_index(). Без этого самопереданное
            // ДОПОЛНИТЕЛЬНОЕ правило на уже существующую голову молча стирало
            // оригинал (get_index() ничего не знает о правилах, заведённых в
            // обход RuleStore), а удаление последнего самопереданного
            // правила у головы оставляло висеть устаревшую копию вместо
            // возврата к оригиналу.
            for head in self.self_mod_managed_heads.union(&current).copied().collect::<Vec<_>>() {
                let mut merged = self.original_rule_index.get(&head).cloned().unwrap_or_default();
                if let Some(rules) = self_mod_index.get(&head) {
                    merged.extend(rules.iter().cloned());
                }
                if merged.is_empty() {
                    self.rule_index.remove(&head);
                } else {
                    self.rule_index.insert(head, merged);
                }
            }
            self.self_mod_managed_heads = current;
            self.rebuild_rule_cache();
        }
        self.self_mod = Some(rule_store);
    }

    /// Часть [`Engine::enable_guarded_self_modification`]: пропускает всё,
    /// кроме `AddRule` с id, которого ЕЩЁ НЕТ среди того, чем реально
    /// владеет самомодификация (расширение своего же модуля не
    /// проверяется — см. doc-комментарий поля `guard_self_modification`).
    /// Для действительно нового или ЧУЖОГО (protected) id — честная
    /// проверка композиции против ОСТАЛЬНОГО текущего набора правил.
    ///
    /// Принимает `rule_store` живым (`&mut`, не `&self.self_mod`), а не
    /// читает `self.rule_index` — потому что `rule_index` обновляется
    /// ОДИН РАЗ в конце всего пакета операций (`absorb_self_modifications`),
    /// а `rule_store` отражает КАЖДУЮ уже принятую операцию немедленно.
    /// Если в одном тике декодируются ДВЕ посылки, конфликтующие ДРУГ С
    /// ДРУГОМ (а не с чем-то предсуществующим), проверка по `rule_index`
    /// не увидела бы первую при проверке второй (обе прошли бы, каждая
    /// как будто в одиночестве) — проверка по `rule_store` видит.
    fn composition_allows(
        &self,
        op: &crate::rule_store::CompletedOp,
        rule_store: &mut crate::rule_store::RuleStore,
    ) -> bool {
        let crate::rule_store::RuleOp::AddRule(rule) = &op.op else {
            return true;
        };
        let Some(&head) = rule.id.first() else { return true };
        let self_mod_index = rule_store.get_index().clone();
        // Расширение id, которым самомодификация уже владеет (сейчас, в том
        // числе принятое РАНЕЕ В ЭТОМ ЖЕ пакете операций) — не проверяем,
        // ЕСЛИ это не чужая (protected) территория: однажды доказанная
        // совместимость одного самопереданного правила с оригиналом головы
        // не означает, что и ВТОРОЕ самопереданное правило туда же тоже
        // совместимо с оригиналом — каждая заявка на protected-голову
        // проверяется свежо.
        if self_mod_index.contains_key(&head) && !self.original_rule_index.contains_key(&head) {
            return true;
        }
        let mut existing: Vec<Rule> = self.original_rule_index.values().flatten().cloned().collect();
        existing.extend(self_mod_index.values().flatten().cloned());
        // `check_composition` returns `Unsafe` for ANY conflict in the
        // combined graph, including a SELF-loop of the new rule alone (e.g.
        // an ordinary shift rule that could collide with another instance
        // of itself at an adjacent position — the same "moving object"
        // limitation as everywhere else in this project, nothing to do
        // with colliding with existing modules). `Unsafe`'s pair list is
        // empty in exactly that case (self-loops never satisfy the
        // cross-set `i < n_a && j >= n_a` filter) — reject only when there
        // is an ACTUAL rule_a×existing pair, not merely a non-empty verdict.
        let grid_ctx = crate::conflict_analyzer::GridContext {
            width: self.grid.width(),
            height: self.grid.height(),
            boundaries: &self.grid.boundaries,
        };
        match crate::ConflictGraph::check_composition_with_grid(std::slice::from_ref(rule), &existing, &grid_ctx) {
            crate::conflict_analyzer::CompositionVerdict::Safe => true,
            crate::conflict_analyzer::CompositionVerdict::Unsafe(pairs) => pairs.is_empty(),
        }
    }
}
