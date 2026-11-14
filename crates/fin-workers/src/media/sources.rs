//! Source pictures for the compositor: each clip layer's asset through the manifest, its
//! open [`FrameReader`](fin_codecs::FrameReader) (kept by the warm worker), and the nearest
//! frame at the layer's source time (`frame_at`, spec §10.2).
//!
//! Colour: the container's tags (`colr`) win field by field over the bitstream's, and
//! untagged fields follow spec §10.1's rule (`resolve_defaults`).

use std::time::Instant;

use fin_codecs::ProbeResult;
use fin_compositor::{LayerIssue, SourceFrame, SourceProvider};
use fin_core::eval::Layer;
use fin_media::jobs::{LookProxyFallback, LookSource, ManifestAsset, RenderManifest};
use fin_media::{ChromaSiting, ColourTags, Matrix, Primaries, Transfer, VideoCodec, frame_at};
use serde_json::json;

use super::open_cache::{Entry, Got, OpenCache, missing};
use super::proxy_source::{LOOK_TIER, Refusal};

/// One frame's view of the worker's sources.
pub struct Sources<'a> {
    pub manifest: &'a RenderManifest,
    pub cache: &'a mut OpenCache,
    pub caps: &'a ProbeResult,
    /// Sources opened and reused while rendering (each layer that asked counts once per frame).
    pub opened: u32,
    pub reused: u32,
    /// The presented frame index each drawn layer got, by item id.
    pub chosen: Vec<(String, usize)>,
    /// Microseconds spent getting each source picture (decoding, or the reader's cache), with
    /// the decoder's backend: one entry per drawn layer.
    pub decodes: Vec<(&'static str, u64)>,
    /// The first source this frame decoded from: its codec and the decoder's backend.
    pub first_decode: Option<(VideoCodec, &'static str)>,
    /// `--originals`: a layer never reads a proxy.
    pub originals: bool,
    /// Layers this frame drew from a proxy.
    pub proxied: u32,
    /// Assets whose listed proxy could not be used, once each, in the order met.
    pub fallbacks: Vec<LookProxyFallback>,
}

impl<'a> Sources<'a> {
    pub fn new(
        manifest: &'a RenderManifest,
        cache: &'a mut OpenCache,
        caps: &'a ProbeResult,
    ) -> Self {
        Sources {
            manifest,
            cache,
            caps,
            opened: 0,
            reused: 0,
            chosen: Vec::new(),
            decodes: Vec::new(),
            first_decode: None,
            originals: false,
            proxied: 0,
            fallbacks: Vec::new(),
        }
    }

    /// Where this frame's pictures came from (`LookFrame.source`).
    pub fn source(&self) -> LookSource {
        if self.proxied > 0 {
            LookSource::Proxy
        } else {
            LookSource::Original
        }
    }

    /// Opens the `1080` proxy of `asset` and checks that frame `at` of it decodes; the frame
    /// index it reads. `Err`: the reason the layer reads the original (recorded once).
    fn pick_proxy(&mut self, asset: &ManifestAsset, time: fin_core::Rational) -> Option<usize> {
        let p = asset.proxy(LOOK_TIER)?;
        let refused = |this: &mut Self, r: Refusal| {
            if !this.fallbacks.iter().any(|f| f.asset == asset.asset) {
                this.fallbacks.push(LookProxyFallback {
                    asset: asset.asset.clone(),
                    reason: r.reason().into(),
                });
            }
            None
        };
        match self.cache.get_proxy(asset, p, self.caps) {
            Ok(Got::Opened) => self.opened += 1,
            Ok(Got::Reused) => self.reused += 1,
            Err(r) => return refused(self, r),
        }
        let (reader, side) = self.cache.last_proxy()?;
        let index = frame_at(&side.table, time).index;
        // A proxy frame that does not decode is not worth a layer: the original is read.
        match reader.frame(index) {
            Ok(_) => Some(index),
            Err(_) => refused(self, Refusal::Decode),
        }
    }

    /// The proxy frame `index` of the entry [`pick_proxy`](Self::pick_proxy) just opened.
    fn proxy_frame(&mut self, item: &str, index: usize) -> Result<SourceFrame<'_>, LayerIssue> {
        self.chosen.push((item.to_owned(), index));
        self.proxied += 1;
        let (reader, side) = self
            .cache
            .last_proxy()
            .ok_or_else(|| missing(item, "proxy", None))?;
        let video = reader
            .info()
            .video
            .clone()
            .ok_or_else(|| missing(item, "noVideo", None))?;
        let backend = reader.backend().0;
        let started = Instant::now();
        let f = reader.frame(index);
        self.decodes.push((backend, micros_since(started)));
        let f = f.map_err(|e| decode_issue(item, e))?;
        self.first_decode.get_or_insert((video.codec, backend));
        // The proxy is tagged BT.709 SDR (tone mapped when it was made); the original's
        // rotation and pixel aspect are applied from the entry (not baked into the pixels).
        let colour = merge(video.colour, f.colour).resolve_defaults(f.size.height);
        Ok(SourceFrame {
            format: f.format,
            size: f.size,
            planes: [&f.planes[0].data, &f.planes[1].data, &f.planes[2].data],
            strides: [f.planes[0].stride, f.planes[1].stride, f.planes[2].stride],
            colour,
            hdr: None,
            rotation: side.rotation,
            pixel_aspect: side.pixel_aspect,
        })
    }
}

/// Container tags over bitstream tags, field by field; the range is the container's when it
/// tags anything.
pub(super) fn merge(container: ColourTags, stream: ColourTags) -> ColourTags {
    let tagged = container.primaries != Primaries::Unknown
        || container.transfer != Transfer::Unknown
        || container.matrix != Matrix::Unknown;
    ColourTags {
        primaries: if container.primaries != Primaries::Unknown {
            container.primaries
        } else {
            stream.primaries
        },
        transfer: if container.transfer != Transfer::Unknown {
            container.transfer
        } else {
            stream.transfer
        },
        matrix: if container.matrix != Matrix::Unknown {
            container.matrix
        } else {
            stream.matrix
        },
        range: if tagged {
            container.range
        } else {
            stream.range
        },
        siting: if container.siting != ChromaSiting::Unknown {
            container.siting
        } else {
            stream.siting
        },
    }
}

/// Microseconds since `started`, saturating.
pub(super) fn micros_since(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn decode_issue(asset: &str, e: fin_core::CoreError) -> LayerIssue {
    let mut details = match e.details {
        serde_json::Value::Object(m) => serde_json::Value::Object(m),
        _ => json!({}),
    };
    details["asset"] = json!(asset);
    details["message"] = json!(e.message);
    LayerIssue {
        code: e.code,
        details,
    }
}

impl SourceProvider for Sources<'_> {
    fn frame(&mut self, l: &Layer) -> Result<SourceFrame<'_>, LayerIssue> {
        let s = l
            .source
            .as_ref()
            .ok_or_else(|| missing(&l.item, "noSource", None))?;
        let asset = self
            .manifest
            .asset(&s.asset)
            .ok_or_else(|| missing(&s.asset, "notInManifest", None))?;
        if !self.originals
            && let Some(index) = self.pick_proxy(asset, s.time)
        {
            return self.proxy_frame(&l.item, index);
        }
        let original = asset
            .original
            .as_ref()
            .ok_or_else(|| missing(&s.asset, "missing", None))?;
        let (entry, got) = self.cache.get(asset, original, self.caps);
        match got {
            Got::Opened => self.opened += 1,
            Got::Reused => self.reused += 1,
        }
        let reader = match entry {
            Entry::Failed(issue) => return Err(issue.clone()),
            Entry::Open(o) => &mut o.reader,
        };
        let video = reader
            .info()
            .video
            .clone()
            .ok_or_else(|| missing(&s.asset, "noVideo", None))?;
        let at = frame_at(reader.table(), s.time);
        self.chosen.push((l.item.clone(), at.index));
        let backend = reader.backend().0;
        let started = Instant::now();
        let f = reader.frame(at.index);
        self.decodes.push((backend, micros_since(started)));
        let f = f.map_err(|e| decode_issue(&s.asset, e))?;
        self.first_decode.get_or_insert((video.codec, backend));
        let colour = merge(video.colour, f.colour).resolve_defaults(f.size.height);
        Ok(SourceFrame {
            format: f.format,
            size: f.size,
            planes: [&f.planes[0].data, &f.planes[1].data, &f.planes[2].data],
            strides: [f.planes[0].stride, f.planes[1].stride, f.planes[2].stride],
            colour,
            hdr: video.hdr,
            rotation: video.rotation,
            pixel_aspect: video.pixel_aspect,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fin_media::Range;

    #[test]
    fn container_tags_win_field_by_field() {
        let stream = ColourTags {
            range: Range::Full,
            ..ColourTags::BT709
        };
        let untagged = ColourTags::default();
        assert_eq!(merge(untagged, stream), stream);
        let container = ColourTags {
            primaries: Primaries::Bt2020,
            ..ColourTags::default()
        };
        let m = merge(container, stream);
        assert_eq!(m.primaries, Primaries::Bt2020);
        assert_eq!(m.matrix, Matrix::Bt709);
        assert_eq!(m.range, Range::Limited, "the container tags, so its range");
    }
}
