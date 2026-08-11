use std::cmp::Reverse;
use std::collections::HashMap;

use rayon::prelude::*;

use crate::conflict_analyzer::{get_rule_data, RuleDataCache};
use crate::engine::matcher::CamPositions;
use crate::engine::{build_head_index, lookup_rule, HeadRuleIndex};
use crate::fast_hash::{FxHashMap, FxHashSet};
use crate::types::{CellType, OverflowAction, Rule, RuleMatch};

/// Счётчики "сколько тиков подряд этот матч проигрывал арбитраж" — см.
/// doc-комментарий `Engine::starvation_counters` и `Rule::starvation_after`.
/// Ключ — тот же `(x, y, rule_idx)`, что и у `CamPositions` (позиция матча +
/// индекс сработавшего правила).
pub(crate) type StarvationCounters = FxHashMap<(u32, u32, usize), u32>;

/// Счётчики "сколько тиков подряд этот матч детектировался" (защёлка, не
/// сбрасывается) — см. `Engine::feedback_counters` и `Rule::feedback`. Тот
/// же ключ `(x, y, rule_idx)`.
pub(crate) type FeedbackCounters = FxHashMap<(u32, u32, usize), u64>;

/// FIFO-буферы наблюдений памяти (см. `Engine::memory_buffers` и
/// `Rule::memory`) — тот же ключ `(x, y, rule_idx)`, что и у остальных
/// per-match Engine-состояний.
pub(crate) type MemoryBuffers = FxHashMap<(u32, u32, usize), std::collections::VecDeque<crate::types::RecordedValue>>;

/// Счётчики побед в арбитраже для правил с `Rule::max_activations` — см. её
/// doc-комментарий и `Engine::activation_counters`. Ключ — `(head, rule_idx)`,
/// БЕЗ позиции (глобальный бюджет для правила, не для клетки) — в отличие
/// от остальных счётчиков в этом модуле, все с ключом `(x, y, rule_idx)`.
pub(crate) type ActivationCounters = FxHashMap<(CellType, usize), u32>;

/// Ниже этого числа матчей накладные расходы rayon (work-stealing,
/// синхронизация пула потоков) не окупаются — та же логика, что и
/// `matcher::PARALLEL_THRESHOLD` (см. её doc-комментарий: там это уже
/// измерено на практике для detect_matches, здесь используем то же число
/// для симметрии — оба места про запуск потоков на маленьком объёме
/// работы).
const PARALLEL_SORT_THRESHOLD: usize = 1024;

thread_local! {
    /// Персистентный (на весь OS-поток, не на один вызов) буфер для
    /// `used_cells` в greedy-цикле `arbitrate_with_cam` — см. её
    /// doc-комментарий про то, почему `thread_local!`, а не поле `Engine`.
    static USED_CELLS_BUF: std::cell::RefCell<FxHashMap<(i32, i32), usize>> =
        std::cell::RefCell::new(FxHashMap::default());
    /// То же самое для `affected` — переиспользуемого буфера affected cells
    /// ОДНОГО матча (см. `get_match_affected_cells`'s doc-комментарий про
    /// сам буфер; здесь персистентна именно ВНЕШНЯЯ аллокация `Vec`,
    /// пережившая один вызов `arbitrate_with_cam`, не только один матч
    /// внутри него — та же цена холодного старта, что и у `used_cells`).
    static AFFECTED_BUF: std::cell::RefCell<Vec<(i32, i32)>> =
        std::cell::RefCell::new(Vec::new());
    /// Персистентный (на весь OS-поток) буфер для `keyed` — decorate-фаза
    /// сортировки в `arbitrate_with_cam` (см. её doc-комментарий у
    /// `USED_CELLS_BUF` про причину `thread_local!`, не поля `Engine`).
    /// Замерено (контролируемый A/B, прогретый `Vec` против свежего
    /// `Vec::new()`+`collect()` на каждый вызов, реалистичный объём
    /// CA-пробки, вместе с самой сортировкой): -30.6%.
    static KEYED_BUF: std::cell::RefCell<Vec<KeyedEntry>> =
        std::cell::RefCell::new(Vec::new());
}

/// Тип одного элемента `keyed` -- см. `KEYED_BUF`/`arbitrate_with_cam`.
type KeyedEntry = (
    (Reverse<u32>, Reverse<u32>, Reverse<u32>, Reverse<RuleIdKey>, Reverse<u32>, Reverse<u32>, Reverse<usize>),
    RuleMatch,
    u32,
    u32,
);

/// Модуль для вращения `Rule::tie_break` по поколениям (см. её doc-комментарий
/// в `types.rs`). КРИТИЧНО, что CPU и GPU (`shader.wgsl::TIE_BREAK_MODULUS`)
/// используют ОДНО И ТО ЖЕ число — иначе побитовое совпадение результатов
/// сломается на любом правиле, где `tie_break != 0`.
///
/// Небольшая степень двойки, а не большое простое: для ДВУХ соперничающих
/// правил с residues `a` и `a+d (mod M)` победитель меняется РОВНО в те `d`
/// поколений из каждого периода `M`, где сложение с generation переносит
/// меньший residue через границу модуля раньше большего — то есть частота
/// чередования определяется РАЗНОСТЬЮ `d`, не абсолютным размером `M`.
/// Отсюда рецепт для СТРОГО поровну (50/50) чередования двух правил:
/// расставить их `tie_break` РОВНО на `M/2` друг от друга (например, `0` и
/// `8` при `M=16` — proверено `test_tie_break_rotates_fairly_when_spaced_half_modulus_apart`).
/// Для K-стороннего round-robin — на `M/K` друг от друга. Небольшой модуль
/// делает это наблюдаемым за единицы-десятки тиков (а не сотни/тысячи, как
/// было бы при большом простом), и `& (M-1)` дешевле `%` на GPU, раз `M` —
/// степень двойки.
pub(crate) const TIE_BREAK_MODULUS: u32 = 16;

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
    // generation=0: без реального счётчика поколений `tie_break` вырождается
    // в постоянное значение (см. doc-комментарий `Rule::tie_break`) —
    // корректно (не паникует, не расходится с CPU-эталоном), просто не
    // вращается между вызовами; только `run_tick`/`Engine::run_tick` знают
    // настоящее поколение (см. doc-комментарий `arbitrate_with_cam` про ту
    // же причину для `cam_positions`). Пустые `StarvationCounters`/
    // `FeedbackCounters` — та же история: свободная функция не хранит
    // состояние МЕЖДУ вызовами, так что `Rule::starvation_after`/
    // `Rule::feedback` для неё всегда no-op (см. их doc-комментарии).
    //
    // `TieBreakDecidedWins` (второй элемент результата `arbitrate_with_cam`)
    // здесь отбрасывается сознательно — эта свободная функция не хранит
    // `starvation_counters` между вызовами (см. выше), поэтому решать по
    // нему всё равно нечего; публичная сигнатура остаётся прежней ради
    // обратной совместимости (`property_arbitration.rs`/
    // `local_arbitration_research.rs` уже держатся на `Vec<RuleMatch>`).
    arbitrate_with_cam(
        all_matches,
        rule_index,
        rule_cache,
        bounds,
        &CamPositions::default(),
        0,
        &StarvationCounters::default(),
        &FeedbackCounters::default(),
        &[],
        get_cell_age,
    )
    .0
}

/// Как [`arbitrate`], но с картой найденных CAM-позиций (см.
/// `matcher::detect_cam_matches`) — `CamPositions` опирается на
/// `pub(crate) FxHashMap`, так что не может появиться в сигнатуре ПУБЛИЧНОЙ
/// `arbitrate` без утечки приватности типа наружу; только `run_tick`/
/// `Engine::run_tick` имеют что сюда передать (см. doc-комментарий
/// `Engine::detect_matches`), поэтому `arbitrate` остаётся с прежней
/// сигнатурой и просто подставляет пустую карту.
///
/// `generation` — текущее поколение (см. `grid.generation()`), используется
/// ТОЛЬКО для вращения `Rule::tie_break` (см. её doc-комментарий и
/// `TIE_BREAK_MODULUS`); не влияет ни на что другое в арбитраже.
///
/// `starvation_counters` — см. `Rule::starvation_after` и
/// `Engine::starvation_counters`: для матча с `counters[(x,y,rule_idx)] >=
/// rule.starvation_after`, эффективный priority на ЭТОТ тик становится
/// `u32::MAX` (побеждает гарантированно). Обновление счётчиков (инкремент
/// проигравших, сброс выигравших) — забота ВЫЗЫВАЮЩЕЙ стороны
/// (`run_tick_with_cache`), не этой функции: она только ЧИТАЕТ счётчики.
/// Ключи (`x, y, rule_idx`) принятых совпадений, чья победа НЕ была решена
/// одним `priority`/`age` — у них был конкурент (пересекающийся affected
/// region) с РАВНЫМИ `priority` и `age`, и решающим оказался `tie_break`
/// (или более низкий уровень тай-брейка — `rule_id`/координаты). См.
/// `Engine::starvation_counters`'s использование этого множества: победа
/// НЕ из этого множества — решительная (сбрасывает счётчик голодания),
/// победа ИЗ этого множества — "ничья, разрешённая жребием" (счётчик не
/// трогается, ни сброс, ни инкремент).
pub(crate) type TieBreakDecidedWins = FxHashSet<(u32, u32, usize)>;

#[allow(clippy::too_many_arguments)]
pub(crate) fn arbitrate_with_cam(
    all_matches: Vec<RuleMatch>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &RuleDataCache,
    bounds: (usize, usize),
    cam_positions: &CamPositions,
    generation: u32,
    starvation_counters: &StarvationCounters,
    feedback_counters: &FeedbackCounters,
    // Клетки, УЖЕ окончательно занятые кем-то ВНЕ этого вызова (см.
    // doc-комментарий `reserved_cells` в `arbitrate_spatial_with_cam` про
    // единственного реального потребителя — `safe_accepted` в финальном
    // merge-проходе). Пусто (`&[]`) для ВСЕХ остальных вызовов — нулевая
    // цена (один лишний пустой цикл `for` по пустому срезу), сигнатура и
    // поведение для них не меняются.
    reserved: &[(i32, i32)],
    get_cell_age: impl Fn(usize, usize) -> u32,
) -> (Vec<RuleMatch>, TieBreakDecidedWins) {
    if all_matches.is_empty() {
        return (Vec::new(), FxHashSet::default());
    }
    // Массив-lookup вместо `rule_index.get()` — см. doc-комментарий
    // `HeadRuleIndex`/`build_head_index` (сессия 2026-08-09, "фантазия" п.1).
    let head_index = build_head_index(rule_index);

    let mut accepted: Vec<RuleMatch> = Vec::new();
    // Priority/age КАЖДОГО принятого совпадения, тем же индексом, что и в
    // `accepted` -- нужно повторно на этапе разрешения конфликтов ниже, чтобы
    // определить, был ли отклонённый кандидат РОВНЕЙ владельцу занятой клетки
    // (т.е. владелец победил не priority/age, а чем-то ниже).
    let mut accepted_priority_age: Vec<(u32, u32)> = Vec::new();
    // Индексы (в `accepted`) совпадений, чья победа оказалась решена не
    // priority/age -- см. `TieBreakDecidedWins`.
    let mut tie_break_decided: FxHashSet<usize> = FxHashSet::default();
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
    // FxHashMap, а не стандартный (SipHash) — чисто внутренняя структура,
    // число записей за тик обычно единицы... НЕТ, неверно для плотных
    // сценариев (см. ниже) — доккомент ошибался, число записей масштабируется
    // с числом принятых матчей, до ~90000 на CA-сценарии "пробка". Значение —
    // индекс В `accepted` совпадения, занявшего клетку (не просто факт
    // занятости, как раньше) — нужен, чтобы при отклонении конфликтующего
    // кандидата понять, ЧЕЙ именно была победа и была ли она решена
    // priority/age или чем-то ниже (см. `tie_break_decided` выше).
    //
    // `thread_local!`, не поле `Engine` (сессия 2026-08-09, "фантазия" п.1,
    // продолжение) — `arbitrate_with_cam` вызывается ПАРАЛЛЕЛЬНО по полосам
    // из `arbitrate_spatial_with_cam` (до ~25 раз за тик на CA через rayon),
    // так что один персистентный буфер уровня `Engine` сюда physически не
    // воткнуть — несколько потоков не могут безопасно разделять один и тот
    // же `&mut`. `thread_local!` даёт КАЖДОМУ rayon-воркеру свой персистентный
    // буфер, переживающий не только вызовы в пределах одного тика (полоса за
    // полосой на одном потоке), но и между тиками — тот же принцип, что и у
    // `Engine::write_buffer`, просто область персистентности не "на Engine",
    // а "на OS-поток". Замерено (контролируемый A/B, прогретый персистентный
    // буфер против холодного `FxHashMap::default()` на каждый вызов,
    // реалистичный объём CA-пробки): -65.2%.

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
    // `KEYED_BUF` -- thread_local, не свежий `Vec::new()`+`collect()` (см.
    // её doc-комментарий) -- decorate/сортировка/распаковка все внутри
    // одного заимствования, `.drain(..)` в конце опустошает буфер, отдавая
    // владение элементами для `sorted`, но СОХРАНЯЯ выросшую capacity для
    // следующего вызова (в отличие от `.into_iter()` на владеющем `Vec`).
    //
    // Приоритет/возраст уносим вместе с матчем в `sorted` (не только для
    // сортировки) -- нужны повторно ниже, чтобы сравнить отклонённого
    // кандидата с владельцем занятой клетки, а не только для построения
    // ключа сортировки.
    //
    // `resolve_sort_fields` находит `&Rule` для тай-брейка (см. её doc-
    // комментарий), и `get_match_affected_cells` ниже находит его ЖЕ ещё
    // раз (для affected cells) — на первый взгляд дублирующийся lookup.
    // Проверено (сессия 2026-08-09, список "вопросы" п.2): перенос
    // найденного `&Rule` через `sorted`, чтобы убрать повторный поиск, дал
    // -1.7% (шум, не выигрыш — измерено A/B на CA-сценарии "пробка",
    // 89700 матчей, интерливинг old/new/new против погрешности прогрева).
    // Не тот lookup доминирует в стоимости цикла — оставлено как есть.
    // РЕАЛЬНЫЙ БАГ, найден 2026-08-11 при замере плотной производительности
    // на масштабе ~1M клеток (`__investigate_city_scale_dense_perf`):
    // "RefCell already borrowed" panic — `keyed.par_sort_unstable_by` ниже
    // раньше вызывался ВНУТРИ `KEYED_BUF.with_borrow_mut(...)`, держа
    // заимствование на всё время параллельной сортировки. rayon's
    // work-stealing может увести ТОТ ЖЕ OS-поток на ДРУГОЙ вызов
    // `arbitrate_with_cam` (соседняя полоса ИЛИ вообще другой тест/движок,
    // делящий один и тот же глобальный rayon-пул) ПОКА исходный вызов ещё
    // не вернулся из сортировки (ждёт свои подзадачи через `join`) —
    // второй вызов пытается занять ТОТ ЖЕ `RefCell` на ТОМ ЖЕ потоке и
    // падает. Раньше не проявлялось: `PARALLEL_SORT_THRESHOLD=1024`
    // редко превышался per-полосным числом матчей до сегодняшнего замера
    // на 1000×1000. Фикс: заимствование берётся ТОЛЬКО на мгновенный
    // `take`/возврат (`std::mem::take`/присваивание) — сама сортировка
    // (и вообще весь текст функции) выполняется на ЛОКАЛЬНОЙ переменной,
    // без удержания `RefCell`-заимствования, значит reentrancy через
    // work-stealing больше не видит "уже занято".
    let mut keyed: Vec<KeyedEntry> = KEYED_BUF.with_borrow_mut(std::mem::take);
    keyed.clear();
    keyed.extend(all_matches.iter().map(|m| {
        let (priority, tie_break, rule_id) = resolve_sort_fields(m, &head_index, starvation_counters);
        let age = get_cell_age(m.x as usize, m.y as usize);
        // Вращаем ОДИН раз здесь (decorate-sort-undecorate), не внутри
        // компаратора сортировки — тот вызывается O(n log n) раз, а
        // повёрнутое значение матча не меняется в течение ОДНОГО вызова
        // arbitrate (одно и то же generation для всех матчей тика).
        let tie_break_rotated = tie_break.wrapping_add(generation) % TIE_BREAK_MODULUS;
        (
            (
                Reverse(priority),
                Reverse(age),
                Reverse(tie_break_rotated),
                Reverse(rule_id),
                Reverse(m.x),
                Reverse(m.y),
                Reverse(m.rule_idx),
            ),
            *m,
            priority,
            age,
        )
    }));
    if keyed.len() >= PARALLEL_SORT_THRESHOLD {
        keyed.par_sort_unstable_by(|a, b| a.0.cmp(&b.0));
    } else {
        keyed.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    }
    let sorted: Vec<(RuleMatch, u32, u32)> = keyed.drain(..).map(|(_, m, priority, age)| (m, priority, age)).collect();
    // Возвращаем (теперь снова пустой, но с сохранённой capacity) буфер в
    // thread_local для следующего вызова -- та же персистентность, что и
    // раньше, просто без удержания заимствования во время сортировки.
    KEYED_BUF.with_borrow_mut(|slot| *slot = keyed);

    // Оба буфера ниже — из `thread_local!` (см. её doc-комментарий и
    // doc-комментарий `used_cells` выше про то, почему не поле `Engine`).
    // `.clear()` в начале, не полагаемся на пустоту с прошлого раза (тот же
    // принцип, что и у `WriteBuffer`/`VecSink` — самоочищающийся буфер
    // безопасен по построению, а не по соглашению).
    USED_CELLS_BUF.with_borrow_mut(|used_cells| {
        used_cells.clear();
        // `reserved` -- клетки, окончательно занятые ВНЕ этого вызова (см.
        // doc-комментарий параметра). `usize::MAX` как индекс-заглушка:
        // ниже единственное место, читающее это значение, использует
        // `.get(...)` (не прямую индексацию `accepted_priority_age[..]`) --
        // `usize::MAX` гарантированно вне диапазона (`accepted` физически
        // не может вырасти настолько), так что сравнение всегда `None ==
        // Some(..)` -- `false`, конфликт с резервом никогда не попадает в
        // `tie_break_decided` (и корректно не должен: резерв — не часть
        // ЭТОГО раунда арбитража, тай-брейку сравнивать не с чем).
        for &coord in reserved {
            used_cells.insert(coord, usize::MAX);
        }
        AFFECTED_BUF.with_borrow_mut(|affected| {
            for (m, priority, age) in sorted {
                // Получаем предвычисленные affected cells из кэша
                get_match_affected_cells(&m, &head_index, rule_cache, bounds, cam_positions, feedback_counters, affected);

                // Заодно с проверкой конфликта собираем владельцев занятых
                // клеток -- если хоть один из них равен этому кандидату по
                // priority/age, его победа была решена НЕ priority/age
                // (иначе кандидат уже проиграл бы на этом уровне сравнения
                // ключа сортировки, а не дошёл бы досюда с РАВНЫМИ
                // priority/age).
                let mut conflict = false;
                for coord in affected.iter() {
                    if let Some(&owner_idx) = used_cells.get(coord) {
                        conflict = true;
                        if accepted_priority_age.get(owner_idx) == Some(&(priority, age)) {
                            tie_break_decided.insert(owner_idx);
                        }
                    }
                }

                if !conflict {
                    let idx = accepted.len();
                    for &coord in affected.iter() {
                        used_cells.insert(coord, idx);
                    }
                    accepted.push(m);
                    accepted_priority_age.push((priority, age));
                }
            }
        });
    });

    let tie_break_decided_keys: TieBreakDecidedWins = tie_break_decided
        .into_iter()
        .map(|idx| {
            let m = &accepted[idx];
            (m.x, m.y, m.rule_idx)
        })
        .collect();
    (accepted, tie_break_decided_keys)
}

/// Ниже какого числа матчей разбиение на полосы не окупается — сама
/// сортировка (уже O(M log M), параллельная выше `PARALLEL_SORT_THRESHOLD`)
/// и линейный проход дешевле накладных расходов на классификацию
/// core/boundary плюс rayon-диспетчеризацию нескольких независимых
/// арбитражей вместо одного.
const SPATIAL_THRESHOLD: usize = 4096;

/// Реализация Theorem 6 (`paper2.md` §6.2, "Spatial Decomposition of
/// Arbitration") — параллелит САМ арбитраж (не просто пропускает его, как
/// `Engine::conflict_partners`/`spatial_bypass_split` в `mod.rs`, а именно
/// распределяет по потокам, когда конфликты реально есть и арбитраж не
/// может быть пропущен целиком).
///
/// `reach` — `K` из Definition 4: наибольшее манхэттенское расстояние от
/// позиции совпадения до любой клетки в PatternCells ∪ Affected среди ВСЕХ
/// правил набора (то же самое, что уже вычисляет
/// `engine::compute_conflict_partners`'s `max_radius` — оба места опираются
/// на один и тот же `RuleData::bbox`, который включает и паттерн, и запись).
///
/// Полосы делятся по оси X с запасом `2K` (Definition 5): совпадение
/// core к полосе, если до ОБЕИХ границ полосы ≥ 2K по x — тогда его
/// affected-регион (не более K от центра) физически не может дотянуться ни
/// до соседней полосы, ни до совпадения, пограничного для своей (Lemma 6).
/// Такие core-совпадения из РАЗНЫХ полос гарантированно не пересекаются —
/// их можно арбитрировать независимо и параллельно. Пограничные совпадения
/// (внутри 2K от какой-либо границы полосы) идут одним общим
/// последовательным проходом — как и раньше.
///
/// Полосы выбираются по РЕАЛЬНОМУ разбросу x-координат матчей этого тика
/// (не по номинальной ширине решётки) — на `ChunkStorage` номинальная
/// ширина `usize::MAX`, а сами матчи почти всегда сгруппированы в узкой
/// области; статичное деление "всей" ширины решётки было бы бессмысленным.
#[allow(clippy::too_many_arguments)]
pub fn arbitrate_spatial(
    all_matches: Vec<RuleMatch>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &RuleDataCache,
    bounds: (usize, usize),
    reach: i32,
    get_cell_age: impl Fn(usize, usize) -> u32 + Sync,
) -> Vec<RuleMatch> {
    // См. doc-комментарий `arbitrate` про то же самое отбрасывание
    // `TieBreakDecidedWins` — публичная сигнатура не меняется.
    arbitrate_spatial_with_cam(
        all_matches,
        rule_index,
        rule_cache,
        bounds,
        reach,
        &CamPositions::default(),
        0,
        &StarvationCounters::default(),
        &FeedbackCounters::default(),
        get_cell_age,
    )
    .0
}

/// Как [`arbitrate_spatial`], но с картой найденных CAM-позиций — см.
/// doc-комментарий [`arbitrate_with_cam`] про ту же причину раздельных
/// публичной/`pub(crate)` версий. `generation`/`starvation_counters`/
/// `feedback_counters` — см. её doc-комментарий там же.
#[allow(clippy::too_many_arguments)]
pub(crate) fn arbitrate_spatial_with_cam(
    all_matches: Vec<RuleMatch>,
    rule_index: &HashMap<CellType, Vec<Rule>>,
    rule_cache: &RuleDataCache,
    bounds: (usize, usize),
    reach: i32,
    cam_positions: &CamPositions,
    generation: u32,
    starvation_counters: &StarvationCounters,
    feedback_counters: &FeedbackCounters,
    get_cell_age: impl Fn(usize, usize) -> u32 + Sync,
) -> (Vec<RuleMatch>, TieBreakDecidedWins) {
    if all_matches.len() < SPATIAL_THRESHOLD || reach <= 0 {
        return arbitrate_with_cam(all_matches, rule_index, rule_cache, bounds, cam_positions, generation, starvation_counters, feedback_counters, &[], get_cell_age);
    }

    let margin = (2 * reach) as u32;
    let (min_x, max_x) = all_matches.iter().fold((u32::MAX, 0u32), |(lo, hi), m| (lo.min(m.x), hi.max(m.x)));
    let spread = max_x.saturating_sub(min_x);

    // Число полос: не больше потоков rayon, и каждая полоса должна быть
    // хотя бы вдвое шире запаса (иначе в ней в принципе не может быть core-
    // совпадений — вся полоса окажется boundary, разбиение того не стоит).
    let max_bands_by_spread = if margin == 0 { usize::MAX } else { (spread / (margin * 2)).max(1) as usize };
    let num_bands = rayon::current_num_threads().min(max_bands_by_spread).max(1);

    if num_bands < 2 {
        return arbitrate_with_cam(all_matches, rule_index, rule_cache, bounds, cam_positions, generation, starvation_counters, feedback_counters, &[], get_cell_age);
    }

    let band_width = (spread / num_bands as u32).max(1);
    let band_of = |x: u32| -> usize {
        (((x - min_x) / band_width) as usize).min(num_bands - 1)
    };
    let band_range = |band: usize| -> (u32, u32) {
        let start = min_x + band as u32 * band_width;
        let end = if band == num_bands - 1 { max_x + 1 } else { min_x + (band as u32 + 1) * band_width };
        (start, end)
    };

    // `safe_margin = margin + reach`: матч на расстоянии >= safe_margin от
    // ОБЕИХ границ своей полосы гарантированно НЕ достижим НИКАКИМ
    // boundary-матчем — худший boundary (dist = margin-1) пишет максимум на
    // `reach` дальше, т.е. до `margin-1+reach = safe_margin-1` — ровно на
    // клетку меньше safe_margin (см. развёрнутое обоснование у финального
    // слияния ниже). Используется ПОСЛЕ арбитража каждой полосы (не для
    // самого разбиения на core/boundary!) — все core-матчи полосы (и
    // "глубокие", и "рискованные") ОБЯЗАНЫ арбитрироваться ВМЕСТЕ, в одном
    // вызове `arbitrate_with_cam` per-полоса, иначе соседние по позиции
    // (но оба core) матчи одной и той же полосы перестанут сравниваться
    // друг с другом — именно так была сломана первая версия этого фикса
    // (разделяла core на safe/at_risk ДО арбитража, x=103/x=104 в одной
    // полосе перестали друг друга видеть).
    let safe_margin = margin + reach as u32;

    let mut core_by_band: Vec<Vec<RuleMatch>> = (0..num_bands).map(|_| Vec::new()).collect();
    // Подмножество `core_by_band` внутри at-risk зоны (dist < safe_margin
    // от какой-либо границы) — ИСХОДНЫЕ кандидаты (и будущие победители, и
    // будущие локальные проигравшие), не результат арбитража. См.
    // doc-комментарий у бага #2 ниже про то, зачем это нужно ОТДЕЛЬНО от
    // `accepted`, который вернёт арбитраж каждой полосы.
    let mut at_risk_zone_by_band: Vec<Vec<RuleMatch>> = (0..num_bands).map(|_| Vec::new()).collect();
    let mut boundary: Vec<RuleMatch> = Vec::new();

    for m in all_matches {
        let band = band_of(m.x);
        let (start, end) = band_range(band);
        // "Расстояние до ОБЕИХ границ полосы ≥ 2K" (Definition 5) — граница
        // полосы это [start, end), расстояние до левой — m.x - start,
        // до правой — (end - 1) - m.x.
        let dist_left = m.x - start;
        let dist_right = (end - 1).saturating_sub(m.x);
        if dist_left >= margin && dist_right >= margin {
            if dist_left < safe_margin || dist_right < safe_margin {
                at_risk_zone_by_band[band].push(m);
            }
            core_by_band[band].push(m);
        } else {
            boundary.push(m);
        }
    }

    // Core-полосы — параллельно (Lemma 6: разные полосы никогда не делят
    // клетки), КАЖДАЯ полоса целиком (safe+at_risk вместе) через тот же
    // детерминированный тотальный порядок. `map` (не `flat_map`, как
    // раньше `Vec<RuleMatch>`-версия) -- каждая полоса теперь возвращает
    // ПАРУ (принятые, tie-break-решённые), и их нужно смержить по
    // отдельности, а не просто сплющить в общий Vec. Слияние — уже ОДНИМ
    // потоком, после `.collect()`: `TieBreakDecidedWins` полос не
    // пересекаются по построению (Lemma 6), так что порядок слияния не важен.
    // `&[]` (нет резервов) -- на этом этапе `safe_accepted` ещё не
    // вычислен, резервировать пока нечего.
    let band_results: Vec<(Vec<RuleMatch>, TieBreakDecidedWins)> = core_by_band
        .into_par_iter()
        .map(|band_matches| {
            if band_matches.is_empty() {
                (Vec::new(), FxHashSet::default())
            } else {
                arbitrate_with_cam(band_matches, rule_index, rule_cache, bounds, cam_positions, generation, starvation_counters, feedback_counters, &[], &get_cell_age)
            }
        })
        .collect();

    // РЕАЛЬНЫЙ БАГ, найден 2026-08-10, исправлен здесь: раньше `core`
    // (принятые в полосах, параллельно) и `boundary` (арбитрированный
    // ОТДЕЛЬНО, сам с собой) просто СКЛЕИВАЛИСЬ (`result.extend(...)` для
    // обоих) — БЕЗ перекрёстной проверки. `margin = 2*reach` доказывает
    // только "core из РАЗНЫХ полос не пересекаются" (Lemma 6, всё ещё
    // верно) — про boundary-против-соседнего-core (в том числе того же
    // самого band'а!) теорема ничего не говорит и не может: boundary-матч
    // прямо на границе своей зоны (dist = margin-1) физически может писать
    // ещё на `reach` клеток дальше, вглубь соседней core-зоны (которая
    // начинается сразу на dist = margin) — ни один конечный margin с этой
    // простой пороговой схемой (dist >= margin ⟹ core) не может это
    // исключить в общем случае (доказано конструктивно: worst-case boundary
    // достигает margin-1+reach, core начинается на margin, и margin-1+reach
    // >= margin для любого reach >= 1). Конкретный репро:
    // `tests/property_arbitration.rs`'s
    // `test_arbitrate_spatial_matches_centralized_recursion_dense_overlapping_writes`
    // — x=7 (boundary) и x=8 (core), запись обоих по 4 клетки, пересечение
    // на {8,9,10}, оба были приняты. Раньше не ловилось НИ РАЗУ, потому что
    // все прежние Lemma-6-тесты писали в одну точку (без сдвигов и без
    // recursion) — точка не может дотянуться через границу вне
    // зависимости от margin.
    //
    // Фикс: КАЖДАЯ полоса уже арбитрировала СВОЙ ПОЛНЫЙ core-набор целиком
    // (см. правку выше) — значит матчи одной полосы уже корректно сверены
    // друг с другом, включая пары вроде x=103/x=104 (оба core одной полосы,
    // но близко к границе), которые сломала бы предварительная делёжка на
    // safe/at_risk ДО арбитража. Теперь, из УЖЕ ПРИНЯТЫХ результатов
    // каждой полосы, делим на `safe_accepted` (>= safe_margin от ОБЕИХ
    // границ СВОЕЙ полосы — доказанно недостижимы НИКАКИМ boundary-матчем,
    // см. doc-комментарий `safe_margin` выше) и `at_risk_accepted`
    // (оставшиеся core-победители, теоретически достижимые boundary).
    // `safe_accepted` — окончательный, без дальнейших проверок (та часть
    // выигрыша от параллелизации, которую фикс бага не обязан приносить в
    // жертву — первая версия фикса сливала ВСЕ core-матчи обратно,
    // измеренная регрессия ~30-50% на CA-пробке даже для reach=1, где
    // риска никогда не было). `at_risk_accepted` идёт ОДНИМ дополнительным
    // проходом `arbitrate_with_cam` вместе с `boundary`, как СЫРЫЕ
    // кандидаты (уже победившие локально, но не факт, что переживут
    // встречу с boundary) — тем же детерминированным тотальным порядком.
    // Lemma 6 гарантирует, что at_risk_accepted-vs-at_risk_accepted (разные
    // полосы) здесь никогда не найдёт конфликт (это ещё core по ИСХОДНОМУ
    // определению) — проход тратит на них время впустую, но не портит
    // результат; единственные РЕАЛЬНЫЕ решения этого прохода —
    // boundary-vs-at_risk_accepted и boundary-vs-boundary, ровно то, чего
    // не хватало.
    //
    // (`at_risk_accepted` — переменная ИЗ ЭТОЙ, уже исторической версии
    // фикса, в текущем коде ниже её нет — см. фикс бага #2 сразу дальше,
    // заменивший её на `at_risk_zone_by_band`.)
    //
    // РЕАЛЬНЫЙ БАГ #2, найден 2026-08-1X целенаправленным тестом
    // (`test_arbitrate_spatial_matches_centralized_at_risk_loser_frees_locally_rejected_weaker_candidate`,
    // сконструированным именно под остаточный риск, задокументированный
    // выше при фиксе бага #1 -- и подтвердившимся: не гипотетический.
    // Раньше в `at_risk_accepted` шли только ПОБЕДИТЕЛИ этапа локального
    // арбитража полосы (`accepted`, отфильтрованный по позиции). Если
    // такой победитель (W) проигрывал boundary-конкуренту (B) в финальном
    // merge-проходе — W отклонялся ЦЕЛИКОМ (греди, не частично), но более
    // слабый кандидат ТОЙ ЖЕ клетки (L), отвергнутый W ещё на этапе
    // локального арбитража полосы, уже не участвовал в финальном проходе
    // вообще -- он не пережил в `accepted`, значит физически не мог
    // попасть в `at_risk_accepted`. Централизованный арбитраж той же
    // тройки (порядок B > W > L): B принят, W отклонён (клетка B занята),
    // L ПРОВЕРЯЕТСЯ НЕЗАВИСИМО -- его клетка так и осталась свободной (W
    // её не забирал, будучи отклонён целиком) -- L принят. Расхождение
    // реально: spatial теряет L, centralized его находит.
    //
    // Фикс: в финальный merge-проход идут не `accepted`-победители полосы,
    // а `at_risk_zone_by_band` -- ИСХОДНЫЕ кандидаты at-risk зоны (и
    // победители, и локально отклонённые), собранные ЕЩЁ НА ЭТАПЕ
    // классификации (см. правку выше), ДО первого арбитража полос. Первый
    // (per-полосный) арбитраж по-прежнему нужен ЦЕЛИКОМ (safe+at_risk
    // вместе) -- он остаётся ЕДИНСТВЕННЫМ источником `safe_accepted`
    // (глубокие победители, никогда не пересматриваются) и корректно
    // решает safe-vs-at_risk конфликты ВНУТРИ полосы (та же причина, что и
    // у бага #1's фикса -- разделять ДО арбитража нельзя). Но его решение
    // для at_risk-зоны теперь ПОЛНОСТЬЮ ПересматривАется в merge-проходе:
    // включая и W, и L, тем же детерминированным тотальным порядком, что
    // при централизованном арбитраже -- если B побеждает W, L получает
    // честный шанс на освободившуюся клетку, ровно как должно быть.
    //
    // Опасность: at_risk-зона МОЖЕТ физически пересекаться с safe-зоной
    // ТОЙ ЖЕ полосы (несмотря на название "safe") -- at_risk-матч на
    // dist=margin (граница at_risk-зоны) с reach=K может писать вплоть до
    // margin+K = safe_margin, то есть ровно на первую "safe" клетку.
    // Значит если at_risk-матч (L) проиграл ИМЕННО safe-матчу (не другому
    // at_risk) на этапе первого (per-полосного) арбитража -- его клетка
    // уже НАВСЕГДА, законно занята (safe_accepted никогда не
    // пересматривается), а слепое повторное участие L в merge-проходе БЕЗ
    // учёта этого могло бы дать L повторно "выиграть" уже занятую клетку
    // -- двойная запись. Фикс от этого не через отслеживание "кто именно
    // отклонил L" (это потребовало бы нового, более дорогого API у
    // `arbitrate_with_cam`, платящего накладными расходами на КАЖДЫЙ
    // вызов, включая горячий путь), а проще и дешевле: `safe_accepted`'s
    // клетки передаются в merge-проход как `reserved` (см. её
    // doc-комментарий у `arbitrate_with_cam`) -- любой at_risk/boundary
    // кандидат, пытающийся занять зарезервированную клетку, корректно
    // отклоняется тем же механизмом, что и обычный конфликт, без
    // необходимости знать, КЕМ именно она была занята.
    let mut safe_accepted: Vec<RuleMatch> = Vec::new();
    let mut safe_tie_break_decided: TieBreakDecidedWins = FxHashSet::default();
    for (band, (accepted, tbd)) in band_results.into_iter().enumerate() {
        let (start, end) = band_range(band);
        let mut band_safe_keys: TieBreakDecidedWins = FxHashSet::default();
        for m in accepted {
            let dist_left = m.x - start;
            let dist_right = (end - 1).saturating_sub(m.x);
            if dist_left >= safe_margin && dist_right >= safe_margin {
                band_safe_keys.insert((m.x, m.y, m.rule_idx));
                safe_accepted.push(m);
            }
            // at_risk-победители этого раунда сюда НЕ идут -- они (вместе
            // с их локальными проигравшими) заново, целиком
            // переарбитрируются ниже, из `at_risk_zone_by_band`, не
            // отсюда (см. комментарий бага #2 выше).
        }
        safe_tie_break_decided.extend(tbd.into_iter().filter(|key| band_safe_keys.contains(key)));
    }

    // `reserved` для merge-прохода -- клетки `safe_accepted`, вычисленные
    // тем же способом (`get_match_affected_cells`), что и внутри
    // `arbitrate_with_cam` (см. её doc-комментарий и bug#2 выше про то,
    // зачем это нужно). `head_index` строится здесь ЗАНОВО (не переиспользуется
    // из внутренних вызовов `arbitrate_with_cam` -- у него нет публичного
    // доступа наружу) -- цена пропорциональна числу РАЗЛИЧНЫХ голов правил,
    // не числу матчей, и платится ОДИН раз на весь вызов
    // `arbitrate_spatial_with_cam`, не на каждый матч.
    let head_index = build_head_index(rule_index);
    let mut reserved_cells: Vec<(i32, i32)> = Vec::new();
    let mut reserved_scratch: Vec<(i32, i32)> = Vec::new();
    for m in &safe_accepted {
        get_match_affected_cells(m, &head_index, rule_cache, bounds, cam_positions, feedback_counters, &mut reserved_scratch);
        reserved_cells.extend(reserved_scratch.iter().copied());
    }

    let mut merge_candidates = boundary;
    for band_matches in at_risk_zone_by_band {
        merge_candidates.extend(band_matches);
    }
    let (merge_accepted, merge_tie_break_decided) =
        arbitrate_with_cam(merge_candidates, rule_index, rule_cache, bounds, cam_positions, generation, starvation_counters, feedback_counters, &reserved_cells, get_cell_age);

    let mut result = safe_accepted;
    result.extend(merge_accepted);
    safe_tie_break_decided.extend(merge_tie_break_decided);

    (result, safe_tie_break_decided)
}

/// Приоритет и id правила, сработавшего в данном match'е, — ОДНИМ поиском
/// в `rule_index`, а не двумя раздельными (как было раньше: `get_priority`
/// и `resolve_rule_id` каждый делали свой `rule_index.get(&m.head)...`).
/// Вызывается один раз на КАЖДЫЙ матч при построении ключа сортировки —
/// на миллионах матчей лишний повторный поиск в HashMap заметен.
/// Использует `rule_idx`, а не поиск по одной лишь `head` — несколько правил
/// могут иметь одинаковую голову, и только `rule_idx` однозначно определяет,
/// какое именно правило сработало.
///
/// `priority` возвращается уже с учётом голодания (см. `Rule::starvation_after`
/// и `Engine::starvation_counters`): если у сработавшего правила это поле
/// установлено И счётчик проигрышей ЭТОГО конкретного `(x, y, rule_idx)` уже
/// достиг порога — возвращается `u32::MAX` вместо номинального `rule.priority`,
/// что гарантирует победу на этот тик (после чего вызывающая сторона сбросит
/// счётчик — см. `run_tick_with_cache`). Без этого поля (`None`, по умолчанию)
/// или пока счётчик ниже порога — обычный номинальный `priority`, без изменений.
fn resolve_sort_fields(
    m: &RuleMatch,
    head_index: &HeadRuleIndex,
    starvation_counters: &StarvationCounters,
) -> (u32, u32, RuleIdKey) {
    match lookup_rule(head_index, m.head, m.rule_idx) {
        Some(rule) => {
            let priority = match rule.starvation_after {
                Some(threshold) if starvation_counters.get(&(m.x, m.y, m.rule_idx)).copied().unwrap_or(0) >= threshold => u32::MAX,
                _ => rule.priority,
            };
            (priority, rule.tie_break, RuleIdKey::from_id(&rule.id))
        }
        None => (0, 0, RuleIdKey::Small([0u8; 16], 0)),
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
    head_index: &HeadRuleIndex,
    rule_cache: &RuleDataCache,
    bounds: (usize, usize),
    cam_positions: &CamPositions,
    feedback_counters: &FeedbackCounters,
    out: &mut Vec<(i32, i32)>,
) {
    out.clear();
    let head = m.head;
    let matched_rule = lookup_rule(head_index, head, m.rule_idx);

    // CAM-матч БЕЗ `recursion`: точные affected cells — найденная позиция
    // (не консервативный весь-диск из `RuleData::write_cells`, который
    // годится только для статического графа конфликтов, см. её
    // doc-комментарий в `conflict_analyzer.rs`) плюс сама позиция магнита.
    // `cam_positions` всегда содержит запись для КАЖДОГО CAM-матча,
    // дошедшего сюда — она заполняется в `detect_cam_matches` синхронно с
    // самим `RuleMatch`.
    //
    // CAM-матч С `recursion`: уровни каскада `k = 1..=max_depth` (см.
    // `applicator::apply_cam_buffered`) находят свои клетки ПРОЦЕДУРНО во
    // время apply, читая `write_buffer`, уже накопленный БОЛЕЕ РАННИМИ
    // (по порядку арбитража) матчами того же тика — то есть их реальные
    // affected cells зависят от порядка применения, который на момент
    // арбитража ещё не определён (та же причина, по которой уровень 0
    // сам не знает своей цели без `cam_positions`, только для уровней
    // 1..=max_depth даже такой пост-детект записи нет). Использовать здесь
    // только [found, magnet] уровня 0, как раньше, — это НЕДОоценка: два
    // разных cam+recursion матча, чьи диски уровня 0 не пересекаются, но
    // чьи каскады (уровни 1..=max_depth) МОГУТ physически пересечься,
    // ошибочно считались бы точно (exact-cells) непересекающимися, даже
    // когда статический граф конфликтов (построенный по union дисков всех
    // уровней, см. `compute_rule_data`) уже верно завёл между ними ребро —
    // exact-path тогда молча занижал бы то, что conservative-path верно
    // расширил. Поэтому при наличии `recursion` используем ПОЛНЫЙ
    // консервативный `rule_data.write_cells` (union дисков всех уровней)
    // вместо точных 2 клеток — падаем в общий путь ниже, тот же приём, что
    // уже применяется к обычной (не-cam) рекурсии через `RuleData::write_cells`.
    let cam_has_recursion = matched_rule.is_some_and(|r| r.cam.is_some() && r.recursion.is_some());
    if !cam_has_recursion {
        if let Some(&(fx, fy)) = cam_positions.get(&(m.x, m.y, m.rule_idx)) {
            out.push((fx as i32, fy as i32));
            out.push((m.x as i32, m.y as i32));
            return;
        }
    }

    let rule_data = match get_rule_data(rule_cache, head, m.rule_idx) {
        Some(rd) => rd,
        None => {
            // Правило не найдено в кэше — в норме недостижимо, т.к.
            // rule_cache строится из того же rule_index, откуда взят и
            // `matched_rule`, и содержит запись под каждый (head, rule_idx).
            // Консервативный фолбэк: длина id сработавшего правила (или 1,
            // если и его не найти) как ширина паттерна вдоль x, как строился
            // бы паттерн из id по умолчанию (см. `config::load_config`).
            let id_len = matched_rule.map_or(1, |rule| rule.id.len().max(1));
            for i in 0..id_len {
                out.push((m.x as i32 + i as i32, m.y as i32));
            }
            return;
        }
    };

    let (w, h) = (bounds.0 as i32, bounds.1 as i32);
    let overflow = matched_rule.map(|rule| rule.overflow);

    // `Rule::feedback` (Лемма 4, `paper/paper4.md` §8, Corollary C): точное
    // (не union) множество для ЭТОГО КОНКРЕТНОГО матча — зависит от того,
    // защёлкнулся ли счётчик обратной связи, а не от `rule_data.write_cells`
    // (тот всегда union обоих направлений, годится только для статического
    // графа — см. её doc-комментарий). Та же логика раздельного пути, что
    // и у CAM выше, только не early-return: клэмпинг на границу решётки
    // нужен точно так же, как и в общем случае ниже.
    if let Some(spec) = matched_rule.and_then(|rule| rule.feedback) {
        let latched = feedback_counters.get(&(m.x, m.y, m.rule_idx)).copied().unwrap_or(0) >= spec.timeout;
        let (cells, direction) = if latched {
            (&rule_data.feedback_alt_write_cells, spec.new_direction)
        } else {
            let declared = matched_rule.and_then(|rule| rule.shifts.iter().flatten().next()).map(|s| s.direction);
            (&rule_data.feedback_normal_write_cells, declared.unwrap_or(spec.new_direction))
        };
        let target = direction_delta(direction);
        out.extend(cells.iter().map(|&(dx, dy)| clamp_shift_target(m, (dx, dy), (dx, dy) == target, overflow, w, h)));
        return;
    }

    // Правило с несколькими сдвигами реплицирует значение в КАЖДУЮ цель
    // независимо (см. RuleData::shift_targets) — клэмпинг при
    // OverflowAction::Write применим к любой из них, не только к первой.
    out.extend(rule_data.write_cells.iter().map(|&(dx, dy)| {
        clamp_shift_target(m, (dx, dy), rule_data.shift_targets.contains(&(dx, dy)), overflow, w, h)
    }));
}

/// (dx, dy) направления сдвига — та же таблица, что и везде в проекте
/// (`applicator::apply_shift_buffered`, `conflict_analyzer::shift_delta`).
pub(crate) fn direction_delta(direction: crate::types::Direction) -> (i32, i32) {
    use crate::types::Direction;
    match direction {
        Direction::Up => (0, -1),
        Direction::Down => (0, 1),
        Direction::Left => (-1, 0),
        Direction::Right => (1, 0),
    }
}

/// Клэмпинг одной относительной ячейки записи на границу решётки при
/// `OverflowAction::Write`/`WriteLiteral` — общая логика для обычного пути и
/// `Rule::feedback`'а, см. doc-комментарий `get_match_affected_cells`.
fn clamp_shift_target(m: &RuleMatch, (dx, dy): (i32, i32), is_shift_target: bool, overflow: Option<OverflowAction>, w: i32, h: i32) -> (i32, i32) {
    let abs = (m.x as i32 + dx, m.y as i32 + dy);
    if w > 0 && h > 0 && is_shift_target {
        if let Some(OverflowAction::Write(_) | OverflowAction::WriteLiteral(_)) = overflow {
            if abs.0 < 0 || abs.0 >= w || abs.1 < 0 || abs.1 >= h {
                return (abs.0.clamp(0, w - 1), abs.1.clamp(0, h - 1));
            }
        }
    }
    abs
}
