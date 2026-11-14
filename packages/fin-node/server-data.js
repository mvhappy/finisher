// The server data's identity and where it may live (review #4).
import { native, sync } from "./native.js";

/** This server data's `server-id` (created on first use; renewed for a copied server data). */
export function serverId(serverData) {
  return sync(() => native.serverId(serverData));
}

/** Throws `SERVER_DATA_NOT_LOCAL` unless `path` is on a known local filesystem (or the override is set). */
export function checkServerDataLocal(path) {
  sync(() => native.checkServerDataLocal(path));
}
