# Design

Skeleton; to be completed when the change is picked up.

- `ps_service::settings::scan_options(&Settings) -> ScanOptions` is the only place that maps settings to scanner
  options. `Library::scan` and the CLI's `Ctx::scan_options` call it.
- The app's engine is opened once; a Scanning setting change marks it stale and the next scan start (not a
  running one) applies the new options (classifier hash changes already trigger reclassification, ADR-0015).
- Power source: a `PowerSource` trait in `ps-service` with a platform implementation (sysfs on Linux, IOKit on
  macOS, `GetSystemPowerStatus` on Windows) and a fake for tests; polled every few seconds by the scan monitor.
