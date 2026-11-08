// The native Windows pipe listener binding (5A Task 11): `PipeServer` over
// crates/fin-node/src/pipe_server.rs. Events cross as JSON; this file parses them.
import { call, native, sync } from "./native.js";

/**
 * finisherd's native Windows named-pipe listener (5A Task 11); see `pipe.d.ts`. Native
 * failures carry their code (`PIPE_SQUATTED`, `ERROR_BROKEN_PIPE`, …).
 */
export class PipeServer {
  #native;
  constructor(name, options) {
    this.#native = sync(() => new native.PipeServer(name, options));
  }
  get userSid() {
    return this.#native.userSid;
  }
  onEvent(callback) {
    this.#native.onEvent((json) => callback(JSON.parse(json)));
  }
  read(id) {
    return call(this.#native.read(id));
  }
  write(id, data) {
    return call(this.#native.write(id, data));
  }
  close(id) {
    this.#native.close(id);
  }
  allow(sid) {
    sync(() => this.#native.allow(sid));
  }
  shutdown() {
    this.#native.shutdown();
  }
}
