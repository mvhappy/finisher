import { readFileSync } from "node:fs";
import { expect, test } from "vitest";

const ts = readFileSync(new URL("./generated/index.ts", import.meta.url), "utf8");
const block = (name: string) => ts.match(new RegExp(`export interface ${name} \\{[\\s\\S]*?\n\\}`))?.[0] ?? "";

test("Rational-typed fields keep the Rational type", () => {
  expect(ts).toMatch(/export type ItemInsert = \{[\s\S]*?at: Rational;[\s\S]*?duration: Rational;[\s\S]*?\} & ItemInsert1;/);
  expect(block("ItemTrim")).toMatch(/delta: Rational;/);
  expect(block("Project")).toMatch(/rate: Rational;/);
  expect(block("Timeline")).toMatch(/rate: Rational;/);
});

test("model v2: timelines, items tagged by kind, font pins", () => {
  expect(block("Project")).toMatch(/timelines: Timeline\[\];/);
  expect(block("Project")).toMatch(/fonts\?: Fonts;/);
  expect(block("Timeline")).toMatch(/resolution: Resolution;/);
  expect(block("Track")).toMatch(/items: Item\[\];/);
  // An item is its common fields and one kind-tagged body.
  expect(ts).toMatch(/export type Item = \{[\s\S]*?start: Rational;[\s\S]*?\} & Item1;/);
  for (const kind of ["clip", "caption", "graphic", "image", "adjustment", "nested"]) {
    expect(ts).toContain(`kind: "${kind}";`);
  }
  expect(block("FontPin")).toMatch(/sha256: string;/);
  expect(ts).toMatch(/export type FontFormat = "ttf" \| "otf" \| "ttc";/);
});

test("item ops carry their payload and tag (plan 4C Task 7)", () => {
  for (const op of ["asset.add", "item.insert", "item.trim", "item.delete", "item.remove", "item.restore"]) {
    expect(ts).toMatch(new RegExp(`\\(\\w+ & \\{\\s*op: "${op.replace(".", "\\.")}";\\s*\\}\\)`));
  }
  expect(block("ItemRestore")).toMatch(/item: Item;/);
  expect(block("ItemRestore")).toMatch(/mode: DeleteMode;/);
  expect(block("AssetAdd")).toMatch(/kind\?: (AssetKind|"media" \| "image" \| "svg" \| "lottieTemplate" \| "font" \| "lut");/);
  // 4C adds kinds task by task (`effect`, ...) after these.
  expect(ts).toMatch(/export type EntityKind =\s*"asset" \| "track" \| "timeline" \| "item" \| "transition"( \| "\w+")*;/);
  // The clip.* aliases stay accepted from clients.
  for (const op of ["clip.insert", "clip.trim", "clip.delete"]) {
    expect(ts).toContain(`op: "${op}";`);
  }
});

test("CoreError.details is required", () => {
  expect(block("CoreError")).toMatch(/\n  details: unknown;/);
});

test("the envelope types above fin-core are generated too", () => {
  // fin-probe's Probe (asset.probe), with its kind and Rational times.
  expect(block("Probe")).toMatch(/kind: AssetKind;/);
  expect(block("Probe")).toMatch(/duration\?: Rational \| null;/);
  expect(block("Probe")).toMatch(/font\?: FontFile \| null;/);
  expect(block("Probe")).toMatch(/size: number;/);
  expect(block("Probe")).toMatch(/video\?: VideoInfo \| null;|video: VideoInfo \| null;/);
  // fin-store's Generation, tx.get's set-aside answer, and the engine events.
  expect(block("Generation")).toMatch(/incarnation: string;/);
  expect(block("Generation")).toMatch(/n: number;/);
  expect(block("TxSetAside")).toMatch(/setAside: SetAside;/);
  expect(block("SetAside")).toMatch(/txId: string;/);
  expect(ts).toMatch(/export type EngineEvent =/);
  for (const type of ["applied", "status", "rewound", "rebuilt", "error", "lagged"]) {
    expect(ts).toContain(`type: "${type}";`);
  }
});

test("Tx carries the optional argsHash", () => {
  expect(block("Tx")).toMatch(/argsHash\?: string \| null;|argsHash\?: string;/);
});

test("the sandbox profiles' inputs are generated, the policy is not (plan 7C Decision 1)", () => {
  for (const name of ["AgentInput", "HookInput", "HookFin", "WorkerInput", "PluginInput", "PluginData", "DeviceRule"]) {
    expect(ts).toContain(`export interface ${name} {`);
  }
  expect(block("AgentInput")).toMatch(/sessionId: string;/);
  expect(block("AgentInput")).toMatch(/mode: AgentMode;/);
  expect(block("AgentInput")).toMatch(/fin\?: FinEndpoint \| null;/);
  expect(block("WorkerInput")).toMatch(/scope: WorkerScope;/);
  expect(ts).toMatch(/export type WorkerScope =[\s\S]*?kind: "cold";[\s\S]*?kind: "warm";/);
  expect(ts).toMatch(/export type FinEndpoint =[\s\S]*?kind: "tcp";[\s\S]*?kind: "unixSocket";[\s\S]*?kind: "pipe";/);
  expect(ts).toMatch(/export type SandboxProfile = "agent" \| "hook" \| "worker" \| "plugin";/);
  expect(ts).not.toContain("export interface SandboxPolicy");
});

test("the sandbox status is generated (plan 7C Task 3)", () => {
  expect(block("SandboxStatus")).toMatch(/worker: SandboxMechanism;/);
  expect(block("SandboxStatus")).toMatch(/os: SandboxOs;/);
  expect(ts).toMatch(/export type SandboxMechanism =[\s\S]*?kind: "enforced";[\s\S]*?kind: "disabled";[\s\S]*?kind: "unavailable";/);
  expect(ts).toMatch(/export type UnconfinedReason = "landlockDisabled" \| "profileNotReady";/);
  expect(block("SandboxUnconfined")).toMatch(/reason: UnconfinedReason;/);
});

test("the Rational schema pattern matches the parser's shape (no `0/5`, no `3/1`)", () => {
  const schema = JSON.parse(readFileSync(new URL("../schema/fin-core.schema.json", import.meta.url), "utf8")) as {
    $defs: { Rational: { pattern: string } };
  };
  const re = new RegExp(schema.$defs.Rational.pattern);
  // What `Rational::from_str` accepts (lowest terms is the parser's check, not the pattern's).
  for (const ok of ["0", "5", "-5", "10", "1/2", "-7/3", "62/5", "30000/1001", "1/10", "7/19", "3/20"]) expect(re.test(ok), ok).toBe(true);
  // What it rejects as non-canonical or malformed.
  for (const bad of ["", "0/5", "-0", "-0/5", "3/1", "-3/1", "1/0", "1/01", "01", "+1", "1/-2", "2/-3", "1.5", "1/", "/2"]) expect(re.test(bad), bad).toBe(false);
});
