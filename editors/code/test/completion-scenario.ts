import * as assert from "node:assert/strict";
import * as vscode from "vscode";

import type { CompletionObservation } from "../src/test-support/completion-observer";
import {
  currentCompletionObservations,
  delay,
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

      // Reuse the same open document and LSP session. The later runs specifically check that an
      // overtaken request did not leave sticky completion state behind for its successor.
      for (let run = 1; run <= 3; run += 1) {
        await scenario.completeAfterOvertakingIncompleteRefresh(run);
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

  private async completeAfterOvertakingIncompleteRefresh(run: number): Promise<void> {
    await this.prepareScratch();
    const before = await currentCompletionObservations();

    // Ordinary quick suggestions first produce one semantic incomplete list. Continuing by one
    // character starts its native incomplete refresh; only then do we overtake that exact attempt.
    await this.type("Comp");
    const initial = this.currentPoint();
    const initialReady = await waitForCompletionObservation(
      before.length,
      (observation) =>
        observation.phase === "finish" &&
        observation.outcome === "ready" &&
        observation.observedVersion === initial.version &&
        observation.incomplete === true &&
        hasSemanticFixture(observation),
      `run ${run} initial semantic quick suggestion`,
    );
    assert.equal(initialReady.outcome, "ready");

    await this.type("l");
    const refresh = this.currentPoint();
    const refreshStart = await waitForCompletionObservation(
      before.length,
      (observation) => observation.phase === "start" && isAt(observation, refresh),
      `run ${run} incomplete-list refresh start`,
    );

    await this.type("etion");
    const overtakenFinish = await waitForCompletionObservation(
      before.length,
      (observation) =>
        observation.phase === "finish" &&
        observation.attempt === refreshStart.attempt &&
        observation.observedVersion !== undefined &&
        observation.observedVersion > refresh.version &&
        (observation.outcome === "ready" || observation.outcome === "cancelled"),
      `run ${run} overtaken refresh finish`,
    );
    assert.ok(
      overtakenFinish.observedVersion !== undefined &&
        overtakenFinish.observedVersion > refresh.version,
      `run ${run} must overtake the refresh with a later document version: ${JSON.stringify(overtakenFinish)}`,
    );
    if (overtakenFinish.outcome === "ready") {
      assert.ok(
        hasSemanticFixture(overtakenFinish),
        `run ${run} live request must return semantic candidates after overtaking: ${JSON.stringify(overtakenFinish)}`,
      );
    }

    // Continue after the overtaken request settles, then require a semantic result observed against
    // the final document version. No test command or extension middleware opens a successor.
    await this.type("Fix");
    const final = this.currentPoint();
    await waitForClientState((state) => {
      const observations = state.session?.completionObservations.slice(before.length) ?? [];
      return (
        state.session?.activeCompletionAttempts === 0 &&
        observations.some(
          (observation) =>
            observation.phase === "finish" &&
            observation.outcome === "ready" &&
            observation.observedVersion === final.version &&
            hasSemanticFixture(observation),
        )
      );
    });

    const observations = (await currentCompletionObservations()).slice(before.length);
    assert.ok(
      observations.some(
        (observation) =>
          observation.phase === "finish" &&
          observation.outcome === "ready" &&
          observation.observedVersion === final.version &&
          hasSemanticFixture(observation),
      ),
      `run ${run} must return semantic candidates against the final document version: ${JSON.stringify(observations)}`,
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
        observation.observedVersion === initial.version,
      "dismissal setup semantic result",
    );

    await this.type("l");
    const unchanged = this.currentPoint();
    const active = await waitForCompletionObservation(
      before.length,
      (observation) => observation.phase === "start" && isAt(observation, unchanged),
      "dismissal provider start",
    );
    await this.keyboardOrThrow().escape();

    const state = await waitForClientState(
      (candidate) => candidate.session?.activeCompletionAttempts === 0,
    );
    assert.equal(state.session?.activeCompletionAttempts, 0);
    await delay(100);
    const observations = (await currentCompletionObservations()).slice(before.length);
    assert.ok(
      !observations.some(
        (observation) =>
          observation.phase === "start" &&
          observation.attempt !== active.attempt &&
          isAt(observation, unchanged),
      ),
      `Escape must not open another provider request at the unchanged point: ${JSON.stringify(observations)}`,
    );
    assert.equal(
      await this.keyboardOrThrow().suggestWidgetVisible(),
      false,
      "Escape must leave suggestions closed",
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
