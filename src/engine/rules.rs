//! Поиск/замена набора правил: `get_priority`/`find_rule`/`rule_index`/
//! `set_rule_index`/`set_rules_for_head`/`rebuild_rule_cache`.

use super::*;

impl<S: GridStorage> Engine<S> {
    // ─── Вспомогательные ───

    pub fn get_priority(&self, rule_id: &[CellType]) -> u32 {
        if let Some(first) = rule_id.first() {
            if let Some(rules) = self.rule_index.get(first) {
                for rule in rules {
                    if rule.id == rule_id {
                        return rule.priority;
                    }
                }
            }
        }
        0
    }

    pub fn find_rule(&self, rule_id: &[CellType]) -> Option<&Rule> {
        if let Some(first) = rule_id.first() {
            if let Some(rules) = self.rule_index.get(first) {
                for rule in rules {
                    if rule.id == rule_id {
                        return Some(rule);
                    }
                }
            }
        }
        None
    }
    /// Пересобрать `rule_cache`/`group_cache` из текущего `rule_index`.
    ///
    /// Нужен только если `rule_index` был изменён напрямую после
    /// `Engine::new` (RuleStore-путь сам работает с отдельным индексом и
    /// передаётся в `Engine` целиком через пересоздание). Без вызова этого
    /// метода после такого изменения `run_tick`/`arbitrate`/`apply_matches`/
    /// `detect_matches` будут использовать устаревшие данные — в лучшем
    /// случае для новых правил, в худшем встретят `rule_idx`, которого ещё
    /// нет в кэше.
    ///
    /// Заодно помечает "грязными" ВСЕ активные клетки. Dirty-tracking следит
    /// за записями в решётку, а не за сменой самого набора правил: клетка,
    /// которая давно не менялась, может теперь подпадать под только что
    /// добавленное правило, но без этого никогда бы не попала в кандидатов
    /// на пересканирование на следующем тике (нашли этот случай на практике —
    /// живое добавление правила иначе просто не срабатывало на "осевших"
    /// клетках). Это может стоить одного лишнего полного прохода по
    /// активному набору сразу после вызова — приемлемая цена за то, что
    /// смена правил на лету всегда честно работает, без необходимости
    /// вызывающему коду знать про это взаимодействие и вручную "трогать"
    /// конкретные клетки.
    pub fn rebuild_rule_cache(&mut self) {
        // Инвалидация переиспользованных `rule_idx` в персистентном
        // состоянии правил — см. `rule_state::RuleStateStore::invalidate_stale`'s
        // doc-комментарий (логика и её обоснование перенесены туда целиком,
        // п.5 сессии 2026-08-09: `RuleStateStore` — единая точка для всего,
        // что касается персистентного состояния правил, не только карт, но
        // и их инвалидации).
        self.state.invalidate_stale(&self.rule_index, &self.grid);
        self.rule_cache = build_rule_data_cache(&self.rule_index);
        self.group_cache = build_group_data(&self.rule_index);
        self.search_radius_cache = compute_search_radius_cache(&self.rule_index);
        self.extension_flags = compute_extension_flags(&self.rule_index);
        let (partners, radius) = compute_conflict_partners(&self.rule_index, &self.rule_cache);
        self.conflict_partners = partners;
        self.max_affected_radius = radius;
        self.resync_original_rule_index();
        let active: Vec<(usize, usize)> = self.grid.active_coords().clone();
        for (x, y) in active {
            self.grid.mark_dirty(x, y);
        }
    }

    /// Только-чтение доступ к текущему набору правил — см. doc-комментарий
    /// поля `rule_index` про то, почему нет прямого мутабельного доступа
    /// извне: единственный способ ИЗМЕНИТЬ состав правил — [`Engine::set_rule_index`]/
    /// [`Engine::set_rules_for_head`], которые сами вызывают
    /// [`Engine::rebuild_rule_cache`], так что забыть — физически нельзя.
    pub fn rule_index(&self) -> &HashMap<CellType, Vec<Rule>> {
        &self.rule_index
    }

    /// Заменить ВЕСЬ набор правил и перестроить кэши — единственный
    /// поддерживаемый способ извне заменить `rule_index` целиком (п.4 сессии
    /// 2026-08-09). Раньше поле было `pub`, и прямая правка `engine.rule_index = ...`
    /// в обход `rebuild_rule_cache()` молча портила кэши/счётчики — этот
    /// метод физически не даёт забыть.
    pub fn set_rule_index(&mut self, new_rule_index: HashMap<CellType, Vec<Rule>>) {
        self.rule_index = new_rule_index;
        self.rebuild_rule_cache();
    }

    /// Заменить список правил ОДНОЙ головы (вставить новую голову, если её
    /// раньше не было) и перестроить кэши — тот же принцип, что и
    /// [`Engine::set_rule_index`], только для точечной правки одной головы
    /// (самый частый случай на практике — см. `strength_live_rules.rs`).
    /// Чтобы заменить/добавить ОДНО конкретное правило внутри уже
    /// существующего списка головы, прочитайте текущий список через
    /// [`Engine::rule_index`], постройте изменённый `Vec<Rule>` и передайте
    /// его сюда целиком — отдельного метода "заменить правило по индексу"
    /// нет специально, чтобы не плодить API на каждый частный случай.
    pub fn set_rules_for_head(&mut self, head: CellType, rules: Vec<Rule>) {
        self.rule_index.insert(head, rules);
        self.rebuild_rule_cache();
    }

    /// Пересчитать `original_rule_index` из ТЕКУЩЕГО `rule_index` — как "то,
    /// что сейчас там стоит, за вычетом того, чем сейчас реально владеет
    /// `RuleStore`". Нужно вызывать при каждой перестройке кэша (не только
    /// один раз при включении самомодификации), иначе прямая правка
    /// `rule_index` ПОСЛЕ того, как самомодификация уже что-то добавила на
    /// лету, либо потеряется при следующем слиянии, либо ошибочно будет
    /// считаться "чужой территорией" навсегда неверно. Ничего не делает,
    /// если самомодификация не включена — `original_rule_index` тогда
    /// просто не используется.
    fn resync_original_rule_index(&mut self) {
        let Some(mut rule_store) = self.self_mod.take() else {
            return;
        };
        let self_mod_index = rule_store.get_index().clone();
        self.self_mod = Some(rule_store);

        let mut resynced: HashMap<CellType, Vec<Rule>> = HashMap::new();
        for (head, rules) in &self.rule_index {
            let self_owned = self_mod_index.get(head);
            let foreign: Vec<Rule> = rules
                .iter()
                .filter(|r| !self_owned.is_some_and(|so| so.contains(r)))
                .cloned()
                .collect();
            if !foreign.is_empty() {
                resynced.insert(*head, foreign);
            }
        }
        self.original_rule_index = resynced;
    }
}
