use thiserror::Error;

/// Единый тип ошибки для Cellaria.
#[derive(Error, Debug)]
pub enum CellariaError {
    /// Ошибка ввода-вывода (чтение файла и т.п.).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Ошибка парсинга конфигурации.
    #[error("Config parse error: {0}")]
    Config(String),

    /// Ошибка валидации правил.
    #[error("Rule validation error: {0}")]
    RuleValidation(String),

    /// Ошибка протокола RuleStore.
    #[error("Protocol error: {0}")]
    Protocol(String),

    /// Ошибка, связанная с границами решётки.
    #[error("Grid bounds error: {0}")]
    GridBounds(String),
}