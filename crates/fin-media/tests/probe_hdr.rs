//! `merge_hdr` (6B-1 Task 11, `media.probeDeep`): SEI fills the HDR fields the boxes lack.

use fin_media::jobs::{HdrSource, merge_hdr};
use fin_media::{ContentLight, DolbyVision, HdrInfo, MasteringDisplay};

fn mastering(max: u32) -> MasteringDisplay {
    MasteringDisplay {
        primaries: [[0; 2]; 3],
        white_point: [0; 2],
        max_luminance: max,
        min_luminance: 0,
    }
}
fn light(max_cll: u16) -> ContentLight {
    ContentLight {
        max_cll,
        max_fall: 0,
    }
}
fn hdr(m: Option<MasteringDisplay>, l: Option<ContentLight>, dv: Option<DolbyVision>) -> HdrInfo {
    HdrInfo {
        mastering: m,
        content_light: l,
        dolby_vision: dv,
    }
}

#[test]
fn complete_boxes_never_read_sei() {
    let boxes = hdr(Some(mastering(1)), Some(light(2)), None);
    let got = merge_hdr(Some(boxes), || panic!("SEI read"));
    assert_eq!(got, (Some(boxes), Some(HdrSource::Container)));
}

#[test]
fn dolby_vision_alone_takes_both_fields_from_sei() {
    let dv = DolbyVision {
        profile: 8,
        level: 6,
        rpu: true,
        enhancement_layer: false,
        base_layer: true,
        compatibility: 1,
    };
    let sei = hdr(Some(mastering(10_000_000)), Some(light(1000)), None);
    let got = merge_hdr(Some(hdr(None, None, Some(dv))), || Some(sei));
    let want = hdr(sei.mastering, sei.content_light, Some(dv));
    assert_eq!(got, (Some(want), Some(HdrSource::ContainerAndSei)));
}

#[test]
fn mastering_box_without_clli_takes_the_light_level_from_sei() {
    let sei = hdr(Some(mastering(99)), Some(light(400)), None);
    let got = merge_hdr(Some(hdr(Some(mastering(1)), None, None)), || Some(sei));
    let want = hdr(Some(mastering(1)), Some(light(400)), None);
    assert_eq!(got, (Some(want), Some(HdrSource::ContainerAndSei)));
}

#[test]
fn no_boxes_is_sei_and_nothing_is_none() {
    let sei = hdr(None, Some(light(400)), None);
    assert_eq!(
        merge_hdr(None, || Some(sei)),
        (Some(sei), Some(HdrSource::Sei))
    );
    assert_eq!(merge_hdr(None, || None), (None, None));
    let empty = HdrInfo::default();
    assert_eq!(merge_hdr(Some(empty), || Some(empty)), (None, None));
}

#[test]
fn sei_with_nothing_new_keeps_the_container_source() {
    let boxes = hdr(Some(mastering(1)), None, None);
    let sei = hdr(Some(mastering(2)), None, None);
    assert_eq!(
        merge_hdr(Some(boxes), || Some(sei)),
        (Some(boxes), Some(HdrSource::Container))
    );
}
