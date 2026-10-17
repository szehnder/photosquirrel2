# Tasks

## 1. Open and migrate

- [ ] 1.1 Data-directory resolution with `PHOTOSQUIRREL_DATA_DIR` override, pragmas and WAL; verify `scenario_catalog_catalog_created_in_data_directory`
- [ ] 1.2 Embedded numbered migrations with schema v1; verify `scenario_catalog_fresh_catalog_migrated_to_latest`
- [ ] 1.3 Upgrade path and newer-version refusal (add a no-op migration 2 to exercise upgrade); verify `scenario_catalog_old_catalog_upgraded` and `scenario_catalog_newer_catalog_refused`

## 2. Writer

- [ ] 2.1 Writer thread with batched commands and error/flush acks; verify `scenario_catalog_batch_flushes_at_2000_rows` and `scenario_catalog_batch_flushes_after_100_ms`
- [ ] 2.2 Stage-with-results commands in one transaction; verify `scenario_catalog_killed_writer_leaves_consistent_state` (child-process kill test)
- [ ] 2.3 File upsert keyed by (source, relative path); verify `scenario_catalog_same_path_upsert_is_idempotent`

## 3. Queries

- [ ] 3.1 Typed `Filter`/`Sort` model and ID-list query with 16-byte sort keys; verify `scenario_catalog_full_id_list_for_filter`
- [ ] 3.2 Keyset detail pages; verify `scenario_catalog_keyset_page_of_512`
- [ ] 3.3 Read-only pool; verify `scenario_catalog_reads_during_writes`

## 4. Benchmarks

- [ ] 4.1 Insert bench and `perf-budgets.toml` entry `catalog-insert` expressed as time per row (max 20 µs/row = 50k rows/s, because budgets are upper bounds); verify `scenario_catalog_insert_50k_rows_per_second`
- [ ] 4.2 ID-list bench and entry `catalog-id-list-1m` (max 300 ms); verify `scenario_catalog_id_list_for_1m_within_300_ms`

## 5. Verification

- [ ] 5.1 `cargo xtask ci` is green
- [ ] 5.2 Gate: `cargo xtask bench --check` passes the two catalog budgets; migration tests pass
- [ ] 5.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [ ] 5.4 `openspec validate catalog --strict` passes
