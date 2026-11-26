# Proposal

## Why

The owner reports that `.mov` and other videos "may not load a preview or play" in the app. The investigation
(2026-11-21) found two separate things.

**A bug: videos have no large preview.** With ffmpeg installed, a scan gives every video a 384 px poster-frame
thumbnail (checked on a generated H.264 `.mov`, an `.mp4` and an HEVC `.mov` tagged like an iPhone clip: all three
get one). The large preview is a different story. `Library::preview` returns `Unavailable("video")` for every video
before trying anything. So the loupe (Enter or E) stretches the small grid thumbnail across the window under a chip
that says "Large preview unavailable (video)", and the inspector shows the same. The `thumbnails` requirement "Large
previews on demand" makes no exception for videos, and the poster-frame tool that would make one is already
configured. This makes videos look broken even when everything is set up.

**An enhancement: playing videos.** The app has never played video. No requirement or ADR promises it, and in-app
playback would need a video and audio decoding pipeline, a new dependency of real consequence. Most of the need is
"let me watch this clip", which the system's own player does well.

## What Changes

- **Large previews of videos (bug fix).** With the poster-frame tool available, a video's large preview is a poster
  frame at up to 2048 px on the long edge, made with one bounded tool call at the same seek as its thumbnail. Large-preview calls run one
  at a time, on top of a scan's own poster lane (ADR-0120), and the result is cached in the preview cache like a
  photo's. Without the tool, or
  when the tool gets no frame, the loupe and the inspector show the video's placeholder with its reason ("needs
  ffmpeg", "poster frame failed: …"), never the bare "unavailable (video)".
- **Videos marked in the loupe.** A video in the loupe shows a play mark, its duration and an **Open in player**
  button.
- **Play in the system player (enhancement, new ADR-0134).** **Open in player**, in the loupe and in the inspector
  for any video copy on an online source, and with the key **O**, asks the operating system to open the file with
  its default application (`xdg-open`, `open`, Explorer). PhotoSquirrel writes nothing and reads nothing for it. On
  an offline source the action is disabled with the reason.

Release: 0.5.1 (patch), with `btrfs-volume-identity`.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `thumbnails`: adds the requirement "Large previews of videos".
- `gallery`: adds the requirement "Videos in the loupe".
- `photo-inspector`: adds the requirement "Open a video in the system player".

## Impact

- **Code:** `ps-media` poster frames take a long edge (384 for thumbnails, 2048 for previews). `ps-service`
  `Library::preview` handles videos. `ui-gallery`'s loupe gets the video mark and the button. The `ui-workspace`
  inspector and the `Opener` trait gain `open`, next to `reveal`.
- **No catalog migration, no new dependency.** The preview cache, its cap and its keys are unchanged; a video's
  key is its content identity, like a photo's.
- **New ADR:** ADR-0134 (videos play in the system's default player; no in-app playback for now).
- **ADRs relied on:** ADR-0004 (sources are read-only), ADR-0114 and ADR-0120 (poster-frame tool, seek, limit and
  failure rules), ADR-0016 (preview and thumbnail stores).

### Out of scope

- Playback inside PhotoSquirrel (ADR-0134 records when to revisit).
- Formats PhotoSquirrel does not recognise as video today (`.wmv`, `.mpg`). Its probe covers MOV, MP4, M4V, AVI,
  MKV/WebM, 3GP and MTS/M2TS.
- Several frames or a scrubber for a video in the loupe.
