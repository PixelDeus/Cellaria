//! Жизненный цикл `Engine`: конструирование, снимки, `replay`, включение
//! самомодификации, доступ к решётке.

use super::*;

impl<S: GridStorage> Engine<S> {
    pub fn new(grid: Grid<S>, rule_index: HashMap<CellType, Vec<Rule>>) -> Self {
        let rule_cache = build_rule_data_cache(&rule_index);
        let group_cache = build_group_data(&rule_index);
        let search_radius_cache = compute_search_radius_cache(&rule_index);
        let extension_flags = compute_extension_flags(&rule_index);
        let (conflict_partners, max_affected_radius) = compute_conflict_partners(&rule_index, &rule_cache);
        // Заполняет `RuleStateStore`'s внутренний снимок `rule_index` СРАЗУ,
        // а не оставляет его пустым до первого `rebuild_rule_cache()` — иначе
        // первый же вызов `rebuild_rule_cache()` после нескольких реальных
        // тиков увидел бы ВСЕ текущие правила как "новые" (диф против
        // пустоты) и ошибочно счёл бы уже накопленные счётчики устаревшими.
        // Счётчики здесь ещё пусты (только что созданы), так что сама
        // чистка — no-op, важен только побочный эффект — правильный снимок.
        let mut state = RuleStateStore::default();
        state.invalidate_stale(&rule_index, &grid);
        Self {
            grid,
            rule_index,
            rule_cache,
            group_cache,
            search_radius_cache,
            extension_flags,
            conflict_partners,
            max_affected_radius,
            self_mod: None,
            guard_self_modification: false,
            // Заполняется в `enable_self_modification` — снимок берётся ТАМ,
            // а не здесь, поскольку `rule_index` может законно поменяться
            // напрямую между `Engine::new` и включением самомодификации.
            original_rule_index: HashMap::new(),
            self_mod_managed_heads: FxHashSet::default(),
            rejected_self_modifications: 0,
            state,
            input_log: None,
            tick_log: None,
            write_buffer: WriteBuffer::default(),
            pattern_buffer: Vec::new(),
        }
    }

    /// Снимок текущего состояния для сохранения (см. [`EngineSnapshot`]) —
    /// НЕ потребляет `self`, движок продолжает работать после вызова как ни
    /// в чём не бывало (нужен `S: Clone`, только для этого метода, не для
    /// всего `Engine<S>` — большинству кода клонировать `storage` незачем).
    pub fn snapshot(&self) -> EngineSnapshot<S>
    where
        S: Clone,
    {
        EngineSnapshot {
            grid: self.grid.clone(),
            rule_index: self.rule_index.clone(),
            state: self.state.clone(),
            self_mod: self.self_mod.clone(),
            guard_self_modification: self.guard_self_modification,
            original_rule_index: self.original_rule_index.clone(),
            self_mod_managed_heads: self.self_mod_managed_heads.clone(),
            rejected_self_modifications: self.rejected_self_modifications,
        }
    }

    /// Восстановить движок из снимка ([`Engine::snapshot`]/сериализованного
    /// [`EngineSnapshot`]). Пересобирает ВСЕ кэши заново из `rule_index`
    /// (та же логика, что `Engine::new` — кэши никогда не входят в снимок,
    /// см. её doc-комментарий), затем восстанавливает персистентное
    /// состояние (`state`, самомодификацию) ТОЧНО как было на момент
    /// снимка, а не заново с нуля.
    pub fn from_snapshot(snapshot: EngineSnapshot<S>) -> Self {
        let mut engine = Self::new(snapshot.grid, snapshot.rule_index);
        engine.state = snapshot.state;
        engine.self_mod = snapshot.self_mod;
        engine.guard_self_modification = snapshot.guard_self_modification;
        engine.original_rule_index = snapshot.original_rule_index;
        engine.self_mod_managed_heads = snapshot.self_mod_managed_heads;
        engine.rejected_self_modifications = snapshot.rejected_self_modifications;
        engine
    }

    /// Включить самомодификацию: с этого момента `run_tick` сам, каждый тик,
    /// дренирует канал 0 всех выходных граничных буферов через протокол
    /// `RuleStore`, применяет готовые операции (`AddRule`/`RemoveRule`/
    /// `ClearAll`) к `rule_index` и перестраивает кэши — решётка может сама
    /// писать себе новые правила во время работы, без внешнего кода,
    /// вызывающего `rebuild_rule_cache` вручную после каждой посылки.
    ///
    /// Идемпотентна: повторный вызов просто ничего не делает, если уже
    /// включено (не сбрасывает уже накопленный внутри `RuleStore` буфер).
    pub fn enable_self_modification(&mut self) {
        if self.self_mod.is_none() {
            self.self_mod = Some(crate::rule_store::RuleStore::new());
            self.original_rule_index = self.rule_index.clone();
        }
    }

    /// Как [`Engine::enable_self_modification`], но с проверкой композиции:
    /// самопереданное правило с НОВЫМ id (ещё не встречавшимся в
    /// `rule_index`) устанавливается только если
    /// `ConflictGraph::check_composition` подтверждает, что оно не
    /// конфликтует с остальным текущим набором правил. Небезопасное правило
    /// молча отбрасывается — уже установленные модули (например, независимо
    /// написанный домен, с которым текущий делит решётку) не могут быть
    /// сломаны чужой самомодификацией, даже недобросовестной или ошибочной.
    pub fn enable_guarded_self_modification(&mut self) {
        self.enable_self_modification();
        self.guard_self_modification = true;
    }

    pub fn grid(&self) -> &Grid<S> {
        &self.grid
    }

    pub fn grid_mut(&mut self) -> &mut Grid<S> {
        &mut self.grid
    }
}
