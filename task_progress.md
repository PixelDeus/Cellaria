# Task Progress

## Задача 1: Детерминизм арбитража
- [ ] Добавить `Clone` для `VecStorage` и `ChunkStorage`
- [ ] Добавить `Clone` для `Grid<S: Clone>` и `Engine<S: Clone>`
- [ ] Написать тест `test_arbitrate_determinism` для `VecStorage`
- [ ] Написать тест `test_arbitrate_determinism_chunk` для `ChunkStorage`

## Задача 2: Достаточные условия остановки
- [ ] Добавить `TerminationVerdict` enum
- [ ] Добавить метод `detect_termination`
- [ ] Написать `test_termination_turing`
- [ ] Написать `test_termination_tag_system`
- [ ] Написать `test_termination_infinite_loop`
- [ ] Написать `test_termination_unknown`

## Проверка
- [ ] `cargo test` — все проходят
- [ ] `cargo clippy` — без новых warnings