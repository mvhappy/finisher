//! The audio `media` object of `job.done` (6C Task 22): it is recorded as written, and a payload
//! naming a codec this build's table lacks (a later build wrote it) reads as `other` instead of
//! being dropped.

mod support;

use fin_core::events::Source;
use fin_core::events::names_jobs::{JOB_KINDS, WORKER_FAMILIES};
use fin_core::events::names_media::{MEDIA_BACKENDS, MEDIA_CODECS};
use fin_core::events::payloads::{
    CopyPath, JobDone, JobMedia, JobOutcome, JobPriority, ResolutionBucket,
};
use fin_journal::Journal;
use support::*;

fn done() -> JobDone {
    JobDone {
        kind: JOB_KINDS.get("media.pcm").unwrap(),
        family: WORKER_FAMILIES.get("media").unwrap(),
        outcome: JobOutcome::Succeeded,
        code: None,
        attempts: 1,
        priority: JobPriority::Normal,
        warm: false,
        queued_ms: 1,
        run_micros: 2_000,
        items: Some(1),
        items_failed: Some(0),
        isolated: true,
        helpers: 0,
        media: Some(JobMedia {
            codec: MEDIA_CODECS.get("aac").unwrap(),
            backend: MEDIA_BACKENDS.get("symphonia").unwrap(),
            hw: false,
            resolution: ResolutionBucket::Sd,
            frames: 47,
            rt_factor_milli: 90_000,
            cache_hit: false,
            copy_path: CopyPath::Copy,
        }),
    }
}

#[test]
fn an_unknown_audio_codec_reads_as_other() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    // `finisherd` hands the payload over as JSON (`record_value`): a codec the table lacks.
    let mut payload = serde_json::to_value(done()).unwrap();
    assert_eq!(payload["media"]["codec"], "aac");
    payload["media"]["codec"] = "wavpack".into();
    j.record_value(
        Source::Finisherd,
        "01JC0000000000000000000002",
        "job.done",
        2,
        None,
        payload,
    )
    .unwrap();
    // A known audio codec is kept as written.
    j.record(Source::Finisherd, None, done().into());
    let all = read_all(&j);
    let entries = of_kind(&all, "job.done");
    assert_eq!(entries.len(), 2);
    let codecs: Vec<_> = entries
        .iter()
        .map(|e| e.payload["media"]["codec"].clone())
        .collect();
    assert_eq!(codecs, ["other", "aac"]);
    assert_eq!(entries[0].payload["media"]["backend"], "symphonia");
}
