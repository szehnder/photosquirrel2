# 0134. Videos play in the system's default player; no in-app playback for now

- Status: proposed
- Date: 2026-11-21
- Deciders: maintainers

## Context
PhotoSquirrel catalogues videos, groups exact copies, shows a poster frame (ADR-0114) and exports them, but has
never played one. The owner asked for videos to "play". Playing inside the app would need:
- a demuxer and decoders for H.264, HEVC, ProRes, VP9 and AV1, with the HEVC patent problem ADR-0089 avoided for
  stills;
- audio output and synchronisation;
- a frame path into GPUI fast enough for 4K at 60 fps.

That is a new dependency of real consequence (GStreamer, libmpv or ffmpeg linked as a library) on three platforms,
for a feature that is not the app's purpose. The app's purpose is finding the best copy and exporting it. Every
desktop already has a video player that handles these formats.

## Decision
"Play" means **Open in player**: PhotoSquirrel asks the operating system to open the video file with its default
application.
- **Linux:** `xdg-open <path>`.
- **macOS:** `open <path>`.
- **Windows:** `explorer.exe <path>`, a single argument with no shell, so a path cannot be read as a command.

It is offered in the loupe and the inspector for a video on an online source. PhotoSquirrel neither reads nor
writes the file for it. The player opens it with its own access, like the file manager in "Reveal" (ADR-0004
holds for PhotoSquirrel's own code paths). The large preview of a video is its poster frame (ADR-0114's tool, at
2048 px).

## Alternatives considered
- **In-app playback with libmpv or GStreamer.** It would give the best experience, but it means a large native
  dependency, platform packaging work, and HEVC licensing questions. Revisit only if review needs frame-accurate
  comparison of videos.
- **Spawning ffplay.** Only when ffmpeg is installed, an unfamiliar window, and no system integration.
- **A sequence of poster frames as a scrubber.** Useful for comparing clips, but not playback. It could come later
  on top of this.

## Consequences
- Videos can be watched at once on every platform, with the user's preferred player and no new dependency.
- The player is outside PhotoSquirrel. If a player edits a file, that is the user's action in another app, as with
  "Reveal".
- Tests use the recording `Opener`. The real launch is checked by hand per platform.

## Revisit when
Duplicate review needs videos compared side by side or frame-accurately, or video perceptual dedup
(`video-perceptual-dedup`) is taken up.
