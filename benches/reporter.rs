//! # Benchmark Reporter
//!
//! Структурированный вывод результатов бенчмарков:
//! - Единый формат таблиц (с ANSI-цветами)
//! - JSON-сериализация для машинной обработки
//! - Сравнение с эталоном (baseline check)
//! - Компактный режим для быстрой верификации

use std::collections::HashMap;
use std::time::Instant;

// ============================================================================
// ANSI-цвета
// ============================================================================

mod ansi {
    pub const RESET: &str = "\x1b[0m";
    pub const GREEN: &str = "\x1b[32m";
    pub const YELLOW:&str = "\x1b[33m";
    pub const CYAN:  &str = "\x1b[36m";
    pub const BOLD:  &str = "\x1b[1m";
    pub const DIM:   &str = "\x1b[2m";
}

// ============================================================================
// Типы
// ============================================================================

/// Статус результата
#[derive(Debug, Clone, PartialEq)]
pub enum BenchStatus {
    /// Тест пройден
    Pass,
    /// Пиковое значение (max throughput и т.п.)
    Peak,
    /// Предупреждение (вышло за baseline)
    Warning(String),
    /// Информационный (не проверяется)
    Info,
}

impl BenchStatus {
    pub fn label(&self) -> &str {
        match self {
            BenchStatus::Pass => "PASS",
            BenchStatus::Peak => "PEAK",
            BenchStatus::Warning(_) => "WARN",
            BenchStatus::Info => "INFO",
        }
    }

    pub fn colored_label(&self) -> String {
        match self {
            BenchStatus::Pass => format!("{}{}PASS{}", ansi::GREEN, ansi::BOLD, ansi::RESET),
            BenchStatus::Peak => format!("{}{}PEAK{}", ansi::CYAN, ansi::BOLD, ansi::RESET),
            BenchStatus::Warning(_) => format!("{}{}WARN{}", ansi::YELLOW, ansi::BOLD, ansi::RESET),
            BenchStatus::Info => format!("{}{}INFO{}", ansi::DIM, "INFO", ansi::RESET),
        }
    }
}

/// Один результат замера
#[derive(Debug, Clone)]
pub struct BenchResult {
    /// Имя группы тестов (например "1A: Без сдвига")
    pub group: String,
    /// Имя параметра (например "N=10")
    pub param: String,
    /// Значение (например количество тиков)
    pub value: String,
    /// Единица измерения
    pub unit: String,
    /// Дополнительная метрика (например тиков/сек)
    pub extra: Option<(String, String)>,
    /// Статус
    pub status: BenchStatus,
    /// Время замера (Unix timestamp)
    pub timestamp: u128,
}

impl BenchResult {
    pub fn new(group: &str, param: &str, value: &str, unit: &str) -> Self {
        Self {
            group: group.to_string(),
            param: param.to_string(),
            value: value.to_string(),
            unit: unit.to_string(),
            extra: None,
            status: BenchStatus::Info,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
        }
    }

    pub fn with_extra(mut self, name: &str, value: &str) -> Self {
        self.extra = Some((name.to_string(), value.to_string()));
        self
    }

    pub fn pass(mut self) -> Self {
        self.status = BenchStatus::Pass;
        self
    }

    pub fn peak(mut self) -> Self {
        self.status = BenchStatus::Peak;
        self
    }
}

// ============================================================================
// Форматированный вывод
// ============================================================================

/// Тип строки таблицы: заголовок, данные
enum TableRow {
    Header(Vec<String>),
    Data(Vec<String>),
}

/// Нарисовать таблицу из строк
fn render_table(rows: &[TableRow], compact: bool) -> String {
    if rows.is_empty() || compact {
        return String::new();
    }

    // Определяем количество колонок
    let cols = match &rows[0] {
        TableRow::Header(h) => h.len(),
        TableRow::Data(d) => d.len(),
    };

    if cols == 0 {
        return String::new();
    }

    // Вычисляем ширину каждой колонки
    let mut widths = vec![0usize; cols];
    for row in rows {
        match row {
            TableRow::Header(h) => {
                for (i, cell) in h.iter().enumerate() {
                    widths[i] = widths[i].max(cell.len());
                }
            }
            TableRow::Data(d) => {
                for (i, cell) in d.iter().enumerate() {
                    widths[i] = widths[i].max(cell.len());
                }
            }
        }
    }

    // Добавляем отступы
    for w in &mut widths {
        *w += 2; // 1 пробел с каждой стороны
    }

    let mut out = String::new();

    for row in rows {
        match row {
            TableRow::Header(h) => {
                // Верхняя граница
                out.push_str("┌");
                for (i, w) in widths.iter().enumerate() {
                    out.push_str(&"─".repeat(*w));
                    if i < widths.len() - 1 {
                        out.push_str("┬");
                    }
                }
                out.push_str("┐\n");

                // Заголовок
                out.push_str("│");
                for (i, cell) in h.iter().enumerate() {
                    let w = widths[i];
                    let padding = w - cell.len() - 1;
                    out.push(' ');
                    out.push_str(cell);
                    out.push_str(&" ".repeat(padding));
                    out.push('│');
                }
                out.push('\n');

                // Разделитель под заголовком
                out.push_str("├");
                for (i, w) in widths.iter().enumerate() {
                    out.push_str(&"─".repeat(*w));
                    if i < widths.len() - 1 {
                        out.push_str("┼");
                    }
                }
                out.push_str("┤\n");
            }
            TableRow::Data(d) => {
                out.push_str("│");
                for (i, cell) in d.iter().enumerate() {
                    let w = widths[i];
                    // Для чисел — выравнивание вправо
                    let is_numeric = cell.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',' || c == ' ' || c == '│' || c == '└' || c == '┘');
                    let (left, right) = if is_numeric && cell.trim().len() > 0 {
                        let trimmed = cell.trim();
                        let padding = w - trimmed.len() - 1;
                        (" ".repeat(padding), " ".to_string())
                    } else {
                        let padding = w - cell.len() - 1;
                        (" ".to_string(), " ".repeat(padding))
                    };
                    out.push_str(&left);
                    out.push_str(cell);
                    out.push_str(&right);
                    out.push('│');
                }
                out.push('\n');
            }
        }
    }

    // Нижняя граница
    out.push_str("└");
    for (i, w) in widths.iter().enumerate() {
        out.push_str(&"─".repeat(*w));
        if i < widths.len() - 1 {
            out.push_str("┴");
        }
    }
    out.push_str("┘\n");

    out
}

// ============================================================================
// Reporter
// ============================================================================

/// Основной репортёр
pub struct Reporter {
    results: Vec<BenchResult>,
    start_time: Instant,
    json_output: bool,
    compact: bool,
    quiet: bool,
    baseline: Option<HashMap<String, f64>>,
    output_path: Option<String>,
}

impl Reporter {
    pub fn new() -> Self {
        Self {
            results: Vec::new(),
            start_time: Instant::now(),
            json_output: false,
            compact: false,
            quiet: false,
            baseline: None,
            output_path: None,
        }
    }

    /// Включить JSON-вывод
    pub fn set_json(&mut self, val: bool) {
        self.json_output = val;
    }

    /// Включить компактный режим (одна строка на результат)
    pub fn set_compact(&mut self, val: bool) {
        self.compact = val;
    }

    /// Включить тихий режим (только summary)
    pub fn set_quiet(&mut self, val: bool) {
        self.quiet = val;
    }

    /// Загрузить baseline из JSON.
    ///
    /// Раньше ожидался только плоский `{"group:param": value}` — но именно
    /// такой файл этот же Reporter никогда не производит: `--output`
    /// сохраняет вложенную структуру `{"metadata": ..., "results": [...]}`
    /// (см. `to_json`). Задокументированный сценарий "сохранить baseline
    /// через --output, потом свериться через --check" падал на этапе
    /// загрузки. Теперь понимаются оба формата.
    pub fn load_baseline(&mut self, path: &str) -> Result<(), String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Не удалось прочитать {}: {}", path, e))?;
        let parsed: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("Не удалось распарсить {}: {}", path, e))?;

        let mut baseline: HashMap<String, f64> = HashMap::new();

        if let Some(results) = parsed.get("results").and_then(|r| r.as_array()) {
            // Формат полного вывода --output: results — массив BenchResult.
            for entry in results {
                let group = entry.get("group").and_then(|v| v.as_str());
                let param = entry.get("param").and_then(|v| v.as_str());
                let value = entry.get("value").and_then(|v| v.as_str());
                if let (Some(group), Some(param), Some(value)) = (group, param, value) {
                    if let Ok(num) = value.replace(' ', "").replace(',', ".").parse::<f64>() {
                        baseline.insert(format!("{}:{}", group, param), num);
                    }
                }
            }
        } else if let Some(obj) = parsed.as_object() {
            // Плоский формат: {"group:param": value}.
            for (k, v) in obj {
                if let Some(num) = v.as_f64() {
                    baseline.insert(k.clone(), num);
                }
            }
        } else {
            return Err(format!("Неизвестный формат baseline-файла {}", path));
        }

        self.baseline = Some(baseline);
        Ok(())
    }

    /// Установить путь для сохранения JSON
    pub fn set_output_path(&mut self, path: &str) {
        self.output_path = Some(path.to_string());
    }

    /// Добавить результат
    pub fn add(&mut self, result: BenchResult) {
        self.results.push(result);
    }

    /// Добавить несколько результатов
    pub fn add_all(&mut self, results: Vec<BenchResult>) {
        self.results.extend(results);
    }

    /// Напечатать заголовок фазы
    pub fn print_header(&self, title: &str) {
        if self.json_output || self.quiet || self.compact {
            return;
        }
        println!("\n{}{}═══════════════════════════════════════{}", ansi::BOLD, ansi::CYAN, ansi::RESET);
        println!("  {}", title);
        println!("═══════════════════════════════════════{}", ansi::RESET);
    }

    /// Напечатать подзаголовок
    pub fn print_subheader(&self, title: &str) {
        if self.json_output || self.quiet || self.compact {
            return;
        }
        println!("\n{}{}─── {} ───{}", ansi::BOLD, ansi::CYAN, title, ansi::RESET);
    }

    /// Напечатать таблицу из результатов
    pub fn print_table(&self, results: &[BenchResult], columns: &[&str]) {
        if self.json_output || self.quiet {
            return;
        }

        if results.is_empty() {
            return;
        }

        if self.compact {
            // Компактный вывод: одна строка на результат
            for r in results {
                let status_str = r.status.colored_label();
                let extra_str = if let Some((name, val)) = &r.extra {
                    format!(" {}{}{}: {}", ansi::DIM, name, ansi::RESET, val)
                } else {
                    String::new()
                };
                println!(
                    "  {}{:<20} {}{:<10} {}{} {} {}",
                    ansi::BOLD, r.group, ansi::DIM, r.param, ansi::RESET,
                    format!("{}{}", r.value, r.unit),
                    extra_str,
                    status_str,
                );
            }
            return;
        }

        let mut rows: Vec<TableRow> = Vec::new();

        // Заголовок
        rows.push(TableRow::Header(
            columns.iter().map(|s| s.to_string()).collect()
        ));

        // Данные
        for r in results {
            let mut row = Vec::new();
            for col in columns {
                match *col {
                    "Параметр" => row.push(r.param.clone()),
                    "Значение" => row.push(format!("{} {}", r.value, r.unit)),
                    "Дополнительно" => {
                        if let Some((name, val)) = &r.extra {
                            row.push(format!("{}: {}", name, val));
                        } else {
                            row.push("—".to_string());
                        }
                    }
                    "Статус" => row.push(r.status.label().to_string()),
                    _ => row.push("?".to_string()),
                }
            }
            rows.push(TableRow::Data(row));
        }

        print!("{}", render_table(&rows, self.compact));
    }

    /// Напечатать информацию
    pub fn print_info(&self, msg: &str) {
        if self.json_output || self.quiet {
            return;
        }
        println!("  {}", msg);
    }

    /// Напечатать пустую строку
    pub fn print_blank(&self) {
        if self.json_output || self.quiet {
            return;
        }
        println!();
    }

    /// Финальная сводка.
    ///
    /// Раньше сохранение в `--output` было зашито только в "полную" ветку
    /// вывода — при `--json` или `--quiet`/`--compact` функция делала
    /// `return` раньше, чем доходила до кода сохранения, так что
    /// `--json --output file.json` (ровно то, что документация называет
    /// способом сохранить baseline) молча ничего не сохраняло. Теперь
    /// сохранение происходит один раз, независимо от режима вывода.
    pub fn print_summary(&self) {
        let total = self.results.len();
        let passed = self.results.iter().filter(|r| r.status == BenchStatus::Pass).count();
        let warnings: Vec<&BenchResult> = self.results.iter().filter(|r| matches!(r.status, BenchStatus::Warning(_))).collect();
        let peaks = self.results.iter().filter(|r| r.status == BenchStatus::Peak).count();

        let save_result = self.output_path.as_ref().map(|path| {
            let json = self.to_json();
            (path.clone(), std::fs::write(path, &json))
        });

        if self.json_output {
            // Вывод JSON
            let json = self.to_json();
            println!("{}", json);
            if let Some((path, result)) = &save_result {
                if let Err(e) = result {
                    eprintln!("Ошибка сохранения JSON в {}: {}", path, e);
                }
            }
            return;
        }

        // Компактная сводка
        if self.quiet || self.compact {
            print!("{}{}═══ СВОДКА ═══{}", ansi::BOLD, ansi::CYAN, ansi::RESET);
            print!("  Всего: {}", total);
            print!("  {}{}{}{}", ansi::GREEN, "✓", passed, ansi::RESET);
            if peaks > 0 {
                print!("  {}{}★{} {}", ansi::CYAN, ansi::BOLD, ansi::RESET, peaks);
            }
            if !warnings.is_empty() {
                print!("  {}{}⚠{} {}", ansi::YELLOW, ansi::BOLD, ansi::RESET, warnings.len());
            }
            let elapsed = self.start_time.elapsed();
            println!("  {}{}.{:03}s{}", ansi::DIM, elapsed.as_secs(), elapsed.subsec_millis(), ansi::RESET);
            if let Some((path, result)) = &save_result {
                match result {
                    Ok(()) => println!("  {}Сохранено в {}{}", ansi::DIM, path, ansi::RESET),
                    Err(e) => eprintln!("  Ошибка сохранения JSON в {}: {}", path, e),
                }
            }
            return;
        }

        println!("\n{}═══════════════════════════════════════{}", ansi::BOLD, ansi::RESET);
        println!("  {}{}СВОДКА{}", ansi::BOLD, ansi::CYAN, ansi::RESET);
        println!("═══════════════════════════════════════");
        println!("  Всего тестов:    {}", total);
        println!("  {}{}Пройдено:        {}{}", ansi::GREEN, ansi::BOLD, passed, ansi::RESET);
        println!("  {}{}Пиковых:         {}{}", ansi::CYAN, ansi::BOLD, peaks, ansi::RESET);
        if !warnings.is_empty() {
            println!("  {}{}Предупреждений:  {}{}", ansi::YELLOW, ansi::BOLD, warnings.len(), ansi::RESET);
            for w in &warnings {
                if let BenchStatus::Warning(msg) = &w.status {
                    println!("    {}{}⚠{} {}: {}", ansi::YELLOW, ansi::BOLD, ansi::RESET, w.group, msg);
                }
            }
        }
        let elapsed = self.start_time.elapsed();
        println!("  {}{}Время выполнения: {}.{:03}s{}", ansi::DIM, ansi::BOLD, elapsed.as_secs(), elapsed.subsec_millis(), ansi::RESET);

        if let Some((path, result)) = &save_result {
            match result {
                Ok(()) => println!("  {}{}Результаты сохранены в {}{}", ansi::GREEN, ansi::BOLD, path, ansi::RESET),
                Err(e) => eprintln!("  Ошибка сохранения JSON в {}: {}", path, e),
            }
        }

        println!("═══════════════════════════════════════{}", ansi::RESET);
    }

    /// Сериализация в JSON
    pub fn to_json(&self) -> String {
        let mut entries = Vec::new();
        for r in &self.results {
            let mut entry = serde_json::json!({
                "group": r.group,
                "param": r.param,
                "value": r.value,
                "unit": r.unit,
                "status": r.status.label(),
                "timestamp": r.timestamp,
            });
            if let Some((name, val)) = &r.extra {
                entry["extra_name"] = serde_json::json!(name);
                entry["extra_value"] = serde_json::json!(val);
            }
            if let BenchStatus::Warning(msg) = &r.status {
                entry["warning"] = serde_json::json!(msg);
            }
            entries.push(entry);
        }

        let output = serde_json::json!({
            "metadata": {
                "timestamp": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
                "total": self.results.len(),
                "passed": self.results.iter().filter(|r| r.status == BenchStatus::Pass).count(),
                "warnings": self.results.iter().filter(|r| matches!(r.status, BenchStatus::Warning(_))).count(),
            },
            "results": entries,
        });

        serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string())
    }

    /// Проверка с baseline.
    ///
    /// Раньше регрессом считался только рост значения (`ratio > 1.5`) — это
    /// верно для метрик времени (ns, µs — рост = медленнее = хуже), но для
    /// throughput-метрик (unit "тиков" — больше = быстрее = лучше) регресс
    /// это ПАДЕНИЕ значения, а не рост. Со старой логикой падение
    /// throughput (например, с 700k до 300k тиков) не ловилось вообще, а
    /// улучшение throughput ложно помечалось как "warning".
    pub fn check_baseline(&self) -> Vec<BenchResult> {
        let Some(baseline) = &self.baseline else {
            return Vec::new();
        };

        let mut warnings = Vec::new();

        for r in &self.results {
            let key = format!("{}:{}", r.group, r.param);
            if let Some(expected) = baseline.get(&key) {
                // Пытаемся распарсить значение как число
                let val: f64 = r.value.replace(' ', "").replace(',', ".").parse().unwrap_or(0.0);
                if *expected <= 0.0 {
                    continue;
                }

                let higher_is_better = r.unit == "тиков";
                let (regressed, message) = if higher_is_better {
                    let ratio = expected / val.max(f64::MIN_POSITIVE);
                    (
                        val < expected / 1.5,
                        format!(
                            "Throughput {:.2} упал относительно baseline {:.2} в {:.2} раз",
                            val, expected, ratio
                        ),
                    )
                } else {
                    let ratio = val / expected;
                    (
                        ratio > 1.5,
                        format!(
                            "Значение {:.2} превышает baseline {:.2} в {:.2} раз",
                            val, expected, ratio
                        ),
                    )
                };

                if regressed {
                    let mut w = r.clone();
                    w.status = BenchStatus::Warning(message);
                    warnings.push(w);
                }
            }
        }

        warnings
    }
}

// ============================================================================
// Утилиты
// ============================================================================

/// Форматировать число с разделителями разрядов
pub fn format_number(n: u128) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.insert(0, ' ');
        }
        result.insert(0, c);
    }
    result
}

/// Форматировать число с плавающей точкой
pub fn format_f64(val: f64, decimals: usize) -> String {
    format!("{:.1$}", val, decimals)
}