// Native binary file names to try, best first (pure, so tests can mock the platform).

/** The libc of this Linux process: `glibcVersionRuntime` is in the diagnostic report only on glibc. */
export function linuxLibc(getReport = () => process.report?.getReport?.()) {
  try {
    const header = getReport()?.header;
    return header && header.glibcVersionRuntime ? "gnu" : "musl";
  } catch {
    return undefined;
  }
}

/**
 * `napi build --platform` appends an ABI on Linux (gnu/musl) and Windows (msvc). On Linux the
 * libc of the running process comes first and the other follows as a fallback; when the report
 * is unavailable both are tried, gnu first.
 */
export function nativeFileNames(platform, arch, getReport) {
  const base = `${platform}-${arch}`;
  let suffixes;
  if (platform === "linux") {
    const libc = linuxLibc(getReport);
    suffixes = libc === "musl" ? ["musl", "gnu"] : ["gnu", "musl"];
  } else {
    suffixes = platform === "win32" ? ["msvc"] : [];
  }
  return [base, ...suffixes.map((s) => `${base}-${s}`)].map((s) => `fin-node.${s}.node`);
}
