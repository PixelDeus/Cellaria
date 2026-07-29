# Cellaria: Arbitration — Determinism, Static Conflict Analysis, and Composition

## Abstract

Cellaria is a cellular automaton-like model of computation based entirely
on local reduction, defined by five axioms. The tick cycle consists of
detection (pattern matching), arbitration (conflict resolution), and
application (modification). We present three contributions about the
arbitration mechanism:

1. **Determinism of arbitration** — the sort key `(priority, age,
   rule_id, center_x, center_y, rule_idx)` defines a total order; greedy
   selection on a totally ordered set is deterministic, guaranteeing
   identical results across non-deterministic iteration orders and
   different implementations.

2. **Static conflict analysis** — a conservative algorithm constructs a
   conflict graph for a given rule set, and we prove its **completeness**:
   if no edge exists between two rules, they cannot conflict for any grid
   state. If the graph is empty, arbitration can be skipped and all matches
   applied simultaneously.

3. **Composition of conflict-free rule sets** — a theorem and catalog of
   operations that preserve the conflict-free property, enabling modular
   construction of parallel Cellaria programs. Extended to a *guarded*
   form of self-modification (Section 4.6): re-running the same
   composition check at every self-installed rule, not just once at
   startup, keeps the guarantee alive as one region's rule set evolves
   at runtime alongside an independently-written neighbor's.

4. **Spatial decomposition for programs with conflicts** — a locality
   theorem showing that arbitration itself, not just conflict-free rule
   sets, can be parallelized: partitioning the grid into regions with a
   sufficiently wide margin lets each region's matches be arbitrated
   independently, with only a thin boundary strip requiring shared
   sequential arbitration, and the combined result is provably identical
   to centralized arbitration over all matches.

5. **A bound on fault propagation** — the same locality argument that
   enables spatial decomposition also bounds how far a single corrupted
   cell's effect can spread after `t` ticks, independent of whether the
   rule set is conflict-free or conflict-aware. Generalized (Section 6.4)
   to rule sets that change mid-run via self-modification: the bound
   becomes a sum over each tick's active reach rather than one constant
   `K`, and we show empirically that the original constant-`K` bound is
   not merely imprecise but actually violated once a wider-reaching rule
   is installed, while the generalized one never is.

6. **Reversibility** — a sufficient condition, built entirely from the
   conflict-free machinery of Sections 2–3, under which a Cellaria tick
   is a bijection on configurations: every rule locally invertible, the
   rule set distinguishable, and the conflict graph empty. Such a rule
   set admits an exact inverse rule set, reconstructing any prior
   configuration cell for cell — not approximately, but exactly. We
   connect this, with an explicit scope caveat, to Landauer's principle:
   reversible computation is the one class not subject to a physical
   minimum-energy-dissipation argument from information erasure.

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
arbitration determinism: the sort key `(priority, age, rule_id, center_x,
center_y, rule_idx)` defines a total order, guaranteeing identical
results across non-deterministic iteration orders. This is non-trivial
because `ChunkStorage` (the infinite grid implementation) uses a
`HashMap` for chunk storage, and `active_cells()` iterates in
non-deterministic order between runs.

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
parallel Cellaria programs. Section 4.6 extends this to *guarded*
self-modification: if either side can install new rules at runtime, the
static, one-time composition check no longer suffices on its own —
re-running it at every self-installed rule, and rejecting one that would
break an already-established neighbor, keeps the guarantee alive for as
long as guarding stays enabled.

The fourth and fifth contributions (Section 6) extend the locality
argument used throughout this paper — that a rule's affected region
extends at most `K` cells from its match center — to two questions that
do not require the conflict graph to be empty. First, whether the grid
can be split into regions and arbitrated in parallel even when the rule
set has real conflicts (Section 6.2), which the composition theorem of
Section 4 does not address, since it requires the *combined* graph to be
empty. Second, how far the effect of a single corrupted cell can spread
after `t` ticks (Section 6.3), a bound that turns out to hold uniformly
for both conflict-free and conflict-aware rule sets.

The sixth contribution (Section 7) asks a different question of the
same conflict-free machinery: when is a Cellaria tick *reversible* — a
bijection on configurations, admitting an exact inverse rather than an
approximate one? We give a sufficient condition built directly on
Theorem 3, validate it on a rule set combining both sources of
information loss (`changes` and `shifts`), and connect it, with an
explicit scope caveat, to a real physical principle (Landauer's bound
on the energy cost of erasing information) rather than an unsupported
efficiency claim.

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
`(priority, age, rule_id, center_x, center_y, rule_idx)` defines a total
order, guaranteeing deterministic greedy selection regardless of input
order.

### 2.2. The Arbitration Algorithm

The arbitration algorithm (`arbitrate`) works as follows:

1. **Sort** all matches by `(priority, age, rule_id, center_x, center_y,
   rule_idx)` in descending order (highest priority first; ties broken
   by highest age, then by `rule_id`, then by center coordinates, then by
   `rule_idx`). `rule_id` is derived from the matching rule's `id` field;
   `rule_idx` is the rule's position within `rule_index[head]`, needed
   because a single head type may have more than one rule registered
   against it (Section 2.3).
2. **Greedy selection:** iterate through the sorted list; accept a match
   if its center cell and all its pattern cells are not already used by
   a previously accepted match.

### 2.3. Multiple Matches per Center

An earlier version of this analysis assumed a grid cell could be the
center of at most one `RuleMatch` per tick, since `detect_matches`
groups rules by head type and rules are stored in `rule_index: HashMap<
CellType, Vec<Rule>>`. That assumption does not hold in general: a head
type may have *more than one* rule registered against it (used
throughout this project — e.g. a counter cell with both an "increment"
rule and a "finalize" rule keyed to the same encoded type, distinguished
by `min_age`). A single cell can therefore be the center of multiple
matches in the same tick, one per rule whose pattern it satisfies.

This is exactly why `rule_id` and `rule_idx` are part of the tie-break:
without them, two distinct matches sharing a center and an equal
`(priority, age)` would be indistinguishable to the sort, and the greedy
algorithm's choice between them would depend on input order — breaking
determinism. `rule_id` (the matching rule's `id` bytes) distinguishes
matches from rules with different identities; `rule_idx` (the rule's
index within `rule_index[head]`) is the final fallback for the residual
case of two distinct rule entries that happen to share identical `id`
bytes.

### 2.4. Total Order

**Lemma 1 (Total order).** The key `(priority, age, rule_id, center_x,
center_y, rule_idx)` defines a total order on the set of all `RuleMatch`
objects in a single tick.

*Proof.* For any two distinct matches `m₁` and `m₂`:

- If `priority(m₁) ≠ priority(m₂)`, the higher priority wins.
- If `priority` is equal, compare `age` (the age of the center cell). If
  ages differ, the higher age wins.
- If `priority` and `age` are equal, compare `rule_id`. If the matched
  rules have different `id` bytes, this distinguishes them.
- If `rule_id` also ties, compare `center_x`, then `center_y`.
- If `priority`, `age`, `rule_id`, `center_x`, and `center_y` are all
  equal — two distinct rule entries, sharing identical `id` bytes,
  matching the same center cell in the same tick — compare `rule_idx`.
  Since `rule_idx` is each rule's distinct position within
  `rule_index[head]`, and `m₁ ≠ m₂` under these conditions can only arise
  from two different entries in that vector, `rule_idx` is guaranteed to
  differ and the comparison is well-defined.

Every component is totally ordered, and the lexicographic combination
(`priority`, `age`, `rule_id` descending; `center_x`, `center_y`
ascending; `rule_idx` descending) is therefore a total order. ∎

### 2.5. Theorem 1: Determinism of Arbitration

**Theorem 1 (Determinism).** For any grid state and any rule set, the
`arbitrate` function returns the same set of accepted matches regardless
of the order in which matches are passed to it.

*Proof.* Let `M` be the set of all matches detected in a tick. The
arbitration algorithm:

1. Sorts `M` by the total order key `(priority, age, rule_id, center_x,
   center_y, rule_idx)` (Lemma 1). Sorting is deterministic: for any
   input order, the sorted order is the same.
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
provided the sort order `(priority, age, rule_id, center_x, center_y,
rule_idx)` is consistently implemented.

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

**Lemma 2 (Type incompatibility).** If two rules `Rᵢ` and `Rⱼ` check
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

**Lemma 3 (Affected regions disjointness).** Even if two rules can match
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
their affected regions are disjoint (Lemma 3).

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
   offset. Since `G` has no self-loops by assumption, Lemma 2 (type
   incompatibility) or Lemma 3 (non-intersecting affected regions)
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

*Proof.* This follows from Lemma 2: rules with no common types cannot
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

### 4.6. Guarded Composability Under Self-Modification

Theorem 4 checks composition *once*, statically, before two rule sets are
merged. If either side can self-modify (installing new rules at runtime
via `RuleStore`, as in [3, self-modification]), that one-time guarantee
does not automatically persist: a self-installed rule could, in
principle, target a head type that an independently-written, already
safe neighboring module depends on, silently invalidating the very
property Theorem 4 established. This section shows the guarantee *can*
be made to persist, by re-running the same composition check at every
self-modification event rather than once at startup.

**Construction.** Let `H₀` be the set of head types present in the rule
set at the moment guarded self-modification is enabled — the "protected"
heads, representing modules that existed before any self-modification
began. For every subsequent self-installed `AddRule` targeting head `h`:

- if `h ∉ H₀` and `h` already has rules in the current index (i.e. `h`
  was itself introduced by an *earlier* self-installed rule — the
  self-modifying process growing its own, previously claimed territory),
  the new rule is installed unconditionally;
- otherwise — `h` is brand new, or `h ∈ H₀` — the rule is installed only
  if `ConflictGraph::check_composition({r_new}, rest)` (Section 4.3),
  where `rest` is the full current rule set excluding `h`'s own entries,
  returns `Safe`, or returns `Unsafe` with an *empty* pair list (meaning
  the only detected conflict is `r_new` against another instance of
  itself — a self-loop, unrelated to anything already installed; see the
  remark below). Otherwise the rule is discarded: neither applied to the
  rule store nor merged into the index.

**Theorem 5 (Guarded composability).** Under this construction, no rule
targeting a protected head `h ∈ H₀` is ever installed if doing so would
create a structural conflict — in the sense of Definition 1 — with the
rule set as it stood immediately before the install.

*Proof.* The check is performed synchronously, exactly once per
candidate rule, before either `rule_store.apply` or the merge into
`rule_index` (`Engine::absorb_self_modifications`). When the check
reports an actual cross-pair conflict, both steps are skipped via
`continue` — the candidate rule never reaches the rule store's internal
state and never reaches `rule_index`, leaving every existing entry,
including `h`'s, bit-for-bit unchanged. Since this is the *only* code
path by which a self-installed rule can reach `rule_index`, no
conflicting rule can ever be merged. ∎

**Remark (self-loops are not composition conflicts).** `check_composition`
reports `Unsafe` whenever the *combined* conflict graph is non-empty,
which includes a candidate rule conflicting with *another instance of
itself* at a different grid position (Section 3.4's self-loop check) —
a property of the candidate rule alone, unrelated to whether it collides
with anything already installed. An ordinary shift rule commonly has
such a self-loop (the same "moving object" pattern discussed throughout
this paper). Treating every `Unsafe` verdict as a rejection would reject
this class of otherwise perfectly safe rules for a reason that has
nothing to do with composability; the construction above rejects only
when the verdict's pair list is non-empty — an actual candidate-versus-
existing conflict.

**Remark (inherited conservatism).** The guard is exactly as conservative
as `ConflictGraph` itself (Section 3.3–3.5): a shift rule's destination
is not type-constrained, so it cannot be excluded from conflicting with
anything spatially reachable, regardless of actual type compatibility —
the same limitation already established for the shuttle in
`big_world.rs`. In practice, a candidate rule with a shift will often be
flagged unsafe if *any* other shift-carrying rule (including the
self-modification channel's own transport carriers, if they happen to
be represented as ordinary rules in the same index) is present. This is
not a defect introduced by the guard; it is the same static-analysis
limitation this paper has been honest about throughout, now visible at
a new point of use.

**Remark (scope: one protected snapshot, not a general race resolver).**
`H₀` is fixed at the moment guarding is enabled. Two independently
self-modifying regions racing to claim the *same*, previously unclaimed
head are not arbitrated by this mechanism — whichever installs first is
thereafter treated as "already existing, not protected" for that head,
and the guard does not intervene between competing self-modifying peers.
This is a real, acknowledged limit of scope, not a general solution to
multi-party self-modification races.

**Empirical validation.** `examples/proof_guarded_self_modification.rs`
sets up a static module (a decay rule, head `1`) and, using
`Engine::enable_guarded_self_modification`, injects two `AddRule`
packets directly: one targeting a fresh head (`50`) with a same-cell
change — accepted, since it cannot structurally overlap with the decay
rule — and one targeting head `1` itself with a *different* same-cell
change — rejected (`rejected_self_modifications` becomes `1`), leaving
the decay module's rule, and its behavior on a fresh run, byte-for-byte
unchanged. An earlier version of this experiment used physical carrier
cells (as in Section 4.5) for the delivery and reported false rejections
for genuinely safe rules; the cause was exactly the conservatism
described above — the carriers themselves, being ordinary shift rules,
were being compared against the candidate and found (correctly, if
uninformatively) self-referentially conflicting. The published example
injects packets directly into the boundary queue instead, isolating the
guard's decision from a transport mechanism already proven separately
in Section 4.5.

**Corollary 4 (Removal never needs guarding).** A self-installed
`RemoveRule` or `ClearAll` operation never requires a composition check.

*Proof.* Immediate from Lemma 5 (Monotonicity), read in reverse: if
adding a rule to a rule set can only add edges to its conflict graph,
then removing a rule can only remove edges (or leave the graph
unchanged) — never add one. A rule set that was composition-safe with
respect to every protected head before a removal remains so afterward,
since the combined conflict graph after the removal is a subgraph of
the one before it. The implementation reflects this directly:
`composition_allows` returns `true` immediately for any operation other
than `AddRule` (Section 4.6's construction), which this corollary shows
is not merely a convenient default but the provably correct behavior. ∎

**Three bugs, found while building this section.** The first working
version of guarded self-modification passed the tests above but had
separate defects in the underlying (unguarded) merge logic, exposed only
once a self-installed rule actually shared a head with a rule from
outside `RuleStore`'s own bookkeeping — a case none of Section 4.5's
demonstrations exercised, since they only ever targeted fresh heads.

First, `RemoveRule`/`ClearAll` updated `RuleStore`'s own internal state
correctly but the merge step, which only ever inserted keys present in
`RuleStore::get_index()`, never removed a head from `rule_index` once
its last rule was gone — a removed rule silently kept acting as if
still installed.

Second, and more seriously: `RuleStore::get_index()` is rebuilt entirely
from rules `RuleStore` has itself processed via `AddRule`, with no
knowledge of rules that were already part of `rule_index` — so merging
by replacing `rule_index[head]` outright with `get_index()[head]`
silently *destroyed* a pre-existing rule the moment self-modification
legitimately added another rule to the same head. The first fix tracked
a one-time snapshot of `rule_index` taken at `Engine::new`
(`original_rule_index`) and reconstructed each affected head on every
merge as *original rules for that head, plus whatever `RuleStore`
currently reports* — rather than a blind overwrite.

Third: a snapshot taken only at `Engine::new` is itself incomplete,
since `rule_index` can be legitimately modified directly at any later
point (the pattern demonstrated in `strength_live_rules.rs` — mutate
`rule_index`, then call `rebuild_rule_cache`). A rule added this way
*after* construction but *before* self-modification began was, by the
second fix, treated as unprotected territory rather than a pre-existing
rule — and was destroyed exactly as in the second bug the moment
self-modification extended the same head. The complete fix moves the
snapshot to the moment self-modification is actually enabled
(`Engine::enable_self_modification`) rather than construction, and —
since `rule_index` can still be edited directly *after* that point —
recomputes `original_rule_index` on every `rebuild_rule_cache` call, as
*whatever is currently in `rule_index`, minus whatever `RuleStore`
currently reports owning*. This correctly tracks "foreign territory" as
it actually stands at any point in the simulation's history, not a
single snapshot that goes stale the moment anything changes around it.

Covered by `test_self_modification_extending_existing_head_preserves_original`,
`test_self_modification_remove_rule_actually_removes`, and
`test_self_modification_preserves_rule_added_after_construction`
(`src/engine/tests.rs`).

**A fourth bug, one level below the `Engine`.** All of the above concerns
the merge into `rule_index`. A separate defect sat underneath it, in
`RuleStore::drain_rule_channel` itself: bytes from *every* output-direction
boundary were collected into one shared accumulator, keyed only by channel
number, before decoding. Two independently self-modifying regions, each
with its own output port, transmitting at overlapping times, would have
their byte streams interleaved into a single corrupted stream — neither
packet decodes correctly, even though each considered alone is
well-formed. This is exactly the scenario Theorem 5 is meant to keep
safe (two modules coexisting, one or both self-modifying), so it is a
direct gap in that guarantee, not a tangential one. Fixed by keying the
accumulator by boundary coordinate instead of channel — each physical
port now has its own independent byte stream and decode state, so
concurrent transmissions on different ports can never corrupt one
another regardless of timing. Covered by
`test_drain_rule_channel_keeps_independent_boundaries_separate`
(`src/rule_store_tests.rs`).

**A fifth bug, once concurrent transmission was possible at all.** Fixing
the fourth bug made it possible for two unrelated self-modification
packets to genuinely complete decoding in the *same* tick, which exposed
a defect in the guard itself: `composition_allows` checked each candidate
rule against `self.rule_index` — the engine's rule set as it stood
*before the current batch of operations began*, since `rule_index` is
only updated once, at the very end of `absorb_self_modifications`. Two
self-installed rules that conflict with *each other* (not with anything
pre-existing) but complete decoding within the same batch would each be
checked against a view that includes neither — both pass, both install,
and the conflict Theorem 5 promises to catch slips through entirely.
Fixed by checking against `RuleStore`'s own current state instead of
`rule_index`: `RuleStore::get_index()` reflects every operation applied
so far, including ones accepted earlier in the very same batch, so the
second of two mutually-conflicting rules now correctly sees the first.
Covered by `test_guarded_self_modification_catches_conflict_within_same_batch`
(`src/engine/tests.rs`).

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

## 6. Spatial Decomposition and Fault Locality

### 6.1. Motivation

Corollary 3 (Section 3.7) parallelizes arbitration by eliminating it:
when the conflict graph is empty, every match is accepted, so there is
nothing left to resolve. The composition theorem (Section 4) extends
this to merging two independently conflict-free rule sets. Neither
result says anything about a rule set whose conflict graph is
*non-empty* — the CA class of Section 3.2, where arbitration is
genuinely required and costs `O(M²)` in the worst case (Theorem 4).

This section shows that arbitration for CA programs can still be
parallelized, by exploiting the same locality property used throughout
this paper: a rule's affected region (Definition 2) extends at most `K`
cells from its match center, where `K` is the same reach bound already
used as the offset range in Algorithm 1. That bound gives two results:
a way to split arbitration itself across independent regions of the
grid (Section 6.2), and a bound on how far a single corrupted cell's
effect can spread over time (Section 6.3).

### 6.2. Spatial Decomposition of Arbitration

**Definition 4 (Reach).** For a rule set `R`, `K = max` over all rules
`Rᵢ ∈ R` of the largest Manhattan distance from a match's center cell to
any cell in `PatternCells(Rᵢ) ∪ Affected(Rᵢ)` (Definition 2). This is the
same `K` used as the offset range in Algorithm 1.

**Definition 5 (Band partition).** Partition the grid into contiguous,
non-overlapping bands `B₁, ..., Bₙ` along one axis. For a match `m` with
center `c`, classify `m` relative to the band `Bᵢ` containing `c`:

- `m` is **core** to `Bᵢ` if the distance from `c` to every edge of
  `Bᵢ` is at least `2K`.
- Otherwise, `m` is **boundary**.

**Lemma 6 (Core isolation).** If `m₁` is core to some band and `m₂` is
either core to a *different* band or boundary, then
`affected(m₁) ∩ affected(m₂) = ∅`.

*Proof.* By Definition 4, `affected(m₁)` lies entirely within Manhattan
distance `K` of `m₁`'s center `c₁`. Since `c₁` is at distance `≥ 2K` from
every edge of its band, `affected(m₁)` — which extends at most `K` from
`c₁` — stays at distance `≥ K` from every edge of that band, and
therefore cannot reach into a neighboring band or into any region within
`K` of an edge. Any `m₂` that is core to a different band or boundary
has its own affected region confined to a different band, or reaching
toward an edge; in either case, by the same radius-`K` argument applied
to `m₂`, the two affected regions are separated by more than `K + K`
distance in the worst case that would bring them together, i.e. they
cannot overlap. ∎

**Theorem 6 (Spatial decomposition correctness).** Let `R` have reach
`K`. Partition the grid into bands with margin `≥ 2K` (Definition 5),
classifying every match as core or boundary. Let:

- `A_core = ⋃ᵢ arbitrate(core matches of Bᵢ)`, with each band's core
  matches arbitrated independently (in parallel, using the same
  deterministic total order of Section 2),
- `A_boundary = arbitrate(all boundary matches)`, arbitrated as one
  shared sequential pass.

Then `A_core ∪ A_boundary = arbitrate(all matches)` computed as a single
centralized pass over the full match set.

*Proof.* The greedy arbitration algorithm (Section 2.2) accepts a match
`m`, in the fixed total order of Lemma 1, if and only if no
already-accepted match with an earlier position in that order shares a
cell with `m` — i.e. has an overlapping affected region.

Take any core match `m`, core to band `Bᵢ`. By Lemma 6, no match outside
`Bᵢ`'s core — whether core to another band or boundary — shares a cell
with `m`. Therefore `m`'s accept/reject outcome under the centralized
algorithm depends *only* on the other core matches of `Bᵢ`, in the same
relative order (the total order of Lemma 1 is fixed and does not depend
on which subset of matches is presented to the algorithm). Running
`arbitrate` on just the core matches of `Bᵢ` therefore reproduces exactly
the centralized decision for every match core to `Bᵢ`. Since this holds
for every band independently, and different bands' core matches never
share cells (Lemma 6), the bands can be arbitrated in parallel: `A_core`
reproduces the centralized decision on every core match.

Take any boundary match `m`. By Lemma 6, no core match shares a cell
with `m` — only other boundary matches can. So `m`'s accept/reject
outcome depends only on other boundary matches, in the same relative
order, and is unaffected by however core matches were resolved. Running
`arbitrate` on the full set of boundary matches therefore reproduces the
centralized decision for every boundary match: `A_boundary` reproduces
the centralized decision on every boundary match.

Core and boundary partition all matches (Definition 5). Since `A_core`
and `A_boundary` each individually reproduce the centralized decision on
their respective subset, their union equals the centralized result on
the full match set. ∎

**Corollary 5 (Parallel arbitration for CA programs).** Unlike
Corollary 3, Theorem 6 places no requirement on the conflict graph of
`R` — it applies to CA programs (non-empty conflict graph) as much as
CF ones. The only requirement is a band margin of at least `2K`, where
`K` depends only on the rule set, not on the grid state.

**Empirical validation.** `examples/decentralized_arbitration.rs`
implements exactly this construction for a rule set with genuine,
unavoidable conflicts (adjacent `R`-movers and `L`-movers, each wanting
to write both of a colliding pair's cells, `K = 1`, margin `= 4 ≥ 2K`).
Centralized arbitration and the banded parallel construction (up to 24
worker threads) are compared directly on every run and found bit-for-bit
identical, matching Theorem 6.

**Remark (relation to domain decomposition).** This is the Cellaria
analogue of halo/ghost-cell exchange in domain-decomposed scientific
computing [8]: a margin of stencil width around each subdomain boundary
lets interior points update independently, with only the halo requiring
communication. Theorem 6 shows the same pattern holds for Cellaria's
arbitration step specifically, including when rules genuinely conflict.

### 6.3. A Bound on Fault Propagation

**Setup.** Suppose at some tick `t₀` a single cell `c₀` is set to an
arbitrary value, independent of what the fault-free execution would
have produced there (a corruption event — e.g. a hardware bit-flip).
All other cells at `t₀` are unaffected. We ask how far, after `t` further
ticks, the set of cells that may now differ from the fault-free run can
extend from `c₀`.

**Theorem 7 (Bounded propagation).** Let `R` have reach `K` (Definition
4). After `t` ticks following a single-cell corruption at `c₀`, every
cell whose value may differ from the fault-free execution lies within
Manhattan distance `2K·t` of `c₀`.

*Proof.* By induction on `t`.

*Base case (`t = 0`).* Only `c₀` differs, at distance `0 ≤ 2K·0`.

*Inductive step.* Assume every divergent cell after `t` ticks lies
within `2Kt` of `c₀`. Consider a cell `d` whose value at tick `t+1`
differs between the corrupted and fault-free runs. Either:

1. `d` was already divergent at tick `t` and no rule wrote to it this
   tick in either run — then `d` is within `2Kt ≤ 2K(t+1)` of `c₀`.
2. The set of matches accepted this tick that affect `d` differs between
   the two runs. This can only happen if some match `m` with `d ∈
   affected(m)` matched differently (fired in one run but not the
   other, or vice versa) — which requires some cell read by `m`'s
   pattern to differ between the runs. By Definition 4, that divergent
   read-cell is within `K` of `m`'s center, which by the induction
   hypothesis is therefore within `2Kt + K` of `c₀`; and `d ∈
   affected(m)` is within a further `K` of `m`'s center. Hence `d` is
   within `2Kt + K + K = 2K(t+1)` of `c₀`.

Both cases keep `d` within `2K(t+1)` of `c₀`. ∎

**Remark (light-cone analogy).** Theorem 7 is Cellaria's analogue of a
Lieb-Robinson bound in quantum lattice systems [7]: a finite local
interaction range implies a finite speed of information propagation,
regardless of the system's global structure.

**Remark (CF and CA alike).** Theorem 7 does not assume the conflict
graph is empty. Arbitration changes *which* matches are accepted, never
*which cells a match can read or write* — every accepted match, whether
selected by Corollary 3's simultaneous-application (CF) or by greedy
arbitration (CA), still only touches cells within `K` of its center. The
propagation bound is therefore a property of the rule set's reach alone,
not of its conflict class. This is a sharper statement than "conflict-free
rule sets contain damage better than conflict-aware ones" — the two
classes differ in arbitration cost (Theorem 4), not in fault locality.

**Empirical validation.** `examples/proof_fault_propagation.rs` runs two
identical Wireworld-style wire simulations (`K = 1`) side by side; at
tick 1, one copy has a single cell externally overwritten (the
corruption event). Comparing the two grids cell-by-cell for 30 ticks,
the maximum distance from the corruption to any differing cell never
exceeds `2Kt`, confirming Theorem 7. The observed spread (the activation
front moves 1 cell/tick in a single direction, since the wire only
propagates one way) is narrower than the bound, which is expected — the
bound is a worst case over all possible rule sets with reach `K`, not a
tight estimate for any particular one.

**Non-result: recovery time.** Theorem 7 bounds how far an error *can*
spread, not how quickly it is *corrected*. Whether — and how fast — a
corrupted cell is overwritten with a correct value depends on the
specific rule set (e.g. whether a cleanup or refresh rule, Axiom 5,
eventually rewrites every reachable cell) and is not a property of the
model in general. We do not claim a universal recovery bound; Theorem 7
is a worst-case containment bound only.

### 6.4. Fault Propagation Under Self-Modification

Theorem 7 assumes a single, fixed reach `K` for the entire span of `t`
ticks. Since [3, self-modification] establishes that a running program
can install new rules at runtime, `K` is not actually fixed in general —
a self-installed rule can have a different reach than anything present
before it. The natural question: does Theorem 7's bound still hold once
the rule set is allowed to change mid-run?

**Theorem 8 (Propagation bound under a changing rule set).** Let
`K_i` denote the reach (Definition 4) of the rule set *active during
tick `i`* — which may differ from tick to tick if rules are installed or
removed between them. After `n` ticks following a single-cell corruption
at `c₀`, every cell whose value may differ from the fault-free execution
lies within Manhattan distance `2·Σᵢ₌₁ⁿ Kᵢ` of `c₀`.

*Proof.* Identical induction to Theorem 7, with the constant `K` in the
inductive step replaced by `K_i`, the reach of whichever rule set is
active at step `i`: the argument only ever needs that matches occurring
*during tick `i`* cannot read or write further than `K_i` from their
center — true regardless of what the rule set looked like on any other
tick. The base case is unchanged (`i = 0`, empty sum, radius `0`). The
inductive step gives `2·Σⱼ₌₁ⁱ⁻¹Kⱍ + 2K_i = 2·Σⱼ₌₁ⁱ Kⱼ`, matching the claim
by induction. Theorem 7 is the special case `K_i = K` for all `i`, where
`Σᵢ₌₁ⁿ Kᵢ = Kn`, recovering `2Kn`. ∎

**Empirical validation.** `examples/proof_fault_propagation_under_selfmod.rs`
runs the same reference-vs-corrupted comparison as Section 6.3's
validation, with `K = 1` for the first 15 ticks and `K = 3` from tick 16
onward (the rule set is changed in both copies, representing a
self-modification event taking effect — the transmission mechanism
itself is Section 4.5's, not re-demonstrated here). The results are
exactly as the two theorems predict: the *naive* bound `2·K_initial·t`
(computed as if `K` had stayed `1` the whole time) is violated starting
at tick 33, once the larger reach has had time to compound; the
*honest* bound `2·Σᵢ Kᵢ` (Theorem 8) is never violated, across all 60
ticks tested.

**Remark.** This does not weaken Theorem 7 for static rule sets — it
generalizes it. The practical implication is for anyone monitoring fault
containment in a self-modifying Cellaria program: the containment radius
must be computed from the *history* of active rule sets, not a single
`K` measured once at startup, or the guarantee silently stops holding
the moment a wider-reaching rule is installed.

---

## 7. Reversibility

### 7.1. Motivation

Every rule set discussed so far *discards* information as a matter of
course: a `changes` write overwrites a cell's old value with a new one,
and nothing records what the old value was. For most programs this is
exactly the intended behavior. But a rule set that never discards
information — where the tick function is a bijection on configurations —
admits an *exact* inverse: not "a plausible earlier state," but the one
and only configuration that could have produced the current one. This
section gives a sufficient condition for this, built entirely from
machinery already established (the conflict graph, Theorem 3).

### 7.2. Local Invertibility

**Definition 6 (Locally invertible rule).** A rule `R` is *locally
invertible* if the map from the values of `R`'s matched pattern cells,
before a match, to the values of `R`'s written cells (shift destinations
and `changes` targets), after applying `R`, is injective: no two distinct
"before" local configurations produce the same "after" local
configuration.

**Definition 7 (Distinguishable rule set).** A rule set `R` is
*distinguishable* if, given the post-tick values of any set of written
cells, one can determine which single rule produced them (e.g. because
each rule's output values lie in a range disjoint from every other
rule's), and cells untouched by any match retain values outside every
rule's output range (so "unchanged" is itself distinguishable from "just
written").

### 7.3. Theorem 9: Global Reversibility

**Theorem 9 (Reversibility).** Let `R` have an empty conflict graph
(Theorem 3's hypothesis), with every rule locally invertible (Definition
6) and `R` distinguishable (Definition 7). Then the tick function
`config_t → config_{t+1}` is a bijection on reachable configurations, and
there exists an *inverse rule set* `R⁻¹` — each rule's local map inverted,
each shift direction reversed — such that applying `R⁻¹` to
`config_{t+1}` exactly reconstructs `config_t`.

*Proof.* By Theorem 3, an empty conflict graph means every match in a
tick is accepted, and the affected regions of distinct matches are
pairwise disjoint. The whole-grid tick transformation therefore
decomposes into independent local transformations, one per accepted
match, applied to disjoint regions, plus the identity on every untouched
cell.

Each local transformation is injective by hypothesis (Definition 6). A
function assembled from injective pieces on pairwise-disjoint domains,
plus the identity elsewhere, is injective overall — *provided* one can
tell, from the output alone, which piece (if any) produced each cell's
new value; this is exactly Definition 7. Hence the tick function is
injective on the space of configurations: no two distinct `config_t`
can produce the same `config_{t+1}`.

Reconstruction of `config_t` from `config_{t+1}` follows directly:
for a cell whose value lies in some rule `Rᵢ`'s output range, apply
`Rᵢ`'s local inverse (well-defined by Definition 6) to recover its
pre-tick value; for a cell whose value lies outside every rule's output
range, it was untouched, so its current value already equals its
pre-tick value. `R⁻¹`, built from these per-rule local inverses with
shift directions reversed, performs exactly this reconstruction as an
ordinary Cellaria tick. ∎

**Empirical validation.** `examples/proof_reversibility.rs` builds a rule
set combining two independent sources of information loss in typical
programs — `changes` (a 6-state cycle, a permutation of
`{10,...,15}`, hence self-evidently a bijection with a well-defined
inverse permutation) and `shifts` (a token moving right, inverted by
moving left) — runs it forward 20 ticks from a hand-built configuration,
builds `R⁻¹` by literally reversing each rule's arrow, and runs it
backward 20 ticks from the final configuration. The recovered grid
matches the original **cell for cell, across the entire grid**, not just
at the positions that changed.

### 7.4. Corollary: A Physical Lower Bound, Honestly Scoped

**Corollary 6 (Landauer's principle).** Landauer's principle [9] states
that erasing one bit of information is necessarily accompanied by a
minimum energy dissipation of `kT ln 2` in *any* physical implementation,
while a computation that never erases information — a bijection, exactly
Theorem 9's hypothesis — carries no such lower bound. Bennett [10] showed
that reversible computation is not merely a curiosity but can, in
principle, be implemented directly in physical hardware (reversible
logic gates, e.g. the Fredkin and Toffoli gates).

**Scope.** This is a statement about a hypothetical *physical*
implementation, not a measurable property of the current Rust simulation
running on ordinary silicon — the same honest caveat that applied to the
energy-efficiency question raised and set aside earlier in this project.
The difference here is that the claim now rests on a well-established
physical principle applied to a rule set we have *proven* satisfies its
precondition (Theorem 9), rather than an unsupported analogy between
"operations per tick" and energy. It identifies *which part* of Cellaria
— the reversible subclass — would be a meaningful target for reversible
hardware research, without claiming anything about energy consumption
today.

---

## 8. Related Work

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

**Domain decomposition.** Splitting a computation across independent
spatial regions with a shared halo of ghost cells is standard practice
in distributed scientific computing [8]. Theorem 6 shows the same
pattern applies to Cellaria's arbitration step, including when the rule
set has genuine conflicts — the "halo" here is the set of boundary
matches requiring shared sequential arbitration, sized by the rule set's
reach `K` rather than by a fixed physical stencil.

**Bounded propagation speed.** Lieb-Robinson bounds [7] establish that
local interactions in quantum lattice systems propagate information at a
finite speed. Theorem 7 is a discrete, combinatorial counterpart for
Cellaria: a rule set's reach `K` bounds how fast a local perturbation can
affect distant cells, independent of the arbitration mechanism.

---

## 9. Open Questions

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

7. Lieb, E. H., & Robinson, D. W. (1972). The finite group velocity of
   quantum spin systems. *Communications in Mathematical Physics*,
   28(3), 251–257.

8. Gropp, W., Lusk, E., & Skjellum, A. (1999). *Using MPI: Portable
   Parallel Programming with the Message-Passing Interface* (2nd ed.).
   MIT Press.

9. Landauer, R. (1961). Irreversibility and heat generation in the
   computing process. *IBM Journal of Research and Development*, 5(3),
   183–191.

10. Bennett, C. H. (1973). Logical reversibility of computation. *IBM
    Journal of Research and Development*, 17(6), 525–532.