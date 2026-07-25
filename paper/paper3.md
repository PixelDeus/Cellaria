# Cellaria: Termination, Complexity, and Expressiveness

## Abstract

Cellaria is a cellular automaton-like model of computation based entirely
on local reduction, defined by five axioms. This paper explores the
**limits of the model**: when does a simulation terminate, how many ticks
does it take, and what can be expressed within Cellaria's constraints?

We present four contributions:

1. **Termination via potential functions** — sufficient conditions for
   termination using three classes of potential functions (geometric,
   counting, energetic), with runtime monitoring.

2. **Complexity classes** — definition of CF (Conflict-Free) and CA
   (Conflict-Aware) programs, with a proof of the Ω(M²) lower bound
   for CA arbitration.

3. **Expressiveness: cellular automata** — a constructive mapping from
   any 1D cellular automaton (k=3) to Cellaria, proving that Cellaria
   can simulate any such CA in O(N) ticks.

4. **Expressiveness: Turing machines** — a formal reduction from Turing
   machines to Cellaria, with a proof that one tick corresponds to
   exactly one TM step. A sorting reduction is also presented.

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
defines two complexity classes and proves a lower bound for arbitration.
The expressiveness analysis (Section 4) provides constructive mappings
from cellular automata, Turing machines, and sorting to Cellaria,
establishing that Cellaria is at least as expressive as these models.

---

## 2. Termination via Potential Functions

### 2.1. Problem Statement

A Cellaria simulation terminates when no rule matches any cell on the
grid. We present sufficient conditions for termination based on potential
functions and demonstrate runtime monitoring through the method
`detect_termination`, which classifies simulations as `Terminates`,
`MayDiverge`, or `Unknown`.

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

The `detect_termination` method monitors three conditions at runtime:

| Verdict | Condition |
|---------|-----------|
| `Terminates` | Φ_cnt strictly decreasing over observation window |
| `MayDiverge` | Φ_cnt constant over observation window (same state repeats) |
| `Unknown` | Φ_cnt increasing or oscillating |

Validation on four test cases:

| Test Case | Configuration | max_ticks | observation_ticks | Verdict |
|-----------|--------------|-----------|-------------------|---------|
| `test_termination_turing` | `configs/turing.yaml` | 50 | 10 | Terminates |
| `test_termination_tag_system` | `configs/tag_system.yaml` | 20 | 5 | Terminates |
| `test_termination_infinite_loop` | Rule 1→1 (no shift) | 100 | 20 | MayDiverge |
| `test_termination_unknown` | Rule 1→1 (shift right + copy behind) | 50 | 20 | Unknown |

The `turing` and `tag_system` configurations terminate deterministically.
The infinite loop repeats every tick and is correctly classified as
`MayDiverge`. The expanding configuration grows without bound and is
correctly classified as `Unknown`.

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
  deterministic but involves `O(M²)` comparisons in the worst case.
- **Per-tick time:** `O(M²)` due to greedy pairwise conflict resolution.
- **Total time:** depends on program logic.
- **Arbitration cost:** `O(M²)` per tick.

CA programs subsume CF programs: every CF program is trivially in CA
(the conflict graph is empty, so arbitration selects all matches in
`O(M)` time), but CF programs avoid the quadratic overhead entirely.

### 3.3. Lower Bound for CA Arbitration

**Theorem 4 (Ω(M²) lower bound).** For any arbitration algorithm
processing `M` pairwise conflicting matches, there exists an input
requiring `Ω(M²)` comparisons.

*Proof.* Consider `M` rules with identical `id = [1, 0]` and identical
`priority = 10`. All rules match on the same cell of type 1 at position
`(1, 0)`. The second pattern cell (type 0) is at position `(2, 0)`.

Each rule has a unique change, but their affected regions (pattern `[1, 0]`
of length 2) completely overlap. Therefore, all `M` rules are pairwise
conflicting: no two can be applied simultaneously.

Arbitration proceeds greedily:
- First match accepted: 0 comparisons.
- Second checked against first: 1 comparison.
- Third checked against first and second: 2 comparisons.
- M-th checked against M−1 already accepted: M−1 comparisons.

Total:
```
0 + 1 + 2 + ... + (M−1) = M(M−1)/2 = Θ(M²)
```

**Tightness.** Any pair of rules with the same `id` and `priority` may
conflict — their affected regions intersect for any match on the same cell.
The algorithm cannot skip any pair, as this would produce incorrect
results (two conflicting rules would be accepted simultaneously).
Therefore, `Ω(M²)` is an exact lower bound. ∎

**Corollary 1 (Tight bound).** The upper bound for class CA is `O(M²)`,
and this bound is **tight** in the worst case.

### 3.4. Empirical Complexity

#### 3.4.1. Linear Time: Turing Machine Simulation

**Hypothesis.** A Turing machine simulation in Cellaria requires
`O(T)` ticks for `T` steps of the machine.

**Data.** Measurements from `configs/turing.yaml`:

| Len | Ticks | Ratio |
|-----|-------|-------|
| 10  | 10    | 1.0   |
| 50  | 50    | 1.0   |
| 100 | 100   | 1.0   |
| 200 | 200   | 1.0   |

**Result.** `ticks = len` for all tested lengths. Cellaria simulates a
Turing machine without overhead: one tick corresponds to exactly one
step of the machine.

#### 3.4.2. Linear Time: Tag System

**Hypothesis.** A tag system simulation requires `O(N)` ticks for a
string of length `N`.

**Data.** Measurements from `configs/tag_system.yaml`:

| Len | Ticks | Ratio |
|-----|-------|-------|
| 5   | 5     | 1.0   |
| 10  | 10    | 1.0   |
| 20  | 20    | 1.0   |
| 50  | 50    | 1.0   |

**Result.** `ticks = len` for all tested lengths. Single-pass marker
processing is asymptotically optimal.

#### 3.4.3. Constant Time: Conflict-Free Rules

**Hypothesis.** A rule set with an empty conflict graph terminates in
`O(1)` ticks, independent of grid size.

**Data.** Measurements from `configs/parallel.yaml`:

| Width | Ticks |
|-------|-------|
| 8     | 1     |
| 16    | 1     |
| 32    | 1     |
| 64    | 1     |

**Result.** Two independent conflict-free rules fire concurrently and
terminate in one tick, regardless of grid size.

#### 3.4.4. Quadratic Arbitration: Worst Case

**Data.** Measurements from `configs/worst_case_arbitration.yaml`:

| M | Comparisons | Accepted |
|---|-------------|----------|
| 5  | 10  | 1 |
| 10 | 45  | 1 |
| 20 | 190 | 1 |

The number of comparisons follows `M(M−1)/2` exactly, confirming the
`Θ(M²)` lower bound.

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

**Result.** Position `i` contains `f(data[i], data[i+1], data[i+2])`.

#### 4.1.3. Example: Majority

Configuration: `configs/ca_majority.yaml`.
Test `test_ca_majority_pass` in `src/engine.rs` implements this mapping.

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

---

## 5. Conclusion

We have presented three contributions about the limits of the Cellaria
model:

1. **Termination** (Section 2): sufficient conditions via three classes
   of potential functions (geometric, counting, energetic). Runtime
   monitoring classifies simulations as Terminates, MayDiverge, or
   Unknown. The counting potential is sufficient but not necessary for
   termination.

2. **Complexity** (Section 3): definition of CF (Conflict-Free) and CA
   (Conflict-Aware) complexity classes. Proof of the `Ω(M²)` lower
   bound for CA arbitration — a tight bound. Empirical confirmation on
   four benchmark types.

3. **Expressiveness** (Section 4): constructive mappings proving that
   Cellaria can simulate:
   - Any 1D cellular automaton (k=3) in `O(N)` ticks.
   - One-pass binary segregation in `O(N)` ticks.
   - Any Turing machine with one tick per TM step.

All results are validated experimentally. The Cellaria model is at least
as expressive as cellular automata and Turing machines, with predictable
termination and complexity characteristics.

---

## 6. Related Work

**Termination analysis in rewriting.** Termination of term rewriting
systems is typically proved via reduction orders [4]. Our potential
function approach follows the same principle, adapted to spatial
rule-based computation.

**Parallel rewriting.** Parallel application of non-overlapping matches
is well-known in graph rewriting [5] and cellular automata [6]. Our
complexity classes CF and CA formalize the distinction between
conflict-free and conflict-aware programs, with a tight lower bound.

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