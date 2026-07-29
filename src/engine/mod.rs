pub mod applicator;
pub mod arbitrator;
pub mod matcher;

use std::collections::HashMap;

use crate::conflict_analyzer::build_rule_data_cache;
use crate::fast_hash::{FxHashMap, FxHashSet};
use crate::grid::Grid;
use crate::storage::GridStorage;
use crate::types::{AffectedRegion, Cell, CellType, CellValue, Rule, RuleMatch, DEFAULT_CELL_VALUE};

pub use applicator::apply_matches;
pub use arbitrator::arbitrate;
pub use matcher::detect_matches;
use matcher::{build_group_data, detect_matches_with_group_data, GroupCache};

/// Двигатель симуляции Cellaria.
pub struct Engine<S: GridStorage> {
    pub grid: Grid<S>,
    pub rule_index: HashMap<CellType, Vec<Rule>>,
    pub rule_cache: HashMap<(CellType, usize), crate::conflict_analyzer::RuleData>,
    /// Кэш `GroupData` матчера (эффективные паттерны, offset-карты,
    /// упакованные u128-паттерны) — то же самое соображение, что и у
    /// `rule_cache`: `matcher::detect_matches` (свободная функция)
    /// пересобирает такой кэш заново на каждый вызов, что заметно на
    /// разреженных сценариях с частыми тиками (см. doc-комментарий
    /// `matcher::GroupCache`). `Engine` строит его один раз и переиспользует.
    group_cache: GroupCache,
    /// `min_age_gated_types`/`max_pattern_radius`/`zero_head_radius`,
    /// посчитанные один раз — см. doc-комментарий `SearchRadiusCache`.
    search_radius_cache: SearchRadiusCache,
    /// Структурная информация из `ConflictGraph`, посчитанная один раз (не на
    /// каждый тик) — используется `run_tick_with_cache`, чтобы не гонять
    /// полный арбитраж для матчей, у которых физически нет никого рядом, кто
    /// мог бы с ними столкнуться.
    ///
    /// Уточнение предыдущей версии ("голова либо безопасна ГЛОБАЛЬНО и
    /// НАВСЕГДА, либо нет"): та версия проверяла ГОЛОВУ целиком — если у
    /// головы есть хоть одно ребро в графе конфликтов (с ЛЮБОЙ другой
    /// головой, на любом расстоянии), она выключала оптимизацию для себя
    /// НАВСЕГДА. Реальный пример (`big_world.rs`): провод и распад (5 голов,
    /// статичные) сами по себе бесконфликтны, но стоит на той же решётке
    /// (за миллион клеток) появиться челноку (сдвигается — см. давно
    /// известный "moving object"-лимит статического анализа), и структурно
    /// он конфликтует уже со ВСЕМИ 7 головами — включая провод и распад,
    /// хотя физически они никогда не соприкасаются. Старая версия гасила
    /// быстрый путь для всей решётки целиком из-за одной детали за миллион
    /// клеток. Здесь вместо одного глобального решения — ДВЕ ступени:
    /// `conflict_partners` только называет, какие ГОЛОВЫ структурно МОГЛИ
    /// БЫ столкнуться (как и раньше), а решение "нужен ли арбитраж
    /// конкретному совпадению" принимается КАЖДЫЙ тик заново — только если
    /// хоть один партнёр этой головы реально совпал где-то в пределах
    /// досягаемости (`max_affected_radius`) от НЕЁ САМОЙ (см.
    /// `spatial_bypass_split`). Провод и распад проходят напрямую каждый
    /// тик, как и должны — челнок им попросту не сосед.
    conflict_partners: FxHashMap<CellType, FxHashSet<CellType>>,
    /// Максимальный "радиус" (наибольшее расстояние по x/y от позиции
    /// совпадения до затронутой клетки — `RuleData::bbox`) среди ВСЕХ
    /// правил набора. Два совпадения дальше `2 × max_affected_radius` друг
    /// от друга по любой оси НИКОГДА не могут иметь пересекающихся
    /// affected-регионов, какие бы конкретно правила ни сработали — граница
    /// консервативна (общий максимум по всем правилам, не по паре), но
    /// звучит, и этого достаточно для корректного пространственного отсева
    /// в `spatial_bypass_split`.
    max_affected_radius: i32,
    /// Если включена самомодификация (см. [`Engine::enable_self_modification`]),
    /// здесь живёт декодер протокола `RuleStore` — `run_tick` сам, без
    /// внешнего кода, дренирует канал 0 выходных граничных буферов после
    /// каждого тика и применяет готовые операции к `rule_index`. `None`
    /// (по умолчанию) означает "как раньше" — нулевые накладные расходы и
    /// нулевая разница в поведении для кода, который об этом не просил.
    self_mod: Option<crate::rule_store::RuleStore>,
    /// Если true (см. [`Engine::enable_guarded_self_modification`]) — новый
    /// самопереданный `AddRule` с id, которого ЕЩЁ НЕТ в `rule_index`,
    /// принимается только если доказуемо не конфликтует с ОСТАЛЬНЫМ текущим
    /// набором правил ([`crate::ConflictGraph::check_composition`]).
    /// Расширение УЖЕ существующего id, добавленного САМОЙ самомодификацией
    /// (например, increment/finalize — `strength_self_modification_computed.rs`),
    /// не проверяется: риск композиции — в столкновении с ЧУЖОЙ территорией,
    /// а не в том, что модуль развивает своё же поведение. "Чужая
    /// территория" — это `protected_heads`: id, существовавшие ДО начала
    /// самомодификации, а не то, что она сама успела вырастить.
    guard_self_modification: bool,
    /// Снимок "чужой территории" — правил, которыми `RuleStore` НЕ
    /// управляет (пришли в обход протокола, либо стояли в `rule_index` с
    /// самого начала, либо были добавлены напрямую позже — оба случая
    /// равноправны). И граница защиты в [`Engine::enable_guarded_self_modification`]
    /// (её ключи — "protected heads"), и то, что обязано пережить любое
    /// слияние в `absorb_self_modifications` — одно и то же множество.
    ///
    /// Инициализируется в [`Engine::enable_self_modification`] (снимок
    /// `rule_index` НА ТОТ МОМЕНТ, а не на момент `Engine::new` — код может
    /// вполне законно менять `rule_index` напрямую между созданием движка и
    /// включением самомодификации, как `strength_live_rules.rs`, и такие
    /// правила ничем не отличаются от заданных при `Engine::new`).
    /// Дальше пересчитывается заново при каждом вызове [`Engine::rebuild_rule_cache`]
    /// (документированная точка входа "я поменял `rule_index` напрямую") —
    /// как "то, что сейчас в `rule_index`, за вычетом того, чем сейчас
    /// реально владеет `RuleStore`" — так что и более ПОЗДНИЕ прямые правки
    /// (после того как самомодификация уже что-то на лету добавила)
    /// корректно учитываются, а не теряются.
    ///
    /// Область действия честно ограничена: защищает только то, чем
    /// `RuleStore` не управляет НА ДАННЫЙ МОМЕНТ. Гонку МЕЖДУ двумя
    /// самомодифицирующимися регионами за один и тот же, ранее НИКЕМ не
    /// занятый id эта версия не разбирает — тот, кто застолбит id первым,
    /// для всех последующих посылок на тот же id считается "уже
    /// существующим" и больше не проверяется. Общая многосторонняя гонка за
    /// территорию — отдельная, более сложная задача.
    original_rule_index: HashMap<CellType, Vec<Rule>>,
    /// Голова-типы, за которыми прямо сейчас числится хоть одно
    /// самопереданное правило (используется вместе с `original_rule_index`
    /// для корректной обработки `RemoveRule`/`ClearAll`: если у головы,
    /// которой самомодификация когда-то управляла, самопереданных правил
    /// больше не осталось, `rule_index[head]` нужно вернуть к ОРИГИНАЛУ
    /// (или убрать вовсе, если оригинала и не было) — а не оставить
    /// висеть устаревшую копию, как было раньше `get_index()` только
    /// добавлял ключи и никогда не убирал исчезнувшие).
    self_mod_managed_heads: FxHashSet<CellType>,
    /// Сколько раз [`Engine::enable_guarded_self_modification`] отклонила
    /// самопереданное правило как небезопасное для композиции.
    pub rejected_self_modifications: u64,
}

impl<S: GridStorage> Engine<S> {
    /// Создать новый двигатель с решёткой и индексом правил.
    pub fn new(
        grid: Grid<S>,
        rule_index: HashMap<CellType, Vec<Rule>>,
    ) -> Self {
        let rule_cache = build_rule_data_cache(&rule_index);
        let group_cache = build_group_data(&rule_index);
        let search_radius_cache = compute_search_radius_cache(&rule_index);
        let (conflict_partners, max_affected_radius) = compute_conflict_partners(&rule_index, &rule_cache);
        Self {
            grid,
            rule_index,
            rule_cache,
            group_cache,
            search_radius_cache,
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
        }
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

    /// Получить ссылку на решётку.
    pub fn grid(&self) -> &Grid<S> {
        &self.grid
    }

    /// Получить мутабельную ссылку на решётку.
    pub fn grid_mut(&mut self) -> &mut Grid<S> {
        &mut self.grid
    }

    // ─── Обнаружение совпадений ───

    /// Обнаружить все совпадения на текущей решётке.
    pub fn detect_matches(&self) -> Vec<RuleMatch> {
        let search_coords = resolve_search_coords_peek(&self.grid, &self.search_radius_cache);
        detect_matches_with_group_data(&self.grid, &self.group_cache, &search_coords)
    }

    // ─── Арбитраж ───

    /// Выбрать непротиворечивый набор совпадений.
    pub fn arbitrate(&self, all_matches: Vec<RuleMatch>) -> Vec<RuleMatch> {
        arbitrate(
            all_matches,
            &self.rule_index,
            &self.rule_cache,
            (self.grid.width(), self.grid.height()),
            |x, y| self.grid.get_age(x, y) as u32,
        )
    }

    // ─── Применение ───

    /// Применить набор совпадений к решётке.
    pub fn apply_matches(
        &mut self,
        matches: Vec<RuleMatch>,
    ) -> (Vec<AffectedRegion>, Vec<(u32, Cell)>) {
        apply_matches(&mut self.grid, matches, &self.rule_index, &self.rule_cache)
    }

    // ─── IO ───

    /// Отправить значение на входной канал граничной ячейки.
    /// Ищет первый граничный буфер с direction == "input" и
    /// помещает значение в очередь указанного канала.
    pub fn push_input(&mut self, ch: u32, value: u8) {
        for (_, buf) in self.grid.iter_boundaries_mut() {
            if buf.direction == "input" {
                buf.enqueue(ch, Cell::new(value));
                return;
            }
        }
    }

    /// Извлечь выходные данные из граничных буферов.
    pub fn pop_output(&mut self) -> Vec<(u32, Cell)> {
        let mut outputs = Vec::new();
        let coords: Vec<(usize, usize)> = self.grid.boundary_coords().collect();
        for (x, y) in coords {
            if let Some(buf) = self.grid.get_boundary_mut(x, y) {
                let channels: Vec<u32> = buf.queues.keys().copied().collect();
                for ch in channels {
                    for cell in buf.dequeue(ch) {
                        outputs.push((x as u32, cell));
                    }
                }
            }
        }
        outputs
    }

    /// Применить данные из входных буферов к решётке.
    ///
    /// Каждый вызов потребляет ровно одно значение с фронта очереди каждого
    /// input-буфера (по первому непустому каналу) и продвигает очередь —
    /// иначе следующий тик увидел бы то же самое значение снова, а
    /// остальные когда-либо запушенные значения никогда бы не дошли до
    /// решётки.
    pub fn apply_input(&mut self) {
        let inputs: Vec<(usize, usize, u32, u8)> = {
            let mut v = Vec::new();
            for (&(x, y), buf) in self.grid.iter_boundaries() {
                if buf.direction == "input" {
                    for (&ch, queue) in &buf.queues {
                        if let Some(cell) = queue.front() {
                            v.push((x, y, ch, cell.value.0 .0));
                            break;
                        }
                    }
                }
            }
            v
        };
        let gen = self.grid.generation();
        for (x, y, ch, val) in inputs {
            self.grid.set_cell(
                x,
                y,
                Cell {
                    value: CellValue::new(val),
                    born_at: gen,
                },
            );
            // Потребляем значение — иначе оно будет применяться повторно
            // на каждом следующем тике, а очередь никогда не продвинется.
            if let Some(buf) = self.grid.get_boundary_mut(x, y) {
                if let Some(queue) = buf.queues.get_mut(&ch) {
                    queue.pop_front();
                }
            }
        }
    }

    /// Извлечь и очистить выходные буферы.
    pub fn drain_output(&mut self) -> Vec<(u32, Cell)> {
        let mut outputs = Vec::new();
        let coords: Vec<(usize, usize)> = self.grid.boundary_coords().collect();
        for (x, y) in coords {
            if let Some(buf) = self.grid.get_boundary_mut(x, y) {
                if buf.direction == "output" {
                    let channels: Vec<u32> = buf.queues.keys().copied().collect();
                    for ch in channels {
                        for cell in buf.dequeue(ch) {
                            outputs.push((x as u32, cell));
                        }
                    }
                }
            }
        }
        outputs
    }

    // ─── Age ───

    /// Увеличить возраст всех активных ячеек на 1.
    pub fn advance_age(&mut self) {
        self.grid.advance_age();
    }

    /// Сбросить возраст в affected regions.
    /// Устанавливает born_at = текущее поколение.
    pub fn reset_age(&mut self, regions: &[AffectedRegion]) {
        let gen = self.grid.generation();
        for region in regions {
            for y in region.y_start..region.y_end {
                for x in region.x_start..region.x_end {
                    if let Some(cell) = self.grid.get_cell(x as usize, y as usize) {
                        self.grid.storage.set(
                            x as usize,
                            y as usize,
                            Cell {
                                value: cell.value,
                                born_at: gen,
                            },
                        );
                    }
                }
            }
        }
    }

    // ─── Терминация ───

    /// Проверить, достигла ли система устойчивого состояния.
    pub fn detect_termination(&self, tick: u32) -> TerminationVerdict {
        let search_coords = resolve_search_coords_peek(&self.grid, &self.search_radius_cache);
        let matches = detect_matches_with_group_data(&self.grid, &self.group_cache, &search_coords);
        if matches.is_empty() {
            TerminationVerdict::Stable
        } else if tick > 0 {
            let accepted = self.arbitrate(matches);
            if accepted.is_empty() {
                TerminationVerdict::Stable
            } else {
                TerminationVerdict::Active
            }
        } else {
            TerminationVerdict::Active
        }
    }

    /// Проверить возможность композиции.
    pub fn compose_with(&mut self) -> CompositionVerdict {
        let initial_cells: Vec<(usize, usize, Cell)> = {
            let mut v = Vec::new();
            for (x, y) in self.grid.iter_active() {
                if let Some(cell) = self.grid.get_cell(x, y) {
                    v.push((x, y, *cell));
                }
            }
            v
        };
        let initial_count = initial_cells.len();

        let mut tick = 0u32;
        loop {
            if tick > 1000 {
                return CompositionVerdict::NonTerminating;
            }
            let search_coords = resolve_search_coords_advance(&mut self.grid, &self.search_radius_cache);
            let matches = detect_matches_with_group_data(&self.grid, &self.group_cache, &search_coords);
            if matches.is_empty() {
                break;
            }
            // См. комментарий в свободной функции run_tick: помечаем
            // ВСЕ найденные совпадения, не только принятые — проигравшее
            // арбитраж совпадение остаётся актуальным условием и должно
            // переоцениваться на следующем тике.
            for m in &matches {
                self.grid.mark_dirty(m.x as usize, m.y as usize);
            }
            let accepted = self.arbitrate(matches);
            if accepted.is_empty() {
                break;
            }
            let (regions, _) = self.apply_matches(accepted);

            // Старение
            self.grid.advance_age();
            self.reset_age(&regions);
            tick += 1;
        }

        let final_cells: Vec<(usize, usize, Cell)> = {
            let mut v = Vec::new();
            for (x, y) in self.grid.iter_active() {
                if let Some(cell) = self.grid.get_cell(x, y) {
                    v.push((x, y, *cell));
                }
            }
            v
        };
        let final_count = final_cells.len();

        if final_count > initial_count * 2 {
            CompositionVerdict::Divergent
        } else if final_count < initial_count / 2 && initial_count > 0 {
            CompositionVerdict::Shrinking
        } else {
            CompositionVerdict::Bounded(final_count)
        }
    }

    // ─── Вспомогательные ───

    /// Получить приоритет правила.
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

    /// Найти правило по ID.
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
        let conflict_ctx = ConflictContext { partners: &self.conflict_partners, max_radius: self.max_affected_radius };
        let result = run_tick_with_cache(&mut self.grid, &self.rule_index, &self.rule_cache, &self.group_cache, &self.search_radius_cache, Some(&conflict_ctx));
        self.absorb_self_modifications();
        result
    }

    /// Если самомодификация включена — дренировать канал 0 выходных
    /// граничных буферов, применить готовые операции к `rule_index` и,
    /// только если что-то реально изменилось, перестроить кэши. Отдельный
    /// метод, а не код прямо в `run_tick`, только чтобы не загромождать его —
    /// логически это часть одного тика.
    fn absorb_self_modifications(&mut self) {
        let Some(mut rule_store) = self.self_mod.take() else { return };
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
    fn composition_allows(&self, op: &crate::rule_store::CompletedOp, rule_store: &mut crate::rule_store::RuleStore) -> bool {
        let crate::rule_store::RuleOp::AddRule(rule) = &op.op else { return true };
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
        self.rule_cache = build_rule_data_cache(&self.rule_index);
        self.group_cache = build_group_data(&self.rule_index);
        self.search_radius_cache = compute_search_radius_cache(&self.rule_index);
        let (partners, radius) = compute_conflict_partners(&self.rule_index, &self.rule_cache);
        self.conflict_partners = partners;
        self.max_affected_radius = radius;
        self.resync_original_rule_index();
        let active: Vec<(usize, usize)> = self.grid.active_coords().clone();
        for (x, y) in active {
            self.grid.mark_dirty(x, y);
        }
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
        let Some(mut rule_store) = self.self_mod.take() else { return };
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

/// Теоремой `ConflictGraph` определить, какие ГОЛОВЫ структурно МОГЛИ БЫ
/// столкнуться друг с другом (включая с самими собой — self-loop), и
/// наибольший "радиус" (bbox affected-региона) среди всех правил набора —
/// см. doc-комментарии `Engine::conflict_partners`/`Engine::max_affected_radius`.
/// Считается один раз при создании/перестройке `Engine`, не на каждый тик —
/// `ConflictGraph::build` сам по себе не бесплатен (O(N²·K²) от числа
/// правил), но правила меняются на порядки реже, чем тикает движок. Само
/// решение "нужен ли арбитраж" для конкретного совпадения принимается
/// заново каждый тик в `spatial_bypass_split`, используя эти структурные
/// данные как вход, а не как готовый ответ.
fn compute_conflict_partners(
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &crate::conflict_analyzer::RuleDataCache,
) -> (FxHashMap<CellType, FxHashSet<CellType>>, i32) {
    let mut rules: Vec<Rule> = Vec::new();
    let mut head_of_rule: Vec<CellType> = Vec::new();
    for (&head, rs) in rule_index {
        for r in rs {
            rules.push(r.clone());
            head_of_rule.push(head);
        }
    }
    let graph = crate::conflict_analyzer::ConflictGraph::build(&rules);
    let mut partners: FxHashMap<CellType, FxHashSet<CellType>> = FxHashMap::default();
    for &(i, j) in graph.potential_conflicts() {
        let (hi, hj) = (head_of_rule[i], head_of_rule[j]);
        partners.entry(hi).or_default().insert(hj);
        partners.entry(hj).or_default().insert(hi);
    }

    let max_radius = rule_cache
        .values()
        .map(|data| {
            let (min_x, max_x, min_y, max_y) = data.bbox;
            min_x.unsigned_abs().max(max_x.unsigned_abs()).max(min_y.unsigned_abs()).max(max_y.unsigned_abs()) as i32
        })
        .max()
        .unwrap_or(0);

    (partners, max_radius)
}

/// Структурный вход для `spatial_bypass_split` — см. doc-комментарии
/// `Engine::conflict_partners`/`Engine::max_affected_radius`.
struct ConflictContext<'a> {
    partners: &'a FxHashMap<CellType, FxHashSet<CellType>>,
    max_radius: i32,
}

/// Выполнить один тик симуляции (свободная функция).
///
/// Пересобирает `rule_cache`/`group_cache` заново на каждый вызов — эта
/// функция не хранит состояния между тиками (в отличие от [`Engine`],
/// который кэширует их в `self.rule_cache`/`self.group_cache`). Для
/// небольшого набора правил стоимость пересборки пренебрежимо мала; для
/// конфигов с десятками-сотнями правил и частыми тиками в цикле
/// предпочтительнее держать `Engine` и звать `Engine::run_tick`.
///
/// НЕ вычисляет `conflict_partners`/`max_affected_radius` (в отличие от
/// `Engine`, где это считается один раз и кэшируется) — `ConflictGraph::build`
/// сам по себе O(N²·K²) от числа правил и размера паттернов, и на наборе в
/// сотни правил (например, полный Game of Life — 228 правил) эта проверка
/// сама по себе дороже целого тика. Пересчитывать её на КАЖДЫЙ вызов
/// свободной функции — не "пренебрежимо мало", как rule_cache/group_cache, а
/// реальная регрессия (найдено экспериментально: наивная версия этой
/// оптимизации замедлила GoL на порядки).
pub fn run_tick<S: GridStorage>(
    grid: &mut Grid<S>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
) -> (Vec<RuleMatch>, Vec<(u32, Cell)>) {
    let rule_cache = crate::conflict_analyzer::build_rule_data_cache(rule_index);
    let group_cache = build_group_data(rule_index);
    let search_radius_cache = compute_search_radius_cache(rule_index);
    run_tick_with_cache(grid, rule_index, &rule_cache, &group_cache, &search_radius_cache, None)
}

/// Общая логика одного тика, параметризованная источниками `rule_cache`/
/// `group_cache` — общий код для свободной функции `run_tick` (строит кэши
/// каждый раз) и `Engine::run_tick` (переиспользует `self.rule_cache`/
/// `self.group_cache`).
///
/// `conflict_ctx: None` — оптимизация выключена, всегда полный арбитраж (путь
/// свободной функции `run_tick`, где считать граф конфликтов заново на
/// каждый вызов не оправдано). `Some(ctx)` — матчи голов, у которых в этом
/// тике физически нет рядом ни одного структурного конфликт-партнёра,
/// принимаются напрямую, без единого сравнения; остальные — через обычный
/// арбитраж (см. `spatial_bypass_split`). Это не эвристика — та же теорема
/// уже проверена тяжёлыми property-тестами
/// (`prop_conflict_free_rules_accept_everything`): для матча, рядом с которым
/// нет ни одного потенциального конфликт-партнёра, арбитраж и так принял бы
/// его в 100% случаев, просто дороже.
/// Разделить матчи на "точно безопасные в этом тике" (принять напрямую) и
/// "нужен арбитраж" — используя `ctx.partners` (структурно: какие головы
/// МОГЛИ БЫ столкнуться) вместе с РЕАЛЬНЫМИ позициями совпадений этого тика.
///
/// Голова, отсутствующая в `ctx.partners` как ключ, безусловно безопасна —
/// без единого сравнения позиций (эквивалент старого "глобально безопасной
/// головы"). Голова-ключ безопасна ТОЛЬКО если ни один из её партнёров не
/// совпал в пределах `2 × ctx.max_radius` по x И по y — это ВЕРХНЯЯ граница
/// на дальность, на которую affected-регион ЛЮБОЙ пары правил может
/// пересечься (см. doc-комментарий `Engine::max_affected_radius`), поэтому
/// проверка консервативна (может послать в арбитраж чуть больше, чем
/// строго нужно), но никогда не бывает наоборот.
///
/// Пространственный отсев — стандартный spatial hashing: решётка совпадений
/// делится на квадратные корзины стороной `bucket = 2 × max_radius`, и для
/// каждого совпадения проверяются только 3×3 соседние корзины — этого
/// достаточно, потому что две точки дальше `bucket` друг от друга по любой
/// оси не могут оказаться в соседних (или той же) корзине.
fn spatial_bypass_split(matches: Vec<RuleMatch>, ctx: &ConflictContext) -> (Vec<RuleMatch>, Vec<RuleMatch>) {
    let (mut safe, candidates): (Vec<RuleMatch>, Vec<RuleMatch>) =
        matches.into_iter().partition(|m| !ctx.partners.contains_key(&m.head));

    if candidates.is_empty() {
        return (safe, candidates);
    }

    let bucket = (2 * ctx.max_radius).max(1);
    let mut buckets: FxHashMap<(i32, i32), Vec<usize>> = FxHashMap::default();
    for (idx, m) in candidates.iter().enumerate() {
        let key = ((m.x as i32).div_euclid(bucket), (m.y as i32).div_euclid(bucket));
        buckets.entry(key).or_default().push(idx);
    }

    let mut needs_arbitration = vec![false; candidates.len()];
    for idx in 0..candidates.len() {
        if needs_arbitration[idx] {
            continue;
        }
        let m = &candidates[idx];
        let Some(my_partners) = ctx.partners.get(&m.head) else { continue };
        let (bx, by) = ((m.x as i32).div_euclid(bucket), (m.y as i32).div_euclid(bucket));
        'neighbors: for dbx in -1..=1 {
            for dby in -1..=1 {
                let Some(members) = buckets.get(&(bx + dbx, by + dby)) else { continue };
                for &other in members {
                    if other != idx && my_partners.contains(&candidates[other].head) {
                        needs_arbitration[idx] = true;
                        needs_arbitration[other] = true;
                        break 'neighbors;
                    }
                }
            }
        }
    }

    let mut unsafe_matches = Vec::new();
    for (idx, m) in candidates.into_iter().enumerate() {
        if needs_arbitration[idx] {
            unsafe_matches.push(m);
        } else {
            safe.push(m);
        }
    }
    (safe, unsafe_matches)
}

fn run_tick_with_cache<S: GridStorage>(
    grid: &mut Grid<S>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &crate::conflict_analyzer::RuleDataCache,
    group_cache: &GroupCache,
    search_radius_cache: &SearchRadiusCache,
    conflict_ctx: Option<&ConflictContext>,
) -> (Vec<RuleMatch>, Vec<(u32, Cell)>) {
    let search_coords = resolve_search_coords_advance(grid, search_radius_cache);

    let matches = detect_matches_with_group_data(grid, group_cache, &search_coords);
    if matches.is_empty() {
        // Время всё равно идёт: без этого симуляция, где на каком-то тике не
        // нашлось ни одного совпадения (например, поле держит `min_age`-
        // клетку, которая ещё не "созрела", и больше ничего не происходит),
        // навсегда замораживает generation — а с ним и возраст, который
        // `min_age` только и проверяет. Раньше `advance_age()` вызывался
        // только на пути с реально применённым тиком, из-за чего вызов
        // `run_tick()` N раз не гарантировал N реально прошедших тиков.
        grid.advance_age();
        return (Vec::new(), Vec::new());
    }

    // Помечаем "грязными" позиции ВСЕХ найденных совпадений — не только тех,
    // что примет арбитраж. Проигравшее арбитраж совпадение — это НЕ
    // исчезнувшее условие: клетка не изменилась, паттерн по-прежнему
    // совпадает, и конфликт может разрешиться иначе на следующем тике (если
    // победитель освободит клетку или сам станет неактуален). Если не
    // помечать проигравших, они выпадают из dirty-множества навсегда, хотя
    // полный скан продолжал бы находить и переоценивать их каждый тик.
    for m in &matches {
        grid.mark_dirty(m.x as usize, m.y as usize);
    }

    // Арбитраж: матчи, у которых в ЭТОМ тике физически нет рядом ни одного
    // структурного конфликт-партнёра, принимаются напрямую, без единого
    // сравнения (см. doc-комментарий функции); остальные — через обычный
    // арбитраж.
    let accepted = match conflict_ctx {
        None => arbitrate(matches, rule_index, rule_cache, (grid.width(), grid.height()), |x, y| {
            grid.get_age(x, y) as u32
        }),
        Some(ctx) => {
            let (safe, unsafe_matches) = spatial_bypass_split(matches, ctx);
            if unsafe_matches.is_empty() {
                safe
            } else {
                let mut accepted = safe;
                accepted.extend(arbitrate(
                    unsafe_matches,
                    rule_index,
                    rule_cache,
                    (grid.width(), grid.height()),
                    |x, y| grid.get_age(x, y) as u32,
                ));
                accepted
            }
        }
    };

    if accepted.is_empty() {
        // См. комментарий выше — то же самое: тик "случился", даже если всё
        // отклонено арбитражем, время должно пройти.
        grid.advance_age();
        return (Vec::new(), Vec::new());
    }

    // Применение
    let (regions, outputs) = apply_matches(grid, accepted.clone(), rule_index, rule_cache);

    // Старение
    grid.advance_age();
    reset_age_for_regions(grid, &regions);

    (accepted, outputs)
}

/// Максимальный |смещение| паттерна среди ВСЕХ правил (любой головы).
///
/// Если клетка D изменилась, то любая клетка-центр C, чей паттерн ссылается
/// на D на смещении (dx,dy) (то есть C+(dx,dy) = D, C = D-(dx,dy)), могла
/// изменить статус совпадения — и C лежит в пределах этого радиуса от D
/// (диапазон -radius..=radius симметричен, так что направление роли не
/// играет). Используется для расширения "грязного" множества (см.
/// `Grid::dirty_coords`) до множества кандидатов на пересканирование.
fn max_pattern_radius(rule_index: &HashMap<CellType, Vec<Rule>>) -> i32 {
    pattern_radius(rule_index.values().flatten())
}

/// То же самое, но только по правилам с head=`DEFAULT_CELL_VALUE` (0).
///
/// Нужен отдельно от `max_pattern_radius` для случая, когда кандидатами уже
/// выступают ВСЕ активные клетки (см. использование в `resolve_search_coords_*`
/// при вырожденно большом dirty-множестве): тогда единственная причина хоть
/// что-то расширять — найти head=0 "рождения" рядом с активными клетками
/// (см. `min_age_gated_types`-подобное рассуждение в doc `max_pattern_radius`).
/// Пропагировать охват от активных клеток к их же соседям другого типа
/// (общий случай `max_pattern_radius`) здесь не нужно — сами активные клетки
/// уже все в списке, расширять есть смысл только в сторону ДЕФОЛТНЫХ клеток.
fn zero_head_radius(rule_index: &HashMap<CellType, Vec<Rule>>) -> i32 {
    match rule_index.get(&CellType(DEFAULT_CELL_VALUE)) {
        Some(rules) => pattern_radius(rules.iter()),
        None => 0,
    }
}

fn pattern_radius<'a>(rules: impl Iterator<Item = &'a Rule>) -> i32 {
    let mut max_r = 0i32;
    for rule in rules {
        if rule.pattern.is_empty() {
            // Паттерн из id: смещения 0..id.len()-1 по x.
            max_r = max_r.max(rule.id.len().saturating_sub(1) as i32);
        } else {
            for &(dx, dy, _) in &rule.pattern {
                max_r = max_r.max(dx.unsigned_abs() as i32).max(dy.unsigned_abs() as i32);
            }
        }
    }
    max_r
}

/// Типы-головы, у которых есть хотя бы одно правило с `min_age > 0`.
///
/// `min_age` — единственный способ, которым статус совпадения клетки может
/// измениться БЕЗ изменения значения у неё или у соседей — просто течением
/// времени (возраст переходит порог). Dirty-множество это не ловит: клетка
/// может стоять нетронутой сто тиков, а затем "созреть" для правила с
/// `min_age=100`. Поэтому клетки таких типов включаются в кандидатов на
/// КАЖДЫЙ тик безусловно, независимо от dirty-состояния — единственная
/// просадка от инкрементального скана, и она касается только типов, у
/// которых реально есть такие правила (в текущих `configs/` — 2 файла из 37).
fn min_age_gated_types(rule_index: &HashMap<CellType, Vec<Rule>>) -> FxHashSet<CellType> {
    rule_index
        .iter()
        .filter(|(_, rules)| rules.iter().any(|r| r.min_age > 0))
        .map(|(&ct, _)| ct)
        .collect()
}

/// `min_age_gated_types`/`max_pattern_radius`/`zero_head_radius` — все
/// чистые функции ТОЛЬКО от `rule_index`, но раньше пересчитывались заново
/// на каждый вызов `resolve_search_coords_*` — то есть на каждый тик,
/// безусловно, даже когда набор правил месяцами не менялся (найдено при
/// проверке производительности: `min_age_gated_types` — O(всех правил)
/// линейный скан HashMap на пустом месте каждый тик). `Engine` считает это
/// один раз при создании/перестройке (см. `Engine::search_radius_cache`) и
/// переиспользует, как уже делает с `rule_cache`/`group_cache`/
/// `conflict_partners`; свободная функция `run_tick` по-прежнему считает
/// заново на каждый вызов — тот же компромисс, что и везде в этом файле.
struct SearchRadiusCache {
    min_age_gated_types: FxHashSet<CellType>,
    max_pattern_radius: i32,
    zero_head_radius: i32,
}

fn compute_search_radius_cache(rule_index: &HashMap<CellType, Vec<Rule>>) -> SearchRadiusCache {
    SearchRadiusCache {
        min_age_gated_types: min_age_gated_types(rule_index),
        max_pattern_radius: max_pattern_radius(rule_index),
        zero_head_radius: zero_head_radius(rule_index),
    }
}

/// Построить кандидатов для detect_matches из уже полученного базового
/// множества (dirty-множество, ЛИБО весь active_coords в вырожденном случае
/// — см. вызывающий код) и заданного радиуса расширения.
fn build_candidates<S: GridStorage>(
    base: Vec<(usize, usize)>,
    radius: i32,
    grid: &Grid<S>,
    cache: &SearchRadiusCache,
) -> Vec<(usize, usize)> {
    // При radius=0 расширять нечего — берём `base` как есть (move), а не
    // через `expand_neighborhood(&base, 0)`: та принимает срез и поэтому
    // ОБЯЗАНА клонировать (`coords.to_vec()`) даже когда возвращает то же
    // самое множество без изменений. Здесь `base` уже во владении —
    // повторное клонирование было бы чистой тратой (при 250 000 элементах —
    // лишний Vec::clone поверх уже сделанного ранее).
    let mut candidates = if radius == 0 {
        base
    } else {
        expand_neighborhood(grid, &base, radius)
    };

    if !cache.min_age_gated_types.is_empty() {
        let mut seen: FxHashSet<(usize, usize)> =
            candidates.iter().copied().collect();
        for &(x, y) in grid.active_coords() {
            if seen.contains(&(x, y)) {
                continue;
            }
            if let Some(cell) = grid.get_cell(x, y) {
                if cache.min_age_gated_types.contains(&cell.value.0) {
                    seen.insert((x, y));
                    candidates.push((x, y));
                }
            }
        }
    }

    candidates
}

/// Выбрать базовое множество кандидатов и радиус его расширения.
///
/// Если "грязных" клеток сравнимо с числом активных — дешевле и не менее
/// корректно взять весь `active_coords` напрямую (cache-friendly Vec), чем
/// прогонять их через HashSet-конвейер dirty-множества (insert на каждый
/// `set_cell`, drain здесь). На тиках, где изменение разом затрагивает почти
/// всю решётку (единичный массовый эффект, или сценарий, где реально каждая
/// активная клетка меняется каждый тик), dirty вырождается в "почти всё
/// активное" — и его HashSet-механика становится чистым оверхедом.
///
/// В этом вырожденном случае радиус расширения тоже другой и ýже:
/// `active_coords` уже содержит вообще все активные клетки, поэтому
/// пропагировать охват на соседей ради поиска "затронутых нейтральных
/// клеток" (общая цель `max_pattern_radius` для маленького dirty-множества)
/// не нужно — единственное, что ещё может понадобиться найти — это head=0
/// "рождения" рядом с активными клетками (`zero_head_radius`, обычно 0).
/// Использование здесь широкого `max_pattern_radius` было бы чистой тратой:
/// на решётке 500×500 с уже-везде-активными клетками и паттерном шире одной
/// клетки это означало бы прогон `expand_neighborhood` над всеми 250 000
/// координатами без какой-либо дополнительной пользы.
fn dirty_base_and_radius<S: GridStorage>(
    dirty: FxHashSet<(usize, usize)>,
    grid: &Grid<S>,
    cache: &SearchRadiusCache,
) -> (Vec<(usize, usize)>, i32) {
    if dirty.len() * 2 >= grid.active_coords().len() {
        (grid.active_coords().clone(), cache.zero_head_radius)
    } else {
        (dirty.into_iter().collect(), cache.max_pattern_radius)
    }
}

/// "Подглядеть" кандидатов для detect_matches, НЕ потребляя dirty-множество.
///
/// Безопасно вызывать сколько угодно раз подряд без побочных эффектов —
/// например, из `detect_termination` в цикле проверки стабилизации.
/// Потребление (`take_dirty`) здесь недопустимо: если "подглядывающий" вызов
/// очистит dirty-множество, а состояние решётки при этом не изменится
/// (никакой тик не применялся), следующий РЕАЛЬНЫЙ `run_tick` решит, что эти
/// клетки уже проверены, и пропустит реальные совпадения.
fn resolve_search_coords_peek<S: GridStorage>(
    grid: &Grid<S>,
    cache: &SearchRadiusCache,
) -> Vec<(usize, usize)> {
    let dirty = grid.peek_dirty();
    let (base, radius) = dirty_base_and_radius(dirty, grid, cache);
    build_candidates(base, radius, grid, cache)
}

/// Получить кандидатов для detect_matches и ОЧИСТИТЬ dirty-множество.
///
/// Вызывать ровно один раз на каждый реально применяемый тик (`run_tick`,
/// `compose_with`) — после этого вызова следующий тик увидит только то, что
/// изменится начиная с текущего момента (apply_matches этого же тика
/// заполнит dirty-множество заново через `set_cell`).
fn resolve_search_coords_advance<S: GridStorage>(
    grid: &mut Grid<S>,
    cache: &SearchRadiusCache,
) -> Vec<(usize, usize)> {
    let dirty = grid.take_dirty();
    let (base, radius) = dirty_base_and_radius(dirty, grid, cache);
    build_candidates(base, radius, grid, cache)
}

/// Расширить список координат на окрестность заданного радиуса.
/// Используется для обнаружения паттернов вокруг активных ячеек (см.
/// `detect_radius` — радиус 0 означает «расширение не нужно вообще»).
fn expand_neighborhood<S: GridStorage>(
    grid: &Grid<S>,
    coords: &[(usize, usize)],
    radius: i32,
) -> Vec<(usize, usize)> {
    if coords.is_empty() || radius == 0 {
        return coords.to_vec();
    }

    // Если кандидатов после расширения будет сравнимо со всей решёткой —
    // дешевле плотный маркер (прямая запись по индексу без хеширования),
    // чем HashSet<(usize,usize)> с хешированием каждой пары координат.
    let side = (2 * radius + 1) as usize;
    let per_cell = side * side;
    if let Some((w, h)) = grid.storage.bounds() {
        if w > 0 && h > 0 && coords.len().saturating_mul(per_cell) >= w * h {
            let mut seen = vec![false; w * h];
            let mut result = Vec::new();
            for &(x, y) in coords {
                let x0 = x.saturating_sub(radius as usize);
                let x1 = (x + radius as usize).min(w - 1);
                let y0 = y.saturating_sub(radius as usize);
                let y1 = (y + radius as usize).min(h - 1);
                for ny in y0..=y1 {
                    let row = ny * w;
                    for nx in x0..=x1 {
                        let idx = row + nx;
                        if !seen[idx] {
                            seen[idx] = true;
                            result.push((nx, ny));
                        }
                    }
                }
            }
            return result;
        }
    }

    // Малое число кандидатов (типичный случай для разреженных сценариев —
    // движение одной головки Тьюринга или маркера сортировки: 1-4 базовых
    // координаты) — линейный dedup по Vec дешевле, чем HashSet<(usize,usize)>:
    // не нужно ни аллокации хеш-таблицы, ни SipHash каждой пары координат,
    // просто последовательный contains() по маленькому cache-friendly Vec.
    let estimated = coords.len().saturating_mul(per_cell);
    if estimated <= 256 {
        let mut result: Vec<(usize, usize)> = Vec::with_capacity(estimated);
        for &(x, y) in coords {
            for dx in -radius..=radius {
                for dy in -radius..=radius {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx >= 0 && ny >= 0 {
                        let coord = (nx as usize, ny as usize);
                        if !result.contains(&coord) {
                            result.push(coord);
                        }
                    }
                }
            }
        }
        return result;
    }

    let mut set: FxHashSet<(usize, usize)> = FxHashSet::default();
    for &(x, y) in coords {
        for dx in -radius..=radius {
            for dy in -radius..=radius {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx >= 0 && ny >= 0 {
                    set.insert((nx as usize, ny as usize));
                }
            }
        }
    }
    set.into_iter().collect()
}

fn reset_age_for_regions<S: GridStorage>(grid: &mut Grid<S>, regions: &[AffectedRegion]) {
    let gen = grid.generation();
    // По точному списку `written_cells`, а не по прямоугольнику bbox —
    // прямоугольник, охватывающий исходную и целевую позицию сдвига на N>1
    // клеток, включает и клетки МЕЖДУ ними, которые сдвиг не трогает вовсе
    // (найдено экспериментально: клетка между позициями получала обнулённый
    // возраст, хотя сама не менялась). `written_cells` — ровно то, что было
    // вставлено в write-буфер при применении, без лишнего.
    for region in regions {
        for &(x, y) in &region.written_cells {
            if let Some(cell) = grid.get_cell(x as usize, y as usize) {
                grid.storage.set(
                    x as usize,
                    y as usize,
                    Cell {
                        value: cell.value,
                        born_at: gen,
                    },
                );
            }
        }
    }
}

/// Вердикт терминации.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminationVerdict {
    /// Система активна — есть совпадения.
    Active,
    /// Система стабильна — нет совпадений или арбитраж отклонил все.
    Stable,
}

/// Вердикт композиции.
#[derive(Debug, Clone, PartialEq)]
pub enum CompositionVerdict {
    /// Композиция ограничена.
    Bounded(usize),
    /// Композиция расходится.
    Divergent,
    /// Композиция сжимается.
    Shrinking,
    /// Композиция не завершается за лимит тиков.
    NonTerminating,
    /// Композиция conflict-free (статический анализ).
    Safe,
    /// Композиция с потенциальными конфликтами (статический анализ).
    Unsafe(Vec<(usize, usize)>),
}

// ──────────────────────────────────────────────────────────────
// Тесты
// ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
