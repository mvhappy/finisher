//! Which libopus `third_party/opus` holds, and the exact source lists this crate compiles
//! (6C plan Decision 11). Shared by `build.rs` (which refuses any other release) and the
//! crate's unit tests (`opus_wrong_version_refused_by_build`), so the check itself is tested.
//!
//! A git checkout of libopus has no version file (`meson/get-version.py` asks `git describe`,
//! which a shallow submodule cannot answer), so the release is recognised by its libtool
//! version in `configure.ac` (1.6.1 bumped the revision to 11:1:11), by `package_version` when
//! a release tarball provides one, and by the `.mk` source lists, which must equal the lists
//! below exactly: moving the submodule fails the build until they are re-checked.

/// The libopus release `third_party/opus` is pinned to.
pub const VERSION: &str = "1.6.1";

/// `OPUS_LT_CURRENT`, `OPUS_LT_REVISION` and `OPUS_LT_AGE` in 1.6.1's `configure.ac`.
pub const LIBTOOL: (u32, u32, u32) = (11, 1, 11);

/// `celt_sources.mk` `CELT_SOURCES`.
pub const CELT_SOURCES: &[&str] = &[
    "celt/bands.c",
    "celt/celt.c",
    "celt/celt_encoder.c",
    "celt/celt_decoder.c",
    "celt/cwrs.c",
    "celt/entcode.c",
    "celt/entdec.c",
    "celt/entenc.c",
    "celt/kiss_fft.c",
    "celt/laplace.c",
    "celt/mathops.c",
    "celt/mdct.c",
    "celt/modes.c",
    "celt/pitch.c",
    "celt/celt_lpc.c",
    "celt/quant_bands.c",
    "celt/rate.c",
    "celt/vq.c",
];

/// `silk_sources.mk` `SILK_SOURCES`.
pub const SILK_SOURCES: &[&str] = &[
    "silk/CNG.c",
    "silk/code_signs.c",
    "silk/init_decoder.c",
    "silk/decode_core.c",
    "silk/decode_frame.c",
    "silk/decode_parameters.c",
    "silk/decode_indices.c",
    "silk/decode_pulses.c",
    "silk/decoder_set_fs.c",
    "silk/dec_API.c",
    "silk/enc_API.c",
    "silk/encode_indices.c",
    "silk/encode_pulses.c",
    "silk/gain_quant.c",
    "silk/interpolate.c",
    "silk/LP_variable_cutoff.c",
    "silk/NLSF_decode.c",
    "silk/NSQ.c",
    "silk/NSQ_del_dec.c",
    "silk/PLC.c",
    "silk/shell_coder.c",
    "silk/tables_gain.c",
    "silk/tables_LTP.c",
    "silk/tables_NLSF_CB_NB_MB.c",
    "silk/tables_NLSF_CB_WB.c",
    "silk/tables_other.c",
    "silk/tables_pitch_lag.c",
    "silk/tables_pulses_per_block.c",
    "silk/VAD.c",
    "silk/control_audio_bandwidth.c",
    "silk/quant_LTP_gains.c",
    "silk/VQ_WMat_EC.c",
    "silk/HP_variable_cutoff.c",
    "silk/NLSF_encode.c",
    "silk/NLSF_VQ.c",
    "silk/NLSF_unpack.c",
    "silk/NLSF_del_dec_quant.c",
    "silk/process_NLSFs.c",
    "silk/stereo_LR_to_MS.c",
    "silk/stereo_MS_to_LR.c",
    "silk/check_control_input.c",
    "silk/control_SNR.c",
    "silk/init_encoder.c",
    "silk/control_codec.c",
    "silk/A2NLSF.c",
    "silk/ana_filt_bank_1.c",
    "silk/biquad_alt.c",
    "silk/bwexpander_32.c",
    "silk/bwexpander.c",
    "silk/debug.c",
    "silk/decode_pitch.c",
    "silk/inner_prod_aligned.c",
    "silk/lin2log.c",
    "silk/log2lin.c",
    "silk/LPC_analysis_filter.c",
    "silk/LPC_inv_pred_gain.c",
    "silk/table_LSF_cos.c",
    "silk/NLSF2A.c",
    "silk/NLSF_stabilize.c",
    "silk/NLSF_VQ_weights_laroia.c",
    "silk/pitch_est_tables.c",
    "silk/resampler.c",
    "silk/resampler_down2_3.c",
    "silk/resampler_down2.c",
    "silk/resampler_private_AR2.c",
    "silk/resampler_private_down_FIR.c",
    "silk/resampler_private_IIR_FIR.c",
    "silk/resampler_private_up2_HQ.c",
    "silk/resampler_rom.c",
    "silk/sigm_Q15.c",
    "silk/sort.c",
    "silk/sum_sqr_shift.c",
    "silk/stereo_decode_pred.c",
    "silk/stereo_encode_pred.c",
    "silk/stereo_find_predictor.c",
    "silk/stereo_quant_pred.c",
    "silk/LPC_fit.c",
];

/// `silk_sources.mk` `SILK_SOURCES_FLOAT`.
pub const SILK_SOURCES_FLOAT: &[&str] = &[
    "silk/float/apply_sine_window_FLP.c",
    "silk/float/corrMatrix_FLP.c",
    "silk/float/encode_frame_FLP.c",
    "silk/float/find_LPC_FLP.c",
    "silk/float/find_LTP_FLP.c",
    "silk/float/find_pitch_lags_FLP.c",
    "silk/float/find_pred_coefs_FLP.c",
    "silk/float/LPC_analysis_filter_FLP.c",
    "silk/float/LTP_analysis_filter_FLP.c",
    "silk/float/LTP_scale_ctrl_FLP.c",
    "silk/float/noise_shape_analysis_FLP.c",
    "silk/float/process_gains_FLP.c",
    "silk/float/regularize_correlations_FLP.c",
    "silk/float/residual_energy_FLP.c",
    "silk/float/warped_autocorrelation_FLP.c",
    "silk/float/wrappers_FLP.c",
    "silk/float/autocorrelation_FLP.c",
    "silk/float/burg_modified_FLP.c",
    "silk/float/bwexpander_FLP.c",
    "silk/float/energy_FLP.c",
    "silk/float/inner_product_FLP.c",
    "silk/float/k2a_FLP.c",
    "silk/float/LPC_inv_pred_gain_FLP.c",
    "silk/float/pitch_analysis_core_FLP.c",
    "silk/float/scale_copy_vector_FLP.c",
    "silk/float/scale_vector_FLP.c",
    "silk/float/schur_FLP.c",
    "silk/float/sort_FLP.c",
];

/// `opus_sources.mk` `OPUS_SOURCES`.
pub const OPUS_SOURCES: &[&str] = &[
    "src/opus.c",
    "src/opus_decoder.c",
    "src/opus_encoder.c",
    "src/extensions.c",
    "src/opus_multistream.c",
    "src/opus_multistream_encoder.c",
    "src/opus_multistream_decoder.c",
    "src/repacketizer.c",
    "src/opus_projection_encoder.c",
    "src/opus_projection_decoder.c",
    "src/mapping_matrix.c",
];

/// `opus_sources.mk` `OPUS_SOURCES_FLOAT`.
pub const OPUS_SOURCES_FLOAT: &[&str] = &["src/analysis.c", "src/mlp.c", "src/mlp_data.c"];

/// Every `.mk` variable compiled, with the file it is in and the list it must equal.
pub const LISTS: &[(&str, &str, &[&str])] = &[
    ("celt_sources.mk", "CELT_SOURCES", CELT_SOURCES),
    ("silk_sources.mk", "SILK_SOURCES", SILK_SOURCES),
    ("silk_sources.mk", "SILK_SOURCES_FLOAT", SILK_SOURCES_FLOAT),
    ("opus_sources.mk", "OPUS_SOURCES", OPUS_SOURCES),
    ("opus_sources.mk", "OPUS_SOURCES_FLOAT", OPUS_SOURCES_FLOAT),
];

/// The files of `var` in a `.mk` text (`VAR = \` then one file a line, to the first line
/// without a trailing backslash).
pub fn mk_list(mk: &str, var: &str) -> Option<Vec<String>> {
    let mut lines = mk.lines();
    let first = lines.find(|l| {
        let mut w = l.split_whitespace();
        w.next() == Some(var) && w.next() == Some("=")
    })?;
    let mut out = Vec::new();
    let mut more = first.trim_end().ends_with('\\');
    let rest = first.split_once('=').map_or("", |(_, r)| r);
    out.extend(words(rest));
    for l in lines {
        if !more {
            break;
        }
        more = l.trim_end().ends_with('\\');
        out.extend(words(l));
    }
    Some(out)
}

fn words(l: &str) -> impl Iterator<Item = String> + '_ {
    l.split_whitespace()
        .filter(|w| *w != "\\")
        .map(str::to_string)
}

/// `NAME=value` in `configure.ac`.
fn lt(configure_ac: &str, name: &str) -> Option<u32> {
    configure_ac.lines().find_map(|l| {
        let (k, v) = l.trim().split_once('=')?;
        (k == name).then(|| v.trim().parse().ok()).flatten()
    })
}

/// `Ok` when the sources are libopus [`VERSION`]: `configure.ac`'s libtool version is
/// [`LIBTOOL`], `package_version` (when present) names [`VERSION`], and `mk(file)` gives each
/// `.mk` file whose lists must equal [`LISTS`]. The error says what differs.
pub fn check(
    configure_ac: &str,
    package_version: Option<&str>,
    mk: impl Fn(&str) -> Option<String>,
) -> Result<(), String> {
    let got = (
        lt(configure_ac, "OPUS_LT_CURRENT"),
        lt(configure_ac, "OPUS_LT_REVISION"),
        lt(configure_ac, "OPUS_LT_AGE"),
    );
    let want = (Some(LIBTOOL.0), Some(LIBTOOL.1), Some(LIBTOOL.2));
    if got != want {
        return Err(format!(
            "third_party/opus is not libopus {VERSION}: configure.ac's libtool version is \
             {got:?}, {VERSION}'s is {LIBTOOL:?}"
        ));
    }
    if let Some(pv) = package_version {
        let named = pv
            .lines()
            .any(|l| l.trim() == format!("PACKAGE_VERSION=\"{VERSION}\""));
        if !named {
            return Err(format!(
                "third_party/opus is not libopus {VERSION}: package_version says {}",
                pv.trim()
            ));
        }
    }
    for (file, var, want) in LISTS {
        let text = mk(file).ok_or_else(|| format!("third_party/opus/{file} is missing"))?;
        let got = mk_list(&text, var).ok_or_else(|| format!("{file} has no {var}"))?;
        if got != *want {
            return Err(format!(
                "{file} {var} differs from fin-opus-sys's list: re-check build.rs's source \
                 lists against libopus and update VERSION, third_party/native.json and the \
                 notices (got {got:?})"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const AC: &str = "AC_INIT([opus],[CURRENT_VERSION])\nOPUS_LT_CURRENT=11\nOPUS_LT_REVISION=1\nOPUS_LT_AGE=11\n";

    fn mk_of(file: &str) -> Option<String> {
        let mut s = String::new();
        for (f, var, list) in LISTS {
            if *f == file {
                s.push_str(&format!("{var} = \\\n{}\n\n", list.join(" \\\n")));
                s.push_str(&format!("{var}_X86_RTCD = \\\nx.c\n\n"));
            }
        }
        (!s.is_empty()).then_some(s)
    }

    #[test]
    fn opus_right_version_accepted() {
        assert_eq!(check(AC, None, mk_of), Ok(()));
        let pv = "PACKAGE_VERSION=\"1.6.1\"\n";
        assert_eq!(check(AC, Some(pv), mk_of), Ok(()));
    }

    #[test]
    fn opus_wrong_version_refused_by_build() {
        let old = AC.replace("OPUS_LT_REVISION=1", "OPUS_LT_REVISION=0");
        let e = check(&old, None, mk_of).unwrap_err();
        assert!(e.contains("not libopus 1.6.1"), "{e}");
        let e = check(AC, Some("PACKAGE_VERSION=\"1.6\"\n"), mk_of).unwrap_err();
        assert!(e.contains("package_version"), "{e}");
        let e = check(&AC.replace("OPUS_LT_AGE=11\n", ""), None, mk_of).unwrap_err();
        assert!(e.contains("libtool version"), "{e}");
    }

    #[test]
    fn a_changed_source_list_is_refused() {
        let moved = |f: &str| mk_of(f).map(|s| s.replace("celt/vq.c", "celt/vq2.c"));
        let e = check(AC, None, moved).unwrap_err();
        assert!(e.contains("CELT_SOURCES differs"), "{e}");
        let missing = |f: &str| (f != "opus_sources.mk").then(|| mk_of(f)).flatten();
        assert!(check(AC, None, missing).unwrap_err().contains("missing"));
    }

    /// The pinned submodule itself passes (the build ran this check too).
    #[test]
    fn the_submodule_is_the_pinned_release() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../third_party/opus/");
        let read = |f: &str| std::fs::read_to_string(format!("{root}{f}")).ok();
        let ac = read("configure.ac").expect("the submodule is checked out");
        assert_eq!(check(&ac, read("package_version").as_deref(), read), Ok(()));
    }
}
