## 1. Workspace
- [x] 1.1 Workspace manifest, toolchain pin, lints and profiles (verify: `cargo metadata` succeeds)
- [x] 1.2 Crate skeletons per ADR-0003 (verify: `cargo build --workspace`)

## 2. Tooling
- [x] 2.1 xtask adapted: ci, doctor, layering, tokens, spec-coverage, bench, screenshot, package (verify: `cargo xtask layering`, `cargo xtask tokens`)
- [x] 2.2 `perf-budgets.toml` scaffold (verify: `cargo xtask bench --list`)

## 3. Docs and CI
- [x] 3.1 ADRs 0001-0032 with index (verify: `docs/adr/README.md` lists every file)
- [x] 3.2 AGENTS.md / CLAUDE.md (verify: file review)
- [x] 3.3 GitHub Actions CI and release workflows (verify: workflow lint on first push)

## 4. Verification
- [x] 4.1 `cargo xtask ci` passes
- [x] 4.2 `openspec validate bootstrap-workspace --strict` passes
