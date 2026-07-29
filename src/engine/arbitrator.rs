use std::cmp::Reverse;
use std::collections::HashMap;

use rayon::prelude::*;

use crate::conflict_analyzer::{get_rule_data, RuleDataCache};
use crate::fast_hash::FxHashSet;
use crate::types::{CellType, OverflowAction, Rule, RuleMatch};

/// Ниже этого числа матчей накладные расходы rayon (work-stealing,
/// синхронизация пула потоков) не окупаются — та же логика, что и
/// `matcher::PARALLEL_THRESHOLD` (см. её doc-комментарий: там это уже
/// измерено на практике для detect_matches, здесь используем то же число
/// для симметрии — оба места про запуск потоков на маленьком объёме
/// работы).
const PARALLEL_SORT_THRESHOLD: usize = 1024;

/// Выбрать непротиворечивый набор совпадений.
///
/// Арбитраж проверяет пересечение РЕАЛЬНЫХ ЗАПИСЕЙ (позиция сдвига +
/// изменения — `RuleData::write_cells`), а не всего паттерна. Это
/// гарантирует, что два совпадения не будут конфликтовать при применении,
/// даже если их паттерны не пересекаются, но их изменения затрагивают одни
/// и те же ячейки — и, симметрично, что два совпадения, чьи паттерны
/// пересекаются (оба читают одну и ту же клетку), но пишут в разные
/// клетки, НЕ считаются конфликтующими: detect_matches всегда читает
/// состояние решётки до тика, так что общее чтение никогда не гонка.
///
/// `bounds` — (width, height) решётки. Нужны для корректного учёта
/// `OverflowAction::Write`: реальная запись при выходе сдвига за границу
/// клэмпится на край решётки (см. `apply_shift_buffered`), а не остаётся на
/// исходной (возможно, отрицательной или запредельной) абстрактной позиции.
/// Без этого два матча — один с обычным сдвигом в пределах решётки, другой
/// с переполняющимся сдвигом — могут писать в одну и ту же реальную клетку,
/// оставаясь "непересекающимися" в абстрактных координатах.
///
/// Использует предвычисленный RuleDataCache для быстрого доступа к
/// affected cells без повторного вычисления.
pub fn arbitrate(
    all_matches: Vec<RuleMatch>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &RuleDataCache,
    bounds: (usize, usize),
    get_cell_age: impl Fn(usize, usize) -> u32,
) -> Vec<RuleMatch> {
    if all_matches.is_empty() {
        return Vec::new();
    }

    let mut accepted: Vec<RuleMatch> = Vec::new();
    // Ключ — (i32, i32), а не (u32, u32): affected cells считаются в
    // абстрактных координатах относительно позиции матча и могут уходить в
    // отрицательные значения (например, у сдвигов/changes с отрицательным
    // смещением рядом с (0,0)). Раньше здесь стоял guard `px >= 0 && py >= 0`,
    // из-за которого такие ячейки вообще не попадали ни в проверку конфликта,
    // ни в used_cells — два матча, оба уходящие в отрицательные координаты,
    // могли пройти арбитраж вместе, хотя их affected regions пересекались.
    // Это особенно опасно с OverflowAction::Write, где реальная запись при
    // overflow клэмпится на границу решётки (уже неотрицательную позицию),
    // а анализ конфликтов смотрел на исходную (отброшенную) координату.
    // FxHashSet, а не стандартный (SipHash) — чисто внутренняя структура,
    // число записей за тик обычно единицы (см. `fast_hash` модуль).
    let mut used_cells: FxHashSet<(i32, i32)> = FxHashSet::default();

    // Полностью детерминированный тай-брейк: priority → age → rule_id →
    // координаты матча → rule_idx. Раньше при равенстве priority+age порядок
    // определялся тем, в каком порядке detect_matches нашёл матчи —
    // implementation-defined и невоспроизводимо в других реализациях (в
    // частности, в раундовом локальном арбитраже, который эту эквивалентность
    // и мотивировал). Явный тай-брейк даёт identical results с любой другой
    // реализацией, использующей тот же порядок сравнения, а не просто
    // "какой-то одинаково безопасный, но другой" результат.
    //
    // `rule_id` здесь — это полный `Rule::id`, а не `RuleMatch::head` (только
    // голова): несколько правил под одной головой могут иметь разный
    // "хвост" id, и тай-брейк должен различать их, а не тут же вырождаться
    // в сравнение по (x, y, rule_idx). `RuleMatch` больше не хранит клон
    // этого id (см. её doc-комментарий) — восстанавливаем его тем же
    // способом, что и `get_priority`/`get_match_affected_cells`.
    //
    // Ключ считаем один раз на элемент вручную (decorate-sort-undecorate), а
    // сортируем `sort_unstable_by` по уже готовому ключу — не
    // `sort_by_cached_key` (стабильная, медленнее на больших объёмах).
    // Стабильность тут не нужна: (x, y, rule_idx) сам по себе уникален для
    // каждого match'а (одно и то же правило не может совпасть в одной и той
    // же позиции дважды в одном `detect_matches`), так что у полного
    // 6-компонентного ключа никогда не бывает двух РАЗНЫХ элементов с
    // одинаковым значением — то есть ничьих, которые стабильность обязана
    // была бы сохранить, попросту не существует: нестабильная сортировка
    // даёт ТОТ ЖЕ порядок, только быстрее.
    //
    // Пробовал заменить на поразрядную (radix) сортировку по каждому полю
    // отдельно — EMPIRICALLY оказалось МЕДЛЕННЕЕ (даже после починки лишнего
    // клонирования): сравнение-based сортировка тут выигрывает за счёт
    // короткого замыкания (tuple-сравнение останавливается на первом
    // несовпавшем поле — для типичных данных, где priority/age у всех
    // matches одинаковы, это 1-2 поля, а не все 6), тогда как radix обязан
    // честно пройти ВСЕ 5 числовых полей на каждый элемент независимо от
    // того, различают ли они вообще что-то в этом конкретном наборе данных.
    // Урок: не всякий "асимптотически лучше" алгоритм быстрее на практике.
    //
    // Параллельная сортировка из rayon (уже зависимость проекта, используется
    // в matcher.rs) — не самописный алгоритм, а готовая, отточенная
    // реализация; порог, ниже которого не стоит платить за потоки, тот же,
    // что и в matcher.rs (см. `PARALLEL_THRESHOLD`).
    let mut keyed: Vec<_> = all_matches
        .iter()
        .map(|m| {
            let (priority, rule_id) = resolve_priority_and_rule_id(m, rule_index);
            let age = get_cell_age(m.x as usize, m.y as usize);
            (
                (
                    Reverse(priority),
                    Reverse(age),
                    Reverse(rule_id),
                    Reverse(m.x),
                    Reverse(m.y),
                    Reverse(m.rule_idx),
                ),
                *m,
            )
        })
        .collect();
    if keyed.len() >= PARALLEL_SORT_THRESHOLD {
        keyed.par_sort_unstable_by(|a, b| a.0.cmp(&b.0));
    } else {
        keyed.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    }
    let sorted: Vec<RuleMatch> = keyed.into_iter().map(|(_, m)| m).collect();

    // Переиспользуемый буфер вместо свежего Vec на каждый матч (см.
    // doc-комментарий `get_match_affected_cells`).
    let mut affected: Vec<(i32, i32)> = Vec::new();
    for m in sorted {
        // Получаем предвычисленные affected cells из кэша
        get_match_affected_cells(&m, rule_index, rule_cache, bounds, &mut affected);
        let conflict = affected.iter().any(|coord| used_cells.contains(coord));

        if !conflict {
            used_cells.extend(affected.iter().copied());
            accepted.push(m);
        }
    }

    accepted
}

/// Приоритет и id правила, сработавшего в данном match'е, — ОДНИМ поиском
/// в `rule_index`, а не двумя раздельными (как было раньше: `get_priority`
/// и `resolve_rule_id` каждый делали свой `rule_index.get(&m.head)...`).
/// Вызывается один раз на КАЖДЫЙ матч при построении ключа сортировки —
/// на миллионах матчей лишний повторный поиск в HashMap заметен.
/// Использует `rule_idx`, а не поиск по одной лишь `head` — несколько правил
/// могут иметь одинаковую голову, и только `rule_idx` однозначно определяет,
/// какое именно правило сработало.
fn resolve_priority_and_rule_id(
    m: &RuleMatch,
    rule_index: &HashMap<CellType, Vec<Rule>>,
) -> (u32, RuleIdKey) {
    match rule_index.get(&m.head).and_then(|rules| rules.get(m.rule_idx)) {
        Some(rule) => (rule.priority, RuleIdKey::from_id(&rule.id)),
        None => (0, RuleIdKey::Small([0u8; 16], 0)),
    }
}

/// Компактная, Copy-версия `Rule::id` для тай-брейка арбитража — вместо
/// `Vec<CellType>` (аллокация в куче на каждый матч). Найдено экспериментально
/// (профилирование `arbitrate()` на 4 млн матчей): `sort_by_cached_key`
/// вычисляет ключ ОДИН раз на элемент, но сам АЛГОРИТМ сортировки сравнивает
/// уже вычисленные ключи O(n log n) раз — и Vec-поле в ключе означает
/// разыменование кучи на КАЖДОЕ такое сравнение, а не только один раз при
/// построении. Для реалистичных id (почти всегда ≤16 клеток) `Small` хранит
/// байты прямо в ключе на стеке — сравнения превращаются в сравнение срезов
/// без единого обращения к куче. Id длиннее 16 клеток (на практике не
/// встречается, но встретиться может) — честный fallback на `Vec<u8>`.
///
/// `CellType` — простая обёртка над `u8` с derived `Ord` (сравнение через
/// внутренний байт), так что сравнение по байтам here даёт РОВНО ТУ ЖЕ
/// сортировку, что и лексикографическое сравнение `Vec<CellType>` раньше.
#[derive(Clone, PartialEq, Eq)]
enum RuleIdKey {
    Small([u8; 16], u8),
    Large(Vec<u8>),
}

impl RuleIdKey {
    fn from_id(id: &[CellType]) -> Self {
        if id.len() <= 16 {
            let mut buf = [0u8; 16];
            for (i, ct) in id.iter().enumerate() {
                buf[i] = ct.0;
            }
            RuleIdKey::Small(buf, id.len() as u8)
        } else {
            RuleIdKey::Large(id.iter().map(|ct| ct.0).collect())
        }
    }

    fn as_slice(&self) -> &[u8] {
        match self {
            RuleIdKey::Small(buf, len) => &buf[..*len as usize],
            RuleIdKey::Large(v) => v.as_slice(),
        }
    }
}

impl PartialOrd for RuleIdKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RuleIdKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

/// Вычислить набор реально ЗАПИСЫВАЕМЫХ ячеек для совпадения, используя кэш.
///
/// Берёт предвычисленные относительные `write_cells` из RuleDataCache (не
/// `affected_cells` — тот включает и клетки паттерна, которые матч только
/// ЧИТАЕТ; конфликтовать могут только записи, см. doc-комментарий
/// `RuleData::write_cells`) и сдвигает их на позицию совпадения. Клетка-цель
/// сдвига дополнительно клэмпится на границы решётки, если у правила
/// `OverflowAction::Write` и сдвиг уходит за пределы решётки — это ровно то,
/// что реально делает `apply_shift_buffered` при overflow-записи.
/// Пишет результат в переданный буфер (очищая его сначала), а не возвращает
/// новый `Vec` — вызывается один раз на КАЖДЫЙ матч в горячем цикле
/// `arbitrate()` (найдено экспериментально при профилировании на 4 млн
/// матчей: свежая аллокация на каждый вызов заметна на таком объёме).
/// Буфер переиспользуется вызывающим кодом между итерациями цикла.
fn get_match_affected_cells(
    m: &RuleMatch,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &RuleDataCache,
    bounds: (usize, usize),
    out: &mut Vec<(i32, i32)>,
) {
    out.clear();
    let head = m.head;

    let rule_data = match get_rule_data(rule_cache, head, m.rule_idx) {
        Some(rd) => rd,
        None => {
            // Правило не найдено в кэше — в норме недостижимо, т.к.
            // rule_cache строится из того же rule_index, что передан сюда,
            // и содержит запись под каждый (head, rule_idx). Консервативный
            // фолбэк: длина id сработавшего правила (или 1, если и его не
            // найти) как ширина паттерна вдоль x, как строился bы паттерн
            // из id по умолчанию (см. `config::load_config`).
            let id_len = rule_index
                .get(&head)
                .and_then(|rules| rules.get(m.rule_idx))
                .map_or(1, |rule| rule.id.len().max(1));
            for i in 0..id_len {
                out.push((m.x as i32 + i as i32, m.y as i32));
            }
            return;
        }
    };

    let (w, h) = (bounds.0 as i32, bounds.1 as i32);
    // Правило с несколькими сдвигами реплицирует значение в КАЖДУЮ цель
    // независимо (см. RuleData::shift_targets) — клэмпинг при
    // OverflowAction::Write применим к любой из них, не только к первой.
    let has_shift = !rule_data.shift_targets.is_empty();
    let overflow: Option<OverflowAction> = if has_shift {
        rule_index
            .get(&head)
            .and_then(|rules| rules.get(m.rule_idx))
            .map(|rule| rule.overflow)
    } else {
        None
    };

    out.extend(rule_data.write_cells.iter().map(|&(dx, dy)| {
        let abs = (m.x as i32 + dx, m.y as i32 + dy);
        if w > 0 && h > 0 && rule_data.shift_targets.contains(&(dx, dy)) {
            if let Some(OverflowAction::Write(_) | OverflowAction::WriteLiteral(_)) = overflow {
                if abs.0 < 0 || abs.0 >= w || abs.1 < 0 || abs.1 >= h {
                    return (abs.0.clamp(0, w - 1), abs.1.clamp(0, h - 1));
                }
            }
        }
        abs
    }));
}
