Outputs of Apple's signing tools that `xtask/src/macos_signing.rs` parses (ADR-0128). They follow the formats of
`codesign -dvv`, `codesign --verify --strict --deep -vv`, `spctl --assess -vv` and `xcrun stapler validate`. Task
3.4 of `openspec/changes/macos-signing` replaces the hand-written ones with the outputs of the first signed run of
the release workflow.
