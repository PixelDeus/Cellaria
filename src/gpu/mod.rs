//! Опциональный GPU-бэкенд (feature `gpu`), сосуществующий с CPU [`crate::Engine`].
//!
//! `v1`-подмножество: статичные правила без сдвигов (см. [`rule_table`]) —
//! шейдер и [`GpuEngine`] появятся отдельными шагами поверх энкодера.

pub mod engine;
pub mod rule_table;

pub use engine::GpuEngine;
pub use rule_table::GpuUnsupportedReason;
