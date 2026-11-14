// Media caches a test opens itself (offline, without a server's scope), closed before its folder
// goes: Windows cannot delete the open `cache/media/index.db`.
import { MediaCache } from "@finisher/fin-node";

const opened: MediaCache[] = [];

/** `MediaCache.open`, remembered for `closeCaches`. */
export async function openCache(serverData: string, capBytes?: number): Promise<MediaCache> {
  const c = await MediaCache.open(serverData, capBytes);
  opened.push(c);
  return c;
}

/** Closes every cache `openCache` opened since the last call. */
export async function closeCaches(): Promise<void> {
  for (const c of opened.splice(0)) await c.close();
}
