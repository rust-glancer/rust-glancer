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
  it("shows Cargo diagnostics progress once the active workspace is ready", () => {
    const status = clientStatus();
    status.starting(DETAILS);
    status.ready(DETAILS);
    status.handleWorkDoneProgress("cargo", {
      kind: "begin",
      title: "Cargo diagnostics",
      message: "cargo check",
    });

    assert.equal(
      render(status),
      "diagnostics-running: $(sync~spin) Rust Glancer: cargo check running",
    );

    status.activeWorkspace("/workspace/project_a", "indexing", undefined);
    assert.equal(render(status), "indexing: $(sync~spin) Rust Glancer: indexing [project_a]");

    status.activeWorkspace("/workspace/project_a", "ready", undefined);
    assert.equal(
      render(status),
      "diagnostics-running: $(sync~spin) Rust Glancer: cargo check running [project_a]",
    );

    status.handleWorkDoneProgress("cargo", { kind: "end", message: "Failed" });
    assert.equal(
      render(status),
      "diagnostics-failed: $(error) Rust Glancer: cargo check failed [project_a]",
    );
  });

  it("keeps active workspace failure above diagnostics state", () => {
    const status = clientStatus();
    status.starting(DETAILS);
    status.ready(DETAILS);
    status.handleWorkDoneProgress("cargo", {
      kind: "begin",
      title: "Cargo diagnostics",
      message: "cargo check",
    });

    status.activeWorkspace("/workspace/project_b", "failed", "index failed");

    assert.equal(render(status), "failed: $(error) Rust Glancer: failed [project_b]");
    assert.equal(status.snapshot().diagnosticsRunning, true);
    assert.equal(status.snapshot().failureReason, undefined);
  });

  it("preserves active workspace label across language-client ready transitions", () => {
    const status = clientStatus();
    status.starting(DETAILS);
    status.ready(DETAILS);
    status.activeWorkspace("/workspace/project_d", "ready", undefined);

    status.ready({
      ...DETAILS,
      workspaceRoot: "/workspace/restarted-window",
    });

    assert.equal(render(status), "ready: $(check) Rust Glancer: ready [project_d]");
    assert.deepEqual(status.snapshot().details, {
      ...DETAILS,
      workspaceRoot: "/workspace/restarted-window",
      activeWorkspaceRoot: "/workspace/project_d",
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
    diagnosticsRunning() {},
    diagnosticsFailed() {},
    stopped() {},
    failed() {},
  };
}
