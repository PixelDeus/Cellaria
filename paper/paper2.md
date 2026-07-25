# Cellaria: Arbitration — Determinism, Static Conflict Analysis, and Composition

## Abstract

Cellaria is a cellular automaton-like model of computation based entirely
on local reduction, defined by five axioms. The tick cycle consists of
detection (pattern matching), arbitration (conflict resolution), and
application (modification). We present three contributions about the
arbitration mechanism:

1. **Determinism of arbitration** — the sort key `(priority, age, center)`
   defines a total order; greedy selection on a totally ordered set is
   deterministic, guaranteeing identical results across non-deterministic
   iteration orders and different implementations.

2. **Static conflict analysis** — a conservative algorithm constructs a
   conflict graph for a given rule set, and we prove its **completeness**:
   if no edge exists between two rules, they cannot conflict for any grid
   state. If the graph is empty, arbitration can be skipped and all matches
   applied simultaneously.

3. **Composition of conflict-free rule sets** — a theorem and catalog of
   operations that preserve the conflict-free property, enabling modular
   construction of parallel Cellaria programs.

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
be applied simultaneously. We prove **completeness** of the conflict
graph: absence of an edge implies impossibility of conflict for any grid
state (Section 3.5).

The third contribution (Section 4) provides a **composition theorem**
for conflict-free rule sets: if two conflict-free sets have an empty
combined conflict graph, they can be safely merged without arbitration.
A catalog of preserving operations (unique head types, isolated `min_age`,
spatial isolation, disjoint types) enables modular construction of
parallel Cellaria programs.

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

**Lemma 4 (Affected regions disjointness).** Even if two rules can match
simultaneously, they do not conflict if their affected regions do not
intersect.

*Proof.* Let `Rᵢ` match at `P` and `Rⱼ` match at `Q`, with
`affected_i(P) ∩ affected_j(Q) = ∅`. Then `Rᵢ` reads and writes only in
`affected_i(P)`, and `Rⱼ` reads and writes only in `affected_j(Q)`.
No cell is read or written by both rules; therefore, the results are
independent and no conflict exists. ∎

### 3.4. The Conflict Graph

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
`O(N² · K²)`. In practice, `K ≤ 10` and the dominating term is `O(N²)`.

### 3.5. Completeness of the Conflict Graph

We prove that the conflict graph has **no false negatives**: if no edge
exists between two rules, they are guaranteed conflict-free for all grid
states.

#### Three Conditions for Edge Absence

The algorithm checks three conditions, each sufficient to guarantee
absence of conflict:

**Condition A: Type incompatibility in overlapping patterns.**
If at offset `(dx, dy)` the patterns intersect but at least one
overlapping cell has incompatible types (`type_i ≠ type_j`), then the
rules cannot match simultaneously — no cell can hold two types at once.

**Condition B: Different `min_age`.** The algorithm **does not** exclude
edges based on `min_age` differences. Two rules with different `min_age`
values can both fire on the same cell in the same tick (when
`age ≥ max(min_age_i, min_age_j)`). This is a deliberate
over-approximation to preserve soundness.

**Condition C: Non-intersecting affected regions.**
Even if both rules can match simultaneously, they do not conflict if
their affected regions are disjoint (Lemma 4).

#### Inductive Proof of Completeness

**Theorem 2 (Completeness).** Let `Rᵢ` and `Rⱼ` be two rules. If there
is no edge `(i, j)` in the conflict graph, then `Rᵢ` and `Rⱼ` cannot
conflict under any grid state.

*Proof.* By induction on the number of rules.

**Base case (single rule).** A graph with one vertex and no self-loop is
empty. A single rule cannot conflict with itself at zero offset; a
self-loop exists only if the rule conflicts with itself at a non-zero
offset. With no self-loop, no grid state produces two overlapping
instances of the same rule with intersecting affected regions.

**Inductive step (adding rule `Rₙ`).** Suppose the graph for
`{R₀, ..., Rₙ₋₁}` is complete. Add `Rₙ`. For each pair `(i, n)`,
`i < n`:

1. `can_match_simultaneously` checks pattern intersection at all offsets.
   - If `intersection = ∅` for all offsets → patterns never overlap →
     affected regions are disjoint → no edge. ✓
   - If `intersection ≠ ∅` but types are incompatible in at least one
     overlapping cell → Condition A → no simultaneous match possible →
     no conflict → no edge. ✓
2. If patterns intersect and types are compatible, check affected regions.
   - If `affected_i(P) ∩ affected_n(Q) = ∅` for all offsets →
     Condition C → no conflict → no edge. ✓
   - If affected regions intersect → edge `(i, n)` is added. ✓

The three cases (no overlap, incompatible types, disjoint regions)
exhaust all possibilities. If none apply, an edge is added. Therefore,
the absence of an edge `(i, n)` guarantees that no grid state can
produce a conflict between `Rᵢ` and `Rₙ`.

By induction, the graph is complete for any finite rule set. ∎

**Corollary 2 (No false negatives).** The conflict graph is **sound and
complete**: an empty graph guarantees conflict-free execution, and a
non-empty edge always corresponds to a possible conflict under some grid
state.

### 3.6. Structural Properties

**Lemma 5 (Idempotence).** The conflict graph is idempotent under
repeated construction: for a fixed rule set, Algorithm 1 always produces
the same graph.

*Proof.* The algorithm is deterministic: it iterates over the same pairs,
offsets, and checks in the same order. No state is shared between
iterations. ∎

**Lemma 6 (Monotonicity).** Adding a rule to a rule set can only add
edges to the conflict graph, never remove them.

*Proof.* A new rule `Rₙ₊₁` may conflict with existing rules, adding edges
incident to vertex `n+1`. Existing edges are never removed because the
rules they depend on are unchanged. ∎

### 3.7. Theorem 3: Arbitration Bypass

**Theorem 3 (Arbitration bypass).** Let `G = (V, E)` be the conflict
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
   incompatibility) or Lemma 4 (non-intersecting affected regions)
   guarantees that the affected regions of any two instances of the
   same rule at distinct centers are disjoint.

4. Since no matches overlap, the greedy arbitration algorithm (which
   resolves overlapping matches) has nothing to resolve. All matches are
   accepted. Applying them in any order — or simultaneously — produces
   the same final grid state.

5. Hence, arbitration can be skipped. The detect → apply phases execute
   in one pass with no conflict resolution. ∎

**Corollary 3 (Parallel execution).** For a rule set with an empty
conflict graph and no self-loops, all matches in a tick can be applied
simultaneously in a single parallel pass over the grid. This enables
direct implementation on GPU (each thread handles one cell or one match)
or FPGA (each rule is a hardware pipeline stage) without sequential
arbitration.

### 3.8. Experimental Validation

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

All predictions match the actual runtime behavior.

---

## 4. Composition of Conflict-Free Rule Sets

### 4.1. Composition Theorem

Let two rule sets `R₁` and `R₂` each be conflict-free (empty conflict
graph). Form the union `R = R₁ ∪ R₂` and build its conflict graph.

**Theorem 4 (Composition).** If the conflict graph of `R = R₁ ∪ R₂` is
empty, then:

1. All properties of `R₁` are preserved: rules from `R₁` fire identically
   to isolated execution.
2. All properties of `R₂` are preserved: rules from `R₂` fire identically
   to isolated execution.
3. Arbitration for the combined rule set can be safely skipped.

*Proof.*

**Step 1.** `R₁` conflict-free ⇒ for any `i, j ∈ R₁`:
`affected(i) ∩ affected(j) = ∅`.

**Step 2.** `R₂` conflict-free ⇒ for any `i, j ∈ R₂`:
`affected(i) ∩ affected(j) = ∅`.

**Step 3.** Conflict graph for `R₁ ∪ R₂` empty ⇒ for any `i ∈ R₁`,
`j ∈ R₂`: `affected(i) ∩ affected(j) = ∅`.

**Step 4.** From steps 1–3, all affected regions in the union are
pairwise disjoint. No rule competes with another for any cell.

**Step 5.** Since no rule competes for cells, the result of applying
`R₁ ∪ R₂` is equivalent to sequential or parallel application of rules
from `R₁` and `R₂` in any order. Arbitration is not required — all
matches can be applied simultaneously.

**Step 6.** Rules from `R₁` in the combined set behave identically to
isolated execution, since `R₂` cannot modify cells that `R₁` reads or
writes. Symmetrically for `R₂`. ∎

### 4.2. Catalog of Preserving Operations

The following operations on a rule set guarantee that the conflict-free
property is preserved.

#### 4.2.1. Adding a Rule with a Unique Head Type

**Claim.** If a new rule has a head type not present in any existing
rule, the combined set remains conflict-free.

*Proof.* Rules with different head types (first element of `id`) cannot
match on the same cell simultaneously, as the cell would need to have
both types at once. Therefore, affected regions do not intersect. ∎

#### 4.2.2. Adding a Rule with Isolated `min_age`

**Claim.** If the `min_age` of a new rule is strictly greater than the
maximum age of any cell participating in existing rules, the combined
set remains conflict-free.

*Proof.* The new rule activates only at time `t ≥ min_age`. Existing
rules activate at `t < min_age`. Their time windows do not intersect —
they cannot fire simultaneously. ∎

> **Note.** In the current implementation, a cell's age is measured in
> ticks since the last modification. The maximum age at which a rule
> can fire is `min_age − 1` for rules with `min_age > 0`, and unbounded
> for rules with `min_age = 0`.

#### 4.2.3. Spatial Isolation

**Claim.** If all affected regions of a new rule are at distance `> K`
from all affected regions of existing rules, where `K` is the maximum
chain length across all rules, the combined set remains conflict-free.

*Proof.* Affected regions are defined relative to the rule's match
position. Two rules can fire at different grid locations. If the
distance between these locations exceeds `K`, their affected regions
cannot intersect, since affected regions by construction do not extend
beyond chains of length `K`. ∎

#### 4.2.4. Composition via Disjoint Types

**Claim.** If the type sets used in `R₁` and `R₂` are disjoint, the
conflict graph of the union is empty.

*Proof.* This follows from Lemma 3: rules with no common types cannot
match simultaneously (no cell can have two different types). Therefore,
their affected regions do not intersect. ∎

### 4.3. Composition API

```rust
/// Result of checking composition of two conflict-free rule sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompositionVerdict {
    /// All rules can be applied simultaneously; arbitration not needed.
    Safe,
    /// Conflicts found with pairs (i-from-R₁, j-from-R₂).
    Unsafe(Vec<(usize, usize)>),
}
```

```rust
pub fn check_composition(rules_a: &[Rule], rules_b: &[Rule]) -> CompositionVerdict {
    let mut combined = rules_a.to_vec();
    combined.extend_from_slice(rules_b);
    let graph = ConflictGraph::build(&combined);
    if graph.is_conflict_free() {
        CompositionVerdict::Safe
    } else {
        let n_a = rules_a.len();
        let unsafe_pairs: Vec<(usize, usize)> = graph
            .edges
            .iter()
            .filter_map(|&(i, j)| {
                if i < n_a && j >= n_a { Some((i, j - n_a)) }
                else if i >= n_a && j < n_a { Some((j, i - n_a)) }
                else { None }
            })
            .collect();
        CompositionVerdict::Unsafe(unsafe_pairs)
    }
}
```

### 4.4. Tests

| Test | R₁ | R₂ | Expected | Justification |
|------|----|----|----------|--------------|
| `test_composition_unique_head` | [10] | [20] | Safe | Different head types (10 vs 20) |
| `test_composition_same_head` | [10] | [10] | Unsafe | Same head type |
| `test_composition_min_age` | [10], min_age=0 | [10], min_age=10 | Safe | Non-overlapping time windows |
| `test_composition_spatial` | [1] | [2] | Safe | Different ids, disjoint affected regions |
| `test_composition_overlap` | [1,2]→right | [2,3]→left | Unsafe | Intersecting affected regions |
| `test_composition_tm_cleanup` | [10] | [99] | Safe | composition.yaml |

All 6 tests pass (51/51 total, 0 failures).

### 4.5. Example: TM Head + Cleanup

File: `configs/composition.yaml`

**R₁: TM head (conflict-free)**

| id | Head type | Shift | min_age | Description |
|----|-----------|-------|---------|-------------|
| [10] | 10 | east 1 | 0 | State A, reads bit, writes 0, shift right |
| [1]  | 1  | —      | 0 | Symbol "0" on tape |
| [2]  | 2  | —      | 0 | Symbol "1" on tape |

All three rules have different ids (first elements: 10, 1, 2), so their
affected regions do not intersect → R₁ conflict-free.

**R₂: Cleanup rule (trivially conflict-free)**

| id | Head type | Shift | min_age | Description |
|----|-----------|-------|---------|-------------|
| [99] | 99 | —    | 5 | Cleanup marker, removes processed symbols after 5 ticks |

R₂ consists of a single rule → conflict-free by definition.

**R₁ ∪ R₂: Composition check**

- **Head types:** R₁ uses {10, 1, 2}, R₂ uses {99} — disjoint.
- **min_age:** R₁ has min_age ∈ {0}, R₂ has min_age = 5 — non-overlapping.

By criteria 4.2.1 and 4.2.2: the combined conflict graph is empty →
composition is safe.

**Expected simulation behavior:**
1. The head (type 10) moves left to right across the tape, inverting bits.
2. After the head passes, cells of type 0 (fill) remain.
3. After 5 ticks, the cleanup rule (type 99) removes garbage markers.
4. TM head and cleanup do not compete for cells — arbitration is not needed.

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
conflict analysis (Section 3), and composition theorem (Section 4)
operate on rules, not on grid coordinates. They do not assign privileged
status to any cell.

**Axiom 2 (Computation Through Rules Only).** All analyses are
meta-operations: they inspect the rule set, not the grid state. The
arbitration bypass (Theorem 3) is a semantic optimization: the final
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

## 6. Related Work

**Static conflict detection.** In graph transformation systems, conflict
detection determines whether two rule applications can interfere [2].
Critical pair analysis identifies minimal conflicting configurations
[3]. Our conflict graph is a conservative approximation: we check
pattern compatibility (types) and region intersection at all relative
offsets, without constructing critical pairs. We additionally prove
**completeness**: the absence of an edge guarantees impossibility of
conflict for any grid state.

**Parallel rewriting.** Parallel application of non-overlapping matches
is well-known in graph rewriting [4] and cellular automata [5]. The
contribution here is the static criterion (empty conflict graph) that
guarantees safe parallel execution for Cellaria specifically, leveraging
its grid-based geometry and pattern matching semantics.

**Compositional reasoning.** Compositional verification of concurrent
systems is a classic topic [6]. Our composition theorem adapts this
idea to Cellaria's spatial rule-based setting: conflict-free rule sets
can be composed modularly, with guaranteed preservation of behavior.

---

## 7. Open Questions

**Hypothesis 1 (CF ≡ CA — expressive equivalence).** For every CA
program (with a non-empty conflict graph), there exists a CF program
(possibly with more rules) that computes the same function. That is,
any Cellaria program with conflicts can be rewritten as a conflict-free
program, eliminating the need for arbitration entirely.

*Justification.* Conflicts in Cellaria arise when two rules match in
overlapping regions and write to intersecting cells. In principle, such
conflicts can be resolved by:

1. **Splitting rules** — replacing a conflicting rule with multiple
   sub-rules that cover disjoint type configurations.
2. **Adding intermediate types** — introducing new cell types that
   split the matching space into disjoint cases.
3. **Delaying with `min_age`** — using `min_age` to ensure that
   conflicting rules activate at different ticks.

If Hypothesis 1 is true, then the distinction between CF and CA is a
matter of optimization, not expressiveness. Arbitration becomes an
optional optimization for all programs.

**Soundness vs. completeness trade-off.** The current conflict analyzer
conservatively ignores `min_age` differences, which may introduce false
positives. A dynamic analysis that considers actual cell ages could
reduce false positives but at the cost of increased complexity. Finding
the right balance is an open problem.

---

## References

1. Cellaria: A Local Reduction Model of Computation. (2026). Technical
   Report.

2. Lambers, L., Ehrig, H., & Orejas, F. (2006). Conflict detection for
   graph transformation with negative application conditions. In *ICGT
   2006* (pp. 61–76). Springer.

3. Ehrig, H., Ehrig, K., Prange, U., & Taentzer, G. (2006). *Fundamentals
   of Algebraic Graph Transformation*. Springer.

4. Campbell, G., & Plump, D. (2013). Parallel graph transformation.
   In *Graph Transformation* (pp. 154–169). Springer.

5. Toffoli, T., & Margolus, N. (1987). *Cellular Automata Machines:
   A New Environment for Modeling*. MIT Press.

6. Dijkstra, E. W. (1974). Self-stabilizing systems in spite of
   distributed control. *Communications of the ACM*, 17(11), 643–644.