//! `RuleStateStore` — единая точка хранения ВСЕГО персистентного (живущего
//! между тиками) состояния правил: `starvation_counters`/`feedback_counters`/
//! `memory_buffers`/`activation_counters`. Раньше это были четыре отдельных
//! поля прямо на `Engine`, каждое отдельно продёргиваемое через сигнатуру
//! `run_tick_with_cache` — при добавлении `Rule::max_activations` это уже
//! было четвёртым почти дословным повторением одного и того же паттерна.
//!
//! Второй, более важный мотив — формализовать в типах дисциплину снимка
//! тика (см. специф. §2.2.1): Detect/Arbitrate обязаны видеть эти карты
//! КАК ОНИ БЫЛИ на начало тика, запись — только после. Раньше это держалось
//! ТОЛЬКО дисциплиной программиста (все карты — простые `&mut FxHashMap`,
//! ничто не мешало прочитать уже гдe-то обновлённое значение) — именно так
//! родился реальный баг с `feedback_counters` (см.
//! `project_feedback_counter_timing_semantics_2026_08_08` в памяти сессии):
//! Arbitrate и Apply читали счётчик в РАЗНЫЕ моменты относительно его же
//! инкремента, и ничто в типах об этом не сигнализировало.
//!
//! `snapshot()`/`mutate()` — не просто разные имена одного и того же:
//! `RuleStateSnapshot` держит `&RuleStateStore` (разделяемое заимствование),
//! `RuleStateWriter` — `&mut RuleStateStore` (эксклюзивное). Borrow checker
//! Rust'а физически не даёт получить `mutate()`, пока где-то ещё жив
//! `snapshot()` — попытка написать код, который читает через снимок и тут же
//! (по ошибке) видит через него же уже изменённое значение, не
//! скомпилируется, а не "полагается, что кто-то не ошибётся".
//!
//! Одно осознанное исключение (см. `applicator::apply_rule_buffered`,
//! "Фаза 1"): для ПОБЕДИВШЕГО матча с `Rule::feedback` чтение счётчика (для
//! `feedback_override`) и его инкремент — единая атомарная операция, обязана
//! произойти ДО релокации записи при сдвиге. Разделять её на снимок и
//! писателя было бы искусственным и опасным (легко забыть релоцировать
//! после инкремента) — это ЕДИНСТВЕННОЕ место, где чтение идёт через
//! `RuleStateWriter` целиком, а не через `RuleStateSnapshot`.

use crate::fast_hash::FxHashSet;
use crate::grid::Grid;
use crate::storage::GridStorage;
use crate::types::{CellType, Rule};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::arbitrator::{ActivationCounters, FeedbackCounters, MemoryBuffers, StarvationCounters};

/// См. doc-комментарий модуля. `Clone`/`Serialize`/`Deserialize` — для
/// [`super::EngineSnapshot`] (сохранение/восстановление состояния
/// симуляции, см. специф.).
#[derive(Default, Clone, Serialize, Deserialize)]
pub(crate) struct RuleStateStore {
    starvation_counters: StarvationCounters,
    feedback_counters: FeedbackCounters,
    memory_buffers: MemoryBuffers,
    activation_counters: ActivationCounters,
    /// Снимок `rule_index` на момент последнего вызова `invalidate_stale` —
    /// см. её doc-комментарий. Раньше жил на `Engine` как
    /// `last_rebuilt_rule_index`; перенесён сюда вместе с остальным
    /// персистентным состоянием правил, которое он и защищает.
    last_rebuilt_rule_index: HashMap<CellType, Vec<Rule>>,
}

impl RuleStateStore {
    /// Только-чтение доступ — для Detect (гейт-фильтры) и Arbitrate
    /// (`resolve_sort_fields`). См. doc-комментарий модуля про то, почему
    /// это не просто соглашение об именовании.
    pub(crate) fn snapshot(&self) -> RuleStateSnapshot<'_> {
        RuleStateSnapshot { store: self }
    }

    /// Доступ на запись — для пост-арбитражных обновлений счётчиков И для
    /// чистки осиротевших записей (та, что технически происходит ДО
    /// арбитража, но пишет только в ключи, которые в этом тике никто не
    /// читает — см. `run_tick_with_cache`'s комментарий на месте вызова).
    pub(crate) fn mutate(&mut self) -> RuleStateWriter<'_> {
        RuleStateWriter { store: self }
    }

    /// Сравнить текущий `rule_index` со снимком на момент прошлого вызова и
    /// почистить все четыре карты для любых `(head, rule_idx)`, на которых
    /// теперь сидит ДРУГОЕ правило, чем раньше — `rule_idx` это ПОЗИЦИЯ в
    /// списке, не стабильный id (см. §3.4 спецификации), так что при
    /// изменении состава правил головы (self-mod или прямая правка
    /// `rule_index`) новое правило может занять `rule_idx`, ранее
    /// принадлежавший другому, уже удалённому правилу — без этой чистки оно
    /// молча наследует его счётчик. Полная история и обоснование — см.
    /// doc-комментарий у бывшего `Engine::last_rebuilt_rule_index`
    /// (сохранена в памяти сессии, `project_session_2026_08_09_status`).
    ///
    /// Ранний выход при полном равенстве снимков — не "dirty-флаг", а сам
    /// факт сравнения: HashMap's `PartialEq` — та же O(число правил)
    /// стоимость, что и сам дифф ниже, так что отдельный флаг не ускорил бы
    /// именно эту часть; экономит он проходы по картам счётчиков
    /// (потенциально много больше числа правил), которые без проверки
    /// выполнялись бы даже когда набор правил не менялся вовсе.
    pub(crate) fn invalidate_stale<S: GridStorage>(&mut self, new_rule_index: &HashMap<CellType, Vec<Rule>>, grid: &Grid<S>) {
        if self.last_rebuilt_rule_index == *new_rule_index {
            return;
        }
        let heads: FxHashSet<CellType> = self.last_rebuilt_rule_index.keys().chain(new_rule_index.keys()).copied().collect();
        for head in heads {
            let old = self.last_rebuilt_rule_index.get(&head);
            let new = new_rule_index.get(&head);
            let len = old.map_or(0, |o| o.len()).max(new.map_or(0, |n| n.len()));
            let stale: FxHashSet<usize> = (0..len).filter(|&i| old.and_then(|o| o.get(i)) != new.and_then(|n| n.get(i))).collect();
            if stale.is_empty() {
                continue;
            }
            let matches_head = |x: u32, y: u32| grid.get_cell(x as usize, y as usize).map(|c| c.value.0) == Some(head);
            self.feedback_counters.retain(|&(x, y, idx), _| !stale.contains(&idx) || !matches_head(x, y));
            self.memory_buffers.retain(|&(x, y, idx), _| !stale.contains(&idx) || !matches_head(x, y));
            self.starvation_counters.retain(|&(x, y, idx), _| !stale.contains(&idx) || !matches_head(x, y));
            // `activation_counters` ключуется `(head, rule_idx)` БЕЗ позиции
            // -- не нужен grid-лукап, `head` уже прямо в ключе.
            self.activation_counters.retain(|&(h, idx), _| h != head || !stale.contains(&idx));
        }
        self.last_rebuilt_rule_index = new_rule_index.clone();
    }
}

pub(crate) struct RuleStateSnapshot<'a> {
    store: &'a RuleStateStore,
}

impl<'a> RuleStateSnapshot<'a> {
    pub(crate) fn starvation_counters(&self) -> &'a StarvationCounters {
        &self.store.starvation_counters
    }
    pub(crate) fn feedback_counters(&self) -> &'a FeedbackCounters {
        &self.store.feedback_counters
    }
    pub(crate) fn memory_buffers(&self) -> &'a MemoryBuffers {
        &self.store.memory_buffers
    }
    pub(crate) fn activation_counters(&self) -> &'a ActivationCounters {
        &self.store.activation_counters
    }
}

pub(crate) struct RuleStateWriter<'a> {
    store: &'a mut RuleStateStore,
}

impl<'a> RuleStateWriter<'a> {
    pub(crate) fn starvation_counters_mut(&mut self) -> &mut StarvationCounters {
        &mut self.store.starvation_counters
    }
    pub(crate) fn feedback_counters_mut(&mut self) -> &mut FeedbackCounters {
        &mut self.store.feedback_counters
    }
    pub(crate) fn memory_buffers_mut(&mut self) -> &mut MemoryBuffers {
        &mut self.store.memory_buffers
    }
    pub(crate) fn activation_counters_mut(&mut self) -> &mut ActivationCounters {
        &mut self.store.activation_counters
    }

    /// Обе карты сразу, как непересекающиеся заимствования ОДНОГО `self.store`
    /// -- нужно там, где вызывающему коду требуются `&mut` на ОБЕ карты
    /// ОДНОВРЕМЕННО в одном вызове (`apply_matches_with_cam`, единственное
    /// исключение из snapshot/writer-дисциплины, см. doc-комментарий модуля):
    /// два отдельных вызова `feedback_counters_mut()`/`memory_buffers_mut()`
    /// как отдельных аргументов ОДНОГО выражения не проходят borrow checker
    /// (два "повторных" заимствования `&mut self` через один и тот же
    /// метод-ресивер, даже в разные поля — Rust не смотрит внутрь тел
    /// методов, чтобы увидеть, что поля разные), а прямая проекция полей
    /// внутри ОДНОГО метода — проходит.
    pub(crate) fn feedback_and_memory_mut(&mut self) -> (&mut FeedbackCounters, &mut MemoryBuffers) {
        (&mut self.store.feedback_counters, &mut self.store.memory_buffers)
    }
}
