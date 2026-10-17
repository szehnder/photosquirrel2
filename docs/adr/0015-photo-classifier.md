# 0015. What counts as a photo: probe-time classifier

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
The user cares about photos (family, pets, travel), not icons, UI assets, web graphics or app caches.

## Decision
A rule-based classifier runs on probe data and assigns `photo`, `likely_photo`, `graphic`, `thumbnail_cache` or `junk`, with reasons:
- camera EXIF (make/model/DateTimeOriginal/exposure) -> `photo`;
- min(width, height) < 200 px or pixel count < 120k -> `junk` unless camera EXIF is present;
- paths inside known cache folders (`.thumbnails`, `Thumbs.db` neighbours, `AppData`, `node_modules`, `.cache`, iPhoto/Photos `resources/proxies`, Lightroom previews) -> `thumbnail_cache` (still analyzed so they can join groups, but never keepers if a better copy exists);
- PNG/GIF/SVG/ICO without camera EXIF and with palette/alpha or tiny sizes -> `graphic`;
- otherwise `likely_photo`.
Videos: phone/camera containers (QuickTime/MP4 with make/model or creation date) -> `photo` class "video"; files < 2 s or < 320 px -> `junk`. Thresholds live in settings. The optional LLM pass (ADR-0020) can confirm or refute `likely_photo`.

## Alternatives considered
- **ML classifier at probe time** - too expensive for millions of files before we know they matter.

## Consequences
- Skipped files remain visible in an "Excluded" view with reasons, so nothing silently disappears.

## Revisit when
Corpus or user review shows systematic misclassification.
