# 0053. Power source probe: platform calls, no dependency

- Status: accepted
- Date: 2026-10-20
- Deciders: maintainers

## Context
`general.pause_on_battery` must pause a running scan when the machine switches to battery and offer to resume on
mains power. That needs the current power source on Linux, macOS and Windows, a few times a minute. Crates that
report it (`battery`, `starship-battery`) bring per-platform dependency trees and are more than one boolean needs.

## Decision
- `ps_service::power::PowerSource` is a one-method trait (`on_battery() -> bool`); tests use a fake. The scan
  monitor polls it every few seconds while the setting is on.
- Linux: read `/sys/class/power_supply/*` (`type`, `online`, `status`): on battery when no mains or USB supply is
  online and a battery is discharging. No unsafe code.
- macOS: `IOPSGetTimeRemainingEstimate()` from IOKit (it returns `kIOPSTimeRemainingUnlimited` on external power).
  Windows: `GetSystemPowerStatus()` from kernel32 (`ACLineStatus == 0` is battery). Each is one `extern` function
  declared in place, the only `unsafe` in the crate, allowed locally (`#[allow(unsafe_code)]`).
- Unknown or desktop machines report mains power, so the setting never pauses a scan by mistake.

## Alternatives considered
- **A battery crate** - a dependency tree for one boolean.
- **Spawning `pmset -g batt` / PowerShell** - a process every few seconds, slow on Windows, and output formats
  that change between OS releases.

## Consequences
- Two small FFI declarations to keep correct; both APIs are stable system interfaces (IOKit since 10.7,
  `GetSystemPowerStatus` since Windows 2000).

## Revisit when
A platform API changes, or more power facts are needed (charge level, low-power mode).
