import * as assert from "node:assert/strict";
import { describe, it } from "node:test";

import { EXTENSION_COMMANDS, SERVER_COMMANDS } from "../src/commands";

describe("command identifiers", () => {
  it("keeps extension and server reindex commands intentionally separate", () => {
    assert.equal(EXTENSION_COMMANDS.reindexWorkspace, "rust-glancer.reindexWorkspace");
    assert.equal(SERVER_COMMANDS.reindexWorkspace, "rust-glancer.internal.reindexWorkspace");
  });
});
