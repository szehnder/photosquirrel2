# Design

## Context

- `ps-media::poster::poster_frame(tool, source, seek, timeout, stop)` runs ffmpeg once and returns a JPEG scaled to
  384 px on the long edge. `thumb::poster_thumbnail` wraps it with the 0 s retry (ADR-0120) and makes the
  thumbnail record. A scan calls these under its per-run `PosterSlots` (2 slots, 1 on rotational or network
  sources).
- `ps-service::Library::preview(id)` serves the loupe and the inspector. For a video it returns
  `Preview::Unavailable("video")` at once. For a still it reads the file, makes a 2048 px JPEG and caches it in the
  `PreviewCache` under `preview_cache_key(identity)`.
- The loupe (`ui-gallery` `loupe.rs`) shows `Preview::Unavailable(why)` as the grid thumbnail with a "Large preview
  unavailable (why)" chip. The placeholder painter it uses when there is no thumbnail already knows videos
  (`s.video`, duration, capture date).
- `ui-workspace` has an `Opener` trait (`reveal(path)`), a platform `SystemOpener` and a `RecordingOpener` for
  tests.

## Goals / Non-Goals

**Goals:**
- A video in the loupe looks as finished as a photo: a sharp frame, marked as a video, one action to watch it.
- One rule for poster-frame failures everywhere: the large preview reports the same reasons as the thumbnail
  (ADR-0114, ADR-0120).

**Non-Goals:**
- Playback inside the app (ADR-0134).
- Recording a failed large preview in the catalog. Only scans record `poster_failed`; a failed preview is just
  not cached.

## Decisions

### D1. `poster_frame` takes the long edge

Change `poster_frame` and `poster_thumbnail` to take a `long_edge: u32` (`THUMB_LONG_EDGE` = 384 for scans,
`PREVIEW_LONG_EDGE` = 2048 for previews). The ffmpeg filter becomes
`scale='if(gt(iw,ih),min({e},iw),-2)':'if(gt(iw,ih),-2,min({e},ih))'`, which never upscales and keeps even
dimensions. Add `thumb::poster_preview(tool, path, duration_ms, timeout, stop) -> Result<Vec<u8>, PosterError>`
with the same 0 s retry, returning the JPEG as ffmpeg made it (sRGB; BT.709 frames are already display-referred,
as for thumbnails).

### D2. `Library::preview` handles videos

For a video:
1. Look in the preview cache first, with the same key rule as a photo.
2. Without a poster tool (the decoder registry's `poster_frames` entry, as the scan uses), return
   `Preview::Unavailable` with the placeholder reason text for "needs a poster-frame tool".
3. Otherwise, if the source is offline, return `Preview::Offline`.
4. Otherwise take the process-wide preview poster gate (a `Mutex<()>`, so one call at a time), call
   `poster_preview` with the scan's timeout, cache the JPEG and return it.
5. On `PosterError`, return `Unavailable` with the same reason text a scan would record ("poster frame failed:
   <error>"). Nothing is written to the catalog.

The `stop` closure is the library's cancellation flag, so closing the app kills the child.

### D3. The loupe marks videos and offers Open in player

When the item is a video, the loupe paints a play glyph and the duration chip in the frame's corner (tokens,
ADR-0028) over the large preview, or over the placeholder when the preview is unavailable. For a video it never
paints the "Large preview unavailable" chip. The placeholder already says why. It adds an **Open in player**
button to the loupe's top bar, and the `OpenInPlayer` action is bound to `o` in the loupe and gallery contexts.

### D4. `Opener::open` (ADR-0134)

Add `fn open(&self, path: &Path) -> anyhow::Result<()>` to `Opener`. `SystemOpener` runs `xdg-open <path>` on
Linux, `open <path>` on macOS and `explorer.exe <path>` on Windows, each with the path as one argument, with no
shell and without waiting. `RecordingOpener` records it. The inspector's copy rows gain **Open in player** for
video copies, next to **Reveal**: enabled for online sources, disabled with "Source offline" otherwise. The loupe
opens the item's first online copy.

## Risks / Trade-offs

- [ffmpeg at 2048 px on a 4K HEVC clip can take a second or more] → The preview loads on the background executor,
  and the loupe shows the grid thumbnail meanwhile, as for a large TIFF. The result is cached.
- [`xdg-open` is missing on a minimal Linux desktop] → `open` returns the spawn error, and the UI shows it as a
  toast ("No program to open videos was found").
- [A player that can edit files] → ADR-0134: it is the user's action in another app, as with Reveal.

## Migration Plan

None. Previously "unavailable" video previews were never cached, so nothing stale remains.

## Verification

```
CARGO_BUILD_JOBS=4 TMPDIR=$PWD/target/tmp cargo test -p ps-media -- --test-threads=4 poster
CARGO_BUILD_JOBS=4 TMPDIR=$PWD/target/tmp cargo test -p ps-service -- --test-threads=4 \
  scenario_thumbnails_large_preview_of_a_mov scenario_thumbnails_large_preview_without_the_tool \
  scenario_thumbnails_large_preview_when_the_tool_fails
CARGO_BUILD_JOBS=4 TMPDIR=$PWD/target/tmp cargo test -p ui-workspace -- --test-threads=4 \
  scenario_gallery_video_ scenario_gallery_open_in_player_from_the_loupe scenario_photo_inspector_
cargo xtask screenshot --all-surfaces    # a new `loupe-video` surface
cargo xtask ci
openspec validate video-previews-and-play --strict
```
Manual (docs/manual-checks): Open in player on Linux, macOS and Windows opens the default player.
