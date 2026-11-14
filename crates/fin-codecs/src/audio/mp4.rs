//! MP4/MOV/M4A audio through `fin-container`'s demuxer (its limits, its first audio track):
//! samples in decode order with their composition time, and the track's `AudioTrackInfo`
//! (priming and padding from the edit list, Opus pre-skip; the reader trims them).

use std::io::{Read, Seek};

use fin_container::{CodecConfig, Demuxer, Limits, TrackRef};
use fin_core::CoreError;
use fin_media::AudioTrackInfo;
use serde_json::json;

/// Packets of one track, in decode order.
pub(crate) trait Packets: Send {
    /// The next packet into `buf`, with its pts in the track's timescale; `None` at the end.
    fn next(&mut self, buf: &mut Vec<u8>) -> Result<Option<i64>, CoreError>;
}

struct Track<R: Read + Seek> {
    demux: Demuxer<R>,
    track: TrackRef,
    next: u32,
}

impl<R: Read + Seek + Send> Packets for Track<R> {
    fn next(&mut self, buf: &mut Vec<u8>) -> Result<Option<i64>, CoreError> {
        let Some(a) = self.demux.audio() else {
            return Ok(None);
        };
        let Some(s) = a.sample(self.next as usize) else {
            return Ok(None);
        };
        self.demux.read_sample(self.track, self.next, buf)?;
        self.next += 1;
        Ok(Some(s.cts))
    }
}

/// The first audio track of an MP4 file: its packets, its info and its configuration record.
pub(crate) fn open<R: Read + Seek + Send + 'static>(
    r: R,
    limits: &Limits,
) -> Result<(Box<dyn Packets>, AudioTrackInfo, CodecConfig), CoreError> {
    let demux = Demuxer::open(r, limits)?;
    let (Some(info), Some(track)) = (demux.info().audio.clone(), demux.audio()) else {
        return Err(fin_container::error::invalid(
            "the file has no audio track",
            json!({ "reason": "noAudioTrack" }),
        ));
    };
    let (track, config) = (track.track, track.config.clone());
    Ok((
        Box::new(Track {
            demux,
            track,
            next: 0,
        }),
        info,
        config,
    ))
}
