import * as assert from "node:assert/strict";
import * as vscode from "vscode";

import { waitFor, withTimeout } from "./async";
import { RendererEditor } from "./renderer-editor";

/** Exercise automatic suggestions and keyboard acceptance against the real language server. */
export async function completeInEditor(document: vscode.TextDocument): Promise<void> {
  assert.equal(document.isDirty, false, "completion fixture should start from its saved contents");
  const editor = await vscode.window.showTextDocument(document);
  const config = vscode.workspace.getConfiguration("editor", document.uri);
  const settings = {
    quickSuggestions: { other: "on", comments: "off", strings: "off" },
    quickSuggestionsDelay: 0,
    wordBasedSuggestions: "allDocuments",
  };
  const previousSettings = Object.keys(settings).map(
    (key) => [key, config.inspect<unknown>(key)?.globalValue] as const,
  );
  const eol = document.eol === vscode.EndOfLine.CRLF ? "\r\n" : "\n";
  const beforeCursor = `${document.getText()}${eol}fn example() {${eol}    let note = model::Note::`;
  const afterCursor = `;${eol}}${eol}`;
  let renderer: RendererEditor | undefined;
  let failed = false;
  let failure: unknown;

  try {
    for (const [key, value] of Object.entries(settings)) {
      await withTimeout(config.update(key, value, vscode.ConfigurationTarget.Global), `set ${key}`);
    }
    renderer = await RendererEditor.connect();

    for (const action of ["accept", "dismiss"] as const) {
      const wholeDocument = new vscode.Range(
        document.positionAt(0),
        document.positionAt(document.getText().length),
      );
      assert.equal(
        await withTimeout(
          editor.edit((edit) => edit.replace(wholeDocument, beforeCursor + afterCursor)),
          "prepare completion input",
        ),
        true,
      );
      const cursor = document.positionAt(beforeCursor.length);
      editor.selection = new vscode.Selection(cursor, cursor);
      editor.revealRange(new vscode.Range(cursor, cursor));
      await withTimeout(
        vscode.commands.executeCommand("workbench.action.focusActiveEditorGroup"),
        "focus completion editor",
      );
      await renderer.focus();

      // Only setup edits use the extension API. Input events must open suggestions automatically,
      // and more typing must leave the desired item available for ordinary keyboard acceptance.
      await renderer.type("ne");
      await renderer.waitForSuggestion("new");
      await renderer.type("w");
      await waitFor(
        "typed completion prefix",
        () => document.getText(),
        (text) => text === `${beforeCursor}new${afterCursor}`,
      );
      await renderer.waitForSuggestion("new");

      if (action === "accept") {
        await renderer.acceptSuggestion("new");
        // Word suggestions can insert a name, but only the semantic provider knows the call's
        // arguments. This checks the accepted result without inspecting provider identities.
        await waitFor(
          "accepted function call",
          () => document.getText(),
          (text) => text === `${beforeCursor}new(id, body)${afterCursor}`,
        );
      } else {
        await renderer.escape();
        assert.equal(document.getText(), `${beforeCursor}new${afterCursor}`);
      }
      await renderer.waitForSuggestionsHidden();
    }
  } catch (error) {
    failed = true;
    failure = error;
    if (renderer !== undefined) {
      try {
        console.error("Renderer state before cleanup:", await renderer.snapshot());
      } catch (diagnosticError) {
        console.error("Could not read renderer state:", diagnosticError);
      }
    }
  } finally {
    renderer?.dispose();
    // Discard the unsaved scratch text. Every cleanup is attempted, and cleanup errors must not
    // hide the original test failure. No source file needs to be written to disk for this test.
    const cleanup = [
      () => vscode.commands.executeCommand("workbench.action.files.revert", document.uri),
      ...previousSettings.map(
        ([key, value]) =>
          () =>
            config.update(key, value, vscode.ConfigurationTarget.Global),
      ),
    ];
    const errors: unknown[] = [];
    for (const restore of cleanup) {
      try {
        await withTimeout(restore(), "restore completion fixture");
      } catch (error) {
        errors.push(error);
      }
    }
    if (errors.length > 0) {
      if (failed) {
        console.error("Completion cleanup failed:", errors);
      } else {
        failed = true;
        failure = new AggregateError(errors, "Completion cleanup failed");
      }
    }
  }
  if (failed) {
    throw failure;
  }
}
