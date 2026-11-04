/** A rejected fin-node call: `message` is the core message; `code` is a stable SCREAMING_SNAKE code. */
export interface FinNodeError extends Error {
  code: string;
  details: Record<string, unknown>;
  fix: string | undefined;
}

/**
 * One project's op log. Every method runs on the libuv thread pool and rejects with a
 * `FinNodeError`. JSON payloads follow the fin-core schema (`@finisher/core-types`).
 */
export declare class Engine {
  private constructor();
  /** Creates a project; `rate` is a rational string such as `"30"` or `"30000/1001"`. */
  static create(serverData: string, projectDir: string, name: string, rate: string): Promise<Engine>;
  static open(serverData: string, projectDir: string): Promise<Engine>;
  readonly projectId: string;
  /** Head revision of `branch` (0 when it does not exist). */
  head(branch: string): Promise<number>;
  /** Commits a `Tx` (JSON); resolves to the `CommittedTx` JSON. */
  commit(txJson: string): Promise<string>;
  /** Resolves to `{ rev, project }` JSON. */
  snapshot(branch: string): Promise<string>;
  /** Resolves to the `CommittedTx[]` JSON with `rev > rev`. */
  opsSince(branch: string, rev: number): Promise<string>;
  /** Closes the database; later calls reject with `ENGINE_CLOSED`. */
  close(): Promise<void>;
  /** @internal Test builds only (`test-hooks`): rejects with `INTERNAL` and kills the engine. */
  panicForTest(): Promise<void>;
}

/** Probes an MP4/MOV file; resolves to `{ duration, video, audio, fingerprint, size }` JSON. */
export declare function probe(path: string): Promise<string>;

/** @internal Present only in binaries built with the `test-hooks` feature (`build:test`). */
export declare const testHooks:
  | {
      writeFixtureMp4(
        path: string,
        opts?: { seconds?: number; fps?: number; audio?: boolean; sampleRate?: number },
      ): void;
    }
  | undefined;
