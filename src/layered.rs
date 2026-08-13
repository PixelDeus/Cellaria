//! Многослойная решётка — стек ОДИНАКОВЫХ по размеру 2D-решёток, НЕ
//! настоящий 3D-мир (см. `Rule::cross_layer_reads`'s doc-комментарий про
//! то, почему это разные вещи). Первый срез: только ЧТЕНИЕ через слои
//! (`Rule::cross_layer_reads`), запись (`changes`/`shifts` на другой слой)
//! не поддерживается — отдельное, более сложное расширение.
//!
//! `LayeredEngine` НЕ трогает `Grid`/`GridStorage` — каждый слой это свой
//! `Engine`, тикающий через `Engine::run_tick_with_cross_layer_filter`
//! (тот же пайплайн, что и обычный `Engine::run_tick` — `cam`,
//! `starvation_after`, `feedback`, `memory`, `max_activations` работают
//! КОРРЕКТНО, персистентное состояние правил не теряется между тиками).
//! Раньше здесь стояла ручная связка `detect_matches`/`arbitrate`/
//! `apply_matches` — "raw"-методы, каждый из которых, по СОБСТВЕННЫМ
//! doc-комментариям, не хранит состояние между вызовами: `cam` отключён
//! в `detect_matches`, `arbitrate`'s starvation/feedback-счётчики всегда
//! no-op, `apply_matches` создаёт свежие пустые `FeedbackCounters`/
//! `MemoryBuffers` на каждый вызов, а `max_activations`'s учёт вообще не
//! существует вне `run_tick_with_cache`. На практике это значило, что
//! ЛЮБОЕ правило с одним из этих расширений через `LayeredEngine` вело
//! себя как будто расширения нет — найдено и исправлено; см.
//! `CHANGELOG.md`.
//!
//! Единственное новое сверх обычного `Engine::run_tick` — фильтр между
//! Detect и Arbitrate: кандидат, ссылающийся на другой слой через
//! `cross_layer_reads`, отсеивается, если условие не выполнено на
//! ПРЕДТИКОВОМ состоянии целевого слоя (та же дисциплина снимка тика
//! 2.2.1 — все слои читаются как они были на начало ЭТОГО тика, включая
//! чужие). Матч, отсеянный фильтром, для остального пайплайна (memory-
//! гейт, max_activations-гейт, starvation/feedback-учёт) выглядит так,
//! будто паттерн вообще не совпал — не наблюдается нигде, не тратит
//! бюджет.
//!
//! Слои без `cross_layer_reads`/`dz` между ними структурно НИКОГДА не
//! конфликтуют по записи (запись остаётся в своём слое) — арбитраж между
//! РАЗНЫМИ слоями не нужен вообще, это не доказывается для конкретного
//! набора правил (как `spatial_bypass_split`), а верно по построению.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::engine::{Engine, EngineSnapshot};
use crate::storage::GridStorage;
use crate::types::{CellType, Rule, RuleMatch};
use crate::Grid;

/// Правила слоя, у которых непусто `Rule::cross_layer_reads`, по ключу
/// `(head, rule_idx)` — см. [`LayeredEngine::run_tick`], пересчитывается
/// заново на каждый слой каждого тика (дёшево: затрагивает только правила
/// с этим расширением, не весь `rule_index`).
type CrossLayerReadsIndex = HashMap<(CellType, usize), Vec<(i8, i8, i8, CellType)>>;

/// Стек слоёв — каждый слой полноценный `Engine` (свой кэш, своё
/// персистентное состояние правил), но ВСЕ слои используют ОДИН И ТОТ ЖЕ
/// набор правил (см. doc-комментарий модуля — правила не привязаны к
/// конкретному слою, `Rule::cross_layer_reads`'s `dz` сам определяет, с
/// каким слоем взаимодействовать).
pub struct LayeredEngine<S: GridStorage> {
    layers: Vec<Engine<S>>,
}

/// Сохраняемый снимок всего стека слоёв — см. [`LayeredEngine::snapshot`]/
/// [`LayeredEngine::from_snapshot`]. Поле приватное по той же причине, что
/// и `EngineSnapshot`'s (работа через `Serialize`/`Deserialize`, не прямой
/// доступ) — тот же `serde_yaml`-совет применим (см. `EngineSnapshot`'s
/// doc-комментарий про non-string ключи `HashMap`, из-за которых
/// `serde_json` не подходит).
#[derive(Serialize, Deserialize)]
#[serde(bound(serialize = "S: Serialize", deserialize = "S: serde::de::DeserializeOwned"))]
pub struct LayeredSnapshot<S: GridStorage> {
    layers: Vec<EngineSnapshot<S>>,
}

impl<S: GridStorage> LayeredEngine<S> {
    /// `grids.len()` слоёв, каждый со СВОИМ `Grid`, но одним общим
    /// `rule_index` (клонируется на каждый слой — `Engine::new` уже
    /// строит из него свои кэши, дублировать эту логику здесь незачем).
    ///
    /// Паникует, если слои разного размера — доку модуля ("стек ОДИНАКОВЫХ
    /// по размеру 2D-решёток") раньше ничто не проверяло: координата,
    /// валидная на слое 0, могла молча указывать на несуществующую или
    /// СОВСЕМ ДРУГУЮ клетку на слое 1 в `cross_layer_condition_holds`, без
    /// единой ошибки ни при постройке, ни во время тика. Это, как и
    /// `Engine::new`, программный конструктор, доверяющий вызывающему коду
    /// (тесты строят его напрямую) — для внешних YAML-конфигов размер
    /// слоёв проверяется заранее, с понятной ошибкой, в
    /// `config::load_layered_config`.
    pub fn new(grids: Vec<Grid<S>>, rule_index: HashMap<CellType, Vec<Rule>>) -> Self {
        if let Some(first) = grids.first() {
            let (w, h) = (first.width(), first.height());
            assert!(
                grids.iter().all(|g| g.width() == w && g.height() == h),
                "LayeredEngine::new: all layers must share the same grid dimensions ({w}x{h}) -- \
                 a stack of differently-sized grids breaks cross_layer_reads coordinate correspondence between layers"
            );
        }
        let layers = grids
            .into_iter()
            .map(|grid| Engine::new(grid, rule_index.clone()))
            .collect();
        Self { layers }
    }

    /// Снимок ВСЕГО стека — по одному [`EngineSnapshot`] на слой (см. его
    /// doc-комментарий про то, почему не хранит кэши). Общий `rule_index`
    /// (одинаковый у всех слоёв на момент `new`) снимком не выделяется
    /// отдельно — он уже часть КАЖДОГО `EngineSnapshot`, ровно как если бы
    /// слои снимались по отдельности через `Engine::snapshot`.
    pub fn snapshot(&self) -> LayeredSnapshot<S>
    where
        S: Clone,
    {
        LayeredSnapshot {
            layers: self.layers.iter().map(Engine::snapshot).collect(),
        }
    }

    /// Восстановить весь стек из снимка ([`LayeredEngine::snapshot`]).
    /// Размеры слоёв снимком гарантированы согласованными (он мог
    /// появиться только из уже провалидированного `LayeredEngine`) —
    /// повторной проверки, в отличие от `new`, здесь не нужно.
    pub fn from_snapshot(snapshot: LayeredSnapshot<S>) -> Self {
        Self {
            layers: snapshot.layers.into_iter().map(Engine::from_snapshot).collect(),
        }
    }

    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    pub fn layer(&self, index: usize) -> &Engine<S> {
        &self.layers[index]
    }

    pub fn layer_mut(&mut self, index: usize) -> &mut Engine<S> {
        &mut self.layers[index]
    }

    /// Один тик по ВСЕМ слоям, через полный `Engine`-пайплайн на каждом
    /// (см. doc-комментарий модуля).
    ///
    /// Если НИ ОДНО правило нигде в стеке не использует
    /// `cross_layer_reads`, слои структурно независимы — тикаются
    /// обычным `Engine::run_tick` без снимка чужих решёток (нулевые
    /// накладные расходы, то же свойство, что и у `ExtensionFlags` для
    /// отдельного `Engine`). Иначе решётки ВСЕХ слоёв клонируются ОДИН
    /// РАЗ, до того как хоть один слой применит свой тик — это разом
    /// даёт и дисциплину снимка тика (чужой слой читается таким, каким
    /// он был на начало тика, а не наполовину применённым), и снимает
    /// конфликт заимствования (`&self.layers[j]` внутри замыкания,
    /// `&mut self.layers[i]` снаружи) — замыкание фильтра захватывает
    /// только собственные (`pre_tick_grids`) локальные данные, не `self`.
    pub fn run_tick(&mut self)
    where
        S: Clone,
    {
        if !self.any_rule_uses_cross_layer_reads() {
            for layer in &mut self.layers {
                layer.run_tick();
            }
            return;
        }

        let pre_tick_grids: Vec<Grid<S>> = self.layers.iter().map(|e| e.grid().clone()).collect();
        let num_layers = self.layers.len();

        for layer_idx in 0..num_layers {
            let cross_layer_reads: CrossLayerReadsIndex = self.layers[layer_idx]
                .rule_index()
                .iter()
                .flat_map(|(&head, rules)| {
                    rules
                        .iter()
                        .enumerate()
                        .filter(|(_, rule)| !rule.cross_layer_reads.is_empty())
                        .map(move |(rule_idx, rule)| ((head, rule_idx), rule.cross_layer_reads.clone()))
                })
                .collect();

            let filter = |m: &RuleMatch| -> bool {
                let Some(reads) = cross_layer_reads.get(&(m.head, m.rule_idx)) else {
                    return true;
                };
                reads.iter().all(|&(dx, dy, dz, expected_type)| {
                    let target_layer = layer_idx as i64 + dz as i64;
                    if target_layer < 0 || target_layer as usize >= pre_tick_grids.len() {
                        return false;
                    }
                    let tx = m.x as i64 + dx as i64;
                    let ty = m.y as i64 + dy as i64;
                    if tx < 0 || ty < 0 {
                        return false;
                    }
                    pre_tick_grids[target_layer as usize]
                        .get_cell(tx as usize, ty as usize)
                        .is_some_and(|cell| cell.value.0 == expected_type)
                })
            };

            self.layers[layer_idx].run_tick_with_cross_layer_filter(&filter);
        }
    }

    /// `true`, если ХОТЬ ОДНО правило в ХОТЬ ОДНОМ слое использует
    /// `cross_layer_reads` — определяет быстрый путь в [`Self::run_tick`].
    /// Все слои используют один и тот же `rule_index` (см. `new`'s doc-
    /// комментарий), так что достаточно проверить первый слой, если он
    /// есть; пустой стек слоёв (`layers.is_empty()`) тривиально `false`.
    fn any_rule_uses_cross_layer_reads(&self) -> bool {
        self.layers.first().is_some_and(|layer| {
            layer
                .rule_index()
                .values()
                .any(|rules| rules.iter().any(|r| !r.cross_layer_reads.is_empty()))
        })
    }
}

#[cfg(test)]
#[path = "layered_tests.rs"]
mod tests;
