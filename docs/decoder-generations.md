# Decoder generations

Every placeholder (a file catalogued without a preview) records the **decoder generation** of its format's family
that made it ([ADR-0109](adr/0109-placeholders-carry-a-reason-and-a-decoder-generation.md)). When the running
version's generation of that family is newer, the placeholder counts as waiting: the Library offers **Fill in
previews**, `photosquirrel-cli previews fill` fills it in, and the next rescan of its source decodes it again, once.

## When to bump

Bump a family's generation in `ps_media::decoders::GENERATIONS` (and in the `decoder_generations_of_this_build`
test beside it) in **any change that makes files of that family decodable that were not before**:

- a new layout, compression, colour model, bit depth or sample format (a TIFF predictor, 16-bit PNG, CMYK JPEG);
- a lifted size or memory limit (files that got "files this large" placeholders);
- a new or newly bundled decoder (the AV1 decoder for AVIF, the system HEIC decoder);
- a probe fix that finds dimensions or data an earlier version missed (TIFF directories beyond the probe window);
- a change that corrects a family's placeholder reason, because the re-read writes the corrected reason
  ([ADR-0115](adr/0115-a-raw-file-cut-short-is-damaged-and-damaged-files-are-counted.md): a RAW file cut short was
  "not supported" and is "could not be read" now).

Do not bump for speed-ups, quality changes or fixes to files that already decoded: every placeholder of the family
is read again once after a bump.

## Families and generations

| Family | Formats | Generation | Since |
|---|---|---|---|
| JPEG | JPEG | 0 | |
| PNG | PNG, GIF, BMP | 0 | |
| TIFF | TIFF | 1 | `tiff-support` (0.3.0) |
| HEIF | HEIC, HEIF | 1 | `heic-system-decoders` (0.3.0) |
| AVIF | AVIF | 1 | `heic-system-decoders` (0.3.0) |
| WebP | WebP | 0 | |
| RAW | RAW embedded previews | 1 | `damaged-raw-reason` (0.3.x): cut-short RAWs are damaged, unknown cameras named |
| Video poster | every video format | 0 | |

Videos waiting for a poster-frame tool do not need a bump: they wait for the tool itself (the tool clause,
[ADR-0114](adr/0114-videos-wait-for-a-poster-frame-tool-and-failed-posters-are-recorded.md)), so installing ffmpeg
makes them fillable. Bump the video poster generation for a change that gets frames from videos ffmpeg could not
read before (a new seek rule, other tool arguments): the bump retries every "poster frame failed" video once, when a
tool is available.

Add a row here, with the change that bumped it, whenever a generation changes. A forgotten bump loses nothing: the
old placeholders simply wait until their file changes.
