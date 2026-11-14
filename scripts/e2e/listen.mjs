// The "listen" scenario (6C Task 22, spec §8): audio end to end through the real finisherd and its
// `media` worker. A WAV of two tones around a digital silence and an MP4 with Opus audio (the
// `make_audio_fixture` example: Task 13's encoder; and a second clip, AAC-LC on Linux and Opus elsewhere, for a cross-codec loudness check) go on two
// audio tracks with an EQ, a compressor and a fade; `fin media pcm` and `fin media wave` make both
// assets' feeds (they appear in `fin cache status`); `fin listen --json` is checked against
// `listen-golden/listen.json` (loudness ±0.2 LU, true peak ±0.2 dB, silence edges exact at the
// detector's 10 ms); `fin view` shows the effects; the journal's `job.done` of the audio kinds
// carries the `media` object and its `render.stats` the audio phases; and no payload names a path
// (`E2E_LISTEN_BLESS=1` rewrites the golden, refused under CI).
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { PATH_LIKE, WAV_SEGMENTS, checkGolden, makeAudioMp4, writeWav } from "./listen-lib.mjs";
import { assertEqual, fin, live, log, removeTmp, scenarioTmp, startServer, stopServer, storedEntries, waitFor } from "./lib.mjs";

const step = (n, text) => log(`step ${n}: ${text}`);

/** The `job.done` entries of `kind`, waited for (the journal is written after the job settles). */
async function jobsDone(server, kind, count) {
  let done;
  await waitFor(
    `${count} job.done entries of ${kind}`,
    async () => {
      done = (await storedEntries(server, { kinds: ["job.done"] })).filter((e) => e.payload.kind === kind);
      return done.length >= count;
    },
    10_000,
    () => `: ${done?.length ?? 0} found`,
    50,
  );
  return done;
}

export async function run() {
  const started = Date.now();
  const tmp = scenarioTmp("finisher-e2e-listen-");
  live.tmp = tmp;
  const sd = join(tmp, "server");
  const project = join(tmp, "listen");
  let server;
  try {
    mkdirSync(sd, { recursive: true });
    writeFileSync(join(sd, "settings.json"), JSON.stringify({ fonts: { autoFetch: false } }));
    server = await startServer(sd);
    const f = (args) => fin(sd, project, args);

    step(1, "fixtures: a WAV (1 kHz, silence, 440 Hz) and an MP4 with Opus audio");
    fin(sd, tmp, ["project", "create", project, "--name", "Listen"]);
    const wavSeconds = writeWav(join(project, "tones.wav"));
    makeAudioMp4(join(project, "voice.mp4"), "opus");
    // The cross-codec check's second clip: AAC-LC where vo-aacenc builds (Linux), a second Opus file elsewhere.
    const otherCodec = process.platform === "linux" ? "aac" : "opus";
    makeAudioMp4(join(project, `voice-${otherCodec}.mp4`), otherCodec);

    step(2, "two audio tracks; the WAV on A1 with an EQ, a compressor and a fade; the Opus clip on A2");
    const track = f(["track", "add", "--kind", "audio"]).bindings.$track;
    assertEqual(track, "t3", "the second audio track");
    assertEqual([f(["asset", "add", "tones.wav"]).resolved.kind, f(["asset", "add", "voice.mp4"]).resolved.kind], ["media", "media"], "asset kinds");
    const wav = f(["clip", "insert", "--track", "t2", "--asset", "a1", "--at", "0", "--dur", `${wavSeconds}s`]).bindings.$clip;
    const opus = f(["clip", "insert", "--track", "t3", "--asset", "a2", "--at", "3s", "--dur", "3s"]).bindings.$clip;
    for (const clip of [wav, opus]) {
      f(["effect", "add", clip, "eq", "--set", "eq.enabled=true", "--set", "eq.band1Type=highPass", "--set", "eq.band1Freq=60", "--set", "eq.band5Type=peak", "--set", "eq.band5Gain=-1.5"]);
      f(["effect", "add", clip, "compressor", "--set", "compressor.enabled=true", "--set", "compressor.threshold=-12", "--set", "compressor.ratio=2"]);
    }
    f(["param", "set", wav, "audio.fadeIn", "1/4"]);
    f(["param", "set", opus, "audio.fadeOut", "1/2"]);
    const view = f(["view"]).lines.join("\n");
    for (const kind of ["eq", "compressor"]) if (!new RegExp(`\\b${kind}\\b`).test(view)) throw new Error(`fin view does not show the ${kind} effect:\n${view}`);

    step(3, "fin media pcm and fin media wave of both assets; the feeds are in the cache");
    const recipes = () => fin(sd, tmp, ["cache", "status"]).byRecipe ?? {};
    for (const asset of ["a1", "a2"]) {
      const pcm = f(["media", "pcm", asset]);
      assertEqual([pcm.state, pcm.recipe], ["ready", "pcm48"], `the pcm48 feed of ${asset}`);
      const wave = f(["media", "wave", asset]);
      assertEqual(wave.state, "ready", `the waveform of ${asset}`);
    }
    const made = recipes();
    assertEqual([made.pcm48?.entries, made.wave?.entries], [2, 2], "cache entries: a pcm48 feed and a waveform per asset");
    assertEqual(f(["media", "pcm", "a1"]).cached, true, "the second ask is a cache hit");

    step(4, "fin listen --json against the golden");
    const listen = f(["listen", "--json"]);
    checkGolden(listen, log);
    assertEqual(listen.mix.silences.length >= 1, true, "the WAV's silence is reported");
    assertEqual(listen.tracks.map((t) => t.track), ["t2", "t3"], "tracks");
    // The two pcm feeds of a1 and a2, before the cross-codec asset adds a third.
    const pcm = await jobsDone(server, "media.pcm", 2);
    // Cross-codec: the same tone as a clean Opus clip (no effects, no fade) and as `other`, each alone
    // on A2 after the golden's range, read by `fin listen --json` over its own 3 s.
    {
      const otherAsset = f(["asset", "add", `voice-${otherCodec}.mp4`]).bindings.$asset;
      f(["clip", "insert", "--track", "t3", "--asset", "a2", "--at", "7s", "--dur", "3s"]);
      f(["clip", "insert", "--track", "t3", "--asset", otherAsset, "--at", "11s", "--dur", "3s"]);
      const t3 = (from, to) => f(["listen", "--json", "--from", from, "--to", to]).tracks.find((t) => t.track === "t3").stats.integrated;
      const [o, a] = [t3("7s", "10s"), t3("11s", "14s")];
      if (!Number.isFinite(o) || !Number.isFinite(a) || Math.abs(o - a) > 0.5) throw new Error(`${otherCodec} ${a} LUFS vs Opus ${o} LUFS for the same tone`);
      log(`cross-codec: opus ${o.toFixed(2)} LUFS, ${otherCodec} ${a.toFixed(2)} LUFS`);
    }

    step(5, "the journal: job.done carries the audio media object; render.stats names the audio phases; no path anywhere");
    for (const kind of ["media.pcm", "media.waveform", "media.listen"]) {
      for (const e of await jobsDone(server, kind, 1)) {
        const m = e.payload.media;
        if (!m) throw new Error(`${kind}: job.done has no media object: ${JSON.stringify(e.payload)}`);
        assertEqual([m.hw, m.resolution, m.copyPath, typeof m.codec], [false, "sd", "copy", "string"], `${kind} job.done.media`);
        if (!Number.isInteger(m.frames) || m.frames <= 0) throw new Error(`${kind}: media.frames ${m.frames}`);
        if (!Number.isInteger(m.rtFactorMilli)) throw new Error(`${kind}: media.rtFactorMilli ${m.rtFactorMilli}`);
      }
    }
    // The first feed of each asset was decoded: Opus by libopus, the WAV by symphonia (`pcm` codec).
    const decoded = pcm.map((e) => [e.payload.media.codec, e.payload.media.backend, e.payload.media.cacheHit]).sort();
    assertEqual(decoded, [["opus", "libopus", false], ["pcm", "symphonia", false]], "what the two pcm feeds decoded");
    const stats = await storedEntries(server, { kinds: ["render.stats"] });
    const phases = new Set(stats.map((e) => e.payload.phase));
    for (const phase of ["decodeAudio", "mix"]) if (!phases.has(phase)) throw new Error(`no render.stats entry for the ${phase} phase (${[...phases].join(", ")})`);
    const text = JSON.stringify([...(await storedEntries(server, { kinds: ["job.done", "render.stats", "media.capabilities"] }))]);
    const leak = PATH_LIKE.exec(text);
    if (leak) throw new Error(`a diagnostics payload names a path or a file: …${text.slice(Math.max(0, leak.index - 40), leak.index + 60)}…`);
    log(`listen scenario done in ${((Date.now() - started) / 1000).toFixed(1)}s (${WAV_SEGMENTS.length} segments)`);
  } finally {
    if (server) await stopServer(server);
    removeTmp(tmp);
    live.tmp = undefined;
  }
}
