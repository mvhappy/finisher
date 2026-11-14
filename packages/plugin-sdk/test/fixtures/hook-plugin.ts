// A test plugin with hook handlers (7F Task 9), bundled by the tests with tools/bundle.mjs:
// `guard` and `audit` both on `PreCommand`, dispatched by the manifest's handler name; `view`
// calls the host inside a hook; `untyped` answers a `modify` reply the types refuse, `extra` an
// allow with an unknown field, `null` a JSON null.
import { definePlugin } from "../../src/index.ts";

const seen: string[] = [];

definePlugin({
  commands: {
    seen: () => ({ data: seen }),
  },
  hooks: {
    guard: (req) => {
      seen.push("guard");
      const payload = req.payload as { command?: string };
      return payload.command === "history.delete" ? { reply: "block", reason: `no deletes in ${req.project?.id}` } : { reply: "allow" };
    },
    audit: (req, host) => {
      seen.push("audit");
      return { reply: "context", text: `${req.event} v${req.v} by ${host.context.plugin.id}` };
    },
    greet: (req) => {
      seen.push("greet");
      return { reply: "context", text: `hello ${req.session?.id}` };
    },
    quiet: () => undefined,
    view: async (_req, host) => ({ reply: "context", text: await host.view() }),
    untyped: () => JSON.parse('{"reply": "modify", "payload": {"command": "history.list"}}'),
    extra: () => JSON.parse('{"reply": "allow", "payload": {}}'),
    null: () => JSON.parse("null"),
  },
});
