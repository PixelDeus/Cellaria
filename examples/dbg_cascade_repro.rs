use std::collections::HashMap;

use cellaria::engine::Engine;
use cellaria::types::{Cell, CellType, CellValue, Direction, Rule, ShiftSpec};
use cellaria::{Grid, VecStorage};

const WIDTH: usize = 40;
const SRC: u8 = 50;

fn emit_rule(direction: Direction) -> HashMap<CellType, Vec<Rule>> {
    let mut idx = HashMap::new();
    idx.insert(
        CellType(SRC),
        vec![Rule {
            id: vec![CellType(SRC)],
            pattern: vec![],
            shifts: vec![vec![ShiftSpec { direction, steps: 1, broadcast: false, keep_source: true }]],
            changes: vec![],
            active_only: false,
            priority: 10,
            min_age: 0,
            overflow: Default::default(),
            cam: None,
            tie_break: 0,
            starvation_after: None,
            feedback: None,
            recursion: None,
            memory: None, max_activations: None, cross_layer_reads: Vec::new(),
        }],
    );
    idx
}

fn snapshot(engine: &Engine<VecStorage>) -> Vec<u8> {
    (0..WIDTH).map(|x| engine.grid().get_cell(x, 0).map(|c| c.value.0 .0).unwrap_or(0)).collect()
}

fn born_ats(engine: &Engine<VecStorage>) -> Vec<(usize, u64)> {
    (0..WIDTH).filter_map(|x| engine.grid().get_cell(x, 0).filter(|c| c.value.0 .0 == SRC).map(|c| (x, c.born_at))).collect()
}

fn main() {
    let mut grid = Grid::new(VecStorage::new(WIDTH, 1), Default::default());
    grid.set_cell(20, 0, Cell { value: CellValue(CellType(SRC)), born_at: 0 });
    let mut engine = Engine::new(grid, emit_rule(Direction::Right));
    println!("start: {:?} gen={} born_ats={:?}", snapshot(&engine), engine.grid().generation(), born_ats(&engine));
    for t in 1..=8 {
        engine.run_tick();
        println!("tick {t}: {:?} gen={} born_ats={:?}", snapshot(&engine), engine.grid().generation(), born_ats(&engine));
    }
}
