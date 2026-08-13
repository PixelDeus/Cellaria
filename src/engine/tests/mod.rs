//! Тесты `Engine` — разбиты по областям (раньше был один файл на ~5000
//! строк/97 тестов, найдено при подготовке модели к 1.0 как единственный
//! файл на порядок больше любого другого в проекте). Файлы позднее в этом
//! списке (`feedback`/`recursion`/`memory`, объединённые в
//! `extensions_interactions`) намеренно НЕ разнесены по одному расширению
//! на файл — они активно комбинируются друг с другом в самих тестах (см.
//! doc-комментарий в начале `extensions_interactions.rs`), искусственное
//! разделение было бы менее честным, чем один связный файл.

mod common;

mod basics;
mod ca_semantics;
mod cam;
mod broadcast_emit;
mod fairness;
mod persistence;
mod max_activations;
mod extensions_interactions;
mod spatial_arbitration;
