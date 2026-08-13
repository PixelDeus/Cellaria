# Cellaria: The Rule Mechanism — Anatomy, Interactions, and Extensions

## Status

Draft. Sections 8-13 cover everything established through 2026-08-03: the
General Conservative Bound Lemma (§8) with all three of its instances
shipped and verified (CAM, bounded recursion, feedback-driven rules); a
full pairwise audit of how the five post-`paper2.md` extensions (CAM,
tie-break, starvation guard, feedback, recursion) interact with each other
and with `min_age`/`active_only` (§9); what does and does not survive
self-modification transmission (§10); the GPU-parity picture as it stood
then (§11, since corrected in place — see below); and an extension of
`paper2.md` §7's reversibility result to systems that cross the grid's
I/O boundary (§12). Still not written: a full field-by-field anatomy of
`Rule` with no accompanying theorem (ruled out as padding — see the
project notes this paper grew from).

**2026-08-13 update.** §11's GPU-parity table was corrected in place — it
had gone stale within days of the original text (see the table's own
"Update note" for what changed and why). §14 was added as a short,
non-formal factual summary of four systems built after 2026-08-03
(`Rule::memory`, `LayeredEngine`, `Rule::max_activations`,
`ChangeValue::Add`/`Sub`) — deliberately scoped as inventory, not new
lemmas; whether any of them warrants its own Lemma-4-style corollary is
left for a future revision.

All definitions/lemmas/theorems from `paper2.md`/`paper3.md` are assumed
and cited by number, not restated.

---

## 8. The General Conservative Bound Lemma

### 8.1. Motivation

`paper2.md` Definition 2 defines `Affected(R, cₓ, cᵧ)` as a single,
statically-determined set of cells — computed once from `R`'s `shifts`/
`changes`, independent of grid content. Every proof in `paper2.md` §3
(Lemma 3, Theorem 2, Corollary 2) depends on `Affected` being *some* fixed
set per `(R, cₓ, cᵧ)`, but nothing in those proofs actually requires that
set to be the *exact* set of cells touched at runtime — only that it be a
**superset** of whatever is actually touched.

Three extensions to Cellaria exploit exactly this slack, independently:

- **CAM** (`Rule::cam`, shipped): the rule's actual write
  target is wherever the nearest matching cell turns out to be within a
  radius — unknown until the tick runs, but always inside a fixed disc.
- **Bounded local recursion** (`Rule.recursion`, shipped — §8.5): the rule's actual
  cascade depth is data-dependent — unknown until the tick runs, but
  always at most a declared maximum.
- **Feedback-driven rules** (`Rule.feedback`, shipped — §8.6): after a declared
  timeout, the rule's effect switches from its normal shift/changes to
  one of a declared list of alternatives — which alternative fires is
  data-dependent, but the *list* is fixed at rule-definition time.

All three have the same shape: *the realized affected region is one of
finitely many statically-enumerable possibilities.* This section proves
once that using the union of those possibilities as the conflict-graph
bound is sound, so each of the three extensions gets soundness for free
as a corollary, rather than needing its own proof from scratch.

### 8.2. Formal Definitions

**Definition 8 (Mode set).** A rule `R` has a *mode set*
`M(R) = {m₁, ..., mₖ}`, a finite set fixed at rule-definition time
(before any tick runs), such that the rule's actual behavior when applied
at `(cₓ, cᵧ)` is determined by exactly one realized mode `m ∈ M(R)`,
chosen by grid content and/or accumulated tick-indexed state at apply
time. A rule with no such extension has `M(R) = {•}` (a single trivial
mode), recovering `paper2.md`'s original model exactly.

**Definition 9 (Mode-dependent affected region).**
`Affected_m(R, cₓ, cᵧ, m)` is the affected region (Definition 2) that is
*actually* read/written when mode `m` is realized.

**Definition 10 (Conservative bound).**

```
Affected⁺(R, cₓ, cᵧ) = ⋃_{m ∈ M(R)} Affected_m(R, cₓ, cᵧ, m)
```

`Affected⁺` is computable at rule-definition time — it depends only on
the statically-known mode set and each mode's statically-known region,
never on runtime grid content.

### 8.3. Lemma 4 (Soundness of the Conservative Bound)

**Claim.** If the conflict graph (`paper2.md` Algorithm 1) is built using
`Affected⁺` in place of `Affected` in every step of Definition 1/2 and
Lemma 3, then Theorem 2 (Completeness) still holds verbatim: absence of
edge `(i, j)` guarantees `Rᵢ` and `Rⱼ` cannot conflict under any grid
state *and any combination of realized modes*.

*Proof.* By construction (Definition 10), for every `m ∈ M(Rᵢ)`:

```
Affected_m(Rᵢ, P, m) ⊆ Affected⁺(Rᵢ, P)
```

and symmetrically for every `m' ∈ M(Rⱼ)` and `Affected⁺(Rⱼ, Q)`. Suppose
the conflict graph (built with `Affected⁺`) has no edge `(i, j)`. By
Theorem 2's proof, this means:

```
Affected⁺(Rᵢ, P) ∩ Affected⁺(Rⱼ, Q) = ∅
```

for every pair of positions `(P, Q)` where both rules can match
simultaneously (Condition A still applies unchanged — mode selection
does not affect *whether* a rule matches, only what it does once it has).
Since `Affected_m(Rᵢ, P, m) ⊆ Affected⁺(Rᵢ, P)` and
`Affected_{m'}(Rⱼ, Q, m') ⊆ Affected⁺(Rⱼ, Q)` for every `m, m'`, it
follows that:

```
Affected_m(Rᵢ, P, m) ∩ Affected_{m'}(Rⱼ, Q, m') = ∅
```

for *every* choice of `m ∈ M(Rᵢ)` and `m' ∈ M(Rⱼ)` — not merely for one
particular realized pair. This is precisely Lemma 3's hypothesis, applied
uniformly across all reachable mode combinations, so its conclusion (no
conflict) holds regardless of which modes are actually realized at
runtime. ∎

**Corollary 3 (Backward compatibility).** For a rule with the trivial
mode set `M(R) = {•}`, `Affected⁺(R, cₓ, cᵧ) = Affected(R, cₓ, cᵧ)`
exactly — Lemma 4 degenerates to `paper2.md`'s original Lemma 3/Theorem 2
with no change in the resulting graph. Extensions that introduce no new
modes (e.g. `tie_break`, `starvation_after`'s `ChangePriority` action —
see §8.6) are outside the scope of this lemma entirely: they modulate
*which* match wins arbitration, never *where* a match writes, so they
never touch `Affected`/`Affected⁺` in the first place.

### 8.4. Corollary A — Content-Addressable Matching (already implemented)

`Rule::cam` fits Definition 8 directly:
`M(R) = {found at p : p within Chebyshev distance `radius` of `(cₓ,cᵧ)`}
∪ {not found}`. For the "not found" mode, `Affected_m = ∅` (no write).
For "found at `p`", `Affected_m(R, cₓ, cᵧ, p) = {p, (cₓ, cᵧ)}` — the
target cell and the magnet's own cell (matches
`arbitrator::get_match_affected_cells`'s CAM branch exactly: it pushes
`(fx, fy)` then `(m.x, m.y)`).

```
Affected⁺(R, cₓ, cᵧ) = ⋃_p {p, (cₓ,cᵧ)} = Disc(cₓ, cᵧ, radius) ∪ {(cₓ,cᵧ)}
```

This is *exactly* `conflict_analyzer::cam_disc_cells(radius)`, already
used as both `affected_cells` and `write_cells` in
`conflict_analyzer::compute_rule_data`'s CAM branch (`src/conflict_analyzer.rs:337-350`).
CAM was implemented (block E) before this lemma was written down
explicitly — this corollary is a retroactive formalization of an already-
shipped, already-tested mechanism, not new work. It serves as the
existence proof that Lemma 4's shape is not merely hypothetical.

### 8.5. Corollary B — Bounded Local Recursion (implemented 2026-08-03)

**Shipped mechanism.** `Rule.recursion: Option<RecursionSpec>`, where
`RecursionSpec { max_depth: u8, direction: Direction }`. Scope was
narrowed from an unconstrained "per-level template" to: the rule
re-applies *itself* (same `pattern`, same `changes`), translated by
`k × direction` for `k = 1..=max_depth`, each level checked against the
grid *as modified by earlier levels of the same cascade* — not the stale
pre-tick snapshot. The cascade stops at the first level whose translated
pattern fails to match (natural termination, exactly like a flood fill
hitting a wall or a non-matching cell — no "exceeded declared bound"
failure mode turned out to be reachable, see below). Restricted to rules
with **no shifts** (`config::load_config` rejects `recursion` combined
with any `shifts`) — recursion expands a `changes` effect along a line;
combining it with a moving head is a different, unaddressed semantics.

`M(R) = {0, 1, ..., max_depth}` — the realized mode is whatever depth the
cascade actually reaches before its first non-match, exactly as
originally sketched. Static bound, implemented in
`conflict_analyzer::compute_rule_data`:

```
Affected⁺(R, cₓ, cᵧ) = ⋃_{k=0}^{max_depth} (pattern_cells ∪ changes_cells) + k·direction
```

— computed directly (no need for a looser ball/`MAX_CAM_RADIUS`-style cap,
since the cascade's shape is a single straight line along one declared
`direction`, not an arbitrary 2D template — the exact union *is* already
the tight bound). Verified with an adversarial test
(`test_recursion_conflict_only_visible_via_cascade_depth_union`,
`src/engine/tests/extensions_interactions.rs`): rule B sits at a relative offset reachable only at
cascade depth `k=2`, unreachable at `k=0` — `ConflictGraph::build` finds
the edge. Passed on the first run.

**The "what if actual reach exceeds the declared bound" question turned
out to not apply, once the mechanism was pinned down concretely**: because
each cascade level re-checks the *same* `pattern`, the cascade is
*structurally* incapable of reaching further than `max_depth` steps — it's
not a separate value that could drift out of sync with the declared cap
(unlike CAM's runtime-found position, which is a genuinely different
quantity from the declared `radius` and needs its own bounds check). There
is nothing analogous to `MAX_CAM_RADIUS`/`CamRadiusTooFar` to add here;
the loop itself is the bound.

**Notably simpler than Corollary C (`feedback`) in one respect:** recursion
needed *no* new `Engine`-level persistent state at all — the whole cascade
lives inside one `apply_rule_buffered` call, reading its own
in-progress `write_buffer` (a single documented, deliberate exception to
"detect always reads pre-tick state," scoped to the one match that won
arbitration this tick). Both new tests
(`test_recursion_cascades_multiple_cells_in_one_tick` and the adversarial
one above) passed on the first run — unlike `feedback`, which needed a
real bug fix (counter relocation) before its test passed. Confirms the
lesson from Corollary C wasn't "every corollary of Lemma 4 hides a bug,"
but specifically "any corollary that introduces *cross-tick* state is
where the risk concentrates" — recursion has none, so it didn't have that
class of problem.

GPU rejects `recursion` rules outright
(`GpuUnsupportedReason::RecursionUnsupported`) — the shader pipeline
parallelizes one thread per cell and has no mechanism for "match A already
wrote what match A is now re-reading" within one dispatch. *(Status note
added 2026-08-13: `recursion` was later ported to GPU too, within a depth
cap — see §11 for the current, much wider GPU-supported subset; this
paragraph's "GPU rejects `recursion` outright" is historical, not
current, the same correction already applied to `feedback` above.)*

### 8.6. Corollary C — Feedback-Driven Rules (implemented 2026-07-31)

**Shipped mechanism.** `Rule.feedback: Option<FeedbackSpec>`, where
`FeedbackSpec { timeout: u64, new_direction: Direction }` — scope was
narrowed from the originally-discussed `FeedbackAction` enum
(`ChangePriority`/`ChangeDirection`/`ChangeResult`) down to just the
direction-changing case, since that was the only one the motivating
scenario (many independent agents, long/varying timeouts, routing via
direction switch after a give-up threshold) actually needed —
`ChangePriority` never touches `Affected` at all (Corollary 3 applies, no
new proof needed) and wasn't built since nothing called for it.

Tracked via `Engine.feedback_counters: FxHashMap<(u32,u32,usize), u64>`
(same key shape as `Engine.starvation_counters`, for the same reason: not
cell-type-encoded, to avoid burning the 256-value type space per
concurrent long-running countdown). Counter increments once per tick the
`(x, y, rule_idx)` match is detected (regardless of arbitration outcome —
unlike `starvation_counters`, which only counts *losses*); once it reaches
`timeout` it **latches** (never resets) — `new_direction` applies
permanently from then on, not just for one tick. This matches the actual
scenario ("agent tried one direction N ticks, gave up, switched for
good") more precisely than a boost-then-reset model would have.

**A subtlety the proof sketch missed, found only by writing a real test:**
the key `(x, y, rule_idx)` is the match's *current* position — but a
`feedback` rule, by construction, physically moves every tick (unlike
`starvation_after`, where the same match keeps re-contesting one fixed
cell). Naively, the counter would reset to a fresh 0 every tick because
the key is a different cell each time, and `timeout` would never be
reached. The fix: `applicator::apply_shift_buffered` explicitly *relocates*
the counter entry from the old position's key to the new one as part of
applying the shift, for every tick, regardless of which direction (normal
or `new_direction`) is currently in effect. This is exactly the kind of
gap this project's whole methodology is built to catch — the design
looked sound on paper and only broke under an actual `Engine::run_tick`
loop with a genuinely moving token.

Static conflict-graph soundness: `compute_rule_data` computes the union
of the normal-direction write cells and the `new_direction` write cells
(`RuleData.write_cells`) for the graph, while keeping both variants
available *separately* (`feedback_normal_write_cells`/
`feedback_alt_write_cells`) for the exact, mode-aware per-tick computation
in `arbitrator::get_match_affected_cells` — the same two-tier split CAM
already established (§8.4): a conservative union for the static graph, the
one actually-realized value for real arbitration. Verified with an
adversarial test (`test_feedback_conflict_only_visible_via_alternate_direction_union`,
`src/conflict_analyzer_tests.rs`): two rules whose *normal* footprints
never overlap, but whose *alternate*-direction footprint does — asserts
`ConflictGraph::build` finds the edge. GPU rejects `feedback` rules
outright (`GpuUnsupportedReason::FeedbackUnsupported`), same policy as
`starvation_after`. *(Status note added 2026-08-08: `feedback` was later
ported to GPU too — see the correction below and §11 for the current,
much wider GPU-supported subset; this section's "GPU rejects `feedback`
outright" is historical, not current.)*

**Correction (2026-08-08): "the one actually-realized value" above was not
actually true from this section's original writing (2026-07-31) until
2026-08-08.** `get_match_affected_cells` (arbitration's read) and
`applicator::apply_rule_buffered`'s `feedback_override` (apply's read) both
decide the effective direction from `feedback_counters`, but the counter's
own increment sat *between* the two reads — arbitration saw the value from
before this tick's own detection, apply saw it already incremented. On the
one tick a match's counter crosses `timeout`, the two reads could disagree
about which direction was "the one actually-realized value," so arbitration
could reserve/check conflicts for one direction while apply silently wrote
to the other — a cell arbitration never verified against other accepted
matches. Found via a GPU-vs-CPU parity test (an independently-built,
differently-structured implementation disagreeing with this one), not by
inspection of this proof. The counter now increments strictly *after* apply
(mirroring `starvation_counters`, whose own read/update was correct from
the start — see its paragraph above), so detect/arbitrate/apply all see the
counter exactly as it stood at the *start* of the tick, the same discipline
`age`/`min_age` already use for grid state — this tick's own detection
becomes visible starting the *next* tick, not this one. The Lemma 4
theorem itself (the union-bound argument two paragraphs up) was never
affected — it reasons about the conservative *static* bound, which is
sound regardless of which mode is realized at runtime; only this section's
prose describing the *exact, per-tick* computation overclaimed a
consistency that the implementation didn't yet have.

**Design note carried over from the block-F discussion, confirmed
unchanged:** the "success" condition (e.g. target reached) is *not* part
of `FeedbackSpec` — it is an ordinary, higher-priority rule matching the
same head, which already preempts the low-priority "keep going" rule via
existing arbitration (zero new mechanism, and — per the matcher's
`exact_lookup` fast path — cheap even when it doesn't match, contrary to
an earlier, since-retracted concern about per-tick arbitration cost).

### 8.7. Summary

| Extension | Status | Mode set | Needs Lemma 4? |
|---|---|---|---|
| CAM | shipped (block E) | found-position ∪ not-found | Yes — retroactively (§8.4) |
| Broadcast shift | shipped (block E) | none (static path, fully enumerable at rule-definition time) | No |
| Modular tie-break | shipped (block F) | none (modulates arbitration order, not `Affected`) | No |
| Starvation guard | shipped (block F) | none (modulates priority only) | No |
| Bounded local recursion (`Rule.recursion`) | **shipped 2026-08-03** | depth 0..max_depth along one `direction` | Yes — new (§8.5), verified by adversarial test |
| Feedback (`Rule.feedback`, `new_direction` only) | **shipped 2026-07-31** | normal ∪ `new_direction` | Yes — new (§8.6), verified by adversarial test |
| Feedback: `ChangePriority` | not built (nothing needed it) | none | No |

All three Lemma-4-requiring rows (CAM, recursion, feedback) are now
implemented and each has a passing adversarial conflict-graph test — the
lemma is no longer just proved on paper for one instance (CAM) with two
more sketched; all three instances are built, and each corollary's test
was written to specifically try to break the union-bound argument, not
just exercise the happy path.

---

## 9. Interaction Audit

`paper2.md`/`paper3.md` were written before any of the five extension
fields discussed in §8 existed (`cam`, `tie_break`, `starvation_after`,
`feedback`, `recursion`). Nothing in either paper says whether they
compose safely with each other or with the original fields (`min_age`,
`active_only`). This section reports a direct audit against the current
implementation, not a re-derivation from first principles — every claim
below was checked by reading the actual code path, and two of the
findings required a code change, not just a note.

### 9.1. Method

For each unordered pair of extensions, the question asked was: does one
field's mechanism *bypass*, *ignore*, or *silently interact with* the
other's, in a way that isn't already covered by an explicit validation or
an existing test? Three outcomes were possible: **orthogonal** (fields
operate on disjoint concerns, provably don't interfere), **compatible**
(fields interact but correctly, verified in existing code/tests), or
**gap** (a real, previously-undetected inconsistency).

### 9.2. Findings — two real gaps, found and closed

**Gap 1: `cam` + `recursion`.** Both `conflict_analyzer::compute_rule_data`
and `applicator::apply_matches_with_cam` branch on `rule.cam.is_some()`
*first* and return/`continue` before ever reaching the code paths that
handle `rule.recursion`. A rule with both fields set would have its
`recursion` silently never applied and never accounted for in the
conflict graph — not unsound (a cascade that never runs cannot cause a
missed conflict), but exactly the kind of confusing, silently-dead
configuration this project's philosophy (`GpuUnsupportedReason`, CAM's
disc-bound, §8's whole approach) has consistently chosen to reject
explicitly rather than allow. Fixed: `config::load_config` now rejects
`cam` combined with `recursion` at load time.

**Gap 2: `recursion` + `min_age > 0`.** The cascade's per-level match
check, `applicator::pattern_matches_effective`, tests only cell *types*
against the translated `pattern` — it does not check `age(x, y) >=
rule.min_age` at each cascade level. The origin match (level 0) is
correctly gated by `min_age` through the ordinary `detect_matches` path,
but levels `1..=max_depth` are not gated at all: a rule declaring "wait
`N` ticks of stability before firing" would have that guarantee silently
hold only at the cascade's origin and not at any cell the cascade expands
into. Fixed: `config::load_config` now rejects `recursion` combined with
`min_age > 0`.

Both fixes follow the same shape as every other exclusion validation
already in `config.rs` (`cam`+`shifts`, `cam`+explicit `pattern`,
`feedback`+shift-count, `recursion`+shift-count) — an explicit
`CellariaError::RuleValidation` at load time, not a runtime surprise.

### 9.3. Findings — checked and clean

- **`cam` × `min_age` / `active_only`.** `matcher::detect_cam_matches`
  explicitly checks both (`center_age < rule.min_age` and the
  `active_only` guard) before searching for a target — CAM's separate
  detection path does *not* bypass either, despite not going through the
  ordinary `detect_matches`/pattern-matching code.
- **`tie_break` / `starvation_after` × everything else.** Neither field
  changes *where* a rule writes — both modulate the arbitration sort key
  only (`arbitrator::resolve_sort_fields`) — so by Corollary 3 they are
  categorically outside Lemma 4's concern and cannot interact with
  `cam`/`recursion`/`feedback`'s write-footprint mechanics. A rule *can*
  combine `starvation_after` with `feedback` or `recursion` (no shift-count
  conflict forces exclusion) — the two mechanisms track independent state
  (`Engine.starvation_counters` vs `Engine.feedback_counters`, disjoint
  `HashMap`s) and don't share keys or interfere.
- **`feedback` × `recursion`.** Mutually exclusive *by construction*
  already, not by an added check: `feedback` requires exactly one shift,
  `recursion` requires zero — no rule can satisfy both shift-count
  constraints simultaneously.
- **Grid-boundary safety for `recursion`'s cascade.** `applicator::read_cell_effective`
  reads through `Grid::get_cell` → `VecStorage::get`, which explicitly
  bounds-checks (`x >= self.width || y >= self.height` → `None`) rather
  than panicking or indexing out of range. A cascade running off the edge
  of the grid reads back `CellValue::default()` (type 0) for every
  out-of-bounds pattern cell, which — for any pattern that isn't itself
  checking for type 0 — simply fails to match, giving exactly the
  intended "cascade stops at the grid boundary" behavior. This mirrors the
  *pre-existing* convention used by ordinary (non-recursive) pattern
  matching elsewhere in the engine (`apply_rule_buffered`'s own
  `pattern_buffer` construction uses the identical
  out-of-bounds-returns-default rule) — recursion introduces no new
  inconsistency here, it inherits one that already existed.

### 9.4. Summary table

| Pair | Result |
|---|---|
| `cam` × `min_age`/`active_only` | Compatible — verified in `detect_cam_matches` |
| `tie_break`/`starvation_after` × anything | Orthogonal — outside Lemma 4 by Corollary 3 |
| `starvation_after` × `feedback`/`recursion` | Compatible — independent counters, no shared state |
| `feedback` × `recursion` | Mutually exclusive by shift-count construction |
| `cam` × `recursion` | **Gap, fixed** — explicit rejection added |
| `recursion` × `min_age > 0` | **Gap, fixed** — explicit rejection added |
| `recursion`'s cascade × grid boundary | Compatible — safe, matches pre-existing convention |

---

## 10. Self-Modification Exclusion

`paper2.md` §4.6 (Guarded Composability Under Self-Modification) proves
composition safety for rules transmitted through the `RuleStore` protocol
— but that proof, and the protocol itself, both predate every extension
field discussed in §8-9. This section states plainly what the wire
protocol actually carries today.

**Fact, corrected 2026-08-13 (originally verified only against the base
`AddRule` op-code — too narrow).**
The protocol now has two op-codes, not one. The original `AddRule` packet
still constructs its `Rule` with `cam: None, tie_break: 0,
starvation_after: None, feedback: None, recursion: None`, unconditionally
— confirmed by `test_add_rule_protocol_cannot_express_feedback_recursion_memory_or_keep_source`
(`src/rule_store_tests.rs`), which is accurate for that op-code
specifically. But `AddRuleExtended` (`src/rule_store_deserialize.rs`,
added 2026-08-08 — see §8.5's status note) carries an `ext_flags` byte
that *can* encode `cam` and `recursion` (bits 0 and 1). `tie_break`,
`starvation_after`, `feedback`, `memory`, and `max_activations` remain
unencodable by either op-code — no bytes exist anywhere in the current
protocol for them.

**Corollary 8 (revised).** For the fields still wire-unreachable
(`tie_break`, `starvation_after`, `feedback`, `memory`,
`max_activations`), the original argument is untouched: `paper2.md`'s
Composition Theorem (§4.1) and its guarded-self-modification extension
(§4.6) apply to them exactly as stated, because they cannot arrive
through this mechanism at all. For `cam` and `recursion`, which now can
arrive via `AddRuleExtended`, the safety argument is different but still
holds — under `Engine::enable_guarded_self_modification` (opt-in, default
off), every decoded rule is checked by `composition_allows` /
`conflict_analyzer::check_composition` before installation, which
computes conflicts through the *same* `compute_rule_data`/`Affected⁺`
machinery §8's Lemma 4 already proved sound for `cam` and `recursion`
(Corollaries A and B). So a transmitted rule using either field is
checked with exactly the bound Lemma 4 established, not exempted from it
— composition remains provably safe under the guard, not because `cam`/
`recursion` are unreachable (they no longer are), but because the guard's
own soundness proof already covers them. Without the guard enabled,
`paper2.md` §4 never claimed safety for *any* self-transmitted rule, `cam`/
`recursion` included, so nothing changes there either.

---

## 11. GPU Parity

**Update note (2026-08-13, factual correction only).** The table below, as it stood through
2026-08-03, claimed broadcast shift, starvation guard, feedback, and
bounded recursion were all "rejected outright" on GPU. That is no longer
true, and per session notes it stopped being true within days of this
paper's cutoff (persistent per-cell GPU counter buffers removed the
"no cross-tick state" obstacle each of those rejections cited). This
section now states current reality; it does not attempt to re-derive
*why* each port works — that belongs in a future revision, not a
same-day factual patch.

Current `gpu::rule_table::GpuUnsupportedReason` variants (`src/gpu/rule_table.rs`),
split the same way as before:

| Category | Variants |
|---|---|
| Structural caps | `NoEffect`, `ChangeIsRef`, `ChangeIsArithmetic`, `OverflowNotDiscard`, `TooManyShifts`, `TooManyChanges`, `PatternTooLarge`, `RuleIdTooLong`, `TooManyRulesForArbitration`, `ShiftTooFar`, `ChangeTooFar` |
| Per-extension reach caps | `CamRadiusTooFar` (`radius > MAX_CAM_RADIUS = 16`), `BroadcastPathTooLong` (`steps > MAX_BROADCAST_REACH = 4`), `RecursionDepthTooLarge` (`max_depth > MAX_RECURSION_DEPTH = 4`), `MemoryWindowTooLarge` (`window > MAX_MEMORY_WINDOW = 4`) |
| Pairwise combination gaps | `CamRecursionUnsupported`, `MemoryRecursionUnsupported`, `MemoryCamUnsupported`, `FeedbackKeepSourceUnsupported`, `MemoryKeepSourceUnsupported`, `FeedbackBroadcastUnsupported`, `MemoryBroadcastUnsupported`, `FeedbackChangeCollidesWithShiftTarget`, `MemoryChangeCollidesWithShiftTarget` |
| Structural, still unconditional | `MaxActivationsUnsupported` (deliberate, not yet ported — see below) |

| Extension | GPU support |
|---|---|
| CAM | Full parity within `radius <= MAX_CAM_RADIUS`. |
| Modular tie-break | Full parity, unconditional. Verified bit-for-bit by `tests/gpu_v2_correctness.rs`. |
| Broadcast shift | Supported within `steps <= MAX_BROADCAST_REACH`; longer paths rejected. |
| Starvation guard | Full parity, unconditional (no dedicated exclusion variant exists). `test_gpu_v2_starvation_matches_cpu_periodic_progress` and related tests in `tests/gpu_v2_correctness.rs`. |
| Feedback | Supported, except combined with `keep_source`, `broadcast`, or a `changes` offset that collides with the shift target. `test_gpu_v2_feedback_switches_direction_after_timeout_matches_cpu` and related tests. |
| Bounded recursion | Supported within `max_depth <= MAX_RECURSION_DEPTH`, except combined with `cam` or `memory`. `test_gpu_v2_recursion_cascade_matches_cpu_in_one_tick` and related tests. |
| `Rule::memory` (not yet built as of this paper's original text; see §14) | Supported within `window <= MAX_MEMORY_WINDOW`, except combined with `keep_source`, `recursion`, `cam`, `broadcast`, or a colliding `changes` offset. `test_gpu_v2_memory_neighbor_type_gate_and_relocation_matches_cpu` and related tests. |
| `Rule::max_activations` (not yet built as of this paper's original text; see §14) | Rejected outright — deliberate, not an oversight: its counter keys on `(head, rule_idx)` alone (no cell position), a different shape than every other persistent GPU counter buffer (all keyed by grid position), and would need its own buffer and gate pass. |

Every rejection above remains a hard build-time error
(`build_gpu_rule_table` returns `Err`), never a silently-dropped rule or
a best-effort approximation — the principle this section originally
stated is unchanged; only the extent of what's rejected has shrunk.

---

## 12. Reversibility Across I/O Boundaries

`paper2.md` §7 (Theorem 9) proves that a conflict-free, locally
invertible, distinguishable rule set has a bijective tick function on
*grid configurations* — but every rule in that proof's supporting example
(`proof_reversibility.rs`) uses `overflow: Discard` throughout; no shift
ever crosses the grid's edge. This section makes explicit an assumption
Theorem 9 carried implicitly, and shows precisely where it stops holding.

### 12.1. The gap

When a shift's target overflows the grid under `OverflowAction::Write`/
`WriteLiteral`, the value that leaves is appended to a boundary buffer's
output queue — external to the grid configuration entirely. The source
cell is still cleared (ordinary shift semantics). From the *grid's*
point of view alone, this looks like plain information loss: nothing
in the post-tick grid configuration records what left, so Definition 6's
"injective local map from pattern cells to written cells" fails — the
written side of the map (within the grid) is smaller than the read side,
and is not invertible from the grid alone.

### 12.2. Extended configurations

**Definition 11 (Extended configuration).** An extended configuration is
a pair `(config, B)`, where `config` is an ordinary grid configuration
(as in `paper2.md`) and `B` is the state of every boundary buffer's
input and output queues.

**Theorem 10 (Reversibility on extended configurations).** Let `R`
satisfy Theorem 9's hypotheses (empty conflict graph, local invertibility,
distinguishability), extended so that a boundary-buffer write counts as
part of a rule's "written cells" (the newly appended output-queue entry is
the local map's output for that piece of the rule, exactly as an ordinary
grid cell write is). Then the tick function `(config_t, B_t) → (config_{t+1},
B_{t+1})` is a bijection on reachable extended configurations, and there
is an inverse rule set `R⁻¹` (shifts reversed, as in Theorem 9) such that
applying `R⁻¹` reconstructs `(config_t, B_t)` from `(config_{t+1}, B_{t+1})`
— **provided** the reverse pass's *input* boundary, at the same physical
port, receives exactly the value the forward pass's *output* boundary
emitted, at the mirror tick.

*Proof.* Identical to Theorem 9's, with `Affected`/written-cells
extended to include boundary-queue slots as a rule's write targets when
`OverflowAction::Write`/`WriteLiteral` fires. The conflict-graph/Theorem-3
decomposition into independent local transformations is unaffected by
this extension (a boundary write is still a single, disjoint write
target, no different in kind from a grid-cell write for the purposes of
Lemma 3's disjointness argument). Local invertibility of the extended map
follows from ordinary shift invertibility (a shift is injective; treating
an out-of-bounds target as "write to the queue" instead of "write to a
clamped grid cell" changes *where* the value goes, not *whether* the
map is injective). ∎

**Corollary 9 (Theorem 9 is the closed special case).** Theorem 9, as
originally stated, is not incomplete or wrong — it is exactly Theorem 10
restricted to rule sets where `B` is always empty (no rule ever invokes
`OverflowAction::Write`/`WriteLiteral`), in which case tracking `B` is
vacuous and the extended and ordinary statements coincide. The general
principle is: **Cellaria's reversibility is a property of the closed
loop (grid *and* boundary channels together), not of the grid in
isolation** — the grid alone is only reversible in the special,
previously-unstated case where nothing ever crosses its boundary.

### 12.3. Empirical validation

`examples/proof_reversibility_boundary_io.rs` (block G, item 2) runs
both halves of this corollary as one program:

1. **Naive reverse (grid only).** A token shifts to the grid's edge,
   overflows via `OverflowAction::Write(0)`, and is gone from the grid
   entirely. Reversing *only* the grid (building `R⁻¹` and running it
   on the final, token-less grid) does not reconstruct the original —
   there is nothing left anywhere in the grid configuration to invert.
   Verified: `naive_result != initial_snapshot`.
2. **Closed-loop reverse.** The exact byte captured from the forward
   pass's output queue is re-injected as input at the *same* boundary
   port, at the tick that mirrors the moment it left (`TOTAL_TICKS -
   exit_tick + 1`, found empirically after an off-by-one from injecting
   before vs. after that tick's `run_tick()` call — the token must
   reappear at the boundary cell *after* that tick's shift rule has
   already run, or it gets shifted one cell too far). Verified:
   `closed_result == initial_snapshot`, cell for cell, across the entire
   grid.

Both assertions pass, confirming Theorem 10/Corollary 9 in both
directions — the failure mode is real (grid-only reversal genuinely
loses information), and the fix is exactly what the theorem predicts
(feed the output back as input at the mirror tick, not merely "try
harder" on the grid alone).

---

## 13. Discussion

Sections 8-12 together give the full accounting for everything about
`Rule`'s extension mechanism that has been checked, as of 2026-08-03:
what's provably safe to combine, what silently wasn't (and now is
rejected instead), what the self-modification channel can and cannot
transmit, what the GPU backend can and cannot run, and where the earlier
reversibility result's implicit closed-system assumption actually stops
holding. Two genuinely new bugs were found in this process (§9.2), both
by writing and running code against the design rather than by re-reading
the proof sketches more carefully — consistent with every other finding
in this project's history (the CAM double-detection bug, the `feedback`
counter-relocation bug, the min_age dirty-tracking investigation): the
proofs constrain what is *possible*, but only running the adversarial
case surfaces what the implementation actually *does*.

Not covered here: a field-by-field anatomy of `Rule` with no accompanying
theorem (deliberately excluded as padding). "Rules with memory," the one
item this section originally left as an open direction, has since been
designed and built — see §14, added as a factual patch rather than a
retroactive rewrite of this section's "as of 2026-08-03" framing.

---

## 14. Post-2026-08-03 Additions (Factual Summary)

Added 2026-08-13. Four systems were built after this paper's original
text; none has a Lemma-4-style corollary or its own theorem here — this
section states what exists and where, deferring formal treatment (if any
turns out to be warranted) to a future revision.

**`Rule::memory` (`Option<MemorySpec>`, `src/types.rs`).** A FIFO-buffer
gate: unlike `starvation_after`/`feedback`'s scalar counters, it can
distinguish one specific past sequence (e.g. `[Missed, Applied, Missed]`)
from any other of the same length. Two trigger kinds
(`RecordTrigger::NeighborType`/`RuleOutcome`); the gate opens only when the
buffer is full and matches `match_pattern` element-for-element. Full
semantics: `specs/specification.md` §4.9. GPU support: §11 above. Not
compatible with `recursion` when the trigger is `RuleOutcome` (no
per-cascade-level arbitration outcome exists to record).

**`LayeredEngine` (`src/layered.rs`).** A stack of same-sized 2D grids,
each its own `Engine` ticking independently through the ordinary
`run_tick`-based pipeline (not the raw, state-dropping phase methods —
an early version used those and silently disabled `cam`/`starvation_after`/
`feedback`/`memory`/`max_activations` for every layered rule; fixed).
First slice: cross-layer *read* only
(`Rule::cross_layer_reads`, `(dx, dy, dz, type)`), no cross-layer write.

**`Rule::max_activations` (`Option<u32>`, `src/types.rs`).** A global
(not per-position) budget on how many times a rule may win arbitration in
total — designed as a backstop against unbounded growth under
`ShiftSpec::keep_source`, but applicable to any rule. Not wire-encodable
(`RuleStore` protocol), not GPU-supported (§11). Full semantics:
`specs/specification.md` §4.10.

**`ChangeValue::Add`/`Sub` (`src/types.rs`).** Two additional `ChangeValue`
variants alongside `Literal`/`Ref`: `u8::wrapping_add`/`wrapping_sub` over
two nested `ChangeValue`s. CPU-side unconditional; GPU rejects rules using
either (`GpuUnsupportedReason::ChangeIsArithmetic`, §11) since the shader
subset expects a ready-made `u32` literal, not an expression to evaluate.
