import * as assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  planRestartTarget,
  resolveWorkspaceIdentity,
  WorkspaceOwners,
} from "../src/workspace/workspace-lifecycle";

describe("resolveWorkspaceIdentity", () => {
  it("keeps the owning folder separate from a parent Cargo workspace root", () => {
    const identity = resolveWorkspaceIdentity({
      cargoRootUri: "file:///repo",
      containingFolderUri: "file:///repo/crates/member",
      configResourceUri: "file:///repo/crates/member/src/lib.rs",
    });

    assert.equal(identity.workspaceKey, "file:///repo");
    assert.equal(identity.ownerKey, "file:///repo/crates/member");
    assert.equal(identity.configResourceUri, "file:///repo/crates/member/src/lib.rs");
  });

  it("falls back to the Cargo root for single-root workspaces", () => {
    const identity = resolveWorkspaceIdentity({
      cargoRootUri: "file:///repo",
    });

    assert.equal(identity.workspaceKey, "file:///repo");
    assert.equal(identity.ownerKey, "file:///repo");
    assert.equal(identity.configResourceUri, "file:///repo");
  });
});

describe("WorkspaceOwners", () => {
  it("keeps a shared Cargo-root client alive until all owner folders are removed", () => {
    const owners = new WorkspaceOwners("file:///repo/crates/a");

    owners.add("file:///repo/crates/b");
    owners.delete("file:///repo/crates/a");

    assert.equal(owners.isEmpty(), false);
    assert.deepEqual(owners.snapshot(), ["file:///repo/crates/b"]);

    owners.delete("file:///repo/crates/b");
    assert.equal(owners.isEmpty(), true);
  });
});

describe("planRestartTarget", () => {
  it("starts an active workspace that is not already tracked", () => {
    assert.deepEqual(
      planRestartTarget({
        activeWorkspaceKey: "file:///repo",
        existingWorkspaceKeys: [],
      }),
      { kind: "start-active", workspaceKey: "file:///repo" },
    );
  });

  it("restarts an already tracked active workspace", () => {
    assert.deepEqual(
      planRestartTarget({
        activeWorkspaceKey: "file:///repo",
        existingWorkspaceKeys: ["file:///repo"],
      }),
      { kind: "restart-existing", workspaceKey: "file:///repo" },
    );
  });

  it("prompts when there are multiple running workspaces and no active Rust file", () => {
    assert.deepEqual(
      planRestartTarget({
        existingWorkspaceKeys: ["file:///repo/a", "file:///repo/b"],
      }),
      { kind: "prompt" },
    );
  });
});
