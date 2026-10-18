# 0037. RAW+JPEG pairs are exempt from the two-camera-originals rule

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
ADR-0035 says two camera originals with the same model and dimensions but different content are two shots and
never derivatives. A camera shooting RAW+JPEG writes two originals of one shot; ADR-0014 wants them kept together
as one keeper set. Applied literally, ADR-0035 would split every RAW+JPEG pair.

## Decision
A RAW file and a JPEG/HEIC file are a **capture pair** (same group, one keeper set) when they share the folder,
the file stem (case-insensitive), the camera make/model, and the capture time to the second (and sub-second when
both record it). Capture pairs are exempt from the two-camera-originals rule. Nothing else is exempt.

## Alternatives considered
- **Pair by capture time alone** - would pair burst frames shot in RAW+JPEG with the wrong partner.

## Consequences
- RAW+JPEG pairs export together; a JPEG renamed into another folder without its RAW is grouped only by the
  normal derivative rules.

## Revisit when
Cameras that write different stems for RAW and JPEG are common in user data.
