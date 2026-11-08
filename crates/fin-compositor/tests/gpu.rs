//! The wgpu compositor against the CPU reference (spec §13.3, T1), on the adapter `FIN_GPU`
//! picks (`auto`, `software`, `metal`, `dx12`, `vulkan`). Without one the tests print a notice
//! and pass; `FIN_REQUIRE_GPU=1` makes them fail instead. A case outside T1 writes the
//! reference, the GPU render and a diff to `<target>/fin-compositor-t1/<backend>/`.
//! `--nocapture` prints each case's measures and timings. Not on WASI (no wgpu there).

#![cfg(not(target_os = "wasi"))]

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{Case, cases, write_png};
use fin_compositor::compare::{diff_image, passes, t1};
use fin_compositor::{AdapterChoice, Compositor, Gpu, Stage, reference};

fn gpu() -> Option<Gpu> {
    let choice = AdapterChoice::from_env().expect("FIN_GPU");
    match Gpu::new(choice) {
        Ok(g) => {
            eprintln!("gpu: {:?}", g.info());
            Some(g)
        }
        Err(e) => {
            assert!(
                std::env::var("FIN_REQUIRE_GPU").as_deref() != Ok("1"),
                "FIN_REQUIRE_GPU=1 but there is no adapter for {choice:?}: {e}"
            );
            eprintln!("notice: no GPU adapter for {choice:?}, skipping ({e})");
            None
        }
    }
}

fn out_dir(gpu: &Gpu) -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target")));
    target.join("fin-compositor-t1").join(gpu.info().backend)
}

fn render(c: &mut Compositor, gpu: &Gpu, case: &Case) -> Vec<u8> {
    let mut src = case.patterns();
    let r = c.render(gpu, &case.plan, &mut src, case.size()).unwrap();
    assert!(
        r.rgba8.chunks(4).all(|p| p[3] == 255),
        "{}: opaque",
        case.name
    );
    r.rgba8
}

#[test]
fn every_corpus_case_is_within_t1_of_the_reference() {
    let Some(gpu) = gpu() else { return };
    let mut c = Compositor::new(&gpu);
    let mut failed = Vec::new();
    let (mut cpu_total, mut gpu_total) = (Duration::ZERO, Duration::ZERO);
    for case in cases() {
        let t = Instant::now();
        let want = reference::render(&case.plan, &mut case.patterns(), case.size());
        let cpu = t.elapsed();
        // The first render of a case pays for pipelines and pool textures; time the second.
        let mut src = case.patterns();
        let first = c.render(&gpu, &case.plan, &mut src, case.size()).unwrap();
        let t = Instant::now();
        let got = c.render(&gpu, &case.plan, &mut src, case.size()).unwrap();
        let took = t.elapsed();
        assert_eq!(
            first.rgba8, got.rgba8,
            "{}: repeated renders agree",
            case.name
        );
        assert_eq!(
            got.skipped, want.skipped,
            "{}: the same layers left out",
            case.name
        );
        let m = t1(&want.rgba8, &got.rgba8);
        eprintln!(
            "{:<32} psnr {:>7.2} dB  max {:>2}  over2 {:.5}  cpu {:>7.1} ms  gpu {:>6.2} ms",
            case.name,
            m.psnr,
            m.max_abs,
            m.over_2,
            cpu.as_secs_f64() * 1e3,
            took.as_secs_f64() * 1e3
        );
        cpu_total += cpu;
        gpu_total += took;
        if !passes(&m) {
            let dir = out_dir(&gpu);
            std::fs::create_dir_all(&dir).unwrap();
            let dir = dir.to_str().unwrap();
            write_png(
                dir,
                &format!("{}-reference", case.name),
                case.size(),
                &want.rgba8,
            );
            write_png(dir, &format!("{}-gpu", case.name), case.size(), &got.rgba8);
            let diff = diff_image(&want.rgba8, &got.rgba8);
            write_png(dir, &format!("{}-diff", case.name), case.size(), &diff);
            failed.push((case.name.clone(), m));
        }
    }
    eprintln!(
        "total: cpu {:.1} ms, gpu {:.1} ms",
        cpu_total.as_secs_f64() * 1e3,
        gpu_total.as_secs_f64() * 1e3
    );
    assert!(
        failed.is_empty(),
        "outside T1 on {:?}: {failed:?} (images in {})",
        gpu.info(),
        out_dir(&gpu).display()
    );
}

/// The fused fixed-function path and `blend.wgsl` give the same pictures for the modes both
/// can draw (`normal`, `screen`, `add`, crossfades of them).
#[test]
fn fixed_function_and_shader_blends_agree() {
    let Some(gpu) = gpu() else { return };
    let (mut fused, mut shader) = (Compositor::new(&gpu), Compositor::new(&gpu));
    shader.set_shader_blends(true);
    let names = [
        "blend-normal",
        "blend-screen",
        "blend-add",
        "two-tracks",
        "crossfade-0.5",
    ];
    let all = cases();
    for name in names {
        let case = all.iter().find(|c| c.name == name).unwrap();
        let (a, b) = (
            render(&mut fused, &gpu, case),
            render(&mut shader, &gpu, case),
        );
        let m = t1(&a, &b);
        assert!(passes(&m), "{name}: fused and shader blends differ: {m:?}");
    }
    assert!(
        fused
            .pipeline_stages()
            .iter()
            .all(|s| !matches!(s, Stage::Blend(_))),
        "the fused renders used no shader blend"
    );
    assert!(
        shader
            .pipeline_stages()
            .iter()
            .all(|s| !matches!(s, Stage::Layer(Some(_)))),
        "the shader renders used no fixed-function blend"
    );
}

/// `blend.wgsl` builds for every `MODE` on this backend: with the override substituted, FXC
/// (D3D12) refused `overlay` while Metal took it ("X3507: not all control paths return a
/// value"). Shader blends route all seven modes through it.
#[test]
fn every_blend_mode_builds_on_this_backend() {
    let Some(gpu) = gpu() else { return };
    let mut c = Compositor::new(&gpu);
    c.set_shader_blends(true);
    let all = cases();
    for name in [
        "blend-normal",
        "blend-multiply",
        "blend-screen",
        "blend-overlay",
        "blend-add",
        "blend-darken",
        "blend-lighten",
    ] {
        let case = all.iter().find(|c| c.name == name).unwrap();
        if let Err(e) = c.render(&gpu, &case.plan, &mut case.patterns(), case.size()) {
            panic!("{name} on {:?}: {e:?}", gpu.info());
        }
    }
    let stages = c.pipeline_stages();
    for m in 0..7 {
        assert!(
            stages.contains(&Stage::Blend(m)),
            "Blend({m}) in {stages:?}"
        );
    }
}

#[test]
fn pipelines_are_created_once_per_stage_and_mode() {
    let Some(gpu) = gpu() else { return };
    let mut c = Compositor::new(&gpu);
    let all = cases();
    for case in &all {
        render(&mut c, &gpu, case);
    }
    let created = c.pipelines_created();
    let stages = c.pipeline_stages();
    assert_eq!(created, stages.len(), "no stage was built twice");
    // Ingest, mip, output, the layer pass fused with normal/screen/add or to a scratch
    // target, and blend.wgsl for the seven modes (dissolves of normal included).
    assert!(created <= 14, "{stages:?}");
    for s in [
        Stage::Ingest,
        Stage::Mip,
        Stage::Output,
        Stage::Layer(None),
        Stage::Blend(1),
    ] {
        assert!(stages.contains(&s), "{s:?} in {stages:?}");
    }
    for case in &all {
        render(&mut c, &gpu, case);
    }
    assert_eq!(
        c.pipelines_created(),
        created,
        "a second pass builds nothing"
    );
}

#[test]
fn the_texture_pool_does_not_grow_over_100_frames() {
    let Some(gpu) = gpu() else { return };
    let all = cases();
    for name in [
        "two-tracks",
        "blend-multiply",
        "dissolve-0.5",
        "minified-0.3",
    ] {
        let case = all.iter().find(|c| c.name == name).unwrap();
        let mut c = Compositor::new(&gpu);
        let first = render(&mut c, &gpu, case);
        let stats = c.pool_stats();
        assert!(stats.textures > 0);
        for _ in 0..100 {
            assert_eq!(render(&mut c, &gpu, case), first, "{name}");
        }
        assert_eq!(
            c.pool_stats(),
            stats,
            "{name}: the pool kept its first frame's set"
        );
    }
}

#[test]
fn a_lost_device_is_gpu_device_lost() {
    let Some(gpu) = gpu() else { return };
    let all = cases();
    let case = all.iter().find(|c| c.name == "single-clip").unwrap();
    let mut c = Compositor::new(&gpu);
    render(&mut c, &gpu, case);
    gpu.device().destroy();
    let err = c
        .render(&gpu, &case.plan, &mut case.patterns(), case.size())
        .unwrap_err();
    assert_eq!(err.code, "GPU_DEVICE_LOST", "{err:?}");
    // And it stays lost.
    let err = c
        .render(&gpu, &case.plan, &mut case.patterns(), case.size())
        .unwrap_err();
    assert_eq!(err.code, "GPU_DEVICE_LOST", "{err:?}");
}

#[test]
fn an_empty_output_renders_nothing() {
    let Some(gpu) = gpu() else { return };
    let all = cases();
    let case = all.iter().find(|c| c.name == "single-clip").unwrap();
    let mut c = Compositor::new(&gpu);
    let r = c
        .render(
            &gpu,
            &case.plan,
            &mut case.patterns(),
            fin_media::Size::new(0, 0),
        )
        .unwrap();
    assert!(r.rgba8.is_empty() && r.skipped.is_empty());
}
