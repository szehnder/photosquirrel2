//! Placeholders that say why (`preview-placeholders`, ADR-0109): the grid's placeholder badge from a catalog row,
//! the "Why no preview?" dialog's content ([`Library::explain`]) and Fill in previews from the Library.
//!
//! The text comes from here and from the current decoder report, so it is right after a decoder was installed and
//! the app restarted.

use anyhow::Result;
use ps_catalog::{FileRecord, WaitingRule};
use ps_model::{
    FillScope, Filter, Format, ItemId, NoPreviewFilter, PlaceholderBadge, PlaceholderReason,
    PlaceholderStatus, PreviewAction, PreviewHelp, PreviewReason, PreviewStatusRow, STILL_WORKS,
    ScanRunId, Sort,
};

use crate::library::Library;

/// The badge a grid cell draws for a placeholder row: its format, reason and status mark, from the row alone
/// (no thumbnail read). `None` for rows with a preview.
pub fn badge(r: &FileRecord, rule: &WaitingRule) -> Option<PlaceholderBadge> {
    let p = r.preview?;
    Some(PlaceholderBadge {
        format_label: format_label(r.format),
        reason: p.reason,
        status: PlaceholderStatus::of(p.reason, rule.fillable(r)),
    })
}

/// "TIFF", "HEIC"; "Unknown format" when the probe found none.
pub fn format_label(f: Option<Format>) -> String {
    f.map_or_else(|| "Unknown format".to_owned(), |f| f.label().to_owned())
}

/// The Library filter that lists the files of a placeholder reason and format (the dialog's "Show them" and the
/// No preview filter).
pub fn no_preview_filter(reason: Option<PreviewReason>, format: Option<Format>) -> Filter {
    Filter {
        classes: Some(ps_catalog::library_classes()),
        no_preview: Some(NoPreviewFilter { reason, format }),
        ..Filter::default()
    }
}

/// The reason in plain words.
fn sentence(reason: &PreviewReason, detail: Option<&PlaceholderReason>, label: &str) -> String {
    match (reason, detail) {
        (PreviewReason::DecoderMissing, Some(PlaceholderReason::DecoderMissing { key })) => {
            format!(
                "This computer has no {} decoder, so PhotoSquirrel cannot draw this photo.",
                key.label()
            )
        }
        (PreviewReason::DecoderMissing, _) => format!(
            "This computer has no decoder for {label} files, so PhotoSquirrel cannot draw this photo."
        ),
        (
            PreviewReason::UnsupportedVariant,
            Some(PlaceholderReason::UnsupportedVariant { detail }),
        ) => {
            format!("This version of PhotoSquirrel can't read {detail} files.")
        }
        (PreviewReason::UnsupportedVariant, _) => {
            format!("This version of PhotoSquirrel can't read this kind of {label} file.")
        }
        (PreviewReason::Unreadable, _) => {
            format!("The {label} decoder could not read this file's picture.")
        }
        (PreviewReason::NoPosterTool, _) => {
            "Videos get their preview frame from a poster-frame tool (ffmpeg), and none is set up."
                .into()
        }
        (PreviewReason::Sequence, _) => {
            "This is an image sequence without a still image to show.".into()
        }
        (PreviewReason::NeedsNewerVersion, _) => {
            "A newer version of PhotoSquirrel made this placeholder.".into()
        }
    }
}

impl Library {
    /// The "Why no preview?" dialog's content for one item: its format and reason in plain words, what still
    /// works, what to do (by reason, from the current decoder report), and how many other Library files have no
    /// preview for the same reason. `None` when the item has a preview. Waits for decoder detection (call off the
    /// UI thread).
    pub fn explain(&self, id: ItemId) -> Result<Option<PreviewHelp>> {
        let scan = self.scan()?;
        let Some(r) = self.catalog()?.read().file_by_id(id.into())? else {
            return Ok(None);
        };
        let stored = match r.identity {
            Some(c) => scan.engine().thumbs().get(&c)?,
            None => None,
        };
        let info = stored
            .as_ref()
            .filter(|t| t.kind == ps_model::ThumbKind::Placeholder)
            .and_then(|t| t.placeholder.clone());
        // The row's reason; a video row is not marked by migration 24 and takes its record's.
        let reason = match (r.preview, info.as_ref()) {
            (Some(p), _) => p.reason,
            (None, Some(i)) => i
                .reason
                .as_ref()
                .map_or(PreviewReason::Unreadable, |x| x.kind()),
            (None, None) => return Ok(None),
        };
        let decoders = scan.decoders();
        scan.engine()
            .update_options(|o| o.decoders = decoders.clone());
        let rule = scan.waiting_rule();
        let fillable = rule.fillable(&r);
        let label = format_label(r.format);
        let detail = info.as_ref().and_then(|i| i.reason.clone());
        let action = if fillable {
            let count = scan
                .engine()
                .catalog()
                .read()
                .fillable_files(
                    &rule,
                    &FillScope {
                        format: r.format,
                        reason: None,
                    },
                    &[],
                )?
                .len() as u64;
            PreviewAction::FillIn {
                format: r.format,
                count,
            }
        } else {
            match (&reason, &detail) {
                (PreviewReason::DecoderMissing, d) => {
                    let key = match d {
                        Some(PlaceholderReason::DecoderMissing { key }) => *key,
                        _ => r.decoder_missing.unwrap_or(ps_model::DecoderKey::Heif),
                    };
                    let e = decoders.report().for_key(key);
                    PreviewAction::Install {
                        key,
                        reason: e.and_then(|e| e.reason.clone()),
                        fix: e.and_then(|e| e.fix.clone()),
                    }
                }
                (PreviewReason::UnsupportedVariant, d) => PreviewAction::UpdateLater {
                    detail: match d {
                        Some(PlaceholderReason::UnsupportedVariant { detail }) => detail.clone(),
                        _ => format!("this kind of {label}"),
                    },
                },
                (PreviewReason::Unreadable, d) => PreviewAction::Damaged {
                    message: match d {
                        Some(PlaceholderReason::Unreadable { message }) => message.clone(),
                        _ => info.as_ref().map(|i| i.note.clone()).unwrap_or_default(),
                    },
                },
                (PreviewReason::NoPosterTool, _) => {
                    let tool = scan.engine().options().poster_tool;
                    let report = ps_media::decoder_report(&decoders, tool.as_deref());
                    PreviewAction::PosterTool {
                        fix: report
                            .entry(ps_model::DecoderFormat::PosterFrames)
                            .and_then(|e| e.fix.clone()),
                    }
                }
                (PreviewReason::Sequence, _) => PreviewAction::Sequence,
                (PreviewReason::NeedsNewerVersion, _) => PreviewAction::NewerVersion,
            }
        };
        let sentence = if fillable && reason != PreviewReason::DecoderMissing {
            format!(
                "This version of PhotoSquirrel reads {label} files that an older version could not, so a \
                 preview can be made now."
            )
        } else if fillable {
            format!("A {label} decoder is available now, so a preview can be made.")
        } else {
            sentence(&reason, detail.as_ref(), &label)
        };
        let same = self
            .catalog()?
            .read()
            .id_list(&no_preview_filter(Some(reason), r.format), &Sort::default())?
            .len() as u64;
        Ok(Some(PreviewHelp {
            item: id,
            name: String::from_utf8_lossy(r.key.path.file_name()).into_owned(),
            format: r.format,
            format_label: label,
            reason,
            status: PlaceholderStatus::of(reason, fillable),
            sentence,
            still_works: STILL_WORKS.to_owned(),
            action,
            others: same.saturating_sub(1),
        }))
    }

    /// Placeholders by format and reason, with how many can be filled in now (the Library notice; waits for
    /// decoder detection).
    pub fn preview_status(&self) -> Result<Vec<PreviewStatusRow>> {
        Ok(self.scan()?.preview_status(&[])?)
    }

    /// Start Fill in previews for `scope` over every source (the dialog's and the notice's button).
    pub fn fill_previews(&self, scope: FillScope) -> Result<ScanRunId> {
        Ok(self.scan()?.fill_previews(scope, &[])?)
    }
}
