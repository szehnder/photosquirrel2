# Tasks

## 1. Packaging command

- [x] 1.1 Confirm formats per OS and archive generation; verify `scenario_release_packaging_package_formats_per_platform` (and `cargo xtask package --docker` locally: AppImage, .deb, .tar.gz)
- [x] 1.2 Packager metadata with app and CLI binaries, icons and desktop file; verify `scenario_release_packaging_packages_contain_the_app_and_the_cli`
- [x] 1.3 Artifact naming and checksums; verify `scenario_release_packaging_artifact_names_include_version_os_and_architecture` and `scenario_release_packaging_every_artifact_has_a_checksum`
- [x] 1.4 Tag check and packaged CLI smoke test; verify `scenario_release_packaging_mismatched_tag_is_rejected` and `scenario_release_packaging_wrong_binary_fails_the_smoke_test`
- [x] 1.5 Debian dependencies and Linux image; verify `scenario_release_packaging_debian_package_declares_runtime_libraries` and `scenario_release_packaging_linux_build_image_pins_glibc_floor`
- [x] 1.6 libheif detection and HEIC fallback note; verify `scenario_release_packaging_heic_fallback_noted` (ADR-0059: HEIC only with libheif >= 1.17 and not in the universal macOS build; the release images install none, so v0.1 packages are HEIC-off and the notes say so per platform)
- [ ] 1.7 Third-party licence notices in every installer and archive: generate a `THIRD-PARTY-LICENSES` file from `Cargo.lock` (e.g. `cargo about generate` with an `about.toml` accepting the workspace licences), bundled next to the binaries and in the macOS app bundle. Binary distributions must acknowledge mozjpeg-sys (`IJG AND Zlib AND BSD-3-Clause`: the IJG README clause and the libjpeg-turbo BSD/Zlib notices) and every other BSD/MIT/Apache crate; verify each package from 1.1 contains the file and it names `mozjpeg-sys` with all three licences

## 2. Release workflow

- [x] 2.1 Clean template leftovers from `release.yml`, `Dockerfile` and `release-notes.md`; verify `grep -ri gremlin .github packaging` finds nothing
- [x] 2.2 Workflow structure test; verify `scenario_release_packaging_release_workflow_covers_every_platform`
- [ ] 2.3 Dry run on a fork with tag `v0.1.0-rc.1`; verify the release lists six packages and six `.sha256` files

## 3. Documentation

- [x] 3.1 Generate screenshots with `cargo xtask screenshot --all-surfaces` on the synthetic corpus and copy the chosen ones to `docs/screenshots/`; verify the files exist (demo library, dark theme, all 14 captures reviewed: library -> `gallery-grid-dark.png`, sources -> `scan-dashboard-dark.png`, duplicates -> `duplicates-group-dark.png`, `export-dark.png`, `reports-dark.png`, `ai-review-dark.png`)
- [x] 3.2 Write `README.md`; verify `scenario_release_packaging_readme_screenshots_exist`

## 4. Verification

- [ ] 4.1 `cargo xtask ci` is green
- [ ] 4.2 Gate: installers for every platform attached to a GitHub Release; README with screenshots on the default branch
- [ ] 4.3 `cargo xtask spec-coverage` reports every scenario of this change mapped
- [ ] 4.4 `openspec validate release-packaging --strict` passes
