/**
 * Adds rust-glancer-specific actions to VS Code hover results.
 *
 * The language server provides navigation data through standard LSP requests; this feature turns
 * that data into safe command links so users can jump from hover text to related declarations.
 */
import * as vscode from "vscode";
import {
  ImplementationRequest,
  TypeDefinitionRequest,
  type ImplementationParams,
  type LanguageClient,
  type LanguageClientOptions,
  type TypeDefinitionParams,
} from "vscode-languageclient/node";

import { EXTENSION_COMMANDS } from "../commands";
import {
  hoverAction,
  hoverActionLinkLine,
  locationsExcludingCurrentHover,
  protocolDefinitionLocations,
  uniqueLocations,
  type HoverAction,
  type SerializedLocation,
  type SerializedRange,
} from "./hover-actions-model";

export function hoverMiddleware(
  clientProvider: () => LanguageClient | undefined,
  output: vscode.LogOutputChannel,
): LanguageClientOptions["middleware"] {
  return {
    async provideHover(document, position, token, next) {
      const hover = await next(document, position, token);
      if (hover == null || token.isCancellationRequested) {
        return hover;
      }

      const client = clientProvider();
      if (client === undefined) {
        return hover;
      }

      try {
        const [rawTypeLocations, rawImplementationLocations] = await Promise.all([
          navigationLocationsOrEmpty(output, "hover go-to-type", () =>
            typeDefinitionLocations(client, document, position),
          ),
          navigationLocationsOrEmpty(output, "hover go-to-implementation", () =>
            implementationLocations(client, document, position),
          ),
        ]);
        const typeLocations = locationsExcludingCurrentHover(
          rawTypeLocations,
          document.uri.toString(),
          hoverRange(hover.range),
        );
        const implementationTargets = locationsExcludingCurrentHover(
          rawImplementationLocations,
          document.uri.toString(),
          hoverRange(hover.range),
        );
        if (token.isCancellationRequested) {
          return hover;
        }

        return appendHoverActions(
          hover,
          [
            hoverAction(
              EXTENSION_COMMANDS.goToTypeFromHover,
              typeLocations,
              "type",
              "type definitions",
            ),
            hoverAction(
              EXTENSION_COMMANDS.goToImplementationFromHover,
              implementationTargets,
              "implementation",
              "implementations",
            ),
          ].filter((action) => action.locations.length > 0),
        );
      } catch (error) {
        output.warn(`hover navigation action failed: ${String(error)}`);
        return hover;
      }
    },
  };
}

export function registerHoverActionCommands(output: vscode.LogOutputChannel): vscode.Disposable {
  return vscode.Disposable.from(
    registerGoToLocationsCommand(
      EXTENSION_COMMANDS.goToTypeFromHover,
      output,
      "hover go-to-type",
      "No type definition found",
    ),
    registerGoToLocationsCommand(
      EXTENSION_COMMANDS.goToImplementationFromHover,
      output,
      "hover go-to-implementation",
      "No implementation found",
    ),
  );
}

function registerGoToLocationsCommand(
  command: string,
  output: vscode.LogOutputChannel,
  logLabel: string,
  notFoundMessage: string,
): vscode.Disposable {
  return vscode.commands.registerCommand(
    command,
    async (serializedLocations?: SerializedLocation[]) => {
      const locations = toVsCodeLocations(serializedLocations);
      if (locations.length === 0) {
        output.warn(`${logLabel} command ignored empty locations`);
        return;
      }

      const activeEditor = vscode.window.activeTextEditor;
      const originUri = activeEditor?.document.uri ?? locations[0].uri;
      const originPosition = activeEditor?.selection.active ?? locations[0].range.start;
      await vscode.commands.executeCommand(
        "editor.action.goToLocations",
        originUri,
        originPosition,
        locations,
        "peek",
        notFoundMessage,
      );
    },
  );
}

async function typeDefinitionLocations(
  client: LanguageClient,
  document: vscode.TextDocument,
  position: vscode.Position,
): Promise<SerializedLocation[]> {
  const params: TypeDefinitionParams = {
    textDocument: { uri: document.uri.toString() },
    position: { line: position.line, character: position.character },
  };

  const definition = await client.sendRequest(TypeDefinitionRequest.type, params);
  return uniqueLocations(protocolDefinitionLocations(definition));
}

async function implementationLocations(
  client: LanguageClient,
  document: vscode.TextDocument,
  position: vscode.Position,
): Promise<SerializedLocation[]> {
  const params: ImplementationParams = {
    textDocument: { uri: document.uri.toString() },
    position: { line: position.line, character: position.character },
  };

  const implementation = await client.sendRequest(ImplementationRequest.type, params);
  return uniqueLocations(protocolDefinitionLocations(implementation));
}

async function navigationLocationsOrEmpty(
  output: vscode.LogOutputChannel,
  label: string,
  load: () => Promise<SerializedLocation[]>,
): Promise<SerializedLocation[]> {
  try {
    return await load();
  } catch (error) {
    output.warn(`${label} action failed: ${String(error)}`);
    return [];
  }
}

function appendHoverActions(hover: vscode.Hover, actions: readonly HoverAction[]): vscode.Hover {
  if (actions.length === 0) {
    return hover;
  }

  const contents = Array.isArray(hover.contents) ? [...hover.contents] : [hover.contents];
  contents.push(commandLinkLine(actions));
  return new vscode.Hover(contents, hover.range);
}

function commandLinkLine(actions: readonly HoverAction[]): vscode.MarkdownString {
  const linkLine = hoverActionLinkLine(actions);
  const line = new vscode.MarkdownString(linkLine.markdown);

  // Only these locally generated command links are trusted. The server-rendered docs and signatures
  // keep VS Code's default untrusted Markdown behavior.
  line.isTrusted = { enabledCommands: [...linkLine.enabledCommands] };
  return line;
}

function toVsCodeLocations(
  locations: readonly SerializedLocation[] | undefined,
): vscode.Location[] {
  return (locations ?? []).map(
    (location) => new vscode.Location(vscode.Uri.parse(location.uri), range(location.range)),
  );
}

function hoverRange(range: vscode.Range | undefined): SerializedRange | undefined {
  if (range === undefined) {
    return undefined;
  }

  return {
    start: range.start,
    end: range.end,
  };
}

function range(range: SerializedRange): vscode.Range {
  return new vscode.Range(
    range.start.line,
    range.start.character,
    range.end.line,
    range.end.character,
  );
}
