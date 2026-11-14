import { test } from "node:test";
import assert from "node:assert/strict";
import { EMAIL, RESETS_AT, SESSION_ID, TIMESTAMP, assertScrubbed, scrubTranscript } from "./scrub-transcript.mjs";

// A made-up machine: the real one is detected at run time and never written down here.
const ID = { homes: ["/home/alice"], names: ["alice", "buildbox"] };
const rec = (dir, line) => JSON.stringify({ t: 1, dir, line: typeof line === "string" ? line : JSON.stringify(line) });
const scrub = (...records) => scrubTranscript(`${records.join("\n")}\n`, ID);
const parse = (text) => text.split("\n").filter(Boolean).map((l) => JSON.parse(l));

const UUID_A = "11111111-2222-4333-8444-555555555555";
const UUID_B = "66666666-7777-4888-8999-aaaaaaaaaaaa";

test("keeps protocol lines in order, drops notes, stderr and non-JSON", () => {
  const out = parse(
    scrub(
      JSON.stringify({ t: 0, note: "argv", v: ["/home/alice/.local/bin/claude"] }),
      rec("in", { type: "control_request", request_id: "init_1", request: { subtype: "initialize" } }),
      JSON.stringify({ t: 1, dir: "err", text: "warning: /home/alice/x" }),
      rec("out", "not json"),
      rec("out", { type: "system", subtype: "status", session_id: UUID_A }),
    ),
  );
  assert.deepEqual(out.map((o) => [o.dir, o.line.type]), [["in", "control_request"], ["out", "system"]]);
});

test("paths, home, user and host names become placeholders", () => {
  const [o] = parse(
    scrub(
      rec("out", {
        type: "system",
        subtype: "init",
        cwd: "/home/alice/.local/share/rec/scratch/t0-ab12_cd/permission/ws",
        user_output_styles_dir: "/home/alice/.local/share/rec/scratch/t0-ab12_cd/permission/cfg/output-styles",
        messaging_socket_path: "/tmp/cc-socks-1000/958867.sock",
        note: "built on buildbox by Alice in /home/alice/.claude-alice",
      }),
    ),
  );
  assert.equal(o.line.cwd, "/srv/finisher/workspace/s1");
  assert.equal(o.line.user_output_styles_dir, "/srv/finisher/agents/s1/provider/output-styles");
  assert.equal(o.line.messaging_socket_path, "/tmp/cc-socks-uid/1.sock");
  assert.equal(o.line.note, "built on user by user in /srv/finisher/home/.claude-user");
});

test("a path split across input_json_delta pieces is scrubbed as a whole", () => {
  const delta = (index, partial_json) => rec("out", { type: "stream_event", event: { type: "content_block_delta", index, delta: { type: "input_json_delta", partial_json } } });
  const text = scrub(
    rec("out", { type: "stream_event", event: { type: "message_start" } }),
    delta(1, '{"file_path":"/ho'),
    delta(1, "me/ali"),
    delta(1, 'ce/work/a.txt"}'),
  );
  assert.ok(!/alice|\/home\/(?!.*srv)/i.test(text.replace(/\/srv\/finisher\/home/g, "")), text);
  const joined = parse(text).filter((o) => o.line.event?.delta).map((o) => o.line.event.delta.partial_json).join("");
  assert.equal(joined, '{"file_path":"/srv/finisher/home/work/a.txt"}');
  assert.equal(parse(text).filter((o) => o.line.event?.delta).length, 3, "the pieces stay as many lines");
});

test("text and thinking deltas split across lines are scrubbed as a whole too", () => {
  const delta = (index, type, field, piece) => rec("out", { type: "stream_event", event: { type: "content_block_delta", index, delta: { type, [field]: piece } } });
  const text = scrub(
    rec("out", { type: "stream_event", event: { type: "message_start" } }),
    delta(0, "text_delta", "text", "Hello ali"),
    delta(0, "text_delta", "text", "ce, wel"),
    delta(0, "text_delta", "text", "come"),
    delta(1, "thinking_delta", "thinking", "mail bob@corp.exa"),
    delta(1, "thinking_delta", "thinking", "mple now"),
    delta(2, "text_delta", "text", "nothing to hide"),
    delta(2, "text_delta", "text", " here"),
  );
  assert.ok(!/alice|bob@|corp\.exa/i.test(text), text);
  const deltas = parse(text).filter((o) => o.line.event?.delta).map((o) => o.line.event.delta);
  assert.equal(deltas.length, 7, "the pieces stay as many lines");
  assert.equal(deltas.filter((d) => d.type === "text_delta").slice(0, 3).map((d) => d.text).join(""), "Hello user, welcome");
  assert.equal(deltas.filter((d) => d.type === "thinking_delta").map((d) => d.thinking).join(""), `mail ${EMAIL} now`);
  // A block with nothing to scrub keeps its own cuts.
  assert.deepEqual(deltas.filter((d) => d.type === "text_delta").slice(3).map((d) => d.text), ["nothing to hide", " here"]);
});

test("capability names that look like ids survive; real ids are still replaced", () => {
  const [a, b] = parse(
    scrub(
      rec("out", { type: "control_response", response: { request_id: "init_1", response: { capabilities: ["interrupt_send_now_v1", "msg_lifecycle_v1", "toolu_cap_v2", "req_marker_v1"] } } }),
      rec("out", { type: "assistant", request_id: "req_011Ch1i4MLYEV56Rr6v1rfjN", message: { id: "msg_011Ch1i4Bed1aTV9K4SYsbsF", content: [{ type: "tool_use", id: "toolu_01Q8aWmZbNpDx3sRtU4vYhJk", name: "Read", input: {} }, { type: "text", text: "see msg_011Ch1i4Bed1aTV9K4SYsbsF" }] } }),
    ),
  );
  assert.deepEqual(a.line.response.response.capabilities, ["interrupt_send_now_v1", "msg_lifecycle_v1", "toolu_cap_v2", "req_marker_v1"]);
  assert.equal(b.line.request_id, "req_0001");
  assert.equal(b.line.message.id, "msg_0001");
  assert.equal(b.line.message.content[0].id, "toolu_0001");
  assert.equal(b.line.message.content[1].text, "see msg_0001");
});

test("ids get stable counters; the session id is the synthetic one", () => {
  const out = parse(
    scrub(
      rec("in", { type: "user", session_id: UUID_A, message: { role: "user", content: [] } }),
      rec("out", { type: "system", subtype: "init", session_id: UUID_B, uuid: UUID_A }),
      rec("out", { type: "assistant", session_id: UUID_B, uuid: "99999999-aaaa-4bbb-8ccc-dddddddddddd", request_id: "req_01AbcDefGhiJklMnoPqrStu", message: { id: "msg_01ZedAbcDefGhiJklMnoPqrS", content: [{ type: "tool_use", id: "toolu_01QAbcDefGhiJklMnoPqrSt", name: "Read", input: {} }] } }),
      rec("out", { type: "user", session_id: UUID_B, message: { content: [{ type: "tool_result", tool_use_id: "toolu_01QAbcDefGhiJklMnoPqrSt" }] } }),
    ),
  );
  assert.equal(out[0].line.session_id, SESSION_ID);
  assert.equal(out[1].line.session_id, SESSION_ID);
  assert.equal(out[1].line.uuid, SESSION_ID);
  assert.equal(out[2].line.uuid, "00000000-0000-4000-8000-000000000001");
  assert.equal(out[2].line.request_id, "req_0001");
  assert.equal(out[2].line.message.id, "msg_0001");
  assert.equal(out[2].line.message.content[0].id, "toolu_0001");
  assert.equal(out[3].line.message.content[0].tool_use_id, "toolu_0001");
});

test("account labels, e-mails, timestamps and rate-limit numbers are normalised", () => {
  const [a, b] = parse(
    scrub(
      rec("out", { type: "control_response", response: { request_id: "init_1", response: { account: { email: "alice@corp.example", subscriptionType: "Acme Gold", apiProvider: "firstParty" }, pid: 4242, agents: [{ name: "claude", description: "Anthropic's own prose" }] } } }),
      rec("out", { type: "rate_limit_event", rate_limit_info: { status: "allowed", resetsAt: 1795555555, utilization: 0.1234 }, timestamp: "2026-11-13T09:33:54.965Z", note: "bob@x.example at 2026-11-13T09:33:54Z" }),
    ),
  );
  const body = a.line.response.response;
  assert.deepEqual(body.account, { email: EMAIL, subscriptionType: "Claude Plan", apiProvider: "firstParty" });
  assert.equal(body.pid, 1);
  assert.deepEqual(body.agents, [{ name: "claude", description: "" }]);
  assert.equal(b.line.rate_limit_info.resetsAt, RESETS_AT);
  assert.equal(b.line.rate_limit_info.utilization, 0.5);
  assert.equal(b.line.timestamp, TIMESTAMP);
  assert.equal(b.line.note, `${EMAIL} at ${TIMESTAMP}`);
});

test("thinking signatures and redacted payloads are opaque placeholders", () => {
  const [o] = parse(scrub(rec("out", { type: "assistant", message: { content: [{ type: "thinking", thinking: "hm", signature: "REAL-SIGNATURE" }, { type: "redacted_thinking", data: "REAL-BLOB" }] } })));
  assert.equal(o.line.message.content[0].signature, "c2lnbmF0dXJl");
  assert.equal(o.line.message.content[1].data, "cmVkYWN0ZWQ=");
});

test("scrubbing is deterministic", () => {
  const raw = [rec("out", { type: "system", subtype: "init", session_id: UUID_A, uuid: UUID_B, cwd: "/home/alice/w" })];
  assert.equal(scrub(...raw), scrub(...raw));
});

test("assertScrubbed refuses what a scrub must never leave", () => {
  assert.doesNotThrow(() => assertScrubbed('{"cwd":"/srv/finisher/home/x"}', ID));
  for (const bad of ['"/home/bob/x"', '"/Users/bob/x"', '"C:\\\\Users\\\\bob"', '"bob@corp.example"', '"/home/alice"', '"Alice"', '"BuildBox"']) {
    assert.throws(() => assertScrubbed(bad, ID), /scrub left/, bad);
  }
});
