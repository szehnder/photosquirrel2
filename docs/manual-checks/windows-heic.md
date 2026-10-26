# Manual check: HEIC on Windows through WIC

CI's windows-2022 runner has the HEIF Image Extensions but no HEVC decoder, so the WIC decode path
(`crates/ps-sysdecode/src/wic.rs`) is compile-checked and its detection and reason mapping are tested there, but it
has **not run** on a machine with the extensions. This check runs it on a Windows 10 (1809 or newer) or Windows 11 PC
with both installed. It takes about five minutes and touches nothing but a temporary folder.

## Prerequisites

- From the Microsoft Store: **HEIF Image Extensions** (<https://apps.microsoft.com/detail/9pmmsr1cgpwg>) and **HEVC
  Video Extensions** (<https://apps.microsoft.com/detail/9nmzlz57r3t7>, or the free "from Device Manufacturer"
  edition, <https://apps.microsoft.com/detail/9n4wgh0z6vhq>, where the PC maker offers it).
- `photosquirrel-cli.exe` from the Windows package of the build under test (the `.zip` or the installed program).
- Optional, for step 4: a Rust toolchain and a checkout of the repository at the same commit.

## Steps (PowerShell)

1. A scratch data folder, so the check never touches your real catalog:

   ```powershell
   $d = Join-Path $env:TEMP "ps-heic-check"; Remove-Item -Recurse -Force $d -ErrorAction Ignore
   New-Item -ItemType Directory "$d\photos" | Out-Null
   ```

2. Detection. Expect `heic` available with backend `WIC`, no reason, and `decoder_override` empty:

   ```powershell
   photosquirrel-cli --data-dir "$d\data" doctor
   photosquirrel-cli --data-dir "$d\data" --json doctor | Out-File "$d\doctor.json"
   ```

   Record the HEIC line and the `os` entry. If HEIC is unavailable, the reason must name the missing extension and
   the fix must list its Store link; that is a finding to report, not a pass.

3. A real scan. Copy two or three HEIC photos from a phone into `$d\photos` (copies, never originals), then:

   ```powershell
   photosquirrel-cli --data-dir "$d\data" sources add "$d\photos"
   photosquirrel-cli --data-dir "$d\data" scan
   photosquirrel-cli --data-dir "$d\data" --json scan
   ```

   Expect no "HEIC photos have no preview" line, `undecoded` empty in the JSON, and 0 errors. Open the app with the
   same data folder (`photosquirrel.exe --data-dir "$d\data"`) and check that each HEIC shows a real, upright
   thumbnail and large preview (portrait photos must be portrait: this is where a backend that applies or ignores the
   container rotation would show).

4. Optional, the real-decoder tests with the require flag (a skip becomes a failure):

   ```powershell
   $env:PHOTOSQUIRREL_REQUIRE_SYSTEM_HEIF = "1"
   cargo test -p ps-sysdecode --test sysdecode -- --nocapture
   ```

   `scenario_decoder_support_windows_decodes_heic_when_the_extensions_are_installed` and
   `scenario_decoder_support_system_decoders_agree_with_the_reference_look` must pass; the second prints the WIC
   hashes, which must be within 4 bits of the reference.

5. Clean up: `Remove-Item -Recurse -Force $d`.

## Record

Add a line to the change's `tasks.md` (or the release notes' verification list) with the Windows edition and build
(`winver`), the extension versions (Settings > Apps), the doctor HEIC line, and the outcome of steps 3 and 4.
