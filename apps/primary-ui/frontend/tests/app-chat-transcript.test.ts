import { describe, it } from "node:test";
import assert from "node:assert/strict";
import type { AgentTurn } from "../src/lib/app-chat";
import { activityStatus, agentLaunches, conversationHref, startedAgents, toolArguments, toolDisplayName, toolSummary, transcriptParts } from "../src/lib/app-chat-transcript";

function turn(values: Partial<AgentTurn>): AgentTurn {
  return { role: "assistant", text: "", seq: 80, at: 0, pending: false, queued: false, images: [], steps: [], ...values };
}

describe("readable tool arguments", () => {
  it("unwraps strings and preserves actual command/edit newlines", () => {
    assert.deepEqual(toolArguments(JSON.stringify({ command: "git status\ngit diff", file_path: "src/chat.tsx" })), [
      { name: "command", value: "git status\ngit diff" },
      { name: "file_path", value: "src/chat.tsx" },
    ]);
  });

  it("describes structured values without JSON object syntax", () => {
    assert.deepEqual(toolArguments('{"options":{"cwd":"/work","quiet":true},"files":["one","two"]}'), [
      { name: "options", value: "cwd: /work\nquiet: true" },
      { name: "files", value: "one\ntwo" },
    ]);
  });

  it("keeps unknown plain inputs and unwraps JSON string inputs", () => {
    assert.deepEqual(toolArguments("plain command\nsecond line"), [{ name: "Input", value: "plain command\nsecond line" }]);
    assert.deepEqual(toolArguments('"string input"'), [{ name: "Input", value: "string input" }]);
    assert.deepEqual(toolArguments(""), []);
  });

  it("labels summaries with tool names while retaining the operation icon", () => {
    assert.deepEqual(toolSummary("Bash", '{"timeout":200,"command":"git status --short"}'), { title: "Bash", kind: "git" });
    assert.deepEqual(toolSummary("functions.exec_command", '{"cmd":"/bin/zsh -lc bun run check"}'), { title: "exec_command", kind: "terminal" });
    assert.deepEqual(toolSummary("functions.apply_patch", '{"patch":"*** Update File: src/chat.tsx"}'), { title: "apply_patch", kind: "edit" });
  });

  it("keeps unanswered distinct from a returned tool error", () => {
    assert.equal(activityStatus("unanswered"), "cancelled");
    assert.equal(activityStatus("error"), "error");
    assert.equal(activityStatus("running"), "running");
    assert.equal(activityStatus("ok"), "completed");
  });

  it("shows tool names without runtime namespace prefixes", () => {
    assert.equal(toolDisplayName("mcp__adi__Bash"), "Bash");
    assert.equal(toolDisplayName("functions.exec_command"), "exec_command");
    assert.equal(toolDisplayName("Read"), "Read");
    assert.deepEqual(toolSummary("mcp__adi__Read", '{"file_path":"src/chat.tsx"}'), { title: "Read", kind: "tool" });
  });
});

describe("agent conversation links", () => {
  it("requires an explicit CLI launch in a terminal tool", () => {
    const output = "Started agent reviewer as background process 123.\n  run:     run-local\n";
    const base = { kind: "tool" as const, status: "ok" as const, output };
    for (const command of ["adi agents run reviewer task", "adi-mono agents run reviewer task", "/work/.bin/adi-agents run reviewer task"]) {
      assert.deepEqual(agentLaunches({ ...base, name: "Bash", input: JSON.stringify({ command }) }), [{ agent: "reviewer", runId: "run-local" }]);
      assert.deepEqual(agentLaunches({ ...base, name: "shell", input: `/bin/zsh -lc '${command}'` }), [{ agent: "reviewer", runId: "run-local" }]);
    }
    assert.deepEqual(agentLaunches({ ...base, name: "functions.exec_command", input: '{"cmd":"adi-agents run reviewer task"}' }), [{ agent: "reviewer", runId: "run-local" }]);
  });

  it("keeps file output and unrelated shell output as ordinary tools", () => {
    const output = "Started agent reviewer as background process 123.\n  run:     run-local\n";
    const base = { kind: "tool" as const, status: "ok" as const, output };
    assert.deepEqual(agentLaunches({ ...base, name: "Read", input: '{"file_path":"docs/adi-agents run example.txt"}' }), []);
    assert.deepEqual(agentLaunches({ ...base, name: "Read", input: '{"command":"adi-mono agents run reviewer task"}' }), []);
    assert.deepEqual(agentLaunches({ ...base, name: "Bash", input: '{"command":"cat fixture.txt"}' }), []);
    assert.deepEqual(agentLaunches({ ...base, name: "Bash", input: '{"command":"adi-agents runs"}' }), []);
    assert.deepEqual(agentLaunches({ ...base, status: "error", name: "Bash", input: '{"command":"adi-agents run reviewer task"}' }), []);
  });

  it("reads the exact successful local and leased launch receipts", () => {
    assert.deepEqual(startedAgents("Started agent reviewer as background process 123.\n  run:     run-local\n  command: example"), [{ agent: "reviewer", runId: "run-local" }]);
    assert.deepEqual(startedAgents("Started agent reviewer through its lease at http://app.adi.\r\n  run:     run-leased\r\n"), [{ agent: "reviewer", runId: "run-leased" }]);
  });

  it("does not guess from a command, unrelated run line, or terminal-session receipt", () => {
    assert.deepEqual(startedAgents('adi-mono agents run reviewer "Check"'), []);
    assert.deepEqual(startedAgents("Started agent reviewer as background process 123.\n  command: something\n  run: run-other"), []);
    assert.deepEqual(startedAgents("Started agent reviewer in session session-name.\n  run: unrelated"), []);
    assert.deepEqual(startedAgents("echo Started agent reviewer as background process 123.\n  run: run-quoted"), []);
  });

  it("keeps multiple launches and removes duplicate receipts", () => {
    const first = "Started agent reviewer as background process 123.\n  run: first";
    const second = "Started agent tester as background process 456.\n  run: second";
    assert.deepEqual(startedAgents([first, second, first].join("\n")), [
      { agent: "reviewer", runId: "first" },
      { agent: "tester", runId: "second" },
    ]);
  });

  it("encodes identities as query parameters", () => {
    assert.equal(conversationHref({ agent: "reviewer & writer", runId: "run/one?x=y" }), "/interface?agent=reviewer%20%26%20writer&run=run%2Fone%3Fx%3Dy");
  });
});

describe("transcript ordering", () => {
  it("preserves tools, interim messages, more activity, and final text in order", () => {
    const parts = transcriptParts(turn({
      text: "Final answer",
      steps: [
        { kind: "thinking", text: "Considering" },
        { kind: "tool", name: "Read", input: "file", output: "contents", status: "ok" },
        { kind: "message", text: "I found the source." },
        { kind: "calls", from: 3, to: 8, count: 5, tools: ["Edit"], preview: "src/chat.tsx", status: "ok" },
        { kind: "unknown" },
      ],
    }));
    assert.deepEqual(parts.map((part) => part.kind === "message" ? part.text : part.steps.map(({ step, index }) => [step.kind, index])), [
      [["thinking", 0], ["tool", 1]],
      "I found the source.",
      [["calls", 3]],
      "Final answer",
    ]);
  });

  it("keeps original indices and ignores blank or unknown messages", () => {
    const parts = transcriptParts(turn({ text: "  ", steps: [
      { kind: "unknown" },
      { kind: "message", text: "\n" },
      { kind: "tool", name: "Read", input: "", output: "", status: "running" },
    ] }));
    assert.equal(parts.length, 1);
    assert.equal(parts[0].key, "activity-2");
    assert.equal(parts[0].kind, "activity");
  });
});
