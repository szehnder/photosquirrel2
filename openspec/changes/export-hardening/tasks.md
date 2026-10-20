# Tasks

## 1. Detection and refusal

- [x] 1.1 `walk::is_export_library` and `walk::find_export_library` (root, ancestors, 4 levels below); `Scanner::add_source` and `relink_source` refuse with `ScanError::ExportLibrary`; verify `scenario_export_library_from_another_catalog_refused_as_a_source`
- [x] 1.2 The walk skips a nested export library; verify `scenario_export_library_from_another_catalog_refused_as_a_source` (deep library part)
- [x] 1.3 Marker names equal ps-export's constants; verify the assertions at the start of `scenario_export_library_from_another_catalog_refused_as_a_source`

## 2. Archive

- [ ] 2.1 `cargo xtask ci` green; archive the change
