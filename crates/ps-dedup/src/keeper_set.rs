//! RAW+JPEG keeper sets (ADR-0014): a RAW and the camera JPEG/HEIC of the same capture (same camera
//! serial/model, DateTimeOriginal within 1 s, same stem in the same folder) are kept together.

use crate::edges::raw_jpeg_companions;
use crate::input::DedupInput;

/// The best-ranked member forming a keeper set with `keeper`, if any. `others` come best first.
pub fn companion<'a>(
    keeper: &DedupInput,
    others: impl IntoIterator<Item = &'a DedupInput>,
) -> Option<&'a DedupInput> {
    others
        .into_iter()
        .find(|o| raw_jpeg_companions(keeper, o, false))
}
