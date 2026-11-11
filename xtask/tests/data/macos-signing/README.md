Outputs of Apple's signing tools that `xtask/src/macos_signing.rs` parses (ADR-0128): `codesign -dvv`,
`codesign --verify --strict --deep -vv`, `spctl --assess -vv` and `xcrun stapler validate`.

The accepted cases (`codesign-dvv-app`, `-cli`, `-dmg`, `codesign-verify-app`, `spctl-app`, `spctl-dmg`,
`stapler-app`, `stapler-dmg`) are copied from the `verify-macos` job of release run 42707503988 (2026-11-09, macOS
15 runner). The refused cases (`codesign-dvv-adhoc`, `spctl-rejected`, `stapler-missing`) are written by hand in
the same formats.
