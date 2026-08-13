# Cellaria: Termination, Complexity, and Expressiveness

## Abstract

Cellaria is a cellular automaton-like model of computation based entirely
on local reduction, defined by five axioms. This paper explores the
**limits of the model**: when does a simulation terminate, how many ticks
does it take, and what can be expressed within Cellaria's constraints?

We present seven contributions:

1. **Termination via potential functions** — sufficient conditions for
   termination using three classes of potential functions (geometric,
   counting, energetic), plus a lightweight runtime check that peeks one
   tick ahead to classify the current state as active or stable.

2. **Complexity classes** — definition of CF (Conflict-Free) and CA
   (Conflict-Aware) programs, with a tight Θ(M log M) bound for CA
   arbitration on bounded-reach rule sets.

3. **Expressiveness: cellular automata** — a constructive mapping from
   any 1D cellular automaton (k=3) to Cellaria, proving that Cellaria
   can simulate any such CA in O(N) ticks.

4. **Expressiveness: Turing machines** — a formal reduction from Turing
   machines to Cellaria, with a proof that one tick corresponds to
   exactly one TM step. A sorting reduction is also presented.

5. **Expressiveness: the converse direction** — Section 4.1 shows every
   classical cellular automaton embeds into Cellaria. We examine whether
   the reverse holds. It does not, unconditionally: Cellaria's
   arbitration tie-break can depend on a match's absolute grid position,
   which a translation-invariant classical CA transition function cannot
   see (Lemma 3). Restricted to rule sets where this never happens — a
   checkable condition we call *tie-break-local* — the converse does
   hold: any such Cellaria program can be simulated by a classical CA
   with a bounded neighborhood and finite alphabet (Theorem 6). Combined
   with Section 4.1, this establishes full equivalence between classical
   CA and tie-break-local Cellaria, not between CA and Cellaria at large.

6. **Universal self-reflection** — a single, fixed rule set, built with
   no advance knowledge of which rule it will ever transmit, suffices to
   compute and install *any* rule expressible in Cellaria's own
   self-modification protocol (`RuleStore`); a new target rule costs only
   data placement, never a new rule (Theorem 7). Closing this required
   one small, additive fix to the engine (`OverflowAction::WriteLiteral`),
   since the existing protocol had no way to transmit a literal zero byte
   — needed by, among other things, every same-cell change.

7. **Self-attestation** — the same self-modification channel can carry
   not just a rule the automaton *decided* to install (Section 4.5), but
   a value the automaton *computed about an extended region of its own
   state* (Theorem 8) — a checksum over a data sequence, verifiable
   against the actual grid by an outside observer, and provably sensitive
   to a single tampered cell. Explicitly scoped as a demonstration of the
   mechanism, not a cryptographically meaningful checksum.

---

## 1. Introduction

The Cellaria model [1] is defined by five axioms and a tick cycle of
detection, arbitration, and application. While companion papers [2, 3]
address the arbitration mechanism in detail, this paper addresses
foundational questions about the model's capabilities:

- **When does a Cellaria program stop?** (Section 2)
- **How fast can it run?** (Section 3)
- **What can it compute?** (Section 4)

The termination analysis (Section 2) provides sufficient conditions
based on potential functions. The complexity analysis (Section 3)
defines two complexity classes and proves a tight `Θ(M log M)` bound for
CA arbitration. The expressiveness analysis (Section 4) provides constructive mappings
from cellular automata, Turing machines, and sorting to Cellaria,
establishing that Cellaria is at least as expressive as these models.

---

## 2. Termination via Potential Functions

### 2.1. Problem Statement

A Cellaria simulation terminates when no rule matches any cell on the
grid. We present sufficient conditions for termination based on potential
functions, plus a lightweight one-tick-lookahead check,
`detect_termination`, that classifies the *current* state as active or
stable.

The standard approach is to find a **potential function** (also called a
ranking function or measure) `Φ: Configuration → ℕ` that strictly
decreases on every tick. If `Φ` is bounded below (e.g., `Φ ≥ 0`), then
the simulation must terminate after at most `Φ(initial)` ticks.

### 2.2. Theorem 1: Sufficient Condition for Termination

**Theorem 1 (Potential function termination).** Let `C` be the set of
all reachable configurations of a Cellaria simulation. If there exists a
function `Φ: C → ℕ` and a constant `c > 0` such that for every tick:

```
Φ(next_configuration) ≤ Φ(current_configuration) − c
```

then the simulation terminates after a finite number of ticks.

*Proof.* Let `Φ₀ = Φ(config₀)` be the initial potential. After `t`
ticks, the potential is at most `Φ₀ − t·c`. Since `Φ ≥ 0` (it maps to
`ℕ`), we must have `t ≤ Φ₀ / c`. Therefore, the simulation cannot run
for more than `⌊Φ₀ / c⌋` ticks. ∎

### 2.3. Classes of Potential Functions

We define three classes of potential functions for Cellaria.

#### 2.3.1. Geometric Potential

**Definition 1 (Geometric potential).** The geometric potential of a
configuration is the sum of distances of all active marker cells to the
grid boundary:

```
Φ_geo = Σ_{(x,y) ∈ active_markers} d((x,y), boundary)
```

where `d` is the Manhattan distance to the nearest boundary cell.

**Example: Turing machine simulation.** In `turing.yaml`, the head
(marker type 10) moves toward the boundary. The geometric potential
decreases when the head moves closer to the boundary. However, since
the head may reflect and move back, the geometric potential alone is
not monotonically decreasing. A combined potential is needed (see
Section 2.4).

#### 2.3.2. Counting Potential

**Definition 2 (Counting potential).** The counting potential of a
configuration is the number of non-default cells on the grid:

```
Φ_cnt = |{ (x,y) | cell(x,y) ≠ default }|
```

**Example: Tag system simulation.** In `tag_system.yaml`, the simulation
processes a string by deleting the first `m` symbols and appending the
production `π(X)` of the first symbol `X`. For a finite input string
with productions of fixed length, the number of active cells decreases
monotonically. The counting potential is `string_length + 1` (for the
marker). Each tick deletes `m` symbols (`m = 2`) and adds at most
`|π(X)|` symbols (`|π(X)| ≤ 2` for the given productions), so `Φ_cnt`
never increases. For `m = 2` and productions of length ≤ 2, `Φ_cnt`
strictly decreases, guaranteeing termination.

#### 2.3.3. Energetic Potential

**Definition 3 (Energetic potential).** The energetic potential of a
configuration is the sum, over all cells that are waiting for a
`min_age`-guarded rule to activate, of the remaining waiting time:

```
Φ_ener = Σ_{c ∈ WaitingCells} (min_age_required(c) − age(c))
```

where:
- `WaitingCells` = the set of cells that will be matched by at least
  one rule with `min_age > 0` in some future tick.
- `min_age_required(c)` = the maximum `min_age` value among all rules
  that can match cell `c`.

**Justification.** For a cell requiring `min_age = 10`, the energetic
potential is 10 − age(c) at each tick. The potential is strictly
positive when the cell is below the threshold, decreases by exactly
1 on each tick (since age increases by 1 and the threshold is fixed),
and reaches 0 when age ≥ min_age — at which point the rule activates,
the cell changes, and it leaves `WaitingCells`.

**Conservative bound.** Since determining the exact set of
`WaitingCells` requires knowledge of future matches, a conservative
over-approximation can be used: treat all active cells with non-zero
`min_age` rules as waiting cells. This may over-count, but preserves
the decreasing property.

**Example.** A cell with `min_age: 10` and current age 0 starts with
energetic potential 10. After 5 ticks, age = 5, potential = 5. After
10 ticks, the rule activates, the cell is modified, and its contribution
to Φ_ener is removed. The energetic potential has strictly decreased
from 10 to 0 over 10 ticks.

**Relationship to cleanup rules.** Cleanup rules (Axiom 5) use
`min_age` to delay cell removal. A cleanup rule with `min_age: 10`
will fire exactly when the cell's age reaches 10, assuming no other
rule modifies the cell first. The energetic potential formalises
this waiting period as a decreasing measure.

### 2.4. Combined Potential

**Theorem 2 (Combined potential).** If `Φ₁, Φ₂, ..., Φₖ` are potential
functions, then any linear combination with non-negative coefficients

```
Φ = a₁·Φ₁ + a₂·Φ₂ + ... + aₖ·Φₖ
```

is also a potential function. If at least one `Φᵢ` strictly decreases
on each tick and the others do not increase, then `Φ` strictly decreases.

*Proof.* If each `Φᵢ` decreases by at least `cᵢ ≥ 0` on each tick, and
at least one `cⱼ > 0`, then `Φ` decreases by `Σ aᵢ·cᵢ ≥ aⱼ·cⱼ > 0`. ∎

**Example: Turing machine termination.** For the bit-inverting Turing
machine in `turing.yaml`, the head moves exclusively rightward (all
transitions shift east). Therefore:

- `Φ₁` = number of unprocessed tape symbols (counting potential).
  Each tick processes one symbol, decreasing Φ₁ by 1.
- `Φ₂` = distance from head to the right boundary (geometric potential).
  Each tick moves the head rightward by 1, decreasing Φ₂ by 1.

The combined potential `Φ = Φ₁ + Φ₂` strictly decreases by at least 1
on each tick. When the head reaches a blank cell (type 0) with no
matching rule, Φ₁ = 0 and the simulation terminates.

For Turing machines with bidirectional head movement, a different
potential function is required. A classical choice is the pair
(number of unprocessed symbols to the left of the head, head position),
ordered lexicographically. The development of a systematic method for
constructing potential functions for arbitrary Cellaria rule sets
remains an open problem.

### 2.5. Static Prediction of Termination

The static conflict analyzer [2] can be extended to predict
termination. For each rule, the analyzer computes:

- **Creation count:** number of non-default cells created by the rule.
- **Destruction count:** number of non-default cells destroyed by the
  rule.

**Lemma 1 (Counting termination).** For a rule set R, let:
- `destroy(Rᵢ)` = number of non-default cells cleared or overwritten by
  rule Rᵢ (pattern cells that become default or are replaced).
- `create(Rᵢ)` = number of default cells that become non-default (shift
  destination, changes).

If for all Rᵢ ∈ R: `destroy(Rᵢ) > create(Rᵢ)`, then:

```
Φ_cnt(config_{t+1}) ≤ Φ_cnt(config_t) − 1
```

for every tick where at least one match fires, guaranteeing termination.

*Proof.* Each accepted match removes `destroy(Rᵢ)` non-default cells and
adds `create(Rᵢ)` new ones. By the condition, the net change per match
is strictly negative (≤ −1). When the conflict graph is empty and has no
self-loops, all matches in a tick are pairwise non-overlapping in their
affected regions [2, Theorem 3], so the net change across all matches is
the sum of individual changes. The total decrease is at least the number
of accepted matches, which is ≥ 1 if any match fires. By Theorem 1, the
simulation terminates. For non-empty conflict graphs, arbitration ensures
pairwise non-overlapping accepted matches, and the same argument applies
to the accepted subset. ∎

This is a conservative criterion: false negatives are possible (a
simulation may terminate even if the criterion is not met), but false
positives are not.

### 2.6. Limitations

**Theorem 3 (Potential is sufficient, not necessary).** The existence
of a decreasing potential function is sufficient for termination
(Theorem 1), but not necessary.

*Proof.* The simulation in `configs/oscillation.yaml` demonstrates this:
a marker (types 1↔2) oscillates between positions 0 and 1, while a timer
(type 99) starts at position 5 and walks west by 1 step per tick. The
timer has higher priority (20) than the marker (10). At tick 4, the
timer reaches position 1 while the marker is at position 0 — the timer
shifts from 1 to 0, overwriting the marker. The simulation terminates
at tick 6. Yet `Φ_cnt` is constant (2) for the first 5 ticks — it never
strictly decreases until the final ticks. Therefore, `Φ_cnt` is not
monotonic: it does not capture all terminating simulations. ∎

All termination criteria presented are **sound but not complete**.
Without restricting the class of rules, a decreasing potential is the
best guarantee that can be given.

### 2.7. Runtime Termination Detection

`Engine::detect_termination(&self, tick: u32) -> TerminationVerdict` gives
a cheap, non-mutating one-tick lookahead, not a trend-based monitor over
an observation window. It re-detects matches against the current grid
state and, for `tick > 0`, runs them through real arbitration (using the
engine's actual `starvation_after`/`feedback` counters, read-only) to see
whether anything would still be accepted. The verdict is binary:

| Verdict | Condition |
|---------|-----------|
| `Stable` | No matches found, or arbitration accepts none of them |
| `Active` | At least one match survives arbitration (or `tick == 0`, before any lookahead is meaningful) |

This is deliberately narrower than a full liveness classifier: it answers
"would the *next* tick change anything?", not "will this simulation ever
stabilize?" — a simulation that alternates between two non-empty states
forever is reported `Active` at every tick, not flagged as diverging. The
method also has a known, documented gap: `memory`/`max_activations` gates
are applied as a match filter inside the real tick pipeline, not inside
the arbitration call `detect_termination` uses directly, so a match that
the real tick would filter out can still make this lookahead report
`Active` when the true next tick would in fact be `Stable`.

Tests `test_detect_termination_stable` and `test_detect_termination_active`
(`src/engine/tests/basics.rs`) cover both verdicts directly.

---

## 3. Complexity of Cellaria Programs

### 3.1. Definitions

We define the computational complexity of a Cellaria program in terms of
the grid configuration and the tick cycle:

- **Input size.** The input size of a Cellaria program is the number of
  active (non-default) cells in the initial configuration.
- **Computation step.** A single computation step is one tick, consisting
  of detection, arbitration, application, aging, and cleanup.
- **Time complexity.** The time complexity is the number of ticks
  executed until termination.
- **Space complexity.** The space complexity is the maximum number of
  active cells observed across all ticks.

### 3.2. Complexity Classes

**Definition 4 (CF — Conflict-Free).** A Cellaria program belongs to
class CF if its conflict graph [2] is empty and contains no self-loops.
For a CF program:

- **Arbitration is bypassed.** By [2, Theorem 3], all matches are
  pairwise non-overlapping and applied simultaneously.
- **Per-tick time:** `O(M)` where `M` is the number of matches.
- **Total time:** depends on logic. TM simulation: `O(N)` ticks;
  tag system: `O(N)` ticks; parallel rules: `O(1)` ticks.
- **Arbitration cost:** none.

CF programs are the simplest class: concurrency is free, no arbitration
is needed, and matches execute in lockstep.

**Definition 5 (CA — Conflict-Aware).** A Cellaria program belongs to
class CA if its conflict graph is non-empty. For a CA program:

- **Arbitration is required.** By [2, Theorem 1], arbitration is
  deterministic; it costs `O(M log M + M·R)` in the worst case, where `R`
  is the largest affected-region size among the `M` matches (Theorem 4,
  Section 3.3) — not the `O(M²)` pairwise-comparison cost a naive
  algorithm would need.
- **Per-tick time:** `O(M log M)` for bounded-reach rule sets (`R = O(1)`,
  the common case).
- **Total time:** depends on program logic.
- **Arbitration cost:** `O(M log M)` per tick for bounded-reach rules.

CA programs subsume CF programs: every CF program is trivially in CA
(the conflict graph is empty, so arbitration selects all matches in
`O(M)` time), but CF programs avoid even the sort-and-hash overhead
entirely.

### 3.3. Bound for CA Arbitration

**Erratum.** An earlier version of this section claimed an `Ω(M²)` lower
bound: *any* arbitration algorithm on `M` pairwise-conflicting matches
supposedly needs `Ω(M²)` comparisons, via a witness construction (`M`
rules with identical `id` and `priority`, all matching the same cell) and
a greedy pairwise-comparison argument. The argument's "tightness" step —
"the algorithm cannot skip any pair, as this would produce incorrect
results" — implicitly assumes conflict-freedom can only be checked by
literally comparing pairs of matches. It cannot: the actually-shipped
algorithm (`arbitrate_with_cam`, `src/engine/arbitrator.rs`) checks
conflicts by looking up each match's affected *cells* in a hash map keyed
by cell coordinate, never comparing one match against another directly.
On the erratum's own witness construction, this reduces the claimed
`Θ(M²)` to `O(M log M)` — confirmed both by reading the algorithm and by
measuring it (below). The mistake was treating a lower bound for one
*restricted* algorithm family (pairwise comparison) as a lower bound for
the arbitration *problem*, the same category of error a naive `Ω(n log
n)`-for-any-sort claim makes against counting/radix sort.

**Theorem 4 (Upper bound for CA arbitration).** For `M` matches with
maximum affected-region size `R`, `arbitrate_with_cam` accepts a
conflict-free, deterministically tie-broken subset in `O(M log M + M·R)`
time.

*Proof.* The algorithm has two phases. **(1) Sort.** All `M` matches are
sorted once by the full tie-break key `(priority, age, rule_id, x, y,
rule_idx)`, descending — `O(M log M)` comparisons (each comparison is
`O(1)`: a fixed-width tuple compare). **(2) Single greedy pass.** For each
match in sorted order, its affected cells (at most `R` of them) are each
looked up in a hash map `used_cells: HashMap<(i32,i32), usize>` — `O(R)`
expected-time lookups. If none are present, the match is accepted and its
≤`R` affected cells are inserted into the map — another `O(R)` expected-time
operation. No match is ever compared against another match directly;
every conflict check is a cell-coordinate lookup against the map built so
far, which costs the same `O(1)` expected time regardless of how many
matches have already been accepted. Total: `O(M log M)` for the sort plus
`O(M·R)` for the pass, i.e. `O(M log M + M·R)`. ∎

**Corollary 1 (Tight bound for bounded reach).** For rule sets with reach
independent of `M` (`R = O(1)` — the common case: a rule's pattern and
`changes` have a fixed size fixed by the rule set, not by how many matches
happen to fire this tick), arbitration costs `O(M log M)`. This is tight:
producing a *deterministic* result requires resolving ties by the full
`(priority, age, rule_id, x, y, rule_idx)` order, and an adversary can
present `M` matches in any of `M!` distinct tie-break orderings, forcing
`Ω(log(M!)) = Ω(M log M)` comparisons to establish that order (the
standard decision-tree argument used for comparison-sort lower bounds).
`arbitrate_with_cam` meets this bound up to the `R` factor — it is
asymptotically optimal for bounded-reach rule sets, not merely fast in
practice. The retracted `O(M²)`/`Ω(M²)` claim (original Theorem 4 /
Corollary 1) no longer appears anywhere in this paper's results.
Empirical confirmation of
this bound is in Section 3.4.4, alongside the paper's other measured
benchmarks; the algorithmic argument above (sort + hash lookup, no
pairwise comparison) is the actual proof, not the timing data.

### 3.4. Empirical Complexity

**Methodology note (2026-08-16).** Every table in this section was
previously captioned "Measurements from `configs/X.yaml`," but the
checked-in config files are each a single fixed instance (e.g.
`configs/turing.yaml` hard-codes one 5-symbol tape), not a parametrized
sweep — they cannot by themselves produce a multi-row "Len → Ticks"
table. The tables below are re-measured directly (same rule sets,
generated inputs at each listed size, run to completion by
`Engine::run_tick` in a loop, ticks counted until no match remains) — the
original tables were found to be unreproducible and, in one case,
numerically wrong rather than merely unsourced.

#### 3.4.1. Linear Time: Turing Machine Simulation

**Hypothesis.** A Turing machine simulation in Cellaria requires
`O(T)` ticks for `T` steps of the machine.

**Data.** The bit-inverting Turing machine of Section 4.3.4
(`test_tm_translator_bit_invert`), run via `translate_tm` on tapes of
each length below:

| Len | Ticks | Ratio |
|-----|-------|-------|
| 10  | 10    | 1.00  |
| 50  | 50    | 1.00  |
| 100 | 100   | 1.00  |
| 200 | 200   | 1.00  |

**Result.** `ticks = len` exactly, at every tested length — confirmed,
not just re-sourced. Cellaria simulates this Turing machine without
overhead: one tick corresponds to exactly one step of the machine.

#### 3.4.2. Linear Time: Tag System

**Hypothesis.** A tag system simulation requires `O(N)` ticks for a
string of length `N`.

**Data.** The same rule set as `configs/tag_system.yaml` (Minsky
`m=2` tag system, one step), run on generated strings of each length
below:

| Len | Ticks | Ratio |
|-----|-------|-------|
| 5   | 6     | 1.20  |
| 10  | 11    | 1.10  |
| 20  | 21    | 1.05  |
| 50  | 51    | 1.02  |

**Result.** `ticks = len + 1`, not `ticks = len` as an earlier version
of this table claimed — the read/delete phase (Phase 1-2 of the rule
set) costs one tick before the traverse phase begins, a fixed overhead
independent of `N`. Still `O(N)` overall, and the ratio still visibly
converges to 1 as `N` grows, but the previous table's claim of an exact
`1.0` ratio at every length did not match reality even at `N=5`. Marker
processing is asymptotically optimal; it is not, as stated before,
overhead-free.

#### 3.4.3. Constant Time: Conflict-Free Rules

**Hypothesis.** A rule set with an empty conflict graph terminates in
`O(1)` ticks, independent of grid size.

**Data.** The same two-rule structure as `configs/parallel.yaml`, on
grids of each width below:

| Width | Ticks |
|-------|-------|
| 8     | 1     |
| 16    | 1     |
| 32    | 1     |
| 64    | 1     |

**Result.** Two independent conflict-free rules fire concurrently and
terminate in one tick, regardless of grid size — confirmed exactly as
originally claimed.

#### 3.4.4. Arbitration Scaling: Worst-Case Construction

`configs/worst_case_arbitration.yaml` encodes the construction from
Section 3.3's Theorem 4 proof at small scale (`M = 5`): `M` rules with
identical `id` and `priority`, all matching the same cell, so exactly
one is ever accepted regardless of `M`. Timing the arbitration phase
alone (`Engine::run_tick_profiled`) at this construction scaled up to
`M = 32 000`:

| M | arbitrate time | M(M−1)/2 would predict (relative to M=100) |
|-------|-----------|-----------|
| 100   | 0.011 ms  | 0.011 ms (baseline) |
| 1 000 | 0.180 ms  | 1.1 ms |
| 4 000 | 0.583 ms  | 17.6 ms |
| 8 000 | 1.315 ms  | 70.4 ms |
| 16 000| 3.504 ms  | 281.6 ms |
| 32 000| 13.29 ms  | 1 126 ms |

A 320× increase in `M` (100 → 32 000) produces roughly a 1 200× increase
in time — consistent with `O(M log M)`-class growth (`log`-factor plus
sort/hash constants), nowhere near the ~102 000× (`320²`) that `Θ(M²)`
would require. This confirms Theorem 4/Corollary 1 (Section 3.3) and
directly contradicts the quadratic growth an earlier version of this
section claimed — see the erratum in Section 3.3 for the source of that
error.

### 3.5. Open Problems

1. **General notion of input size.** For arbitrary configurations with
   multiple interacting markers, the number of active cells may not
   capture the "amount of work." A more general definition is needed.

2. **Lower bound for tape inversion.** Proving an `Ω(N)` lower bound
   for tape inversion — that no Cellaria program can invert a tape of
   length `N` in fewer than `N` ticks — would establish a fundamental
   limit on computational speed in the model.

3. **Hypothesis: CF ≡ CA.** For every CA program, there exists a CF
   program (possibly with more rules) that computes the same function.
   If true, arbitration is optional for all programs, not just those
   with an empty conflict graph. This remains an open conjecture [2].

---

## 4. Expressiveness

### 4.1. Cellular Automata (k=3) → Cellaria

We define a constructive mapping `ca_to_cellaria` from any 1D cellular
automaton with window k=3 to a Cellaria rule set.

#### 4.1.1. Definition

Let a 1D CA be defined by a local transition function:

```
f: {0,1}³ → {0,1}
```

For example, the majority function:
```
f(a,b,c) = 1 if a + b + c ≥ 2, else 0
```

The CA computes the new state of position `i` as `f(data[i], data[i+1], data[i+2])`.

#### 4.1.2. Mapping `ca_to_cellaria(f)`

**Initial configuration:**
- Grid: width N+3, height 1.
- Cell (0,0): marker type 99 (age 0).
- Cells (1..=N, 0): `CellType(data[i])`.
- Cells (N+1, 0) and (N+2, 0): spare (type 0).

**Rules:** For each combination `(a,b,c) ∈ {0,1}³`:

```
new = f(a,b,c)

Rule {
    id: [99, a, b, c],
    shifts: [Right(1)],
    changes: [(-1, 0, new)],
    priority: 10,
    min_age: 0
}
```

**Invariant.** After each application, marker 99 shifts right by 1, and
the cell to its left receives value `f(a,b,c)`.

**Termination.** When the marker reaches the right edge (position > N),
no matches remain — simulation stops.

**Result.** Grid position `i − 1` contains `f(data[i], data[i+1],
data[i+2])`, for `i = 1..=N`. The `−1` offset is fixed and deterministic,
not an error: the marker occupies grid position `0` (outside the data
range `1..=N`), and each rule writes to its *pre-shift* position after
moving — so CA position `i`'s result lands one cell behind where
`data[i]` itself was read from.

#### 4.1.3. Example: Majority

Configuration: `configs/ca_majority.yaml`. `examples/proof_ca_majority.rs`
runs this mapping against the definition above (including the `−1`
offset) and asserts the two agree cell by cell.

### 4.2. Sorting → Cellaria

#### 4.2.1. Limitation

Cellaria **does not support swap** (exchanging two values) in a single
step. The marker overwrites `data[i]` on shift, and simultaneous writes
to two positions are impossible due to the sequential nature of
`apply_rule`.

Therefore, we implement **one-pass segregation**: shifting all 1s left
and all 0s right, rather than full bubble sort.

#### 4.2.2. Mapping `sorting_to_cellaria` (Segregation)

**Initial data:** `data: [u8; N]` — 1D array of 0s and 1s.

**Configuration:**
- Grid: width N+1, height 1.
- Cell (0,0): marker type 99.
- Cells (1..=N, 0): `CellType(data[i])`.

**Rules:**

```
// (99, 0, 1): swap — 1 moves left, 0 moves right
Rule { id: [99, 0, 1], shifts: [Right(1)], changes: [(0,0,1), (1,0,0)], priority: 10, min_age: 0 }

// (99, 1, 0): correct order — marker continues
Rule { id: [99, 1, 0], shifts: [Right(1)], changes: [], priority: 10, min_age: 0 }

// (99, 0, 0): both 0 — marker continues
Rule { id: [99, 0, 0], shifts: [Right(1)], changes: [], priority: 10, min_age: 0 }

// (99, 1, 1): both 1 — marker continues
Rule { id: [99, 1, 1], shifts: [Right(1)], changes: [], priority: 10, min_age: 0 }
```

**Limitation.** Correct only for binary data (0/1). Sorting arbitrary
numbers requires multi-pass with markers — left as future work.

### 4.3. Turing Machine → Cellaria

The function `translate_tm` (implemented in `src/tm_translator.rs`)
constructs a Cellaria rule set from a Turing machine specification
`M = (Q, Σ, δ, q₀, F)`.

#### 4.3.1. Encoding

- State `q ∈ Q` → cell type from `states[]`
- Symbol `a ∈ Σ` → cell type from `symbols[]`
- Blank `_` → type 0 (default)
- Head over symbol → pattern `[q_type, a_type]` (d=R) or `[a_type, q_type]` (d=L)

#### 4.3.2. Rules for `δ(q, a) = (q', a', d)`

**d=R:**
```
id: [q_type, a_type]
shifts: [{east, 1}]
changes: [(-1, 0, a'_type), (0, 0, q'_type)]
```

**d=L:**
```
id: [a_type, q_type]
shifts: [{west, 1}]
changes: [(0, 0, q'_type), (1, 0, a'_type)]
```

**q' ∈ F (halt):**
```
id: [q'_type, a'_type]
shifts: []
changes: [(0, 0, a'_type)]
```
A rule without shifts — the head is absorbed and simulation stops.

#### 4.3.3. Correctness Proof

**Lemma 2 (One TM step ≡ one Cellaria tick).** After applying the rule
for `δ(q, a) = (q', a', d)`, the grid state encodes the configuration
of `M` after one step.

*Proof.* Consider each transition type.

**Case d=R.** Pattern `[q_type, a_type]` matches at cells `(cx, cx+1)`.
Shift east by 1 moves the head to position `cx+1`. Changes:
- `(-1, 0, a'_type)` writes `a'_type` at position `cx` (old head position):
  `nx = cx + 1 + (-1) = cx`.
- `(0, 0, q'_type)` writes `q'_type` at position `cx+1` (new head position):
  `nx = cx + 1 + 0 = cx+1`.

Result: `[a'_type, q'_type]` at `(cx, cx+1)`, encoding the post-R-step
configuration: symbol `a'` under head `q'`.

**Case d=L.** Pattern `[a_type, q_type]` matches at `(cx, cx+1)`.
Shift west by 1 moves the head to position `cx-1`. Changes:
- `(0, 0, q'_type)` writes `q'_type` at position `cx-1`:
  `nx = cx + (-1) + 0 = cx-1`.
- `(1, 0, a'_type)` writes `a'_type` at position `cx`:
  `nx = cx + (-1) + 1 = cx`.

Result: `[q'_type, a'_type]` at `(cx-1, cx)`, encoding the post-L-step
configuration: head `q'` over symbol `a'`.

**Case q' ∈ F.** After an L-step: `[q'_type, a'_type]` at `(cx-1, cx)`.
The halt rule with id `[q'_type, a'_type]` (no shift) matches at `(cx-1, cx)`
and writes `a'_type` to position `cx-1`, absorbing the head.
After an R-step: `[q'_type, a'_type]` at `(cx, cx+1)`. The halt rule
matches at `(cx, cx+1)` and writes `a'_type` to position `cx`.

In both cases, after the halt rule fires, the pattern `[q'_type, a'_type]`
cannot match again (no head remains), so simulation stops. ∎

**Theorem 5 (Correctness of translate_tm).** For any Turing machine `M`,
the rule set `translate_tm(M)` simulates `M`: the sequence of `M`'s
configurations is isomorphic to the sequence of rule matches in Cellaria.

*Proof.* By induction on the number of steps.
- **Base:** Initial state — head `q₀` over the first tape symbol.
- **Step:** By Lemma 2, each TM step maps to one Cellaria tick.
- **Termination:** If `M` halts (`q' ∈ F`), the halt rule absorbs the
  head and simulation stops. If `M` does not halt, neither does the
  simulation. ∎

#### 4.3.4. Tests

Five tests in `src/tm_translator.rs` validate the implementation:

1. **test_tm_translator_bit_invert**: TM inverting bits (1 state,
   δ(q₀, 0)=(q₀, 1, R), δ(q₀, 1)=(q₀, 0, R)). Tape [1,0,1] → [0,1,0].
2. **test_tm_translator_bit_invert_4bit**: Same TM on tape [1,1,0,0] → [0,0,1,1].
3. **test_tm_translator_no_final**: TM without final states.
   Simulation stops when the head walks onto a blank tape.
4. **test_tm_translator_no_left_rule_match**: Manual check of L-rule structure.
5. **test_tm_translator_final_via_L**: TM with final state reached via L-rule.

All tests pass.

### 4.4. Cellaria → Cellular Automata (Partial Converse)

Section 4.1 embeds any classical CA into Cellaria. We now ask the
converse question: can a given Cellaria program be simulated by a
classical CA? A classical 1D CA (as in Section 4.1) applies a single
transition function `f`, synchronously, to every position, where `f`
depends only on the contents of a bounded local window and is invariant
under translating that window across the grid — no cell has privileged
status by absolute coordinate. This is the same homogeneity Cellaria
itself assumes (Axiom 1 [1]; see also [2], Section 5).

#### 4.4.1. Obstruction: Absolute Position in Arbitration

Cellaria's arbitration tie-break [2, Section 2] orders matches by
`(priority, age, rule_id, x, y, rule_idx)`, descending. The `x, y`
components compare the **absolute grid coordinates** of the match's
center. This is where the converse can fail: a classical CA's transition
function cannot depend on absolute position, but Cellaria's tie-break
sometimes must.

**Lemma 3 (`x, y`-dependence is a genuine obstruction).** There exists a
Cellaria rule set and grid configuration for which the arbitration
outcome cannot be reproduced by any translation-invariant local update
function.

*Proof (construction).* Let `R` consist of a single rule:
```
Rule { id: [5], pattern: [], shifts: [East(4)], priority: 10, min_age: 0 }
```
Place two cells of type 5 at positions `x = 0` and `x = 4`, both born at
generation 0 (equal age). Both cells match `R` (same `rule_id`, same
`rule_idx` — it is literally the same rule). The match at `x = 0` shifts
its cell to `x = 4` (writing there, clearing `x = 0`). The match at
`x = 4` shifts its cell to `x = 8` (writing there, clearing `x = 4`).
Their affected regions intersect at cell `x = 4`: one match writes there,
the other clears it as its own origin — a genuine conflict.

`priority`, `age`, and `rule_id` are identical for both matches (same
rule, same age). The tie-break falls through to `x`: the match with the
larger `x` value wins by the sort order in [2, Section 2.2]. Both
matches see an **identical local window** (a lone type-5 cell, empty
pattern) — a translation-invariant function applied to each would have
to make the *same* decision for both by definition of translation
invariance, either accepting both (incorrect — they conflict) or
rejecting both (differs from Cellaria's actual output, which accepts
exactly one). No translation-invariant local rule can pick out the
`x = 4` instance over the `x = 0` instance using only window content,
since the windows are indistinguishable. ∎

This is not a defect of the construction above — it is a structural
consequence of `x, y` appearing in the tie-break at all. Whenever two
matches with identical `(priority, age, rule_id)` conflict, the outcome
depends on which has the larger absolute coordinate, which is
information a homogeneous local rule cannot access.

#### 4.4.2. The Tie-Break-Local Subclass

**Definition 6 (Tie-break-local).** A rule set `R` is *tie-break-local*
if, for every pair of matches that can conflict under some reachable
grid state (i.e., every edge or self-loop in the conflict graph [2,
Section 3.4]), `(priority, age, rule_id)` are guaranteed to differ
whenever both matches actually fire simultaneously. Equivalently,
arbitration for `R` never needs to compare `x, y` to produce its result.

This is a checkable, syntactic-adjacent condition: it can be verified by
extending the conflict analyzer of [2] to additionally check, for every
conflicting pair, whether their priorities differ, or whether their
`min_age` values guarantee disjoint activation windows (as in [2,
Section 4.2.2]), or whether they are different rules entirely (distinct
`rule_id`). Every CF rule set (empty conflict graph, [2, Definition 4])
is trivially tie-break-local — there is nothing to arbitrate. The
counterexample of Lemma 3 is, by construction, *not* tie-break-local:
its one conflicting pair ties on all three fields.

**Theorem 6 (Simulation by classical CA).** Let `R` be a tie-break-local
Cellaria rule set with reach `K` [2, Definition 4]. There exists a
classical CA with neighborhood radius `K` (window
`2K+1` in 1D, or a `(2K+1)×(2K+1)` Moore neighborhood in 2D) and a
finite alphabet, whose synchronous update reproduces the sequence of
grid configurations produced by `R`, tick for tick.

*Proof sketch.* Fix a cell `d`. Every rule match that could possibly
write to `d` has its center within distance `K` of `d` ([2, Section 6.2,
Definition 4]) — the full window of radius `K` around `d` contains every
cell any such match could read or write. Define the CA's transition
function `f` on that window by: (a) enumerate every candidate match
consistent with the window's contents; (b) among conflicting candidates,
apply Cellaria's tie-break restricted to `(priority, age, rule_id)` —
well-defined and window-local by the tie-break-local assumption, since
`x, y` are never needed; (c) output the resulting value for `d` (or `d`'s
unchanged value, if no match wins). Age is folded into each cell's state
as a bounded counter, capped at the largest `min_age` appearing in `R`
(ages beyond the cap behave identically for every rule in a finite `R`,
so collapsing them into one bucket preserves the alphabet's finiteness).
`f` is applied synchronously to every cell, reproducing exactly what
Cellaria's detect → arbitrate → apply cycle would have produced at `d`,
for every `d` simultaneously. By induction on ticks, the two systems'
configuration sequences coincide. ∎

**Corollary 2 (Equivalence, restricted).** Classical 1D (or 2D) cellular
automata and tie-break-local Cellaria programs are equally expressive:
Section 4.1 embeds the former into the latter; Theorem 6 embeds the
latter into the former. Full, unconditional equivalence between
classical CA and Cellaria at large does not hold, by Lemma 3.

#### 4.4.3. Discussion

The obstruction is specific to arbitration between matches that are
otherwise indistinguishable by rule-level properties — it does not
depend on Cellaria's other features (shifts, `min_age`, unbounded grids),
all of which admit standard local-encoding techniques (Section 4.4.2's
proof sketch) and do not by themselves break translation invariance.
Most rule sets encountered in practice are tie-break-local, since
distinct behaviors are usually given distinct priorities or distinct
`id`s; Lemma 3's counterexample requires deliberately using the *same*
rule twice at positions that happen to collide. Whether every non-CF
rule set can be rewritten into a tie-break-local one without changing
its input-output behavior — trading the `x, y` dependency for extra
rules or priorities — is left open; if true, it would extend Corollary 2
to strictly more of Cellaria's expressiveness, though never to the
literal counterexample of Lemma 3 itself, which is inherent to any
tie-break that inspects absolute position.

### 4.5. Universal Self-Reflection

Cellaria's `RuleStore` protocol (encoding an `AddRule` operation as a
byte packet, decoded from an output boundary's channel) lets a running
program transmit and install a new rule in itself — closing a loop that
was part of the project's original design intent but had never been
connected end-to-end. Early demonstrations of this (in the project's
`examples/`, not formalized in a paper until now) each built a bespoke
set of carrier rules tailored to transmit one specific target rule `R`.
This section shows something stronger: a single, fixed machine, built
with no knowledge of which `R` will ever be transmitted, suffices to
transmit *any* rule expressible in the wire protocol — adding a new `R`
costs only data placement, never a new rule.

**Constraint inherited from the protocol.** The packet terminator is
always byte `0xFF` (255), and the parser scans for this value
unconditionally wherever it appears in the accumulated stream — so no
packet position other than the deliberate final terminator may hold
value 255. Every data byte (priority, id bytes, shift direction/steps,
change offsets/values) is therefore restricted to `0..254`, a constraint
of the existing protocol, not one introduced here.

**Theorem 7 (Universal relay).** There exists a fixed rule set `U`
(independent of `R`) such that: for any rule `R` whose `AddRule` packet
uses only data bytes in `0..254`, placing one carrier cell per data byte
— spaced with margin `≥ 2K` as in Definition 5, since adjacent carriers
all shifting the same direction at the same speed would otherwise have
overlapping affected regions and spuriously conflict with each other
under arbitration, exactly the moving-object limitation already
identified for the shuttle in `big_world.rs` — followed by the fixed
terminator, causes `U` to transmit exactly `R`'s packet, and `RuleStore`
to install exactly `R`, without any change to `U` itself.

*Construction.* `U` consists of 255 rules, one per carrier type
`1..=255`, all of the form "shift toward the output port; on overflow,
emit a byte." For types `1..=254`, the emitted byte is the carrier's own
value (`OverflowAction::Write(0)`, i.e. "carry self") — a direct
bijection with data bytes `1..254`. The remaining data byte, `0`, cannot
be produced this way: a cell holding value `0` is the grid's own
default/empty marker (`Cell::is_default`), so no carrier can legitimately
*be* `0` while remaining a tracked, moving cell — and `Write`'s existing
convention (`0` means "carry self") makes a literal `0` output otherwise
inexpressible through `OverflowAction` at all. Type `255`'s rule instead
uses `OverflowAction::WriteLiteral(0)` — a variant added in this session
specifically to close this gap, always emitting its literal argument
regardless of value, unlike `Write`. Together, all 255 rules cover every
possible data byte (`0..254`) with a construction that does not depend
on `R`.

**Remark (the terminator is not data).** Byte 255 is never chosen by
`R`'s author — it is always the same fixed value at the same fixed
(final) position, true for every conceivable `R`. It is protocol framing,
not part of what `R` specifies, so it is appended once, by whoever
operates `U`, exactly like a modem appending a fixed stop bit — `U`'s 255
rules need not represent it.

**Remark (self-consuming slots are expected, not a defect).** `R`'s own
target id may coincide with one of `U`'s 255 carrier types (there is no
256th type to spare it, since type `0` is structurally unavailable as a
tracked cell). This is not a flaw: once `R` installs, `rule_index[R.id]`
is *replaced* by `R`'s actual behavior, exactly as intended — the carrier
for that type has already completed its relay and left the grid before
installation occurs (installation only happens once the terminator, the
*last* transmitted byte, arrives).

**Empirical validation.** `examples/proof_universal_self_reflection.rs`
builds `U` once, then picks `R` deliberately unlike anything transmitted
in earlier, bespoke demonstrations: a shift `Up` (the one direction
encoded as byte `0`) together with a same-cell change (`dx = dy = 0`) —
striking both places the protocol's zero-byte previously made
untransmittable. `U` correctly transmits and installs `R`; the installed
rule matches the intended one exactly (priority, shift, and change all
verified by direct comparison, not just "a rule appeared").

**Scope.** Theorem 7 covers exactly the expressiveness of the current
`RuleStore` wire protocol — four fixed shift directions, `Literal`-only
changes (`ChangeValue::Ref` is explicitly not wire-encodable, per the
protocol's own implementation), id length bounded by `i8::MAX` — the
same limits every rule transmitted through this protocol already had.
It is not a claim about encoding arbitrary future protocol extensions,
only about the protocol as it exists.

### 4.6. Self-Attestation

Section 4.5's construction transmits a value the automaton *decided* —
a rule to install. This section shows the same channel can carry a value
the automaton *computed about an extended region of its own state*: not
one cell (trivial) and not a value chosen by external code (Section
4.5's carriers each carry a literal fixed at construction time), but a
genuine function of a whole sequence of cells, useful for an outside
observer to check the automaton's report against the actual grid.

#### 4.6.1. Construction

Two tracks, one row apart: row `y = 0` holds a data sequence
`d_0, ..., d_{L-1}` with each `dᵢ ∈ {1, ..., 6}`, never written by any
rule in this construction. Row `y = 1` holds a scanning marker, initially
at `x = 0` with type `ACC_BASE` (accumulator `0`).

For each accumulator value `a ∈ {0, ..., 6}` and each data value
`d ∈ {1, ..., 6}`, a rule:
```
id: [ACC_BASE + a]
pattern: [(0, 0, ACC_BASE + a), (0, -1, d)]
shifts: [East(1)]
changes: [(0, 0, Literal(ACC_BASE + (a + d) mod 7))]
```
reads the data cell directly "above" the marker (pattern reads use the
grid state *before* any writes this tick, so this is a faithful read of
`dᵢ`), moves the marker one cell right, and updates its accumulator —
all without writing to row `0`. A second rule per `a`, with no pattern
requirement on row `0` and `min_age` large enough to exceed any
legitimate scan step, converts a marker that has run out of data (no
rule above matched, so it simply stopped — the same "no match, halt"
behavior used throughout Section 2) into a literal, transmittable value
`FINAL_BASE + a`, which then behaves exactly like a Section 4.5 carrier:
shift toward an output boundary, `OverflowAction::Write(0)` (carry own
value).

#### 4.6.2. Correctness

**Lemma 4 (One data cell per tick).** If the marker is at position `x`
with accumulator `a` at the start of a tick, and `dₓ` exists, then at the
end of the tick the marker is at `x + 1` with accumulator `(a + dₓ) mod
7`, and row `0` is unchanged.

*Proof.* The pattern `(0, -1, d)` matches the unique `d = dₓ` present at
`(x, -1)` relative to the marker (there is exactly one, since the data
row holds one value per position); the shift moves the marker to
`x + 1`; the change, at offset `(0, 0)` relative to the *post-shift*
position (the convention already established and used by the Turing
machine translation, Section 4.3.3), overwrites the marker's own new
cell with the updated accumulator type. Nothing in `changes` targets row
`0`. ∎

**Theorem 8 (Correctness of the attestation scan).** After processing
data `d_0, ..., d_{L-1}`, the automaton transmits
`FINAL_BASE + (Σᵢ dᵢ) mod 7`.

*Proof.* By induction on `i`, using Lemma 4 for the step: after
processing `i` cells the marker holds accumulator `(Σ_{j<i} dⱼ) mod 7` at
position `i`, with row `0` still exactly as given. At `i = L`, position
`L` has no data cell (default value, outside `{1,...,6}`), so no scanning
rule matches — the marker halts, holding `(Σᵢ dᵢ) mod 7`, exactly as in
the termination-by-no-match behavior of Section 2. After `min_age` ticks
of quiet, the finalize rule fires and the value is transmitted as in
Section 4.5. ∎

**Empirical validation.** `examples/proof_self_attestation.rs` runs this
construction on `[3, 1, 4, 1, 5, 2]`, transmitting `2`
(`3+1+4+1+5+2 = 16 ≡ 2 (mod 7)`), then on the same sequence with one
cell tampered (`d_3`: `1 → 6`), transmitting `0` — a different value,
demonstrating that the transmitted attestation genuinely depends on the
full data sequence, not a constant or a single cell.

**Scope — not a cryptographic primitive.** A sum mod 7 is trivially
collision-prone (e.g. `[1, 6]` and `[2, 5]` both attest `0`) and
trivially forgeable by anyone who can write to row `0` before the scan.
This section demonstrates the *mechanism* — computing and transmitting a
function of extended state through the existing self-modification
channel — not a cryptographically meaningful checksum. A construction
with cryptographic properties (collision resistance, forgery resistance)
would need a much larger accumulator space and a nonlinear mixing step
per cell, at proportionally more rules (linear in alphabet size times
accumulator range, the same trade-off already seen in Section 4.5); nothing
here rules that out, but it is not built.

---

## 5. Conclusion

We have presented three contributions about the limits of the Cellaria
model:

1. **Termination** (Section 2): sufficient conditions via three classes
   of potential functions (geometric, counting, energetic). A lightweight
   runtime check (`Engine::detect_termination`) classifies the *current*
   state as active or stable by peeking one tick ahead — a narrower
   contribution than a trend-based liveness monitor. The counting
   potential is sufficient but not necessary for termination.

2. **Complexity** (Section 3): definition of CF (Conflict-Free) and CA
   (Conflict-Aware) complexity classes. Proof of a tight `Θ(M log M)`
   bound for CA arbitration on bounded-reach rule sets — the shipped
   sort-and-hash algorithm is asymptotically optimal, not the `O(M²)`
   originally (and incorrectly) claimed. Empirical confirmation on four
   benchmark types.

3. **Expressiveness** (Section 4): constructive mappings proving that
   Cellaria can simulate:
   - Any 1D cellular automaton (k=3) in `O(N)` ticks.
   - One-pass binary segregation in `O(N)` ticks.
   - Any Turing machine with one tick per TM step.
   - The converse (Cellaria → classical CA) holds only for the
     tie-break-local subclass, not unconditionally (Section 4.4) — the
     one place this paper's expressiveness results have a genuine,
     proven boundary rather than a further construction.
   - Any rule expressible in the `RuleStore` self-modification protocol,
     via a single fixed relay machine independent of which rule is
     transmitted (Section 4.5).
   - A checksum over an extended region of its own state, computed and
     transmitted through the same channel, verifiably sensitive to a
     single tampered cell (Section 4.6).

All results are validated experimentally. The Cellaria model is at least
as expressive as cellular automata and Turing machines, matches classical
CA exactly on the tie-break-local subclass, can transmit and install any
of its own expressible rules through a single universal mechanism, can
attest to an extended region of its own state through that same
mechanism, and has predictable termination and complexity characteristics.

---

## 6. Related Work

**Termination analysis in rewriting.** Termination of term rewriting
systems is typically proved via reduction orders [4]. Our potential
function approach follows the same principle, adapted to spatial
rule-based computation.

**Parallel rewriting.** Parallel application of non-overlapping matches
is well-known in graph rewriting [5] and cellular automata [6]. Our
complexity classes CF and CA formalize the distinction between
conflict-free and conflict-aware programs, with a tight `Θ(M log M)`
bound for CA arbitration on bounded-reach rule sets.

**Computational complexity of CA.** Cellular automata are known to be
Turing-complete [7]. Our mapping from CA to Cellaria establishes that
Cellaria inherits this expressiveness, while providing additional
features (conflict analysis, composition) not present in standard CA.

---

## References

1. Cellaria: A Local Reduction Model of Computation. (2026). Technical
   Report.

2. Cellaria: Arbitration — Determinism, Static Conflict Analysis, and
   Composition. (2026). Companion Paper.

3. Cellaria: Five Axioms and the Tick Cycle. (2026). Companion Paper.

4. Dershowitz, N. (1987). Termination of rewriting. *Journal of Symbolic
   Computation*, 3(1-2), 69–115.

5. Campbell, G., & Plump, D. (2013). Parallel graph transformation.
   In *Graph Transformation* (pp. 154–169). Springer.

6. Toffoli, T., & Margolus, N. (1987). *Cellular Automata Machines:
   A New Environment for Modeling*. MIT Press.

7. Wolfram, S. (2002). *A New Kind of Science*. Wolfram Media.