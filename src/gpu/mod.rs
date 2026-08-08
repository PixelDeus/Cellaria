//! Опциональный GPU-бэкенд (feature `gpu`), сосуществующий с CPU [`crate::Engine`].
//!
//! Два пайплайна, выбираемые автоматически по конфигу (не только "v1
//! статичные правила без сдвигов" — сдвиги и запись в соседа тоже
//! поддержаны через раундовый lock-free арбитраж) — см. doc-комментарии
//! [`engine::GpuEngine`] и [`rule_table::GpuRuleTable::needs_arbitration`].
//! Подмножество, которое GPU-путь НЕ поддерживает — [`GpuUnsupportedReason`].

pub mod engine;
pub mod rule_table;

pub use engine::GpuEngine;
pub use rule_table::GpuUnsupportedReason;
