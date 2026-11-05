export interface DiagnosticReport {
  header?: { glibcVersionRuntime?: string };
}
/** "gnu" when the report names a glibc runtime, "musl" when it does not, undefined when unreadable. */
export declare function linuxLibc(getReport?: () => DiagnosticReport | undefined): "gnu" | "musl" | undefined;
export declare function nativeFileNames(platform: string, arch: string, getReport?: () => DiagnosticReport | undefined): string[];
