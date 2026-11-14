// The CLI harnesses' model lists, discovered from the installed CLI, never hardcoded (owner
// decision after review 7A-1; the approach t3code takes):
// - Codex: the app-server's `model/list` request, paged by `nextCursor`; each entry names its
//   `model` id, `displayName`, `isDefault` and `hidden`.
// - Claude Code: the `initialize` control response of the stream-JSON protocol carries
//   `models: [{value, displayName, description}]` for the account the CLI is signed in to.
// 7D and 7E start the processes (under the sandbox) and pass the messages here; these parsers
// and the paging loop are all 7A needs. Ids are kept raw; the journal records an id
// `AGENT_MODELS` lacks as a keyed hash.
import { finError } from "../../errors.ts";

export interface CliModel {
  readonly id: string;
  readonly displayName: string;
  readonly isDefault: boolean;
  /** Claude Code: the concrete model id an alias resolves to (a model is accepted by `id` or this). */
  readonly resolvedModel?: string;
}

const str = (v: unknown): string | undefined => (typeof v === "string" && v !== "" ? v : undefined);

/** One page of Codex's `model/list` result: its visible models and the next cursor. */
export function parseCodexModelPage(result: unknown): { models: CliModel[]; nextCursor: string | null } {
  const data = (result as { data?: unknown })?.data;
  if (!Array.isArray(data)) throw finError("PROVIDER_PROTOCOL", "codex model/list returned no data", {});
  const models: CliModel[] = [];
  for (const m of data as Record<string, unknown>[]) {
    const id = str(m?.model) ?? str(m?.id);
    if (id === undefined || m.hidden === true) continue;
    models.push({ id, displayName: str(m.displayName) ?? id, isDefault: m.isDefault === true });
  }
  const next = (result as { nextCursor?: unknown }).nextCursor;
  return { models, nextCursor: typeof next === "string" && next !== "" ? next : null };
}

/** Every page of `model/list` through `request` (the app-server client's call). At most 50 pages. */
export async function discoverCodexModels(request: (method: string, params: Record<string, unknown>) => Promise<unknown>): Promise<CliModel[]> {
  const out: CliModel[] = [];
  let cursor: string | null = null;
  for (let page = 0; page < 50; page++) {
    const { models, nextCursor } = parseCodexModelPage(await request("model/list", cursor === null ? {} : { cursor }));
    out.push(...models);
    if (nextCursor === null) return dedupe(out);
    cursor = nextCursor;
  }
  throw finError("PROVIDER_PROTOCOL", "codex model/list did not end after 50 pages", {});
}

/** The models of Claude Code's `initialize` control response (`response.response` or the bare object). */
export function parseClaudeInitialize(message: unknown): CliModel[] {
  const m = message as Record<string, unknown> | null;
  const inner = (m?.response as Record<string, unknown> | undefined)?.response ?? m?.response ?? m;
  const models = (inner as { models?: unknown } | undefined)?.models;
  if (!Array.isArray(models)) throw finError("PROVIDER_PROTOCOL", "claude initialize carried no models list", {});
  const out: CliModel[] = [];
  for (const x of models as Record<string, unknown>[]) {
    const id = str(x?.value);
    if (id === undefined) continue;
    const resolvedModel = str(x.resolvedModel);
    out.push({ id, displayName: str(x.displayName) ?? id, isDefault: x.value === "default" || x.isDefault === true, ...(resolvedModel === undefined ? {} : { resolvedModel }) });
  }
  return dedupe(out);
}

function dedupe(models: CliModel[]): CliModel[] {
  const seen = new Set<string>();
  return models.filter((m) => !seen.has(m.id) && seen.add(m.id));
}
