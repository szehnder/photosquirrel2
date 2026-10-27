# Spec Delta

## ADDED Requirements

### Requirement: Sources record whether file ids are stable
When a source is added, re-linked or scanned, the application SHALL record from the operating system whether its filesystem keeps file ids across mounts (not on FAT or exFAT; unknown where it cannot tell), and `sources list` SHALL show it. A quarantine move SHALL NOT compare file ids on a filesystem that does not keep them.

#### Scenario: exFAT source shows unstable file ids
- **WHEN** a source is added on a filesystem the probe reports as exFAT and `sources list --json` is run
- **THEN** the source has `stable_file_ids` false

#### Scenario: Quarantine after a remount is not refused for a new inode
- **WHEN** a file on a filesystem without stable file ids is planned for quarantine and its inode number differs from the catalog's while its size, time and content are unchanged
- **THEN** the move goes ahead
