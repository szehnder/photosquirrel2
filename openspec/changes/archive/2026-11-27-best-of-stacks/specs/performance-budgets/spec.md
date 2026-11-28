## ADDED Requirements

### Requirement: Stacks budgets
Stacks SHALL have three budgets: `stacks-compute`, the time to compute every stack and pick on a catalog of 1M files; `stacks-retune`, the time to recompute them after a threshold change on 100,000 photos; and `shot-quality-fill`, the time to fill in the shot-quality scores of 100,000 photos from stored thumbnails. All three SHALL run off the interface thread.

#### Scenario: Stacks computed within budget
- **WHEN** the stacks benchmark computes stacks on a generated 1M-file catalog
- **THEN** the time is reported as the `stacks-compute` measurement and is within its budget

#### Scenario: Retune within budget
- **WHEN** the benchmark changes the similarity on 100,000 photos
- **THEN** the time to the new stacks is reported as the `stacks-retune` measurement and is within its budget

#### Scenario: Shot quality filled within budget
- **WHEN** the benchmark fills in shot quality for 100,000 stored thumbnails
- **THEN** the time is reported as the `shot-quality-fill` measurement and is within its budget
