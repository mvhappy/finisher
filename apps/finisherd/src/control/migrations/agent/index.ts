// The agent chain's migrations, in order (`agent/store.ts` registers them).
import { agent0001 } from "./0001.ts";
import { agent0002 } from "./0002.ts";
import { agent0003 } from "./0003.ts";
import { agent0004 } from "./0004.ts";
import { agent0005 } from "./0005.ts";
import { agent0006 } from "./0006.ts";
import { agent0007 } from "./0007.ts";
import { agent0008 } from "./0008.ts";

export const AGENT_MIGRATIONS = [agent0001, agent0002, agent0003, agent0004, agent0005, agent0006, agent0007, agent0008] as const;
