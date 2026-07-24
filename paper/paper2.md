# Cellaria: Determinism, Static Conflict Analysis, and Termination

## Abstract

Cellaria is a cellular automaton-like model of computation based entirely
on local reduction, defined by five axioms. The tick cycle consists of
detection (pattern matching), arbitration (conflict resolution), and
application (modification). We present three contributions about the
model:

1. **Determinism of arbitration** — the sort key `(priority, age, center)`
   defines a total order; greedy selection on a totally ordered set is
   deterministic, guaranteeing identical results across non-deterministic
   iteration orders and different implementations.

2. **Static conflict analysis** — a conservative algorithm constructs a
   conflict graph for a given rule set. If the graph is empty, arbitration
   can be skipped and all matches applied simultaneously, enabling parallel
   execution on GPU/FPGA architectures without semantic changes.

3. **Termination via potential functions** — sufficient conditions for
   termination of a Cellaria simulation. Three classes of potential
   functions (geometric, counting, energetic) are defined, and a general
   scheme for constructing a decreasing measure is presented. An
   implementation monitors these conditions at runtime.

---

## 1. Introduction

Cellaria [1] is a computational model based on local reduction on a
homogeneous grid. Computation proceeds in discrete ticks, each
consisting of three phases:

1. **Detection** — pattern matching finds all rule matches on the grid.
2. **Arbitration** — greedy conflict resolution selects a non-overlapping
   subset of matches.
3. **Application** — the selected matches are applied to the grid.

The first contribution of this paper (Section 2) is a formal proof of
arbitration determinism: the sort key `(priority, age, center)` defines
a total order, guaranteeing identical results across non-deterministic
iteration orders. This is non-trivial because `ChunkStorage` (the
infinite grid implementation) uses a `HashMap` for chunk storage, and
`active_cells()` iterates in non-deterministic order between runs.

The second contribution (Section 3) addresses the sequential bottleneck
of greedy arbitration. For conflict-free rule sets, arbitration can be
skipped entirely. A static conflict analyzer constructs a conflict graph:
if the graph is empty, all matches are pairwise non-overlapping and can
be applied simultaneously. This provides a direct path to parallel
execution on GPU or FPGA architectures.

The third contribution (Section 4) provides sufficient conditions for
termination. A potential function that strictly decreases on every tick
guarantees termination. Three classes of potential functions (geometric,
counting, energetic) are defined and applied to the Turing machine and
tag system simulations from previous work.

---

## 2. Determinism of Arbitration

### 2.1. Problem Statement

The detection phase (`detect_matches`) iterates over active cells via
`active_cells()`. For `VecStorage` (finite grid), this iteration is
deterministic (left-to-right, top-to-bottom). For `ChunkStorage`
(infinite grid), the implementation uses a `HashMap` for chunk storage,
and `active_cells()` iterates in non-deterministic order between runs.
Despite this, the arbitration algorithm must produce the same set of
accepted matches for any input order. We prove that the sort key
`(priority, age, center_x, center_y)` defines a total order, guaranteeing
deterministic greedy selection regardless of input order.

### 2.2. The Arbitration Algorithm

The arbitration algorithm (`arbitrate`) works as follows:

1. **Sort** all matches by `(priority, age, center_x, center_y)` in
   descending order (highest priority first; for equal priority, highest
   age first; for equal age, lexicographic order of center coordinates).
2. **Greedy selection:** iterate through the sorted list; accept a match
   if its center cell and all its pattern cells are not already used by
   a previously accepted match.

### 2.3. Uniqueness of the Center Cell

**Lemma 1 (Unique center).** In a single tick, a given grid cell can be
the center of at most one `RuleMatch`.

*Proof.* A `RuleMatch` is created when the center cell's type triggers a
rule. The center cell has a single type value. Rules are indexed by the
first cell type in their `id` field. A cell with type `t` can only
trigger rules that have `t` as their first `id` element. For a given
cell, `detect_matches` checks all rules with matching first type, but
all resulting matches share the same center cell. During arbitration,
if a match with this center is accepted, all subsequent matches with the
same center are rejected (the center is in `used_cells`). Thus, at most
one match per center survives arbitration. ∎

### 2.4. Total Order

**Lemma 2 (Total order).** The key `(priority, age, center_x, center_y)`
defines a total order on the set of all `RuleMatch` objects in a single
tick.

*Proof.* For any two distinct matches `m₁` and `m₂`:

- If `priority(m₁) ≠ priority(m₂)`, the higher priority wins.
- If `priority(m₁) = priority(m₂)`, compare `age`. The age is the age of
  the center cell. If ages differ, the higher age wins.
- If `priority` and `age` are equal, compare `center_x`, then `center_y`.
  By Lemma 1, two distinct matches cannot have the same center. Therefore,
  the coordinate comparison is well-defined and distinguishes the two
  matches.

All components are totally ordered, and the lexicographic combination
(`priority` descending, `age` descending, `center_x` ascending, `center_y`
ascending) is a total order. ∎

### 2.5. Theorem 1: Determinism of Arbitration

**Theorem 1 (Determinism).** For any grid state and any rule set, the
`arbitrate` function returns the same set of accepted matches regardless
of the order in which matches are passed to it.

*Proof.* Let `M` be the set of all matches detected in a tick. The
arbitration algorithm:

1. Sorts `M` by the total order key `(priority, age, center_x, center_y)`
   (Lemma 2). Sorting is deterministic: for any input order, the sorted
   order is the same.
2. Iterates through the sorted list greedily. The greedy selection is
   deterministic: a match is accepted if and only if its cells are not
   already used by a previously accepted match. Since the sorted order
   is fixed, the set of accepted matches is uniquely determined by the
   set of all matches `M`, which in turn is uniquely determined by the
   grid state and rule set.

Therefore, the output of `arbitrate` is independent of the input order.
∎

### 2.6. Corollary: Portability

**Corollary 1 (Portability).** Any Cellaria configuration can be executed
on different implementations (Rust, GPU, FPGA) and produce the same result,
provided the sort order `(priority, age, center_x, center_y)` is
consistently implemented.

This is a strong portability guarantee: the model's semantics are
independent of the underlying execution platform.

### 2.7. Practical Verification

The theorem is validated experimentally on all 10 configurations from
`configs/`. Repeated runs with identical parameters produce identical
logs. The test suite (`#[cfg(test)]` in `src/engine.rs`) includes
specific tests for arbitration determinism.

---

## 3. Static Conflict Analysis

### 3.1. Motivation

Greedy arbitration is the sequential bottleneck of the Cellaria tick
cycle. In the worst case, resolving conflicts among overlapping matches
requires `O(M²)` comparisons, where `M` is the number of matches in a
single tick. For rule sets provably free of conflicts, arbitration can
be skipped entirely: all matches are accepted and applied simultaneously.

### 3.2. Formal Definitions

**Definition 1 (Conflict).** Two rules `Rᵢ` and `Rⱼ` conflict if there
exists a pair of positions on the grid such that:

1. The patterns of both rules match simultaneously at those positions.
2. Their affected regions intersect.

**Definition 2 (Affected region).** The affected region of a rule `R`
applied with center cell `(cₓ, cᵧ)` is the set of all grid cells read
or written during the detect → apply phases:

```
Affected(R, cₓ, cᵧ) = PatternCells(R, cₓ, cᵧ)
                     ∪ { (cₓ, cᵧ) }            // shift origin (cleared)
                     ∪ { (cₓ+dx, cᵧ+dy) }      // shift destination
                     ∪ ChangeCells(R, cₓ, cᵧ)  // post-shift changes
```

### 3.3. Conflict Elimination Lemmas

**Lemma 3 (Type incompatibility).** If two rules `Rᵢ` and `Rⱼ` check
overlapping cells for a given offset, and the required types in the
overlapping cells are incompatible (no cell can simultaneously satisfy
both type requirements), then the rules do not conflict for that offset.

*Proof.* A conflict requires both rules to match simultaneously. If the
overlapping cells require incompatible types, no grid state can satisfy
both pattern requirements simultaneously. Hence, the rules cannot match
simultaneously, and no conflict is possible. ∎

**Note on `min_age`.** The `min_age` field in a Cellaria rule specifies
a *lower bound* on the age of the center cell (`age ≥ min_age`), not an
exact value. Therefore, two rules with different `min_age` values can
both activate on the same cell in the same tick (e.g., `min_age = 5`
and `min_age = 10` both fire when `age ≥ 10`). The static conflict
analyzer **cannot** use `min_age` differences to exclude conflicts.
This is a deliberate over-approximation: at the cost of potential false
positives in the conflict graph (reporting conflicts where none exist
at runtime), we guarantee soundness for all reachable grid states.

### 3.4. Completeness of the Conflict Graph

**Theorem 6 (Completeness).** Let `Rᵢ` and `Rⱼ` be two rules. If there
is no edge `(i, j)` in the conflict graph, then `Rᵢ` and `Rⱼ` cannot
conflict under any grid state.

*Proof.* By Algorithm 1, the absence of an edge `(i, j)` means that for
every mutual offset `(dx, dy)` of the two patterns, at least one of the
following holds:

1. **Empty intersection.** The patterns do not overlap for this offset,
   so their affected regions are disjoint by construction.
2. **Type incompatibility.** The overlapping cells require incompatible
   types. By Lemma 3, no grid state can satisfy both patterns simultaneously
   for this offset.
3. **Non-intersecting affected regions.** The patterns intersect (some
   overlapping cells are type-compatible), but the affected regions of the
   two rules (pattern cells, shift origin, shift destination, change cells)
   are disjoint for this offset.

We prove, by induction on the number of conditions eliminated, that if
no edge exists, no conflict is possible:

*Base case.* If for all offsets condition 1 holds (patterns never overlap),
then `Rᵢ` and `Rⱼ` can never match in positions close enough for their
effects to interfere. No conflict is possible.

*Inductive step.* For a given offset `(dx, dy)` where the patterns do
overlap, suppose condition 2 holds (type incompatibility). Then the
patterns cannot both match simultaneously at this offset (Lemma 3).
If condition 3 holds instead, then even if both patterns match
simultaneously, the resulting affected regions do not intersect, so
no cell is read or written by both rules.

If for all offsets at least one of the three conditions holds, then no
offset admits a simultaneous match with intersecting affected regions.
By Definition 1, `Rᵢ` and `Rⱼ` do not conflict. By Lemma 4, the graph
construction is deterministic, so the absence of `(i, j)` is definitive.

Therefore, the conflict graph is **complete**: it has no false negatives.
If the graph reports no edge between two rules, they are guaranteed
conflict-free for all grid states. ∎

**Generalized criterion.** Rules `Rᵢ` and `Rⱼ` are conflict-free if for
all possible mutual offsets of their patterns:

- Either the types in the intersecting cells are incompatible (Lemma 3),
- Or the resulting affected regions do not intersect (Definition 2).

### 3.5. The Conflict Graph (continued)

**Definition 3 (Conflict graph).** Given a rule set `R = {R₁, ..., Rₙ}`,
the conflict graph `G = (V, E)` is an undirected graph where:

- `V = {1, ..., n}` is the set of rule indices (vertices).
- `(i, j) ∈ E` if and only if `Rᵢ` and `Rⱼ` conflict per Definition 1.
The graph may contain self-loops: `(i, i) ∈ E` if `Rᵢ` conflicts
with itself at a non-zero offset.

**Algorithm 1 (Build conflict graph).**

```
Input:  rules — a slice of Rule references &[Rule]
Output: ConflictGraph with vertices and edges

1.  Let n = len(rules)
2.  Let edges = empty list
3.  For each pair (i, j) where 0 ≤ i < j < n:
4.      Let K = max(pattern_size(rules[i]), pattern_size(rules[j]))
5.      For each offset (dx, dy) in range -K..K × -K..K:
6.          Let intersection = overlapping_cells(rules[i], 0, 0,
7.                                               rules[j], dx, dy)
8.          If intersection is empty:
9.              Continue to next offset
10.         If types_incompatible(rules[i], rules[j], intersection):
11.             Continue to next offset
12.         Let region_i = affected_region(rules[i], 0, 0)
13.         Let region_j = affected_region(rules[j], dx, dy)
14.         If regions_intersect(region_i, region_j):
15.             Add edge (i, j) to edges
16.             Break to next pair
17. // Self-conflict check: rule against itself at non-zero offsets
18. For each rule i where 0 ≤ i < n:
19.     Let K = pattern_size(rules[i])
20.     For each offset (dx, dy) in range -K..K × -K..K
21.         where (dx, dy) ≠ (0, 0):
22.         Let intersection = overlapping_cells(rules[i], 0, 0,
23.                                              rules[i], dx, dy)
24.         If intersection is empty:
25.             Continue to next offset
26.         If types_incompatible(rules[i], rules[i], intersection):
27.             Continue to next offset
28.         Let region_1 = affected_region(rules[i], 0, 0)
29.         Let region_2 = affected_region(rules[i], dx, dy)
30.         If regions_intersect(region_1, region_2):
31.             Add self-loop edge (i, i) to edges
32.             Break to next rule
33. Return ConflictGraph { rule_count: n, edges }
```

The offset range `-K..K` in both dimensions is sufficient because two
patterns of size at most `K` can only intersect if their center positions
are within `K` cells of each other in any direction. Offsets beyond this
range guarantee that the affected regions are disjoint.

**Complexity.** The algorithm checks `O(N²)` rule pairs and `N`
self-conflict checks. For each pair, it examines `O(K²)` offsets,
where `K` is the maximum pattern size. Total worst-case complexity:
`O(N² · K³)`. In practice, `K ≤ 10` and the dominating term is `O(N²)`.

### 3.6. Theorem 2: Conflict-Free Arbitration Bypass

**Theorem 2 (Arbitration bypass).** Let `G = (V, E)` be the conflict
graph for rule set `R`. If `G` is empty (`E = ∅`) and no rule has a
self-loop, then for any grid state, the greedy arbitration phase can be
skipped without changing the final configuration. All detected matches
are pairwise non-overlapping and can be applied simultaneously in any
order.

*Proof.* By construction of the conflict graph:

1. If `E = ∅`, then for every pair of distinct rules `(i, j)`, no offset
   exists where both rules match simultaneously with intersecting
   affected regions (Definition 1).

2. Suppose, for contradiction, that two matches `m₁` and `m₂` from rules
   `Rᵢ` and `Rⱼ` overlap in the grid. Then their affected regions
   intersect. By Definition 1, this would require an edge `(i, j) ∈ E`.
   But `E = ∅`, a contradiction.

3. Therefore, no two matches from distinct rules can overlap. For
   matches from the same rule: a self-loop `(i, i)` in the conflict
   graph indicates that the rule conflicts with itself at some non-zero
   offset. Since `G` has no self-loops by assumption, Lemma 3 (type
   incompatibility) or Definition 2 (non-intersecting affected regions)
   guarantees that the affected regions of any two instances of the
   same rule at distinct centers are disjoint. Combined with the
   non-self-overlapping property of the rule's pattern, this ensures
   same-rule matches are pairwise disjoint.

4. Since no matches overlap, the greedy arbitration algorithm (which
   resolves overlapping matches) has nothing to resolve. All matches are
   accepted. Applying them in any order — or simultaneously — produces
   the same final grid state.

5. Hence, arbitration can be skipped. The detect → apply phases execute
   in one pass with no conflict resolution. ∎

**Corollary 2 (Parallel execution).** For a rule set with an empty
conflict graph and no self-loops, all matches in a tick can be applied
simultaneously in a single parallel pass over the grid. This enables
direct implementation on GPU (each thread handles one cell or one match)
or FPGA (each rule is a hardware pipeline stage) without sequential
arbitration.

### 3.7. Structural Properties

**Lemma 4 (Idempotence).** The conflict graph is idempotent under
repeated construction: for a fixed rule set, Algorithm 1 always produces
the same graph.

*Proof.* The algorithm is deterministic: it iterates over the same pairs,
offsets, and checks in the same order. No state is shared between
iterations. ∎

**Lemma 5 (Monotonicity).** Adding a rule to a rule set can only add
edges to the conflict graph, never remove them.

*Proof.* A new rule `Rₙ₊₁` may conflict with existing rules, adding edges
incident to vertex `n+1`. Existing edges are never removed because the
rules they depend on are unchanged. ∎

---

## 4. Termination via Potential Functions

### 4.1. Problem Statement

A Cellaria simulation terminates when no rule matches any cell on the
grid. We present sufficient conditions for termination based on potential
functions and demonstrate runtime monitoring through the method
`detect_termination`, which classifies simulations as `Terminates`,
`MayDiverge`, or `Unknown`.

The standard approach is to find a **potential function** (also called a
ranking function or measure) `Φ: Configuration → ℕ` that strictly
decreases on every tick. If `Φ` is bounded below (e.g., `Φ ≥ 0`), then
the simulation must terminate after at most `Φ(initial)` ticks.

### 4.2. Theorem 3: Sufficient Condition for Termination

**Theorem 3 (Potential function termination).** Let `C` be the set of
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

### 4.3. Classes of Potential Functions

We define three classes of potential functions for Cellaria.

#### 4.3.1. Geometric Potential

**Definition 4 (Geometric potential).** The geometric potential of a
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
Section 4.4).

#### 4.3.2. Counting Potential

**Definition 5 (Counting potential).** The counting potential of a
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

#### 4.3.3. Energetic Potential

**Definition 6 (Energetic potential).** The energetic potential of a
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

### 4.4. Combined Potential

**Theorem 4 (Combined potential).** If `Φ₁, Φ₂, ..., Φₖ` are potential
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

### 4.5. Static Prediction of Termination

The static conflict analyzer (Section 3) can be extended to predict
termination. For each rule, the analyzer computes:

- **Creation count:** number of non-default cells created by the rule.
- **Destruction count:** number of non-default cells destroyed by the
  rule.

**Lemma 6 (Counting termination).** For a rule set R, let:
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
affected regions (Theorem 2), so the net change across all matches is
the sum of individual changes. The total decrease is at least the number
of accepted matches, which is ≥ 1 if any match fires. By Theorem 3, the
simulation terminates. For non-empty conflict graphs, arbitration ensures
pairwise non-overlapping accepted matches, and the same argument applies
to the accepted subset. ∎

This is a conservative criterion: false negatives are possible (a
simulation may terminate even if the criterion is not met), but false
positives are not.

### 4.6. Limitations and Scope

All three analyses presented in this paper are **sound but not complete**:

- **Conflict graph** — an empty graph guarantees conflict-free execution
  (Theorem 2), but a non-empty graph does not guarantee conflicts at
  runtime. The static analyzer over-approximates: it checks all possible
  offsets and type compatibility without considering actual grid state.
  A non-empty graph is a warning, not a proof of conflict.

- **Termination criteria** — Lemma 6 (destruction > creation) is a
  sufficient but not necessary condition. A simulation may terminate
  even if the criterion fails (e.g., a rule that temporarily increases
  the cell count but eventually converges).

- **Potential functions** — Theorem 3 requires the user to find a
  suitable potential function. The paper provides three classes and a
  combination method, but does not provide an automated synthesis
  procedure. Finding a potential function for an arbitrary rule set
  remains a manual task.

**Necessity of the counting potential.** The counting potential `Φ_cnt`
is a sufficient condition for termination (Lemma 6), but it is not a
necessary condition. A counterexample (`configs/oscillation.yaml`)
demonstrates this: a marker (types 1↔2) oscillates between positions 0
and 1, while a timer (type 99) starts at position 5 and walks west
by 1 step per tick. The timer has higher priority (20) than the marker
(10). At tick 4, the timer reaches position 1 while the marker is at
position 0 — the timer shifts from 1 to 0, overwriting the marker.
The simulation terminates at tick 6. Yet `Φ_cnt` is constant (2) for
the first 5 ticks — it never strictly decreases until the final ticks.
Therefore, `Φ_cnt` is not monotonic: it does not capture all terminating
simulations.

**Theorem 5 (Potential is sufficient, not necessary).** The existence
of a decreasing potential function is sufficient for termination
(Theorem 3), but not necessary. The simulation in
`configs/oscillation.yaml` terminates despite `Φ_cnt` not being
monotonically decreasing. Hence, without restricting the class of rules,
a decreasing potential is the best guarantee that can be given.

*Proof of non-necessity.* The simulation in `configs/oscillation.yaml`
uses three rules: `[1]`, `[2]`, and `[99]`. Rules `[1]` and `[2]`
toggle the marker type and shift it east/west respectively (oscillation
0↔1), while rule `[99]` walks west by 1 step per tick from position 5.
All three rules have `min_age: 0`. The timer has priority 20, the marker
rules have priority 10. Over ticks 0–4 (5 ticks), `Φ_cnt = 2` (marker
oscillating at 0↔1, timer walking west). At tick 4, the timer reaches
position 1 and the marker is at position 0 — the timer shifts from 1 to
0, overwriting the marker in the conflict resolution, giving `Φ_cnt = 1`.
At tick 5, the timer on position 0 shifts west out of bounds, giving
`Φ_cnt = 0`. Tick 6: zero matches, termination. The simulation
terminates, but `Φ_cnt` did not monotonically decrease (it was constant
for 5 ticks). Thus, `Φ_cnt` is not a valid decreasing potential function
for this simulation, yet the simulation terminates.
∎

**Assumptions.** The determinism proof (Theorem 1) assumes a consistent
sort implementation. Since the sort key `(priority, age, center_x,
center_y)` is a total order with unique keys (Lemma 1 and Lemma 2),
sort stability is not required — all keys are distinct, so any correct
sorting algorithm produces identical output. The conflict analysis
correctly treats `min_age` as a lower bound and does not use it to
exclude conflicts. Rules with additional dynamic conditions (future
extensions) may require re-analysis.

---

## 5. Relationship to the Five Axioms

Cellaria is defined by five axioms [1]:

1. **Homogeneous Grid** — no cell has privileged status by coordinate.
2. **Computation Through Rules Only** — all state changes happen through
   the detect → arbitrate → apply cycle.
3. **Interface Through Boundary Only** — input and output cross the
   system boundary; they are not computation.
4. **Rules Outside the Grid** — rules are stored externally and updated
   atomically between ticks.
5. **Cleanup Through Rules** — if a cell disappears, a rule must do it;
   the `min_age` field provides time-based cleanup.

All three analyses in this paper respect these constraints.

**Axiom 1 (Homogeneous Grid).** The determinism proof (Section 2),
conflict analysis (Section 3), and termination analysis (Section 4)
operate on rules, not on grid coordinates. They do not assign privileged
status to any cell.

**Axiom 2 (Computation Through Rules Only).** All three analyses are
meta-operations: they inspect the rule set, not the grid state. The
arbitration bypass (Theorem 2) is a semantic optimization: the final
grid state is identical to the state that would be produced with
arbitration.

**Axiom 3 (Interface Through Boundary Only).** The analyses do not
perform I/O. They are purely internal optimizations.

**Axiom 4 (Rules Outside the Grid).** The analyses read rules from the
external rule store. They do not store rules inside the grid.

**Axiom 5 (Cleanup Through Rules).** The analyses do not introduce
hidden cleanup. If a rule with `min_age > 0` is the only rule that can
clear a cell, and the conflict graph is empty, the rule will still fire
when its `min_age` precondition is met.

---

## 6. Experimental Validation

### 6.1. Test Configurations

All 10 configurations from `configs/` were analyzed:

| Config | Rules | Conflict Graph | Self-loops | Prediction | Actual Behavior |
|---|---|---|---|---|---|
| `parallel.yaml` | 2 | Empty | None | Conflict-free | 2 matches/tick, no arbitration needed |
| `conflict.yaml` | 2 | 1 edge | None | Conflicts possible | One shift wins, arbitration required |
| `turing.yaml` | 7 | Empty | None | Conflict-free | 1 match/tick, no arbitration needed |
| `tag_system.yaml` | 4 | Empty | None | Conflict-free | 1 match/tick, no arbitration needed |
| `cascade.yaml` | 3 | 1 edge | None | Conflicts possible | Sequential application required |
| `collision.yaml` | 2 | Empty | None | Conflict-free | Independent matches |
| `io.yaml` | 1 | Empty | None | Conflict-free | Single rule, no conflicts |
| `overflow.yaml` | 1 | Empty | None | Conflict-free | Single rule, no conflicts |
| `composition.yaml` | 3 | 1 edge | None | Conflicts possible | Priority-based arbitration needed |
| `oscillation.yaml` | 3 | 1 edge | None | Conflicts possible | Timer overwrites marker by priority |

### 6.2. Verification of Determinism

The determinism of arbitration (Theorem 1) was verified by running each
configuration 10 times with identical parameters. All runs produced
identical logs, confirming the theoretical result.

### 6.3. Termination Detection

The `detect_termination` method was validated on four test cases:

| Test Case | Configuration | max_ticks | observation_ticks | Verdict |
|---|---|---|---|---|
| `test_termination_turing` | `configs/turing.yaml` | 50 | 10 | Terminates |
| `test_termination_tag_system` | `configs/tag_system.yaml` | 20 | 5 | Terminates |
| `test_termination_infinite_loop` | Rule 1→1 (no shift) | 100 | 20 | MayDiverge |
| `test_termination_unknown` | Rule 1→1 (shift right + copy behind) | 50 | 20 | Unknown |

The `turing` and `tag_system` configurations terminate deterministically.
The infinite loop repeats every tick and is correctly classified as
`MayDiverge`. The expanding configuration grows without bound and is
correctly classified as `Unknown`.

### 6.4. Test Suite

The test suite includes 55 tests covering:

- **Unit tests for conflict detection:** parallel rules (no conflict),
  conflicting rules (edge detected), Turing rules (no conflict),
  tag system rules (no conflict), different `min_age` (no conflict),
  incompatible types (no conflict), compatible types (conflict detected).
- **Integration tests:** all 10 configurations validated against actual
  runtime behavior.
- **Engine tests:** tick execution, arbitration, boundary I/O, shift
  operations, pattern matching, determinism, termination detection.

### 6.5. Verification

```
$ cargo test
55 passed; 0 failed

$ cargo clippy
0 warnings (new)
```

All predictions match the actual runtime behavior of the Cellaria engine.

---

## 7. Related Work

**Termination analysis.** Termination of rewriting systems has been
extensively studied. For term rewriting systems, the standard method is
to find a reduction order — a well-founded order on terms that is
compatible with rewrite rules [2, 3]. For graph rewriting systems,
termination is often proved via weighted type graphs [4] or by mapping
to term rewriting [5]. Our potential function approach follows the same
principle: a measure that strictly decreases on each rule application
guarantees termination.

**Termination tools.** Automated termination provers for term rewriting
(e.g., AProVE, TTT2) use techniques like dependency pairs [3] and
matrix interpretations. Our potential functions are inspired by
polynomial interpretations used in these tools, adapted to the spatial
grid setting of Cellaria. Extending these automated techniques to
rule-based spatial computation is an open direction.

**Static conflict detection.** In graph transformation systems, conflict
detection determines whether two rule applications can interfere [6].
Critical pair analysis identifies minimal conflicting configurations
[7]. Our conflict graph is a conservative approximation: we check
pattern compatibility (types) and region intersection at all relative
offsets, without constructing critical pairs.

**Parallel rewriting.** Parallel application of non-overlapping matches
is well-known in graph rewriting [8] and cellular automata [9]. The
contribution here is the static criterion (empty conflict graph) that
guarantees safe parallel execution for Cellaria specifically, leveraging
its grid-based geometry and pattern matching semantics.

**Potential functions in distributed computing.** Potential functions
(also called ranking functions or variant functions) are standard tools
for proving termination of distributed algorithms [10, 11]. Our three
classes (geometric, counting, energetic) adapt this idea to the
spatial rule-based setting of Cellaria.

---

## 8. Conclusion

We have presented three contributions about the Cellaria model:

1. **Section 2 (Formal proof of arbitration determinism)** (Theorem 1):
   the sort key `(priority, age, center_x, center_y)` is a total order
   and greedy selection on it is deterministic, guaranteeing portability
   across different implementations.

2. **Section 3 (Static conflict analysis)** (Theorem 2): a conflict graph
   can be constructed to determine whether arbitration is necessary. For
   conflict-free rule sets, arbitration can be skipped and all matches
   applied simultaneously, enabling parallel execution on GPU/FPGA.

3. **Section 4 (Termination via potential functions)** (Theorem 3):
   sufficient conditions for termination can be derived from the rule set
   alone. Three classes of potential functions (geometric, counting,
   energetic) provide practical tools for proving termination of Cellaria
   programs. An implementation monitors these conditions at runtime,
   classifying simulations as Terminates, MayDiverge, or Unknown.

All three results are validated experimentally on 10 configurations and
55 tests, with all predictions matching actual runtime behavior.

## 10. Computational Complexity of Cellaria Programs

### 10.1. Definitions

We define the computational complexity of a Cellaria program in terms of
the grid configuration and the tick cycle:

- **Input size.** The input size of a Cellaria program is the number of
  active (non-default) cells in the initial configuration. This is the
  natural measure of problem size: more cells mean more data to process.
- **Computation step.** A single computation step is one tick, consisting
  of all five phases: detection, arbitration, application, aging, and
  cleanup.
- **Time complexity.** The time complexity of a simulation is the number
  of ticks executed until termination (i.e., until the detection phase
  returns zero matches in `accepted`).
- **Space complexity.** The space complexity of a simulation is the
  maximum number of active cells observed across all ticks of the
  simulation.

These definitions mirror the standard complexity-theoretic notions of
input length, elementary operation, and memory usage, adapted to the
tick-based execution model of Cellaria.

### 10.2. Linear Time: Turing Machine Simulation

**Hypothesis.** A Turing machine simulation in Cellaria requires
`O(T)` ticks for `T` steps of the machine.

**Method.** The benchmark `tm_bench(len)` creates a tape of length `len`
with random bits (cell types 1 and 2) and a head (type 10) at position 0.
The head moves rightward, inverting each bit, and stops when it reaches
a blank cell. The number of ticks until termination is recorded.

**Data.** Measurements from `configs/turing.yaml`:

| Len | Ticks | Ratio |
|-----|-------|-------|
| 10  | 10    | 1.0   |
| 50  | 50    | 1.0   |
| 100 | 100   | 1.0   |
| 200 | 200   | 1.0   |

**Result.** For all tested lengths, `ticks = len`. The ratio `ticks / len`
is exactly 1.0 for all sizes, confirming the linear hypothesis within the
bound `ticks ≤ 3·len`.

**Conclusion.** Cellaria simulates a Turing machine without overhead:
one tick corresponds to exactly one step of the machine. The head
processes each tape symbol in a single tick, and the simulation
terminates immediately upon reaching a blank cell. No additional ticks
are required for bookkeeping or state transitions.

### 10.3. Linear Time: Tag System

**Hypothesis.** A tag system simulation in Cellaria requires `O(N)`
ticks for a string of length `N`.

**Method.** The benchmark `tag_bench(len)` creates a string of length
`len` with random symbols A and B (cell types 1 and 2) and a marker
(type 10) at position 0. The marker moves rightward, consuming each
symbol, and stops when it reaches an empty cell. The number of ticks
until termination is recorded.

**Data.** Measurements from `configs/tag_system.yaml`:

| Len | Ticks | Ratio |
|-----|-------|-------|
| 5   | 5     | 1.0   |
| 10  | 10    | 1.0   |
| 20  | 20    | 1.0   |
| 50  | 50    | 1.0   |

**Result.** For all tested lengths, `ticks = len`. The ratio `ticks / len`
is exactly 1.0 for all sizes, confirming the linear hypothesis within the
bound `ticks ≤ 2·len`.

**Conclusion.** A single-pass marker processes the input string in linear
time. Each symbol is consumed in one tick, and the marker stops
immediately after the last symbol. This is optimal asymptotically: no
algorithm can process a string of length `N` in fewer than `N` ticks
when each tick processes at most one symbol.

### 10.4. Constant Time: Conflict-Free Rules

**Hypothesis.** A rule set with an empty conflict graph and no self-loops
terminates in `O(1)` ticks, independent of the grid size.

**Method.** The benchmark `conflict_free_bench(width)` creates a grid of
size `width × 1` with two independent patterns: `[1, 2]` at position 0
and `[3, 4]` near the end. Both rules are conflict-free (verified by the
static conflict analyzer, Section 3). The number of ticks until
termination is recorded.

**Data.** Measurements from `configs/parallel.yaml`:

| Width | Ticks |
|-------|-------|
| 8     | 1     |
| 16    | 1     |
| 32    | 1     |
| 64    | 1     |

**Result.** For all tested widths, `ticks = 1`. The number of ticks is
constant and does not depend on the grid size. The bound `ticks ≤ 5` is
easily satisfied.

**Conclusion.** Conflict-free rules are applied in a single tick. Since
the static conflict graph is empty (Theorem 2, Section 3), arbitration
is bypassed and all matches are applied simultaneously. The grid size
does not affect the number of ticks required: two independent rules fire
concurrently and terminate in one step. This confirms the theoretical
prediction of the static conflict analyzer.

### 10.5. Open Questions

The complexity analysis of Cellaria programs raises several open questions:

1. **General definition of input size.** For the Turing machine and tag
   system, the input size is naturally the number of active cells. For
   arbitrary configurations — for example, those with multiple interacting
   markers, overlapping patterns, or complex spatial structures — the
   appropriate notion of input size is less clear. The number of active
   cells may not capture the "amount of work" to be done. A more general
   definition, analogous to the encoding length in Turing machines, would
   enable a broader complexity theory for Cellaria.

2. **Complexity classes for rules with conflicts.** The benchmarks in
   this section cover only rule sets with an empty conflict graph. For
   rule sets with conflicts, arbitration adds a worst-case overhead of
   `O(M²)` comparisons per tick, where `M` is the number of matches.
   The empirical complexity of conflict-rich simulations — and the
   question of whether the `O(M²)` bound is tight — remains unexplored.

3. **Lower bound for tape inversion.** The Turing machine simulation
   (Section 10.2) achieves exactly `len` ticks for inverting a tape of
   length `len`. Proving an `Ω(N)` lower bound for this problem — i.e.,
   that no Cellaria program can invert a tape of length `N` in fewer
   than `N` ticks — would establish a fundamental limit on the speed of
   computation in the model. Such a proof would likely rely on the
   locality of rule application: each tick can only affect cells within
   a bounded radius of the matched patterns.

---

### 10.6. Complexity Classes for Cellaria Programs

We define two complexity classes for Cellaria programs based on the
conflict graph:

**Definition 7 (CF — Conflict-Free).** A Cellaria program belongs to
class CF if its conflict graph (Section 3.4) is empty and contains no
self-loops. For a CF program:

- **Arbitration is bypassed.** By Theorem 2, all matches in every tick
  are pairwise non-overlapping and can be applied simultaneously.
- **Per-tick time:** `O(M)` where `M` is the number of matches in the
  tick, since each match is applied independently without conflict
  resolution overhead.
- **Total time:** depends on the logic encoded in the rules. For TM
  simulation (Section 10.2), total ticks = `O(N)`; for tag system
  (Section 10.3), total ticks = `O(N)`; for conflict-free parallel
  rules (Section 10.4), total ticks = `O(1)`.
- **Arbitration cost:** none — the constant factor per tick is minimal.

CF programs are the simplest class: concurrency is free, no arbitration
is needed, and matches execute in lockstep.

**Definition 8 (CA — Conflict-Aware).** A Cellaria program belongs to
class CA if its conflict graph is non-empty. For a CA program:

- **Arbitration is required.** By Theorem 1, arbitration is deterministic
  but involves `O(M²)` comparisons in the worst case, where `M` is the
  number of matches detected in a tick.
- **Per-tick time:** `O(M²)` due to greedy pairwise conflict resolution.
  In the worst case, `M` is proportional to the number of active cells.
- **Total time:** depends on the program logic. The arbitration overhead
  is the dominant factor.
- **Arbitration cost:** `O(M²)` per tick.

CA programs subsume CF programs: every CF program is trivially in CA
(the conflict graph is empty, so arbitration selects all matches in
`O(M)` time), but CF programs avoid the quadratic overhead entirely.

**Hypothesis 1 (CF ≡ CA — expressive equivalence).** For every CA
program `P`, there exists a CF program `P'` (possibly with more rules)
that computes the same function. That is, any Cellaria program with
conflicts can be rewritten as a conflict-free program, eliminating the
need for arbitration entirely.

*Justification.* Conflicts in Cellaria arise when two rules match in
overlapping regions and write to intersecting cells. In principle, such
conflicts can be resolved by:

1. **Splitting rules** — replacing a conflicting rule with multiple
   sub-rules that cover disjoint type configurations, so the original
   conflict disappears.
2. **Adding intermediate types** — introducing new cell types that
   split the matching space into disjoint cases.
3. **Delaying with `min_age`** — using `min_age` to ensure that
   conflicting rules activate at different ticks (though the static
   conflict analyzer conservatively ignores `min_age`, the actual
   runtime ordering can separate them).

If Hypothesis 1 is true, then the distinction between CF and CA is a
matter of optimization, not expressiveness. Arbitration becomes an
optional optimization: CF programs do not need it, and CA programs
can be transformed into CF programs that also do not need it.

*Proof sketch (informal).* Consider a pair of conflicting rules
`Rᵢ` and `Rⱼ` with overlapping affected regions. Let `Tᵢ` and `Tⱼ`
be the sets of cell types that trigger `Rᵢ` and `Rⱼ` respectively.
Construct a new rule `Rₖ` whose pattern is the union `Tᵢ × Tⱼ` for
the overlapping cells, and whose effect is the composition of the two
original rules (applied in priority order). Replace `Rᵢ` and `Rⱼ` with
`Rₖ` and adjusted versions of `Rᵢ` and `Rⱼ` that handle the
non-overlapping cases. By induction on the number of conflicting pairs,
any CA program can be reduced to CF.

Proving Hypothesis 1 would mean that arbitration is optional for *all*
Cellaria programs, not just those that happen to have an empty conflict
graph. This is a strong result and a direction for future work.

---

## References

1. Cellaria: A Local Reduction Model of Computation. (2026). Technical
   Report.

2. Dershowitz, N. (1987). Termination of rewriting. *Journal of Symbolic
   Computation*, 3(1-2), 69–115.

3. Arts, T., & Giesl, J. (2000). Termination of term rewriting using
   dependency pairs. *Theoretical Computer Science*, 236(1-2), 133–178.

4. Bruggink, H. J. S., König, B., & Zantema, H. (2015). Termination
   analysis for graph transformation systems. *Information and
   Computation*, 240, 56–73.

5. Plump, D. (2018). Termination of graph transformation systems.
   In *Graph Transformation, Specifications, and Nets* (pp. 87–105).
   Springer.

6. Lambers, L., Ehrig, H., & Orejas, F. (2006). Conflict detection for
   graph transformation with negative application conditions. In *ICGT
   2006* (pp. 61–76). Springer.

7. Ehrig, H., Ehrig, K., Prange, U., & Taentzer, G. (2006). *Fundamentals
   of Algebraic Graph Transformation*. Springer.

8. Campbell, G., & Plump, D. (2013). Parallel graph transformation.
   In *Graph Transformation* (pp. 154–169). Springer.

9. Toffoli, T., & Margolus, N. (1987). *Cellular Automata Machines:
   A New Environment for Modeling*. MIT Press.

10. Dijkstra, E. W. (1974). Self-stabilizing systems in spite of
    distributed control. *Communications of the ACM*, 17(11), 643–644.

11. Lynch, N. A. (1996). *Distributed Algorithms*. Morgan Kaufmann.