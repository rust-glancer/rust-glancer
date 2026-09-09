import * as fs from "node:fs/promises";
import * as path from "node:path";

import { waitFor, withTimeout } from "./async";

// The extension API cannot drive automatic suggestions or inspect their widget. Keep the
// private workbench driver and DOM selectors here, behind the few actions our smoke test needs.
export class RendererEditor {
  private nextRequestId = 1;
  private failure: Error | undefined;
  private readonly pending = new Map<
    number,
    { resolve: (value: unknown) => void; reject: (error: Error) => void }
  >();

  private constructor(private readonly socket: WebSocket) {
    socket.addEventListener("close", () => this.close(new Error("renderer debugger disconnected")));
    socket.addEventListener("error", () =>
      this.close(new Error("renderer debugger socket failed")),
    );
    socket.addEventListener("message", (event) => {
      try {
        const message = JSON.parse(String(event.data)) as DevToolsResponse;
        if (message.id === undefined) {
          return;
        }
        const pending = this.pending.get(message.id);
        if (message.error === undefined) {
          pending?.resolve(message.result);
        } else {
          pending?.reject(new Error(`DevTools command failed: ${message.error.message}`));
        }
      } catch (error) {
        this.close(new Error("invalid renderer debugger response", { cause: error }));
      }
    });
  }

  public static async connect(): Promise<RendererEditor> {
    const socket = new WebSocket(await RendererEditor.debuggerUrl());
    const opened = () => resolveOpen();
    const failed = () => rejectOpen(new Error("connect renderer debugger"));
    let resolveOpen: () => void;
    let rejectOpen: (error: Error) => void;
    const connected = new Promise<void>((resolve, reject) => {
      resolveOpen = resolve;
      rejectOpen = reject;
      socket.addEventListener("open", opened);
      socket.addEventListener("error", failed);
      socket.addEventListener("close", failed);
    });
    try {
      await withTimeout(connected, "connect renderer debugger");
      return new RendererEditor(socket);
    } catch (error) {
      socket.close();
      throw error;
    } finally {
      socket.removeEventListener("open", opened);
      socket.removeEventListener("error", failed);
      socket.removeEventListener("close", failed);
    }
  }

  /** Establish focus once per interaction; typing and acceptance must preserve it themselves. */
  public async focus(): Promise<void> {
    await this.send("Page.bringToFront", {});
    // Extension tests can run in an inactive OS window. Let Chromium deliver editor input while
    // keeping focus changes out of the sequence of typing, acceptance, and dismissal being tested.
    await this.send("Emulation.setFocusEmulationEnabled", { enabled: true });
    await this.requireEditorFocus();
  }

  /** VS Code's smoke driver delivers a synthetic text update to the focused editor input. */
  public async type(text: string): Promise<void> {
    await this.requireEditorFocus();
    await this.evaluate(`globalThis.driver.typeInEditor(
      ".native-edit-context:focus, textarea.inputarea:focus", ${JSON.stringify(text)}
    )`);
  }

  public async waitForSuggestion(label: string): Promise<void> {
    await waitFor(
      `suggestion ${label}`,
      () => this.snapshot(),
      (state) => state.suggestions.some((item) => item.label === label),
    );
  }

  public async acceptSuggestion(label: string): Promise<void> {
    await waitFor(
      `selected suggestion ${label}`,
      () => this.snapshot(),
      (state) => state.suggestions.some((item) => item.label === label && item.focused),
    );
    await this.pressKey("Tab", 9);
  }

  public async escape(): Promise<void> {
    await this.pressKey("Escape", 27);
  }

  public async waitForSuggestionsHidden(): Promise<void> {
    await waitFor(
      "suggestions hidden",
      () => this.snapshot(),
      (state) => !state.suggestVisible,
    );
  }

  /** Read rendered colors to check that semantic tokens really overlay the grammar colors. */
  public async tokenColors(): Promise<{ text: string; color: string }[]> {
    return this.evaluate(`Array.from(document.querySelectorAll('.view-lines .view-line span'))
      .filter(span => span.childElementCount === 0)
      .map(span => ({ text: span.textContent ?? '', color: getComputedStyle(span).color }))`);
  }

  /** The same renderer state used for readiness is retained in failure messages. */
  public async snapshot(): Promise<RendererSnapshot> {
    return this.evaluate(`(() => {
      const input = document.activeElement;
      const widget = document.querySelector('.suggest-widget.visible');
      return {
        documentFocused: document.hasFocus(),
        activeElement: input ? input.tagName + '.' + input.className : '<none>',
        suggestVisible: widget !== null,
        suggestions: Array.from(widget?.querySelectorAll('.monaco-list-row') ?? []).map(row => ({
          label: row.querySelector('.label-name')?.textContent ?? '',
          focused: row.classList.contains('focused')
        }))
      };
    })()`);
  }

  public dispose(): void {
    this.close(new Error("renderer debugger disposed"));
  }

  private close(error: Error): void {
    if (this.failure !== undefined) {
      return;
    }
    this.failure = error;
    for (const pending of this.pending.values()) {
      pending.reject(error);
    }
    this.pending.clear();
    this.socket.close();
  }

  private async send(method: string, params: Record<string, unknown>): Promise<unknown> {
    if (this.failure !== undefined) {
      throw this.failure;
    }
    const id = this.nextRequestId++;
    try {
      return await withTimeout(
        new Promise((resolve, reject) => {
          this.pending.set(id, { resolve, reject });
          this.socket.send(JSON.stringify({ id, method, params }));
        }),
        `DevTools ${method}`,
      );
    } finally {
      // Timed-out requests must not accumulate or consume replies from later interactions.
      this.pending.delete(id);
    }
  }

  private async evaluate<T>(expression: string): Promise<T> {
    const evaluation = (await this.send("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    })) as RuntimeEvaluation;
    if (evaluation.exceptionDetails !== undefined) {
      throw new Error(
        evaluation.exceptionDetails.exception?.description ?? evaluation.exceptionDetails.text,
      );
    }
    return evaluation.result.value as T;
  }

  private async requireEditorFocus(): Promise<void> {
    await this.evaluate(`(() => {
      if (!document.hasFocus() ||
          !document.activeElement?.matches('.native-edit-context, textarea.inputarea')) {
        throw new Error('Expected the editor input to retain focus');
      }
    })()`);
  }

  private async pressKey(key: string, windowsVirtualKeyCode: number): Promise<void> {
    await this.requireEditorFocus();
    const event = { key, code: key, windowsVirtualKeyCode };
    await this.send("Input.dispatchKeyEvent", { type: "keyDown", ...event });
    await this.send("Input.dispatchKeyEvent", { type: "keyUp", ...event });
  }

  private static async debuggerUrl(): Promise<string> {
    const userDataDir = process.env.RUST_GLANCER_VSCODE_USER_DATA_DIR;
    if (userDataDir === undefined) {
      throw new Error("test launcher did not expose the VS Code user-data directory");
    }
    const activePortFile = path.join(userDataDir, "DevToolsActivePort");
    const discovered = await waitFor(
      "renderer debugger address",
      async () => {
        try {
          const [port] = (await fs.readFile(activePortFile, "utf8")).trim().split("\n");
          const response = await fetch(`http://127.0.0.1:${port}/json/list`, {
            signal: AbortSignal.timeout(1_000),
          });
          const targets = (await response.json()) as DevToolsTarget[];
          const target = targets.find(
            (item) => item.type === "page" && item.url.includes("workbench"),
          );
          return { url: target?.webSocketDebuggerUrl, error: undefined };
        } catch (error) {
          return { url: undefined, error: String(error) };
        }
      },
      (result) => result.url !== undefined,
    );
    return discovered.url!;
  }
}

interface RendererSnapshot {
  readonly documentFocused: boolean;
  readonly activeElement: string;
  readonly suggestVisible: boolean;
  readonly suggestions: readonly { readonly label: string; readonly focused: boolean }[];
}

interface DevToolsResponse {
  readonly id?: number;
  readonly result?: unknown;
  readonly error?: { readonly message: string };
}

interface RuntimeEvaluation {
  readonly result: { readonly value?: unknown };
  readonly exceptionDetails?: {
    readonly text: string;
    readonly exception?: { readonly description?: string };
  };
}

interface DevToolsTarget {
  readonly type: string;
  readonly url: string;
  readonly webSocketDebuggerUrl?: string;
}
