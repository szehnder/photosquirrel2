# 0047. Hosted AI consent is bound to a provider and a host; no redirects

- Status: accepted
- Date: 2026-10-20
- Deciders: maintainers

## Context
ADR-0021 lets previews leave the machine only after explicit consent for a named hosted provider. The settings
stored that consent as a bare `ai.hosted_consent = true`. Choosing another provider in the UI cleared it, but
editing the server address did not: consent given for `api.anthropic.com` silently applied to any host typed into
the address field afterwards (or written into `settings.toml` by hand). The HTTP client also followed redirects,
so a local server (not hosted, no consent needed) could answer 3xx and have the request, preview included,
re-sent to a host the user never saw.

## Decision
- Settings version 2 replaces `ai.hosted_consent` with `ai.consent = { provider, host }`: the provider id and the
  normalized destination host (lower case, `host[:port]` with the port only when not the scheme's default), as
  `ps_llm::consent::destination_host` derives it from the effective base URL.
- Consent covers exactly that provider at that host (`ps_llm::consent::covers`); the queue and `check_consent`
  refuse anything else and the prompt names the provider and the host. A version-1 file's `hosted_consent` key
  is ignored (unknown keys are dropped), so earlier consent must be given again; the file is not "corrupt".
- The settings window withdraws consent when the provider or the server address changes, shows the consent card
  whenever the effective configuration is hosted (also an OpenAI-compatible provider at a non-local address),
  and names the destination host on the card.
- The provider HTTP client never follows redirects (`reqwest::redirect::Policy::none()`); a 3xx is an error.

## Alternatives considered
- **Bind consent to the full URL** - a path or trailing-slash edit would withdraw it for no privacy reason.
- **Migrate `hosted_consent = true` to the current provider and host** - the address may have been edited after
  the consent was given, which is the bug being fixed; asking again once is cheap.
- **Follow redirects only to the same host** - no provider needs redirects for these APIs.

## Consequences
- Users of a hosted provider confirm consent once more after upgrading.
- Settings files gain an `[ai.consent]` table.

## Revisit when
A provider legitimately serves its API through redirects or several hosts (then consent would name a host set).
