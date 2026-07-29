//! Восьмая сила: статический анализатор конфликтов проверен не на
//! придуманных примерах, а на РЕАЛЬНЫХ конфигах проекта — часть из них
//! специально спроектирована конфликтной, часть — специально
//! бесконфликтной (это видно из их названий/комментариев в repo). Смотрим,
//! совпадает ли вердикт `ConflictGraph::build` с тем, что заявлено.

use cellaria::config::load_config;
use cellaria::ConflictGraph;

fn main() {
    // (файл, ожидаемый вердикт, откуда известно ожидание)
    let cases: &[(&str, bool)] = &[
        ("configs/parallel.yaml", true),   // "Параллельное применение: два непересекающихся правила"
        ("configs/turing.yaml", true),     // "одно правило на состояние" — conflict-free по конструкции
        ("configs/conflict.yaml", false),  // "цепочки пересекаются"
        ("configs/cf_ca_counterexample.yaml", false), // "недетерминизм при равных приоритетах"
        ("configs/worst_case_arbitration.yaml", false), // "худший случай для CA-арбитража"
    ];

    println!("{:<45} | {:>10} | {:>10} | {}", "конфиг", "ожидание", "вердикт", "совпало?");
    println!("{}", "-".repeat(80));

    let mut all_match = true;
    for &(path, expect_cf) in cases {
        let (_grid, rule_index) = match load_config(path) {
            Ok(v) => v,
            Err(e) => {
                println!("{:<45} | не удалось загрузить: {}", path, e);
                continue;
            }
        };
        let mut rules = Vec::new();
        for (_, rs) in rule_index {
            rules.extend(rs);
        }
        let graph = ConflictGraph::build(&rules);
        let actual_cf = graph.is_conflict_free();
        let matched = actual_cf == expect_cf;
        all_match &= matched;
        println!(
            "{:<45} | {:>10} | {:>10} | {}",
            path,
            if expect_cf { "safe" } else { "unsafe" },
            if actual_cf { "safe" } else { "unsafe" },
            if matched { "да" } else { "НЕТ" }
        );
    }

    println!(
        "\n{}",
        if all_match {
            "Все вердикты совпали с ожиданием из репозитория."
        } else {
            "Есть расхождение — см. таблицу выше."
        }
    );
}
