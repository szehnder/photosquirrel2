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

3. A real scan on the fresh data folder. Copy two or three HEIC photos from a phone into `$d\photos` (copies, never
   originals). For each one, also save a smaller JPEG copy beside it: open the HEIC in the Photos app, choose
   "..." > "Resize image", pick the smallest size, save it as JPEG into `$d\photos` (any name). Then scan once,
   keeping the JSON summary of that first scan (a second scan would re-analyse nothing and report 0 errors
   whatever happened):

   ```powershell
   photosquirrel-cli --data-dir "$d\data" sources add "$d\photos"
   $lines = photosquirrel-cli --data-dir "$d\data" --json scan | ForEach-Object { $_ | ConvertFrom-Json }
   $summary = $lines[-1].data
   "errors: $($summary.errors)   undecoded: $(($summary.undecoded | Measure-Object).Count)"
   $lines | ForEach-Object { $_.data.recent_errors } | Where-Object { $_ } |
       ForEach-Object { "$($_.path): $($_.message)" } | Sort-Object -Unique
   ```

   `--json scan` prints a progress line now and then and the run's summary last. Expect `errors: 0   undecoded: 0`
   and no error lines below it; any error is a finding: record its line.

   Then check that every HEIC really decoded: only a HEIC with pixels has perceptual hashes, and only those make
   it a duplicate group with its resized JPEG.

   ```powershell
   photosquirrel-cli --data-dir "$d\data" groups list
   $groups = (photosquirrel-cli --data-dir "$d\data" --json groups list | ConvertFrom-Json).data.groups
   foreach ($g in $groups) { photosquirrel-cli --data-dir "$d\data" groups show $g.id }
   ```

   Expect one group per HEIC, each with 2 members: the HEIC as keeper ("more pixels") and its JPEG copy, joined as
   a derivative with a pHash distance. A HEIC without a group (or a group without its HEIC) did not decode: a
   finding. Open the app with the same data folder (`photosquirrel.exe --data-dir "$d\data"`) and check that each
   HEIC shows a real, upright thumbnail and large preview (portrait photos must be portrait: this is where a
   backend that applies or ignores the container rotation would show).

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
(`winver`), the extension versions (Settings > Apps), the doctor HEIC line, the errors and group counts of step 3,
and the outcome of step 4.
