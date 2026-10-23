# 0077. The release attaches the AppImage runtime's relinking sources, all byte-stable (amends 0073)

- Status: accepted
- Date: 2026-10-23
- Deciders: maintainers
- Amends: [0073](0073-pinned-appimage-runtime-with-its-own-notice.md)

## Context
ADR-0073 attaches the libfuse 3.15.0 release and the patch the runtime applies to it. LGPL-2.1 (section 6) also
lets a user relink the work that uses libfuse against a modified copy, which needs the rest of that work: the
type2-runtime program at commit `dd6cebe` (MIT), which links libfuse statically, and squashfuse 0.5.2
(BSD-2-Clause), linked with it. ADR-0073 only linked to them. Every attached file is checked against a SHA-256, so
it must be byte-stable. GitHub's generated archives (`/archive/<tag>.tar.gz`) are not promised to be: their bytes
have changed before when GitHub changed its compressor. type2-runtime publishes no source archive at all, only
binaries; squashfuse publishes a release file for each tag.

## Decision
- `[[source]]` entries name their component (`name`, `version`) and are either a **download** of an upstream
  release file (`url`, `sha256`) or a **git archive** (`git`, `commit`, `tree`, `sha256`, an uncompressed `.tar`).
- squashfuse is the release file `squashfuse-0.5.2.tar.gz` of tag 0.5.2 (its `make dist`, byte-stable once
  uploaded). type2-runtime built from GitHub's archive of the same tag, which holds the same sources without the
  generated build files.
- type2-runtime is `type2-runtime-dd6cebe.tar`: `cargo xtask runtime-sources` fetches the pinned commit by hash,
  fails unless its tree hash is the pinned tree (the content check, independent of any tar writer), and runs
  `git archive --format=tar --prefix=type2-runtime-dd6cebe/` with `core.autocrlf=false` and `tar.umask=0002`.
  The tar's mtimes come from the commit, so the bytes depend only on the tree, the commit and git's tar writer;
  the SHA-256 pin checks that writer too. If a git release ever changes its tar layout, the tree check still passes
  and the error says so: review, then update `sha256`.
- `cargo xtask runtime-sources --out <dir>` makes every `[[source]]` and checks it. The release workflow runs it in
  its own read-only job and uploads the files; the publishing job, which holds the only write token, compiles
  nothing: it downloads them, checks each against the pin again and attaches them. `THIRD-PARTY-LICENSES` names
  them under "To relink the runtime against a modified libfuse", and the release notes and README list them.

## Alternatives considered
- **GitHub's tag archives** - what type2-runtime itself pins for squashfuse, but GitHub does not promise their bytes.
- **Compress the git archive** - xz and gzip output depends on the compressor's version and settings; the
  uncompressed tar of its ~85 KB of files is 120 KB.
- **Vendor the sources in this repository** - stable, but third-party trees in git for a file the release can make.

## Consequences
- The release carries two more files (about 560 KB) and a short xtask job.
- A pin change to the runtime now also updates the type2-runtime commit, tree and tar hash, and the squashfuse
  release if its version changes; `RuntimePin::parse` refuses a source without a component or a git source without
  full hashes.

## Revisit when
type2-runtime publishes source archives, squashfuse stops publishing release files, or git changes its tar output.
