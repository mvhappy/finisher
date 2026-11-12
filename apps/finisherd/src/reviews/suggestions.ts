// Plugin suggestions as review items (5E plan Decision 11, the 8A Task 9b follow-up):
// `plugin.suggest` writes a `pluginSuggestion` item (`subject {pluginId}`, `title` the plugin id,
// `summary` the reason, `requestedBy` the suggester); installing the plugin supersedes its open
// suggestions; rows an earlier build left in `plugin_suggestions` move here once. The table
// stays, unwritten: a later `plugins` migration drops it.
import type { ReviewItem } from "@finisher/contracts";
import type { ControlStore } from "../control/db.ts";
import { finError } from "../errors.ts";
import type { ReviewStore } from "./store.ts";

/** Open suggestions one principal may have for one project (8A review M2). */
export const MAX_SUGGESTIONS = 20;

const SYSTEM = "system:plugins";

/** What the plugin lifecycle needs of the reviews. */
export interface PluginSuggestions {
  /** Records (or answers the existing) suggestion of `pluginId` by `by` for the project; throws `FinError`. */
  suggest(req: { readonly pluginId: string; readonly projectId: string; readonly reason: string }, by: { readonly kind: string; readonly id: string }): { suggestionId: string };
  /** `pluginId` was installed: its open suggestions, in every project, become `superseded`. */
  installed(pluginId: string): void;
}

const subjectPlugin = (item: ReviewItem): unknown => (item.subject as { pluginId?: unknown } | undefined)?.pluginId;

export function pluginSuggestions(store: ReviewStore): PluginSuggestions {
  return {
    suggest: (req, by) => {
      const principal = `${by.kind}:${by.id}`;
      const mine = store.list(req.projectId, { kind: "pluginSuggestion" }).filter((i) => (i.state === "open" || i.state === "changesRequested") && i.requestedBy.principal === principal);
      const same = mine.find((i) => subjectPlugin(i) === req.pluginId);
      if (same !== undefined) return { suggestionId: same.id };
      if (mine.length >= MAX_SUGGESTIONS) {
        throw finError("TOO_MANY_SUGGESTIONS", `${by.kind} principals may have ${MAX_SUGGESTIONS} suggestions per project`, { limit: MAX_SUGGESTIONS }, "Wait for the owner to look at the ones you made.");
      }
      const item = store.open({
        projectId: req.projectId,
        kind: "pluginSuggestion",
        title: req.pluginId,
        summary: req.reason,
        requestedBy: { principal },
        subject: { pluginId: req.pluginId },
      });
      return { suggestionId: item.id };
    },
    installed: (pluginId) => {
      for (const item of store.openOfKind("pluginSuggestion")) if (subjectPlugin(item) === pluginId) store.setState(item.id, "superseded", SYSTEM);
    },
  };
}

interface Row {
  id: string;
  pluginId: string;
  projectId: string;
  reason: string;
  byKind: string;
  byId: string;
  createdAt: number;
}

/**
 * Moves the `plugin_suggestions` rows into items, once: in one transaction each row is inserted
 * (by its id, if absent) and then deleted, so a crash between the two moves nothing twice. Run at
 * start, after every chain has migrated. Answers how many rows it moved.
 */
export function moveSuggestionRows(control: ControlStore, store: ReviewStore): number {
  return control.tx(() => {
    const rows = control.db.prepare("SELECT id, pluginId, projectId, reason, byKind, byId, createdAt FROM plugin_suggestions ORDER BY createdAt, id").all() as unknown as Row[];
    for (const r of rows) {
      store.adopt({
        id: r.id,
        createdAt: r.createdAt,
        projectId: r.projectId,
        kind: "pluginSuggestion",
        title: r.pluginId,
        summary: r.reason,
        requestedBy: { principal: `${r.byKind}:${r.byId}` },
        subject: { pluginId: r.pluginId },
      });
    }
    control.db.prepare("DELETE FROM plugin_suggestions").run();
    return rows.length;
  });
}
