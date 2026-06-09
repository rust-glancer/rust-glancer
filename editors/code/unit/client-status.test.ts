import * as assert from "node:assert/strict";
import { describe, it } from "node:test";

import { ClientStatus, type ClientStatusView } from "../src/status/client-status";
import type { StatusDetails } from "../src/status/status-model";

const DETAILS: StatusDetails = {
  workspaceRoot: "/workspace/window",
  serverCommand: "rust-glancer lsp",
  serverSource: "test",
};

describe("client status state precedence", () => {
  it("lets engine state, dirty files, and diagnostics win in that order", () => {
    const status = clientStatus();
    status.starting(DETAILS);
    status.ready(DETAILS);
    status.handleWorkDoneProgress(
      "cargo",
      { kind: "begin", title: "Cargo diagnostics", message: "cargo check" },
      false,
    );

    assert.equal(
      render(status),
      "diagnostics-running: $(sync~spin) Rust Glancer: cargo check running",
    );

    status.activeWorkspace("/workspace/project_a", "indexing", undefined, true);
    assert.equal(render(status), "indexing: $(sync~spin) Rust Glancer: indexing [project_a]");

    status.activeWorkspace("/workspace/project_a", "ready", undefined, true);
    assert.equal(render(status), "stale: $(warning) Rust Glancer: stale until save [project_a]");

    status.refresh(false);
    assert.equal(
      render(status),
      "diagnostics-running: $(sync~spin) Rust Glancer: cargo check running [project_a]",
    );

    status.handleWorkDoneProgress("cargo", { kind: "end", message: "Failed" }, false);
    assert.equal(
      render(status),
      "diagnostics-failed: $(error) Rust Glancer: cargo check failed [project_a]",
    );
  });

  it("keeps active workspace failure above dirty and diagnostics state", () => {
    const status = clientStatus();
    status.starting(DETAILS);
    status.ready(DETAILS);
    status.handleWorkDoneProgress(
      "cargo",
      { kind: "begin", title: "Cargo diagnostics", message: "cargo check" },
      false,
    );

    status.activeWorkspace("/workspace/project_b", "failed", "index failed", true);

    assert.equal(render(status), "failed: $(error) Rust Glancer: failed [project_b]");
    assert.equal(status.snapshot().diagnosticsRunning, true);
    assert.equal(status.snapshot().failureReason, undefined);
  });

  it("preserves active workspace label across language-client ready transitions", () => {
    const status = clientStatus();
    status.starting(DETAILS);
    status.ready(DETAILS);
    status.activeWorkspace("/workspace/project_c", "ready", undefined, false);

    status.ready({
      ...DETAILS,
      workspaceRoot: "/workspace/restarted-window",
    });

    assert.equal(render(status), "ready: $(check) Rust Glancer: ready [project_c]");
    assert.deepEqual(status.snapshot().details, {
      ...DETAILS,
      workspaceRoot: "/workspace/restarted-window",
      activeWorkspaceRoot: "/workspace/project_c",
    });
  });
});

function clientStatus(): ClientStatus {
  return new ClientStatus(noopView(), () => false);
}

function render(status: ClientStatus): string {
  const snapshot = status.snapshot().status;
  return `${snapshot.state}: ${snapshot.text}`;
}

function noopView(): ClientStatusView {
  return {
    starting() {},
    indexing() {},
    ready() {},
    stale() {},
    diagnosticsRunning() {},
    diagnosticsFailed() {},
    stopped() {},
    failed() {},
  };
}
