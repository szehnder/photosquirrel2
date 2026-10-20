# 0049. Who owns `settings.toml` while the app runs; external consent withdrawals win

- Status: accepted
- Date: 2026-10-20
- Deciders: maintainers

## Context
ADR-0047 bound hosted consent to a destination and pauses a running AI queue when the app's own settings
change it. Two gaps remained, both about `settings.toml` being changed by someone else while a process runs:
- The app reads the file once at start-up and writes its in-memory copy on every change. Withdrawing consent by
  editing the file (or with the CLI) while the app ran was silently undone by the app's next save.
- `photosquirrel-cli ai start` read the settings once; a consent withdrawn or an address changed in the file did
  not stop a running CLI queue.
The file says "safe to edit by hand while it is closed", but consent is a privacy control: a withdrawal must
hold however it is made.

## Decision
This extends ADR-0047 (which stays as written).
- **The running app owns the file.** Its in-memory settings are the truth for every value except hosted consent.
  Before each save it re-reads the file; when the file changed since the app last loaded or saved it
  (`SettingsStore::save_merged`, compared by a hash of the bytes), the consent is merged three ways (last
  loaded/saved, in memory, on disk): it is kept only when the file still agrees, otherwise **withdrawn**, in the
  file and in memory (which pauses a running queue, ADR-0047). A merge never adds or resurrects consent. Other
  external edits made while the app runs are overwritten by its next save, as before.
- **A CLI queue re-checks the file before every request** (`ps_llm::queue::SendGate`,
  `ps_service::ai::settings_file_gate`): when the provider, the normalized destination or the consent coverage
  differ from what the queue started with, or a hosted destination is no longer covered, the queue pauses
  (recorded like a user pause) and sends nothing more.

## Alternatives considered
- **Watch the file (inotify/FSEvents) and reload everything** - a new dependency and live reload of every value
  for one privacy rule; editing other values while the app runs stays unsupported.
- **Lock the file while the app runs** - the CLI must still be able to change settings (ADR-0027).
- **Disk wins for every value** - the app would lose changes the user just made in the settings window.

## Consequences
- An app queue still learns about an external withdrawal only at the app's next settings save; a CLI queue
  learns before its next request. Each CLI request re-reads one small file.
- Consent can only be given in the app's settings window or by editing the file while the app is closed.

## Revisit when
Settings need live reload for other reasons (then watch the file and reuse the same merge rule).
