import * as assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  formatRawServerLogLine,
  formatServerLogLine,
  formatServerLogRecord,
  parseServerLogLine,
} from "../src/logging/server-log";

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
        "[rg_lsp_engine::engine::worker] [engine:simple_crate] workspace indexing finished elapsed_ms=7630 workspace_root=/workspace/simple_crate",
      );
    }
  });

  it("parses the Rust log level used for channel routing", () => {
    const parsed = parseServerLogLine(
      JSON.stringify({
        schema: "rust-glancer-log/v1",
        level: "TRACE",
        component: "server",
        target: "rg_lsp_server::backend",
        message: "request received",
        fields: {},
      }),
    );

    assert.equal(parsed.kind, "structured");
    if (parsed.kind === "structured") {
      assert.equal(parsed.record.level, "trace");
      assert.equal(
        formatServerLogRecord(parsed.record),
        "[rg_lsp_server::backend] [server] request received",
      );
    }
  });

  it("formats complete structured log lines with the Rust log level", () => {
    assert.equal(
      formatServerLogLine(
        {
          level: "trace",
          component: "server",
          target: "rg_lsp_server::backend",
          message: "request received",
          fields: {
            method: "hover",
          },
        },
        new Date(2026, 4, 13, 22, 23, 58, 537),
      ),
      "2026-05-13 22:23:58.537 [trace] [rg_lsp_server::backend] [server] request received method=hover",
    );
  });

  it("formats raw stderr lines without relying on VS Code log-channel chrome", () => {
    assert.equal(
      formatRawServerLogLine(
        "error",
        "error: failed to compile",
        new Date(2026, 4, 13, 22, 23, 58, 537),
      ),
      "2026-05-13 22:23:58.537 [error] error: failed to compile",
    );
  });

  it("formats structured log lines without a target", () => {
    assert.equal(
      formatServerLogRecord({
        level: "info",
        component: "server",
        message: "server started",
        fields: {},
      }),
      "[server] server started",
    );
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
