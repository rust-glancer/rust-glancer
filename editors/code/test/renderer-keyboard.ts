import * as fs from "node:fs/promises";
import * as path from "node:path";

// Extension-host APIs can edit text but cannot synthesize the renderer key events that normally
// trigger completion. This test-only Chrome DevTools Protocol adapter types through VS Code's real
// editor input path; using a completion command would bypass the behavior the acceptance test is
// meant to prove.
export class RendererKeyboard {
  private nextRequestId = 1;
  private readonly pending = new Map<
    number,
    { resolve: (value: unknown) => void; reject: (error: Error) => void }
  >();

  private constructor(private readonly socket: WebSocket) {
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data)) as DevToolsResponse;
      if (message.id === undefined) {
        return;
      }
      const pending = this.pending.get(message.id);
      if (pending === undefined) {
        return;
      }
      this.pending.delete(message.id);
      if (message.error === undefined) {
        pending.resolve(message.result);
      } else {
        pending.reject(new Error(`DevTools command failed: ${message.error.message}`));
      }
    });
  }

  public static async connect(): Promise<RendererKeyboard> {
    const socket = new WebSocket(await rendererDebuggerUrl());
    await new Promise<void>((resolve, reject) => {
      socket.addEventListener("open", () => resolve(), { once: true });
      socket.addEventListener("error", () => reject(new Error("connect renderer debugger")), {
        once: true,
      });
    });
    return new RendererKeyboard(socket);
  }

  /** Type through VS Code's own workbench test driver and native editor input. */
  public async type(text: string): Promise<void> {
    await this.focusEditorInput();
    const evaluation = (await this.send("Runtime.evaluate", {
      expression: `globalThis.driver.typeInEditor(".native-edit-context", ${JSON.stringify(text)})`,
      awaitPromise: true,
      returnByValue: true,
    })) as RuntimeEvaluation;
    if (evaluation.exceptionDetails !== undefined) {
      throw new Error(
        evaluation.exceptionDetails.exception?.description ?? evaluation.exceptionDetails.text,
      );
    }
  }

  public async escape(): Promise<void> {
    await this.pressKey("Escape", "Escape", 27);
  }

  /** Accept the selected suggestion through the renderer's ordinary keyboard path. */
  public async acceptSelectedSuggestion(): Promise<void> {
    await this.pressKey("Tab", "Tab", 9);
  }

  public async activeElementDescription(): Promise<string> {
    const response = (await this.send("Runtime.evaluate", {
      expression:
        "document.activeElement ? `${document.activeElement.tagName}.${document.activeElement.className}` : '<none>'",
      returnByValue: true,
    })) as { result?: { value?: unknown } };
    return String(response.result?.value ?? "<unknown>");
  }

  public async suggestWidgetVisible(): Promise<boolean> {
    const response = (await this.send("Runtime.evaluate", {
      expression: "document.querySelector('.suggest-widget.visible') !== null",
      returnByValue: true,
    })) as { result?: { value?: unknown } };
    return response.result?.value === true;
  }

  /** Wait for a concrete renderer state; the deadline only bounds a broken test run. */
  public async waitForSuggestWidgetVisibility(expected: boolean): Promise<void> {
    const startedAt = Date.now();
    while (Date.now() - startedAt < 10_000) {
      if ((await this.suggestWidgetVisible()) === expected) {
        return;
      }
      await new Promise((resolve) => setTimeout(resolve, 5));
    }

    throw new Error(`suggest widget did not become ${expected ? "visible" : "hidden"}`);
  }

  public dispose(): void {
    this.socket.close();
    for (const pending of this.pending.values()) {
      pending.reject(new Error("renderer debugger closed"));
    }
    this.pending.clear();
  }

  private send(method: string, params: Record<string, unknown>): Promise<unknown> {
    const id = this.nextRequestId;
    this.nextRequestId += 1;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  private async pressKey(key: string, code: string, keyCode: number): Promise<void> {
    await this.focusEditorInput();
    const event = {
      key,
      code,
      windowsVirtualKeyCode: keyCode,
      nativeVirtualKeyCode: keyCode,
    };
    await this.send("Input.dispatchKeyEvent", { type: "keyDown", ...event });
    await this.send("Input.dispatchKeyEvent", { type: "keyUp", ...event });
  }

  private async focusEditorInput(): Promise<void> {
    await this.send("Page.bringToFront", {});
    // VS Code deliberately avoids activating the OS window while extension tests run. Chromium
    // would otherwise report the workbench document as unfocused, which makes VS Code's editor
    // input controller discard both key and EditContext events before they reach the model.
    await this.send("Emulation.setFocusEmulationEnabled", { enabled: true });
    const evaluation = (await this.send("Runtime.evaluate", {
      expression: `(() => {
        window.focus();
        const editor = document.activeElement?.classList.contains("native-edit-context")
          ? document.activeElement
          : document.querySelector(".native-edit-context");
        editor?.blur();
        editor?.focus({ preventScroll: true });
        return editor != null;
      })()`,
      returnByValue: true,
    })) as RuntimeEvaluation;
    if (evaluation.exceptionDetails !== undefined) {
      throw new Error(
        evaluation.exceptionDetails.exception?.description ?? evaluation.exceptionDetails.text,
      );
    }
    if (evaluation.result?.value !== true) {
      throw new Error("native editor input not found");
    }
  }
}

interface DevToolsResponse {
  readonly id?: number;
  readonly result?: unknown;
  readonly error?: { readonly message: string };
}

interface RuntimeEvaluation {
  readonly result?: { readonly value?: unknown };
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

async function rendererDebuggerUrl(): Promise<string> {
  const userDataDir = process.env.RUST_GLANCER_VSCODE_USER_DATA_DIR;
  if (userDataDir === undefined) {
    throw new Error("test launcher did not expose the VS Code user-data directory");
  }
  const activePortFile = path.join(userDataDir, "DevToolsActivePort");
  const startedAt = Date.now();
  let lastError: unknown;

  while (Date.now() - startedAt < 10_000) {
    try {
      const [port] = (await fs.readFile(activePortFile, "utf8")).trim().split("\n");
      const response = await fetch(`http://127.0.0.1:${port}/json/list`);
      const targets = (await response.json()) as DevToolsTarget[];
      const renderer = targets.find(
        (target) => target.type === "page" && target.url.includes("workbench"),
      );
      if (renderer?.webSocketDebuggerUrl !== undefined) {
        return renderer.webSocketDebuggerUrl;
      }
    } catch (error) {
      lastError = error;
    }

    await new Promise((resolve) => setTimeout(resolve, 25));
  }

  throw new Error(`could not discover VS Code renderer debugger: ${String(lastError)}`);
}
