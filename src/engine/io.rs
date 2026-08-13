//! Входные/выходные граничные каналы: `push_input`/`apply_input`/
//! `pop_output`/`drain_output`, запись `input_log`/`tick_log`, `replay`.

use super::*;

impl<S: GridStorage> Engine<S> {
    // ─── IO ───

    /// Ищет первый граничный буфер с `direction == "input"`.
    ///
    /// Если включена запись ([`Engine::enable_input_recording`]) — вызов
    /// ЗАПИСЫВАЕТСЯ в `input_log` ДО поиска буфера, безусловно (даже если
    /// подходящего input-буфера почему-то не нашлось) — [`Engine::replay`]
    /// должен воспроизвести ТУ ЖЕ последовательность вызовов, что была на
    /// самом деле, а не только те, что успешно на что-то подействовали.
    pub fn push_input(&mut self, ch: u32, value: u8) {
        if let Some(log) = self.input_log.as_mut() {
            log.push(InputEvent {
                tick: self.grid.generation(),
                channel: ch,
                value,
            });
        }
        for (_, buf) in self.grid.iter_boundaries_mut() {
            if buf.direction == "input" {
                buf.enqueue(ch, Cell::new(value));
                return;
            }
        }
    }

    /// Включить запись вызовов `push_input` в `input_log` (для
    /// [`Engine::replay`]) — с этого момента, не с начала работы движка;
    /// уже прошедшие вызовы `push_input` не восстанавливаются задним числом.
    pub fn enable_input_recording(&mut self) {
        self.input_log.get_or_insert_with(Vec::new);
    }

    /// Текущий журнал записанных вызовов `push_input` — `None`, если запись
    /// не включена.
    pub fn input_log(&self) -> Option<&[InputEvent]> {
        self.input_log.as_deref()
    }

    /// Включить запись структурированного лога тиков ([`TickLogEntry`]) —
    /// с этого момента, не с начала работы движка; уже прошедшие тики не
    /// восстанавливаются задним числом. Каждый последующий вызов
    /// [`Engine::run_tick`] добавляет одну запись.
    pub fn enable_tick_logging(&mut self) {
        self.tick_log.get_or_insert_with(Vec::new);
    }

    /// Текущий структурированный лог тиков — `None`, если запись не
    /// включена. Сериализуется через `serde_json` (не `serde_yaml`, в
    /// отличие от [`Engine::snapshot`] — здесь нет нестроковых ключей
    /// `HashMap`, только плоский `Vec` из полей-примитивов, так что
    /// ограничение `EngineSnapshot`'s doc-комментария сюда не относится).
    pub fn tick_log(&self) -> Option<&[TickLogEntry]> {
        self.tick_log.as_deref()
    }

    /// Восстановить движок из снимка и повторно применить журнал ввода до
    /// (не включая) `target_tick` — воспроизводит РЕАЛЬНУЮ
    /// последовательность `push_input`/`run_tick`, а не просто "дошли до
    /// нужного тика как-то". Пример использования (отладка): нашли
    /// расхождение на тике 1000 → взять снимок и `input_log`, снятые на
    /// тике 900 (или раньше) → `Engine::replay(snapshot, &log, 1000)` →
    /// получить движок ровно в том состоянии, в котором он был бы на тике
    /// 1000 в оригинальном прогоне, и продолжить исследовать оттуда, не
    /// пересчитывая весь прогон с нуля вручную.
    ///
    /// `target_tick` сравнивается с `grid.generation()` — то же число,
    /// которое `InputEvent::tick` записывает при `push_input`, так что
    /// каждое событие подаётся РОВНО перед тем `run_tick()`, который
    /// изначально его и забрал (см. doc-комментарий `InputEvent`).
    ///
    /// `apply_input()` вызывается КАЖДУЮ итерацию, БЕЗУСЛОВНО (даже если
    /// на этот тик нет ни одного события в `log`) — `push_input` только
    /// кладёт значение в очередь граничного буфера, реальный перенос на
    /// решётку делает `apply_input()`, отдельный шаг, не часть `run_tick()`
    /// (см. её doc-комментарий) — канонический паттерн использования (см.
    /// `examples/strength_live_io.rs`) вызывает его каждый тик безусловно,
    /// не только когда только что был `push_input`; `replay` обязан
    /// воспроизвести ТУ ЖЕ последовательность вызовов.
    pub fn replay(snapshot: EngineSnapshot<S>, log: &[InputEvent], target_tick: u64) -> Self {
        let mut engine = Self::from_snapshot(snapshot);
        while engine.grid.generation() < target_tick {
            let current = engine.grid.generation();
            for event in log.iter().filter(|e| e.tick == current) {
                engine.push_input(event.channel, event.value);
            }
            engine.apply_input();
            engine.run_tick();
        }
        engine
    }

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
}
