//! "Сырые" стейтлес-методы одной фазы тика (`detect_matches`/`arbitrate`/
//! `apply_matches`) — НЕ хранят состояние между вызовами, `cam`/
//! `starvation_after`/`feedback`/`memory` для них no-op. Полный
//! стейтфул-пайплайн — `Engine::run_tick` (`tick.rs`), не эти методы.

use super::*;

impl<S: GridStorage> Engine<S> {
    // ─── Обнаружение совпадений ───

    /// НЕ включает `cam`-совпадения (`types::CamSearch`) — их детекция
    /// (`matcher::detect_cam_matches`) требует найденные позиции для
    /// последующего арбитража, а этот путь (ручная связка
    /// `detect_matches` → `arbitrate` → `apply_matches`) их никуда не
    /// передаёт. Полностью работают только через `run_tick`/`Engine::run_tick`.
    pub fn detect_matches(&self) -> Vec<RuleMatch> {
        let search_coords = resolve_search_coords_peek(&self.grid, &self.search_radius_cache);
        detect_matches_with_group_data(&self.grid, &self.group_cache, &search_coords)
    }

    // ─── Арбитраж ───

    /// См. doc-комментарий `detect_matches` — `cam`-матчи сюда не попадают
    /// (детектируются отдельно, только внутри `run_tick`); `arbitrate`
    /// (свободная функция) сама подставляет пустую карту позиций внутри.
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

    pub fn apply_matches(&mut self, matches: Vec<RuleMatch>) -> (Vec<AffectedRegion>, Vec<(u32, Cell)>) {
        apply_matches(&mut self.grid, matches, &self.rule_index, &self.rule_cache)
    }
}
