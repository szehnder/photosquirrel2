//! Hosted-provider consent (ADR-0021): previews leave the machine only after the user agreed for this
//! provider by name.

use crate::config::ProviderConfig;

/// What the consent prompt says. The UI shows it verbatim; the queue refuses to start without consent.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ConsentRequired {
    /// The provider's name, as shown to the user.
    pub provider: String,
    pub message: String,
}

/// The consent prompt for `provider`.
pub fn consent_prompt(provider: &str) -> ConsentRequired {
    ConsentRequired {
        provider: provider.to_owned(),
        message: format!(
            "Send photos to {provider}? PhotoSquirrel will upload a 1024 px preview of each photo in the AI queue \
             to {provider} to describe it. Originals, file names and folders are never uploaded. Nothing is sent \
             until you start the AI queue."
        ),
    }
}

/// `Ok` for local providers, or hosted ones the user consented to; otherwise the prompt to show.
pub fn check(cfg: &ProviderConfig, hosted_consent: bool) -> Result<(), ConsentRequired> {
    if cfg.is_hosted() && !hosted_consent {
        return Err(consent_prompt(&cfg.name));
    }
    Ok(())
}
