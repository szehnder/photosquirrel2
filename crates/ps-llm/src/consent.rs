//! Hosted-provider consent (ADR-0021, ADR-0047): previews leave the machine only after the user agreed for this
//! provider, by name, and this destination host. Consent for one host never covers another: editing the server
//! address or choosing another provider needs consent again.

use ps_model::settings::{HostedConsent, ProviderId};

use crate::config::ProviderConfig;

/// What the consent prompt says. The UI shows it verbatim; the queue refuses to start without consent.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ConsentRequired {
    /// The provider's name, as shown to the user.
    pub provider: String,
    /// Where previews would go (`host[:port]`).
    pub host: String,
    pub message: String,
}

/// The consent prompt for `provider` at `host`.
pub fn consent_prompt(provider: &str, host: &str) -> ConsentRequired {
    ConsentRequired {
        provider: provider.to_owned(),
        host: host.to_owned(),
        message: format!(
            "Send photos to {provider} at {host}? PhotoSquirrel will upload a 1024 px preview of each photo in the \
             AI queue to {host} to describe it. Originals, file names and folders are never uploaded. Nothing is \
             sent until you start the AI queue."
        ),
    }
}

/// The normalized destination of a base URL: lower-case host, with the port only when it is not the scheme's
/// default (`api.anthropic.com`, `192.168.1.20:11434`). `None` when the URL does not parse or has no host.
pub fn destination_host(base_url: &str) -> Option<String> {
    let u = url::Url::parse(base_url.trim()).ok()?;
    let host = u.host_str()?.to_ascii_lowercase();
    Some(match u.port() {
        Some(p) => format!("{host}:{p}"),
        None => host,
    })
}

/// The consent the user gives for `provider` at `base_url`.
pub fn consent_for(provider: ProviderId, base_url: &str) -> Option<HostedConsent> {
    Some(HostedConsent {
        provider,
        host: destination_host(base_url)?,
    })
}

/// `consent` was given for exactly this provider and this configuration's destination host.
pub fn covers(cfg: &ProviderConfig, provider: ProviderId, consent: Option<&HostedConsent>) -> bool {
    consent.is_some_and(|c| {
        c.provider == provider
            && destination_host(&cfg.base_url).as_deref() == Some(c.host.as_str())
    })
}

/// `Ok` for local providers, or hosted ones the user consented to for this provider and host; otherwise the
/// prompt to show.
pub fn check(
    cfg: &ProviderConfig,
    provider: ProviderId,
    consent: Option<&HostedConsent>,
) -> Result<(), ConsentRequired> {
    if cfg.is_hosted() && !covers(cfg, provider, consent) {
        return Err(consent_prompt(
            &cfg.name,
            &destination_host(&cfg.base_url).unwrap_or_else(|| cfg.base_url.clone()),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_normalized() {
        assert_eq!(
            destination_host("https://API.Anthropic.com/v1/").as_deref(),
            Some("api.anthropic.com")
        );
        assert_eq!(
            destination_host("https://api.anthropic.com:443/v1").as_deref(),
            Some("api.anthropic.com"),
            "default port dropped"
        );
        assert_eq!(
            destination_host("http://192.168.1.20:11434/v1").as_deref(),
            Some("192.168.1.20:11434")
        );
        assert_eq!(destination_host("not a url"), None);
    }
}
