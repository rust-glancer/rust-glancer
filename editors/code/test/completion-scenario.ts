import * as assert from "node:assert/strict";
import * as vscode from "vscode";

import type { CompletionObservation } from "../src/test-support/completion-observer";
import {
  currentCompletionObservations,
  readySession,
  waitForClientState,
  waitForCompletionObservation,
} from "./extension-harness";
import { RendererKeyboard } from "./renderer-keyboard";

interface EditorSettings {
  readonly quickSuggestions: unknown;
  readonly quickSuggestionsDelay: unknown;
  readonly wordBasedSuggestions: unknown;
}

interface EditorPoint {
  readonly version: number;
  readonly line: number;
  readonly character: number;
}

/** Drives native renderer typing while exposing the completion lifecycle as named scenarios. */
export class CompletionScenario {
  private keyboard: RendererKeyboard | undefined;

  private constructor(
    private readonly editor: vscode.TextEditor,
    private readonly editorConfig: vscode.WorkspaceConfiguration,
    private readonly originalText: string,
    private readonly previousSettings: EditorSettings,
  ) {}

  public static async run(document: vscode.TextDocument): Promise<void> {
    const editor = await vscode.window.showTextDocument(document);
    const editorConfig = vscode.workspace.getConfiguration("editor", document.uri);
    const scenario = new CompletionScenario(editor, editorConfig, document.getText(), {
      quickSuggestions: editorConfig.inspect<unknown>("quickSuggestions")?.globalValue,
      quickSuggestionsDelay: editorConfig.inspect<unknown>("quickSuggestionsDelay")?.globalValue,
      wordBasedSuggestions: editorConfig.inspect<unknown>("wordBasedSuggestions")?.globalValue,
    });

    try {
      await scenario.enableImmediateQuickSuggestions();
      scenario.keyboard = await RendererKeyboard.connect();

      // Reuse the same open document and LSP session. Each run checks that VS Code can keep
      // filtering one complete semantic list locally without starting a provider refresh for
      // every additional character.
      for (let run = 1; run <= 3; run += 1) {
        await scenario.completeThroughRapidClientFiltering(run);
      }
      await scenario.dismissWithoutOpeningASuccessor();
    } finally {
      await scenario.restore();
    }
  }

  private async enableImmediateQuickSuggestions(): Promise<void> {
    await this.editorConfig.update(
      "quickSuggestions",
      { other: "on", comments: "off", strings: "off" },
      vscode.ConfigurationTarget.Global,
    );
    await this.editorConfig.update("quickSuggestionsDelay", 0, vscode.ConfigurationTarget.Global);
    await this.editorConfig.update(
      "wordBasedSuggestions",
      "allDocuments",
      vscode.ConfigurationTarget.Global,
    );
    await vscode.commands.executeCommand("workbench.action.focusActiveEditorGroup");
  }

  private async completeThroughRapidClientFiltering(run: number): Promise<void> {
    await this.prepareScratch();
    const before = await currentCompletionObservations();

    // Ordinary quick suggestions produce one complete semantic list. The remaining prefix arrives
    // through the renderer's native input path and should be filtered from that list client-side.
    await this.type("Comp");
    const initial = this.currentPoint();
    const initialReady = await waitForCompletionObservation(
      before.length,
      (observation) =>
        observation.phase === "finish" &&
        observation.outcome === "ready" &&
        observation.observedVersion === initial.version &&
        observation.incomplete === false &&
        hasSemanticFixture(observation),
      `run ${run} initial semantic quick suggestion`,
    );
    assert.equal(initialReady.outcome, "ready");

    await this.type("l");
    await this.type("etion");
    await this.type("Fix");
    await this.keyboardOrThrow().waitForSuggestWidgetVisibility(true);

    // Accepting the only matching semantic item gives this completion session a concrete end. By
    // the time the exact edit and the hidden widget are observed, any refresh belonging to the
    // session must already have entered the provider middleware.
    const acceptedText = `${this.originalText}\nimpl CompletionFixture`;
    const accepted = waitForDocumentText(this.editor.document, acceptedText);
    await this.keyboardOrThrow().acceptSelectedSuggestion();
    await accepted;
    await this.keyboardOrThrow().waitForSuggestWidgetVisibility(false);
    await waitForClientState((state) => state.session?.activeCompletionAttempts === 0);

    const observations = (await currentCompletionObservations()).slice(before.length);
    assert.deepEqual(
      observations
        .filter((observation) => observation.phase === "start")
        .map((observation) => observation.attempt),
      [initialReady.attempt],
      `run ${run} must filter the complete semantic result without another provider request: ${JSON.stringify(observations)}`,
    );
  }

  private async dismissWithoutOpeningASuccessor(): Promise<void> {
    await this.prepareScratch();
    const before = await currentCompletionObservations();

    await this.type("Comp");
    const initial = this.currentPoint();
    await waitForCompletionObservation(
      before.length,
      (observation) =>
        observation.phase === "finish" &&
        observation.outcome === "ready" &&
        observation.observedVersion === initial.version &&
        observation.incomplete === false &&
        hasSemanticFixture(observation),
      "dismissal setup semantic result",
    );

    await this.type("l");
    const unchanged = this.currentPoint();
    await this.keyboardOrThrow().escape();
    await this.keyboardOrThrow().waitForSuggestWidgetVisibility(false);

    const state = await waitForClientState(
      (candidate) => candidate.session?.activeCompletionAttempts === 0,
    );
    assert.equal(state.session?.activeCompletionAttempts, 0);
    const observations = (await currentCompletionObservations()).slice(before.length);
    assert.ok(
      !observations.some(
        (observation) => observation.phase === "start" && isAt(observation, unchanged),
      ),
      `complete results and Escape must not open a provider request at the unchanged point: ${JSON.stringify(observations)}`,
    );
  }

  private async prepareScratch(): Promise<void> {
    const document = this.editor.document;
    const currentText = document.getText();
    const wholeDocument = new vscode.Range(
      document.positionAt(0),
      document.positionAt(currentText.length),
    );
    const scratch = `${this.originalText}\nimpl `;
    assert.equal(
      await this.editor.edit((edit) => edit.replace(wholeDocument, scratch)),
      true,
      "completion scratch setup should apply",
    );
    const end = document.positionAt(scratch.length);
    this.editor.selection = new vscode.Selection(end, end);
    await vscode.commands.executeCommand("workbench.action.focusActiveEditorGroup");
  }

  private async type(text: string): Promise<void> {
    await vscode.commands.executeCommand("workbench.action.focusActiveEditorGroup");
    const document = this.editor.document;
    const insertionOffset = document.offsetAt(this.editor.selection.active);
    const textBefore = document.getText();
    const expectedText = `${textBefore.slice(0, insertionOffset)}${text}${textBefore.slice(insertionOffset)}`;
    const changed = waitForDocumentText(document, expectedText);
    await this.keyboardOrThrow().type(text);
    await changed.catch(async () => {
      const activeElement = await this.keyboardOrThrow().activeElementDescription();
      const currentText = document.getText();
      assert.fail(
        [
          `renderer keyboard did not type ${JSON.stringify(text)}`,
          `active element: ${activeElement}`,
          `document version: ${document.version}`,
          `selection: ${this.editor.selection.active.line}:${this.editor.selection.active.character}`,
          `expected tail: ${JSON.stringify(expectedText.slice(-80))}`,
          `current tail: ${JSON.stringify(currentText.slice(-80))}`,
        ].join("; "),
      );
    });
  }

  private currentPoint(): EditorPoint {
    return {
      version: this.editor.document.version,
      line: this.editor.selection.active.line,
      character: this.editor.selection.active.character,
    };
  }

  private keyboardOrThrow(): RendererKeyboard {
    assert.ok(this.keyboard, "renderer keyboard should be connected");
    return this.keyboard;
  }

  private async restore(): Promise<void> {
    this.keyboard?.dispose();
    const document = this.editor.document;
    const currentText = document.getText();
    const currentDocument = new vscode.Range(
      document.positionAt(0),
      document.positionAt(currentText.length),
    );
    assert.equal(
      await this.editor.edit((edit) => edit.replace(currentDocument, this.originalText)),
      true,
      "completion fixture cleanup should restore the document",
    );
    assert.equal(
      await document.save(),
      true,
      "completion fixture cleanup should restore the saved editor state",
    );
    await this.editorConfig.update(
      "quickSuggestions",
      this.previousSettings.quickSuggestions,
      vscode.ConfigurationTarget.Global,
    );
    await this.editorConfig.update(
      "quickSuggestionsDelay",
      this.previousSettings.quickSuggestionsDelay,
      vscode.ConfigurationTarget.Global,
    );
    await this.editorConfig.update(
      "wordBasedSuggestions",
      this.previousSettings.wordBasedSuggestions,
      vscode.ConfigurationTarget.Global,
    );
    await waitForClientState((state) => readySession(state) !== undefined);
  }
}

function waitForDocumentText(document: vscode.TextDocument, expectedText: string): Promise<void> {
  return new Promise((resolve, reject) => {
    let settled = false;
    const finish = (error?: Error): void => {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(timeout);
      changed.dispose();
      if (error === undefined) {
        resolve();
      } else {
        reject(error);
      }
    };
    const changed = vscode.workspace.onDidChangeTextDocument((event) => {
      if (event.document === document && document.getText() === expectedText) {
        finish();
      }
    });
    const timeout = setTimeout(
      () => finish(new Error("document did not observe renderer text")),
      2_000,
    );
    if (document.getText() === expectedText) {
      finish();
    }
  });
}

function isAt(observation: CompletionObservation, point: EditorPoint): boolean {
  return (
    observation.version === point.version &&
    observation.line === point.line &&
    observation.character === point.character
  );
}

function hasSemanticFixture(observation: CompletionObservation): boolean {
  return (
    observation.candidates?.some(
      (candidate) =>
        candidate.label === "CompletionFixture" &&
        candidate.kind === vscode.CompletionItemKind.Struct,
    ) === true
  );
}
