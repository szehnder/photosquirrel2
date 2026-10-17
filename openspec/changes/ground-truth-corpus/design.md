# Design

## Context

ADR-0025 defines the corpus and its gate; ADR-0013/0014 define what the truth must encode (groups, must-not-merge
sets, keepers); ADR-0015 defines expected classes. `ps-fixtures` is a core crate (ADR-0003), so generation is
pure computation plus writes into a caller-chosen output folder via a small writer trait; the CLI provides the
file-system writer. ADR-0027 lists `fixtures` as a CLI subcommand.

## Goals / Non-Goals

**Goals:**
- Byte-for-byte determinism across OSes (seeded `rand_chacha`, no `HashMap` iteration order, fixed mtimes).
- Realistic enough that the classifier, hashes and ranking behave as on real photos.
- Fast: the small preset in < 30 s so it can run in every `cargo test`.

**Non-Goals:**
- Photorealism; real camera RAW variety; HEIC generation.

## Decisions

- **Image synthesis (`ps-fixtures::synth`).** Each original is a seeded composition: sky/ground gradient,
  2-6 soft shapes, value-noise texture at 3 octaves and film-grain noise, rendered at 2-4 MP for `small`/`medium`
  and 12 MP for the 12 MP bench subset. Encoded with the `image` JPEG encoder at quality 92-95 and a camera-like
  EXIF/maker-note block written by `ps-fixtures::exif` (minimal TIFF IFD writer: Make, Model, BodySerialNumber,
  DateTimeOriginal, SubSecTimeOriginal, ExposureTime, FNumber, ISO, FocalLength, GPS IFD, and an opaque
  MakerNote blob).
- **Derivatives.** Resized via `fast_image_resize` then re-encoded; recompressions re-encode the decoded original;
  EXIF-stripped copies drop the APP1 segment; renamed copies use `IMG_1234 (1).jpg`, `DSC_...`, `copy of ...`
  patterns. RAW+JPEG pairs: a minimal DNG (`ps-fixtures::dng`) carrying the full-size JPEG as its preview plus
  a tiny fake sensor strip.
- **Must-not-merge.** Crops (10-30% removed, aspect change > 2%); bursts: same scene re-rendered with small shape
  offsets and timestamps 0.3-5 s apart; same scene minutes apart; lookalikes from a different camera model/serial.
  Truth marks each set's originals; which of them merge under ADR-0013's rules is *not* assumed by the generator.
- **Videos.** Minimal ISO-BMFF writer (`ftyp`, `moov/mvhd/tkhd/mdhd/udta` with `©mak`, `©mod`, creation time)
  and a zero-filled `mdat`; enough for metadata and classification, not decoding.
- **Layout.** `drive-a/`, `drive-b/`, `drive-c/` roots; nested `Backup YYYY/Old Laptop/Pictures/...`, album
  folders, `.thumbnails/`, `Lightroom/Catalog Previews.lrdata/`, `AppData/` paths. mtimes are set from the seed
  with `File::set_modified`.
- **Truth (`truth.json`, versioned `truth_version: 1`).** `{ seed, preset, files: [{ path, drive, category,
  expected_class, group, keeper, keeper_set }], must_not_merge: [{ kind, groups: [...] }] }`, sorted by path.
- **Evaluator (`ps-fixtures::eval`).** Input: `Vec<ResultGroup { members, keeper }>` keyed by path; pairwise
  precision/recall over same-group pairs excluding junk; keeper accuracy over truth groups with >= 2 members;
  violations = result groups containing originals of two different truth groups from one must-not-merge set.
  Returns a serialisable `Score` reused by `duplicate-groups` and `headless-cli`.
- **CLI.** `photosquirrel-cli fixtures generate --seed <u64> --preset <small|medium|large> --out <dir> [--json]`.
  This change adds only this subcommand and the clap skeleton; `headless-cli` builds the rest.

## Risks / Trade-offs

- Bursts (ADR-0035): include frames 0.3-2 s apart, same-second frames with different sub-second times, and same-second frames without sub-second times but with maker notes and equal dimensions; all must stay apart.

  change's design.
- [Encoder output differs across `image` versions] -> determinism is pinned by `Cargo.lock`; the
  same-seed test compares within one build.

## Verification

- `cargo test -p ps-fixtures` (all `scenario_test_corpus_*` except the CLI ones).
- `cargo test -p photosquirrel-cli fixtures` (CLI scenarios).
- `cargo run -p photosquirrel-cli -- fixtures generate --seed 42 --preset small --out /tmp/ps-corpus && jq '.files | length' /tmp/ps-corpus/truth.json`.
- `cargo xtask ci`, `openspec validate ground-truth-corpus --strict`.
