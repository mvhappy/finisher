// Types of `pipe.js`: the native Windows pipe listener (re-exported by `index.d.ts`).

/** `new PipeServer(name, options)`. */
export interface PipeOptions {
  /** SIDs allowed beside this user (sub-project 7: AppContainer SIDs). */
  allow?: string[];
  /** Client instances at most (the connections and the one waiting), 2..=253; default 64. */
  maxConnections?: number;
}

/** What `PipeServer.onEvent` delivers. */
export type PipeEvent =
  /** A client that passed the identity check: its connection id, process and token user SID. */
  | { type: "connection"; id: number; pid: number; sid: string; appContainer?: string | null }
  /** A client disconnected before its first byte was read: `peer` (another user, or a container not allowed) or `unidentified`. */
  | { type: "refused"; pid: number; sid?: string | null; appContainer?: string | null; reason: "peer" | "unidentified" };

/**
 * finisherd's native Windows named-pipe listener (5A Task 11, spec §4.3). Every instance is
 * created with the descriptor `D:P(A;;GA;;;<user SID>)(A;;GA;;;SY)` (plus read and write-data
 * for each allowed SID), `PIPE_REJECT_REMOTE_CLIENTS`, and `FILE_FLAG_FIRST_PIPE_INSTANCE` on
 * the first, so the constructor throws `PIPE_SQUATTED` when the name exists already. A client
 * whose token is neither this user nor an allowed SID is disconnected before anything is read
 * (`refused`). Off Windows the constructor throws `PIPE_UNSUPPORTED`.
 */
export declare class PipeServer {
  constructor(name: string, options?: PipeOptions);
  /** This user's SID, which the descriptor grants. */
  readonly userSid: string;
  /** Delivers every event, those before the call first; does not keep the process alive. */
  onEvent(callback: (event: PipeEvent) => void): void;
  /** The next bytes (at most 64 KiB) of connection `id`, or `null` once it has ended. One read at a time. */
  read(id: number): Promise<Buffer | null>;
  /** Resolves once `data` is in the pipe; rejects `ERROR_BROKEN_PIPE` when the client went away. */
  write(id: number, data: Uint8Array): Promise<void>;
  /** Closes connection `id` once its client has read what was written (at most 2 s). */
  close(id: number): void;
  /** Allows `sid` too (sub-project 7: an AppContainer SID); throws `INVALID_SID`. */
  allow(sid: string): void;
  /** Closes every connection and the pipe; returns once the name is free. Idempotent. */
  shutdown(): void;
}
