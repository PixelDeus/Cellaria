//! Многослойная решётка — стек ОДИНАКОВЫХ по размеру 2D-решёток, НЕ
//! настоящий 3D-мир (см. `Rule::cross_layer_reads`'s doc-комментарий про
//! то, почему это разные вещи). Первый срез: только ЧТЕНИЕ через слои
//! (`Rule::cross_layer_reads`), запись (`changes`/`shifts` на другой слой)
//! не поддерживается — отдельное, более сложное расширение.
//!
//! `LayeredEngine` НЕ трогает `Engine`/`Grid`/`GridStorage` — каждый слой
//! это независимый, немодифицированный `Engine`, арбитрирующий СВОЙ тик
//! полностью самостоятельно (существующий, уже проверенный код).
//! Единственное новое — фильтр между Detect и Arbitrate: кандидат,
//! ссылающийся на другой слой через `cross_layer_reads`, отсеивается,
//! если условие не выполнено на ПРЕДТИКОВОМ состоянии целевого слоя (та же
//! дисциплина снимка тика 2.2.1 — все слои читаются как они были на
//! начало ЭТОГО тика, включая чужие).
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
        let layers = grids.into_iter().map(|grid| Engine::new(grid, rule_index.clone())).collect();
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
        LayeredSnapshot { layers: self.layers.iter().map(Engine::snapshot).collect() }
    }

    /// Восстановить весь стек из снимка ([`LayeredEngine::snapshot`]).
    /// Размеры слоёв снимком гарантированы согласованными (он мог
    /// появиться только из уже провалидированного `LayeredEngine`) —
    /// повторной проверки, в отличие от `new`, здесь не нужно.
    pub fn from_snapshot(snapshot: LayeredSnapshot<S>) -> Self {
        Self { layers: snapshot.layers.into_iter().map(Engine::from_snapshot).collect() }
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

    /// Один тик по ВСЕМ слоям. Порядок:
    /// 1. Detect на КАЖДОМ слое независимо (уже читает только свой,
    ///    предтиковый `Grid` — ничего не меняется здесь).
    /// 2. Фильтр `cross_layer_reads`: отсеивает кандидатов, чьё условие на
    ///    ЧУЖОМ слое (тоже предтиковом — ни один слой ещё не применял свой
    ///    тик на этом шаге) не выполнено.
    /// 3. Arbitrate + Apply на КАЖДОМ слое независимо, существующим,
    ///    немодифицированным `Engine`'s кодом — слои никогда не делят
    ///    клетки по записи (первый срез — только чтение через слои),
    ///    значит и арбитрировать между ними нечего.
    pub fn run_tick(&mut self) {
        let per_layer_matches: Vec<Vec<RuleMatch>> = self.layers.iter().map(Engine::detect_matches).collect();

        let filtered: Vec<Vec<RuleMatch>> = per_layer_matches
            .into_iter()
            .enumerate()
            .map(|(layer_idx, matches)| {
                matches.into_iter().filter(|m| self.cross_layer_condition_holds(layer_idx, m)).collect()
            })
            .collect();

        for (layer_idx, matches) in filtered.into_iter().enumerate() {
            let accepted = self.layers[layer_idx].arbitrate(matches);
            self.layers[layer_idx].apply_matches(accepted);
        }
    }

    /// `true`, если У ПРАВИЛА этого матча нет `cross_layer_reads` вообще
    /// (обычный случай, нулевые накладные расходы) ИЛИ все условия
    /// выполнены. Отсутствующий слой (`dz` уводит за пределы стека) или
    /// отрицательная итоговая координата — жёсткий отказ (условие не
    /// выполнено), симметрично тому, как `pattern` читает за пределами
    /// решётки как `CellValue::default()`, но здесь именно НЕСУЩЕСТВОВАНИЕ
    /// слоя, а не просто пустая клетка — семантически отказ, не "клетка 0".
    fn cross_layer_condition_holds(&self, layer_idx: usize, m: &RuleMatch) -> bool {
        let Some(rule) = self.layers[layer_idx].rule_index().get(&m.head).and_then(|rules| rules.get(m.rule_idx)) else {
            return true; // недостижимо в норме -- см. аналогичный fallback в arbitrator.rs
        };
        if rule.cross_layer_reads.is_empty() {
            return true;
        }
        rule.cross_layer_reads.iter().all(|&(dx, dy, dz, expected_type)| {
            let target_layer = layer_idx as i64 + dz as i64;
            if target_layer < 0 || target_layer as usize >= self.layers.len() {
                return false;
            }
            let tx = m.x as i64 + dx as i64;
            let ty = m.y as i64 + dy as i64;
            if tx < 0 || ty < 0 {
                return false;
            }
            self.layers[target_layer as usize]
                .grid()
                .get_cell(tx as usize, ty as usize)
                .is_some_and(|cell| cell.value.0 == expected_type)
        })
    }
}

#[cfg(test)]
#[path = "layered_tests.rs"]
mod tests;
