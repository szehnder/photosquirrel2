//! A small horizontal strip of thumbnails (the scan dashboard's "recent finds", change `scan-dashboard`).
//!
//! It reuses the gallery's machinery: thumbnails come from the same [`GallerySource`](crate::source)
//! (`thumbs.db`), are decoded off the UI thread with [`decode_jpeg_bgra`] and kept in a private
//! [`TextureCache`] of [`STRIP_TEXTURES`] entries. Evicted textures are released from the GPU atlas on the
//! next render. The strip only ever holds the newest [`STRIP_TEXTURES`] keys, whatever the scan rate.

use std::collections::HashSet;
use std::sync::Arc;

use gpui_kit::*;

use crate::decode::decode_jpeg_bgra;
use crate::model::render_image;
use crate::source::{SharedSource, ThumbKey};
use crate::textures::{Budget, TextureCache};

/// Thumbnails kept (the dashboard shows the last 64 photos found).
pub const STRIP_TEXTURES: usize = 64;

/// The strip's data: the keys to show (oldest first), their textures and the decodes in flight.
pub struct ThumbStrip {
    source: SharedSource,
    keys: Vec<ThumbKey>,
    textures: TextureCache<ThumbKey, Arc<RenderImage>>,
    /// Decoded or found to have no thumbnail (cached negatively so it is not asked again).
    missing: HashSet<ThumbKey>,
    in_flight: HashSet<ThumbKey>,
    evicted: Vec<Arc<RenderImage>>,
    task: Option<Task<()>>,
}

impl ThumbStrip {
    pub fn new(source: SharedSource) -> Self {
        Self {
            source,
            keys: Vec::new(),
            textures: TextureCache::new(Budget {
                entries: STRIP_TEXTURES,
                bytes: 64 * 1024 * 1024,
            }),
            missing: HashSet::new(),
            in_flight: HashSet::new(),
            evicted: Vec::new(),
            task: None,
        }
    }

    /// The keys to show, oldest first (only the newest [`STRIP_TEXTURES`] are kept).
    pub fn set_keys(
        &mut self,
        keys: impl IntoIterator<Item = ThumbKey>,
        cx: &mut Context<'_, Self>,
    ) {
        let mut keys: Vec<ThumbKey> = keys.into_iter().collect();
        if keys.len() > STRIP_TEXTURES {
            keys.drain(..keys.len() - STRIP_TEXTURES);
        }
        if keys == self.keys {
            return;
        }
        self.keys = keys;
        self.pump(cx);
        cx.notify();
    }

    pub fn keys(&self) -> &[ThumbKey] {
        &self.keys
    }

    /// Shown keys whose texture is ready.
    pub fn loaded(&self) -> usize {
        self.keys
            .iter()
            .filter(|k| self.textures.contains(k))
            .count()
    }

    pub fn is_loaded(&self, key: &ThumbKey) -> bool {
        self.textures.contains(key)
    }

    pub fn texture(&mut self, key: &ThumbKey) -> Option<Arc<RenderImage>> {
        self.textures.get(key).cloned()
    }

    /// Decode every shown thumbnail now, on the calling thread. Only for scripted screenshots of a simulated
    /// scan, before the first frame; the dashboard otherwise decodes in the background.
    pub fn prime_blocking(&mut self) {
        for k in self.keys.clone() {
            if self.textures.contains(&k) || self.missing.contains(&k) {
                continue;
            }
            let image = self
                .source
                .thumbnail(&k)
                .ok()
                .flatten()
                .and_then(|jpeg| decode_jpeg_bgra(&jpeg, 1).ok())
                .and_then(render_image);
            match image {
                Some(image) => {
                    let size = image.size(0);
                    let bytes = (size.width.0.max(1) as u64) * (size.height.0.max(1) as u64) * 4;
                    let evicted = self.textures.insert(k, image, bytes);
                    self.evicted.extend(evicted);
                }
                None => {
                    self.missing.insert(k);
                }
            }
        }
    }

    /// Release evicted textures from the atlas (call while rendering).
    pub fn release(&mut self, window: &mut Window) {
        for image in self.evicted.drain(..) {
            let _ = window.drop_image(image);
        }
    }

    fn pump(&mut self, cx: &mut Context<'_, Self>) {
        if self.task.is_some() {
            return;
        }
        let wanted: Vec<ThumbKey> = self
            .keys
            .iter()
            .rev()
            .filter(|k| {
                !self.textures.contains(k)
                    && !self.missing.contains(k)
                    && !self.in_flight.contains(k)
            })
            .take(16)
            .copied()
            .collect();
        if wanted.is_empty() {
            return;
        }
        self.in_flight.extend(wanted.iter().copied());
        let source = self.source.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let decoded = cx
                .background_spawn(async move {
                    wanted
                        .into_iter()
                        .map(|k| {
                            let image = source
                                .thumbnail(&k)
                                .ok()
                                .flatten()
                                .and_then(|jpeg| decode_jpeg_bgra(&jpeg, 1).ok())
                                .and_then(render_image);
                            (k, image)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                for (k, image) in decoded {
                    this.in_flight.remove(&k);
                    match image {
                        Some(image) => {
                            let size = image.size(0);
                            let bytes =
                                (size.width.0.max(1) as u64) * (size.height.0.max(1) as u64) * 4;
                            let evicted = this.textures.insert(k, image, bytes);
                            this.evicted.extend(evicted);
                        }
                        None => {
                            this.missing.insert(k);
                        }
                    }
                }
                this.task = None;
                this.pump(cx);
                cx.notify();
            });
        }));
    }
}
