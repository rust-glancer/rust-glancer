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

  it("shows a ready status while deferred indexing finishes", () => {
    const status = clientStatus();
    status.starting(DETAILS);
    status.ready(DETAILS);

    status.activeWorkspace("/workspace/project_c", "ready", undefined, false);
    status.deferredIndexingStarted("/workspace/project_c", false);
    assert.equal(render(status), "ready: ~ Rust Glancer: ready [project_c]");

    status.deferredIndexingFinished("/workspace/project_c", false);
    assert.equal(render(status), "ready: $(check) Rust Glancer: ready [project_c]");
  });

  it("returns through indexing and explicit deferred-ready after a saved project rebuild", () => {
    const status = clientStatus();
    status.starting(DETAILS);
    status.ready(DETAILS);
    status.activeWorkspace("/workspace/project_c", "ready", undefined, false);
    status.deferredIndexingFinished("/workspace/project_c", false);
    assert.equal(render(status), "ready: $(check) Rust Glancer: ready [project_c]");

    status.activeWorkspace("/workspace/project_c", "indexing", undefined, false);
    assert.equal(render(status), "indexing: $(sync~spin) Rust Glancer: indexing [project_c]");

    status.deferredIndexingStarted("/workspace/project_c", false);
    status.activeWorkspace("/workspace/project_c", "ready", undefined, false);
    assert.equal(render(status), "ready: ~ Rust Glancer: ready [project_c]");
    status.deferredIndexingFinished("/workspace/project_c", false);
    assert.equal(render(status), "ready: $(check) Rust Glancer: ready [project_c]");
  });

  it("does not invent deferred work for an ordinary indexing cycle", () => {
    const status = clientStatus();
    status.starting(DETAILS);
    status.ready(DETAILS);
    status.activeWorkspace("/workspace/project_c", "ready", undefined, false);
    status.deferredIndexingStarted("/workspace/project_c", false);
    status.deferredIndexingFinished("/workspace/project_c", false);

    status.indexing();
    status.activeWorkspace("/workspace/project_c", "indexing", undefined, false);
    status.activeWorkspace("/workspace/project_c", "ready", undefined, false);

    assert.equal(render(status), "ready: $(check) Rust Glancer: ready [project_c]");
  });

  it("keeps deferred indexing state scoped to workspace roots", () => {
    const status = clientStatus();
    status.starting(DETAILS);
    status.ready(DETAILS);

    status.activeWorkspace("/workspace/project_a", "ready", undefined, false);
    status.deferredIndexingStarted("/workspace/project_b", false);
    assert.equal(render(status), "ready: $(check) Rust Glancer: ready [project_a]");

    status.activeWorkspace("/workspace/project_b", "ready", undefined, false);
    assert.equal(render(status), "ready: ~ Rust Glancer: ready [project_b]");
  });

  it("does not show deferred indexing when finish arrives before ready", () => {
    const status = clientStatus();
    status.starting(DETAILS);
    status.ready(DETAILS);

    status.deferredIndexingStarted("/workspace/project_e", false);
    status.deferredIndexingFinished("/workspace/project_e", false);
    status.activeWorkspace("/workspace/project_e", "ready", undefined, false);

    assert.equal(render(status), "ready: $(check) Rust Glancer: ready [project_e]");
  });

  it("preserves active workspace label across language-client ready transitions", () => {
    const status = clientStatus();
    status.starting(DETAILS);
    status.ready(DETAILS);
    status.activeWorkspace("/workspace/project_d", "ready", undefined, false);

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
    readyWithDeferredIndexing() {},
    stale() {},
    diagnosticsRunning() {},
    diagnosticsFailed() {},
    stopped() {},
    failed() {},
  };
}
