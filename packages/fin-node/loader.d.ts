export interface DiagnosticReport {
  header?: { glibcVersionRuntime?: string };
}
/** "gnu" when the report names a glibc runtime, "musl" when it does not, undefined when unreadable. */
export declare function linuxLibc(getReport?: () => DiagnosticReport | undefined): "gnu" | "musl" | undefined;
export declare function nativeFileNames(platform: string, arch: string, getReport?: () => DiagnosticReport | undefined): string[];
/** The first native binary in `dir` (ending in a separator) that exists and loads; throws the first load failure, or a helpful error when none exists. */
export declare function loadNative(opts: {
  dir: string;
  platform: string;
  arch: string;
  exists: (path: string) => boolean;
  load: (path: string) => unknown;
  getReport?: () => DiagnosticReport | undefined;
}): any;
/** `fin-workers.exe` on win32, else `fin-workers`. */
export declare function workersFileName(platform: string): string;
/** `fin-sandbox-exec.exe` on win32, else `fin-sandbox-exec`. */
export declare function sandboxExecFileName(platform: string): string;
/** `fin-sandbox-exec[.exe]` in `dir` (trailing separators dropped). */
export declare function sandboxExecPath(dir: string, platform: string): string;
/** `fin-workers[.exe]` in `dir` (trailing separators dropped). */
export declare function workersBinaryPath(dir: string, platform: string): string;
