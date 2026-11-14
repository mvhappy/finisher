// What `serve` (`main.ts`) takes: the server data and the tests' seams. Kept apart so `main.ts`
// stays within its size limit.
import type { ServerInfo } from "./config.ts";
import type { ListenerRules } from "./connections.ts";
import type { AgentCore } from "./agent/core.ts";
import type { AgentListener } from "./agent/listener.ts";
import type { AgentRuntimeParts, AgentServeOptions } from "./agent/wiring.ts";
import type { DiagnosticsOptions } from "./diagnostics.ts";
import type { FontsOptions } from "./fonts.ts";
import type { JobEngineOptions } from "./jobs/layer.ts";
import type { RemoteListener } from "./net/remote.ts";
import type { DevPlugin } from "./plugins/host/dev.ts";
import type { EgressTestHooks } from "./plugins/host/egress.ts";
import type { RegistryOptions } from "./projects.ts";

export interface ServeOptions {
  readonly serverData: string;
  readonly registry?: RegistryOptions;
  readonly fonts?: FontsOptions;
  readonly diagnostics?: DiagnosticsOptions;
  /** Agent and MCP credentials (tests): the clock expiry reads and the sweep's period (60 s). */
  readonly credentials?: { readonly clock?: () => number; readonly sweepMs?: number };
  /** The agent runtime's shared pieces, handed to tests once built (7A-2). */
  readonly onAgentCore?: (core: AgentCore) => void;
  /** The agent runtime's options (tests: the fake harness's scripts) and its parts once booted. */
  readonly agents?: AgentServeOptions;
  readonly onAgentRuntime?: (parts: AgentRuntimeParts) => void;
  /** The job engine's options (tests: the test kinds). */
  readonly jobs?: JobEngineOptions;
  /**
   * The remote listener in tests (5B Decision 34): `forceTls` starts it on `listen` (e.g.
   * `127.0.0.1:0`) whatever the settings say; `remoteOf` injects remote addresses.
   */
  readonly remote?: { readonly forceTls?: boolean; readonly listen?: string; readonly remoteOf?: (s: never) => string };
  /** The remote listener, handed to tests once made. */
  readonly onRemote?: (r: RemoteListener) => void;
  /** The agent listener (plan 7C Task 6), handed to tests and the session wiring once built. */
  readonly onAgentListener?: (listener: AgentListener) => void;
  /** The agent listener's handshake timeout and connection cap (tests lower them). */
  readonly agentListener?: Partial<Pick<ListenerRules, "handshakeTimeoutMs" | "maxConnections" | "maxUnproved">>;
  /** Called once both transports listen and `finisherd.json` is written. */
  readonly onReady?: (info: ServerInfo) => void;
  /** Developer mode (sp8b Task 11): the plugin the person approved for this run. */
  readonly dev?: DevPlugin;
  /**
   * The plugin host in tests: `egress`, the broker's test hooks (`egress-test-hooks.ts` makes
   * them; refused unless fin-node is its `test-hooks` build). The e2e `plugin-code` scenario's
   * launcher passes them; `main` never does.
   */
  readonly plugins?: { readonly egress?: EgressTestHooks };
}
