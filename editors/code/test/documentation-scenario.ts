import * as assert from "node:assert/strict";
import * as vscode from "vscode";

import { assertDefinition } from "./extension-harness";
import { waitFor, withTimeout } from "./async";
import { RendererEditor } from "./renderer-editor";

/** Verify that the ordinary language client exposes source documentation through editor APIs. */
export async function inspectDocumentation(document: vscode.TextDocument): Promise<void> {
  const config = vscode.workspace.getConfiguration("editor", document.uri);
  const previous = config.inspect("semanticHighlighting.enabled")?.globalValue;
  let renderer: RendererEditor | undefined;
  try {
    await config.update("semanticHighlighting.enabled", true, vscode.ConfigurationTarget.Global);
    await assertDefinition(document, "[`Self::|new`]", "pub fn |new");
    const link = document.getText().indexOf("[`Self::new`]");
    const hovers = await withTimeout(
      vscode.commands.executeCommand<vscode.Hover[]>(
        "vscode.executeHoverProvider",
        document.uri,
        document.positionAt(link + 8),
      ),
      "hover a documentation link",
    );
    assert.ok(
      hovers?.some((hover) =>
        hover.contents.some((content) =>
          typeof content === "string"
            ? content.includes("fn new")
            : content.value.includes("fn new"),
        ),
      ),
      "source link should show the method declaration",
    );

    const legend = await withTimeout(
      vscode.commands.executeCommand<vscode.SemanticTokensLegend>(
        "vscode.provideDocumentSemanticTokensLegend",
        document.uri,
      ),
      "read semantic token legend",
    );
    assert.ok(legend, "the language client should register semantic tokens");
    const tokens = await waitFor(
      "source documentation tokens",
      () =>
        vscode.commands.executeCommand<vscode.SemanticTokens>(
          "vscode.provideDocumentSemanticTokens",
          document.uri,
        ),
      (tokens) => tokens !== undefined && tokens.data.length > 0,
    );
    assert.ok(tokens);
    const colored = new Map<string, string>();
    let line = 0;
    let column = 0;
    for (let index = 0; index < tokens.data.length; index += 5) {
      const [deltaLine, deltaColumn, length, kind] = tokens.data.slice(index, index + 5);
      line += deltaLine;
      column = deltaLine === 0 ? column + deltaColumn : deltaColumn;
      const text = document.getText(new vscode.Range(line, column, line, column + length));
      colored.set(text, legend.tokenTypes[kind]);
    }
    assert.equal(colored.get("[`Self::new`]"), "method");
    assert.equal(colored.get("let"), "keyword");
    assert.equal(colored.get('"hello"'), "string");
    renderer = await RendererEditor.connect();
    const editor = await vscode.window.showTextDocument(document);
    editor.revealRange(new vscode.Range(0, 0, 8, 0));
    await renderer.focus();
    await waitFor(
      "rendered documentation colors",
      () => renderer!.tokenColors(),
      (tokens) => {
        const prose = tokens.find((token) => token.text.includes("Create"));
        const link = tokens.find((token) => token.text.includes("Self::new"));
        const keyword = tokens.find((token) => token.text.trim() === "let");
        return (
          prose !== undefined &&
          link !== undefined &&
          keyword !== undefined &&
          link.color !== prose.color &&
          keyword.color !== prose.color
        );
      },
    );
  } finally {
    renderer?.dispose();
    await config.update(
      "semanticHighlighting.enabled",
      previous,
      vscode.ConfigurationTarget.Global,
    );
  }
}
