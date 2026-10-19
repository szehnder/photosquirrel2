# 0039. Contradictions are group-level cannot-link constraints

- Status: accepted
- Date: 2026-10-19
- Deciders: maintainers

## Context
ADR-0013 and ADR-0035 test metadata contradictions (capture second, sub-second, camera serial, camera model,
two camera originals) only for the two ends of a hash-matching pair. A metadata-free copy (an EXIF-stripped
thumbnail) has no contradiction with anyone, so it can bridge two burst frames A and C: A-B and B-C are both
derivative edges and union-find joins {A, B, C}. Export then keeps one keeper of that group and drops a burst
frame, contradicting "bursts are suggestions only" and the owner's first priority. The M1 gate review found
this. The same bridging let a file that ended up in no group shape another group through the 60 s chain guard,
which broke "incremental regroup equals full regroup" (an ungrouped removed file left no trace to recompute).

## Decision
Grouping treats every ADR-0013/0035 metadata contradiction as a **cannot-link constraint between the two
files, enforced for the whole group**, like the user separations of ADR-0033:
- Union-find runs Kruskal-style: exact copies first, then derivative edges strongest first (smallest
  pHash + dHash distance, deterministic file-id tie-break). A union is refused when the merged component would
  contain any pair of members whose metadata contradicts. A metadata-free member therefore joins its strongest
  compatible partner only.
- The two-camera-originals contradiction is waived between the contents of a RAW+JPEG capture pair
  (ADR-0037), wherever copies of either half live.
- Camera models compare normalized: case-insensitive, whitespace collapsed, a leading make word stripped
  ("Canon EOS 70D" = "EOS 70D").
- User merges (ADR-0033) are explicit and still join whole groups; separations still win over merges.
The chain guard of ADR-0013 stays as a backstop; with these constraints a group's timed members share one
capture second, so it no longer cuts groups and a file in no group never influences another group.

The ground-truth corpus includes EXIF-stripped thumbnails of burst frames. Their truth lists the sibling frames
as `alternate_groups`; scoring accepts such a file with any one candidate frame and counts a group joining two
frames as a must-not-merge violation.

## Alternatives considered
- **Pairwise checks only (status quo)** - merges bursts through stripped thumbnails.
- **Drop metadata-free files from grouping** - loses the common "thumbnail of a camera original" duplicate.
- **Split bad components afterwards** - order-dependent repair with no clear rule for where the bridge goes.

## Consequences
- Precision on the corpus is unchanged at 1.0 and no must-not-merge violations remain, including the new
  ambiguous-thumbnail cases (they produced 2 violations before).
- An ambiguous thumbnail may be grouped with the "wrong" frame of a burst; that is harmless (it is a
  derivative, never a keeper over a camera original) and explained in the UI.
- Union cost grows with the number of distinct metadata-carrying members per component (deduplicated by content
  and metadata); groups are small in practice.

## Revisit when
Users show legitimate derivatives whose capture metadata differs from the original (time-shifted exports); then
add offset-aware matching (ADR-0035 "Revisit when").
