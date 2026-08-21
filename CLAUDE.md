## Comments and docstrings

**Default to no comment.** Most code needs none — assume an experienced reader. Signatures and
type names are the documentation.

When a comment does earn its place:

- **Single line, and only the *why*.** Never restate what the code does. The cases worth a
  comment are non-obvious constraints and rejected alternatives — a threshold chosen for
  hysteresis rather than the break-even point, a bound that must stay conservative, an
  invariant a caller has to uphold.
- **Never document changes.** No "changed from A to B", "now uses B", "previously did A", and no
  commented-out old versions. The code is the current state; its history is git's job.
- **When touching existing comments, consider shortening or deleting them.** A comment that has
  drifted from the code, restates it, or narrates an old edit should go — removing it is a real
  improvement, not a side task.
