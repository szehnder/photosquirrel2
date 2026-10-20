# 0048. Hosted consent binds the scheme; hosted providers need HTTPS

- Status: accepted
- Date: 2026-10-20
- Deciders: maintainers

## Context
ADR-0047 bound hosted consent to a provider and a normalized `host[:port]`. The scheme was not part of it, so
consent given for `https://api.anthropic.com` also covered `http://api.anthropic.com/v1`, and because the port
was dropped only when it was the *scheme's* default, `http://api.anthropic.com:443` was accepted as well. A
mistyped or hand-edited address could therefore send previews, and the API key in its header, unencrypted,
under a consent the user gave for an encrypted connection. Settings validation accepted any `http://` address
for every provider.

## Decision
This extends ADR-0047 (which stays as written).
- The consent destination includes the scheme: `ps_llm::consent::destination_host` keeps writing HTTPS
  destinations as `host[:port]` (so existing consents stay valid) and writes every other scheme in full
  (`http://host[:port]`). The port is dropped only when it is that scheme's default. Consent therefore binds
  scheme, host and port.
- A hosted provider (OpenAI, Anthropic, Gemini) over a non-HTTPS address is never covered by any consent
  (`covers` is false, `consent_for` returns `None`), whatever `settings.toml` says.
- Settings validation rejects a non-`https://` server address for those providers with a message that says
  photos and the API key would otherwise travel unencrypted. OpenAI-compatible servers may still use `http://`
  (a LAN server); their consent, when they are hosted, names the `http://` destination.

## Alternatives considered
- **A separate `scheme` field in `[ai.consent]`** - a settings format change for no gain; folding the scheme
  into the stored destination keeps every HTTPS consent valid and makes plain-HTTP destinations visible on the
  consent card.
- **Upgrade `http://` to `https://` silently for hosted providers** - hides a misconfiguration and would make
  the stored address differ from what is used.

## Consequences
- Consent for an OpenAI-compatible server over plain HTTP at a non-local address is asked once more (its stored
  destination gains the `http://` prefix).
- The consent card shows `http://…` for unencrypted destinations.

## Revisit when
A hosted provider needs a non-HTTPS transport (it should not).
