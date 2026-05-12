import * as assert from "node:assert/strict";
import { describe, it } from "node:test";

import { formatServerLogRecord, parseServerLogLine } from "../src/logging/server-log";

describe("server log parsing", () => {
  it("parses structured rust-glancer log lines", () => {
    const parsed = parseServerLogLine(
      JSON.stringify({
        schema: "rust-glancer-log/v1",
        level: "INFO",
        component: "engine",
        engine: "simple_crate",
        target: "rg_lsp_engine::engine::worker",
        message: "workspace indexing finished",
        fields: {
          elapsed_ms: 7630,
          workspace_root: "/workspace/simple_crate",
        },
      }),
    );

    assert.equal(parsed.kind, "structured");
    if (parsed.kind === "structured") {
      assert.equal(parsed.record.level, "info");
      assert.equal(parsed.record.component, "engine");
      assert.equal(parsed.record.engine, "simple_crate");
      assert.equal(
        formatServerLogRecord(parsed.record),
        "[engine:simple_crate] workspace indexing finished elapsed_ms=7630 workspace_root=/workspace/simple_crate",
      );
    }
  });

  it("keeps non-json stderr visible", () => {
    const parsed = parseServerLogLine("error: failed to compile");

    assert.deepEqual(parsed, {
      kind: "raw",
      level: "error",
      message: "error: failed to compile",
    });
  });
});
