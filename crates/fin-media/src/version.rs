/// The render version (spec §2.5): part of every cache key and golden. Bump it with any change
/// that alters the CPU reference's T0 hashes or the T1 references on purpose; the T0 golden
/// test fails when hashes move without a bump.
pub const RENDER_VERSION: u32 = 2;

/// The audio version (6C plan Global Constraints): to audio what [`RENDER_VERSION`] is to
/// frames, a copy of `fin_audio::AUDIO_VERSION` (which the A0 golden test guards; `fin-audio`
/// fails to build while the two differ), kept here so the hosts, which never link `fin-audio`,
/// put it into audio cache keys (Decision 7).
pub const AUDIO_VERSION: u32 = 1;
