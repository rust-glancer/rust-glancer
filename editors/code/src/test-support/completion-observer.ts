import * as vscode from "vscode";
import type { Middleware } from "vscode-languageclient";

export interface CompletionCandidateObservation {
  readonly label: string;
  readonly kind?: vscode.CompletionItemKind;
}

export type CompletionAttemptOutcome = "ready" | "empty" | "cancelled" | "failed";

/** Passive test evidence captured at the language-client boundary. */
export interface CompletionObservation {
  readonly uri: string;
  readonly version: number;
  readonly line: number;
  readonly character: number;
  readonly attempt: number;
  readonly phase: "start" | "token-cancel" | "finish";
  readonly observedVersion?: number;
  readonly outcome?: CompletionAttemptOutcome;
  readonly incomplete?: boolean;
  readonly candidates?: readonly CompletionCandidateObservation[];
}

/** Records provider inputs and outputs in extension-test mode without changing their lifecycle. */
export class CompletionObserver implements vscode.Disposable {
  private nextAttempt = 1;
  private readonly active = new Set<number>();
  private readonly observations: CompletionObservation[] = [];
  private disposed = false;

  public middleware(): Pick<Middleware, "provideCompletionItem"> {
    return {
      provideCompletionItem: async (document, position, context, token, next) => {
        const attempt = this.nextAttempt;
        this.nextAttempt += 1;
        const request = point(document, position);
        this.active.add(attempt);
        this.record({ ...request, attempt, phase: "start" });

        let cancellationObserved = false;
        const cancellation = token.onCancellationRequested(() => {
          if (cancellationObserved) {
            return;
          }
          cancellationObserved = true;
          this.record({
            ...request,
            attempt,
            phase: "token-cancel",
            observedVersion: document.version,
          });
        });

        try {
          const result = await next(document, position, context, token);
          if (token.isCancellationRequested) {
            this.finish(request, attempt, document.version, "cancelled");
            return result;
          }
          if (result == null) {
            this.finish(request, attempt, document.version, "empty");
            return result;
          }

          const items = Array.isArray(result) ? result : result.items;
          this.finish(request, attempt, document.version, items.length === 0 ? "empty" : "ready", {
            incomplete: !Array.isArray(result) && result.isIncomplete,
            candidates: items.map((item) => ({
              label: typeof item.label === "string" ? item.label : item.label.label,
              kind: item.kind,
            })),
          });
          return result;
        } catch (error) {
          const outcome =
            token.isCancellationRequested || error instanceof vscode.CancellationError
              ? "cancelled"
              : "failed";
          this.finish(request, attempt, document.version, outcome);
          throw error;
        } finally {
          cancellation.dispose();
          this.active.delete(attempt);
        }
      },
    };
  }

  public snapshot(): readonly CompletionObservation[] {
    return this.observations.map((observation) => ({
      ...observation,
      candidates: observation.candidates?.map((candidate) => ({ ...candidate })),
    }));
  }

  public activeAttemptCount(): number {
    return this.active.size;
  }

  public dispose(): void {
    this.disposed = true;
    this.active.clear();
  }

  private finish(
    request: CompletionPoint,
    attempt: number,
    observedVersion: number,
    outcome: CompletionAttemptOutcome,
    details: Pick<CompletionObservation, "incomplete" | "candidates"> = {},
  ): void {
    this.record({
      ...request,
      ...details,
      attempt,
      phase: "finish",
      observedVersion,
      outcome,
    });
  }

  private record(observation: CompletionObservation): void {
    if (this.disposed) {
      return;
    }
    this.observations.push(observation);
    if (this.observations.length > 256) {
      this.observations.shift();
    }
  }
}

interface CompletionPoint {
  readonly uri: string;
  readonly version: number;
  readonly line: number;
  readonly character: number;
}

function point(document: vscode.TextDocument, position: vscode.Position): CompletionPoint {
  return {
    uri: document.uri.toString(),
    version: document.version,
    line: position.line,
    character: position.character,
  };
}
