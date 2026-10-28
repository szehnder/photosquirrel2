# Spec Delta

## ADDED Requirements

### Requirement: Third-party decoder diagnostics hidden by default
Unless `RUST_LOG` is set, the CLI and the application SHALL log warnings and errors of PhotoSquirrel's own crates and SHALL hide warnings of the third-party EXIF and RAW decoders (nom-exif, rawler), showing only their errors. When `RUST_LOG` is set it SHALL replace this default.

#### Scenario: Lens database warnings are hidden by default
- **WHEN** a folder with a Canon RAW fixture whose lens is not in rawler's lens table is scanned with `RUST_LOG` unset
- **THEN** standard error contains no "No lens definition found" line and the scan exits with 0

#### Scenario: RUST_LOG shows them again
- **WHEN** the same folder is scanned with `RUST_LOG=rawler=warn`
- **THEN** standard error contains the "No lens definition found" warning

#### Scenario: The default filter keeps errors and our own warnings
- **WHEN** the default log filter is evaluated
- **THEN** rawler and nom-exif warnings are disabled, their errors are enabled, and warnings of `ps_scan` are enabled
