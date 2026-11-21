# Tasks

Patch release 0.5.1, with `btrfs-volume-identity`. No catalog migration, no new dependency. ADR-0134 is written
`proposed` with this change. Tests use the recorded or failing fake poster tools of the existing poster-frame tests
and the `RecordingOpener`; the real-ffmpeg test runs where ffmpeg is installed, as today.

## 1. Decisions

- [ ] 1.1 Owner: review ADR-0134, then set it to `accepted` and drop "(proposed)" from its index row. Verify: the
  file reads `Status: accepted`.

## 2. Large previews of videos

- [ ] 2.1 `poster_frame` and `poster_thumbnail` take a long edge, and add `poster_preview` (design D1). Verify:
  `ps-media` poster tests pass with 384, and a real-ffmpeg test on a 1920x1080 clip returns 1920x1080 at 2048 and
  never upscales a 640x360 clip.
- [ ] 2.2 `Library::preview` handles videos: cache, tool check, offline, one call at a time, reasons (design D2).
  Verify: `scenario_thumbnails_large_preview_of_a_mov` (the second request does not call the tool, counted by the
  fake tool), `scenario_thumbnails_large_preview_without_the_tool` and
  `scenario_thumbnails_large_preview_when_the_tool_fails` pass, and a test shows two concurrent requests run the
  tool one after the other.

## 3. Loupe and inspector

- [ ] 3.1 Loupe: the play mark, the duration, no "unavailable" chip for videos, the placeholder with its reason
  (design D3). Verify: `scenario_gallery_video_shown_large_in_the_loupe` and
  `scenario_gallery_video_without_a_poster_tool_in_the_loupe` pass.
- [ ] 3.2 `Opener::open` on every platform, the `OpenInPlayer` action on `o`, and the loupe button (design D4).
  Verify: `scenario_gallery_open_in_player_from_the_loupe` passes with the `RecordingOpener`, and the file's bytes
  and mtime are unchanged.
- [ ] 3.3 Inspector: Open in player on video copy rows, disabled for offline sources. Verify:
  `scenario_photo_inspector_open_a_video_copy_in_the_player` and `scenario_photo_inspector_offline_video_copy`
  pass.
- [ ] 3.4 A `loupe-video` screenshot surface (a seeded library with a video, opened in the loupe). Add the manual
  check "Open in player" for each platform to `docs/manual-checks`. Verify: the surface renders, and the
  manual-check file lists the three platforms.

## 4. Verification

- [ ] 4.1 Screenshot review by a fresh reviewer: the loupe with a video in both themes, and the inspector's Open in
  player. Verify: the PNGs reviewed.
- [ ] 4.2 Manual: Open in player on this Linux machine opens the default player for a `.mov`. Verify: noted here.
- [ ] 4.3 `cargo xtask ci` passes. Verify: the command is green.
- [ ] 4.4 `openspec validate video-previews-and-play --strict` passes. Verify: the command reports it valid.

## Workflow follow-up

- Archive with `/opsx:archive video-previews-and-play` once CI is green; ship in 0.5.1.
