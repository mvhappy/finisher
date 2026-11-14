/**
 * `asset.probe` of the catalogued library file at `rel` (the raw chain, `{ components }` of
 * base64 names) below `root`, opened once through the safe opener (`open_for_read`) with the
 * row's `expect` (`{ size, mtimeNs, fileId?, dev? }`) and the library's `hydrate`. Resolves to
 * the `Probe` JSON; rejects `FILE_CLOUD_ONLY`, `FILE_CHANGED`, `FILE_NOT_FOUND`,
 * `PATH_NOT_ALLOWED` (a link, a path leaving the root) or the probe's own error. `display`
 * names the file in errors only; it is never opened.
 */
export declare function libraryProbeFile(
  root: string,
  rel: { readonly components: ReadonlyArray<string> },
  expect: { readonly size: number; readonly mtimeNs: string; readonly fileId?: string; readonly dev?: string },
  hydrate: boolean,
  display: string,
): Promise<string>;
