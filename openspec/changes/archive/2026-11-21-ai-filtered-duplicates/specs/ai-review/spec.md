# Spec Delta

## ADDED Requirements

### Requirement: Open matching duplicates
The AI Review view SHALL offer "Review duplicates of these", which opens the Duplicates view with the AI Review view's current insight filter applied to the group list.

#### Scenario: Review duplicates of filtered photos
- **WHEN** the AI Review view is filtered by "contains pets" and the user chooses "Review duplicates of these"
- **THEN** the Duplicates view opens filtered by "contains pets" and lists exactly the groups that contain pets
