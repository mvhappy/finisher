/// The render version (spec §2.5): part of every cache key and golden. Bump it with any change
/// that alters the CPU reference's T0 hashes or the T1 references on purpose; the T0 golden
/// test fails when hashes move without a bump.
pub const RENDER_VERSION: u32 = 2;
