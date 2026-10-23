# Spec Delta

## ADDED Requirements

### Requirement: Toolbar wraps without stray dividers
When the Library toolbar's filters wrap onto more lines, no separator SHALL appear at the start or end of a line or on a line of its own; filter groups SHALL stay visually separated and every control SHALL stay inside the toolbar.

#### Scenario: Toolbar has no stray divider
- **WHEN** the Library is laid out at window widths from 960 px to 1920 px in 20 px steps, with the inspector closed and open
- **THEN** the toolbar contains no free-standing separator element, the gap between filter groups is wider than the gap within a group, and every filter control lies inside the toolbar
