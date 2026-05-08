/**
 * Adds rust-glancer-specific actions to VS Code hover results.
 *
 * The language server provides type-definition data through standard LSP requests; this feature
 * turns that data into safe command links so users can jump from hover text to the underlying type.
 */
import * as vscode from "vscode";
import {
  TypeDefinitionRequest,
  type Definition,
  type LanguageClient,
  type LanguageClientOptions,
  type Location as ProtocolLocation,
  type LocationLink as ProtocolLocationLink,
  type TypeDefinitionParams,
} from "vscode-languageclient/node";

import { EXTENSION_COMMANDS } from "../commands";

interface SerializedLocation {
  readonly uri: string;
  readonly range: SerializedRange;
}

interface SerializedRange {
  readonly start: SerializedPosition;
  readonly end: SerializedPosition;
}

interface SerializedPosition {
  readonly line: number;
  readonly character: number;
}

export function hoverMiddleware(
  clientProvider: () => LanguageClient | undefined,
  output: vscode.OutputChannel,
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
        const locations = typeDefinitionLocationsExcludingCurrentHover(
          await typeDefinitionLocations(client, document, position),
          document.uri,
          hover.range,
        );
        if (token.isCancellationRequested || locations.length === 0) {
          return hover;
        }

        return appendGoToTypeAction(hover, locations);
      } catch (error) {
        output.appendLine(`hover go-to-type action failed: ${String(error)}`);
        return hover;
      }
    },
  };
}

export function registerHoverActionCommands(output: vscode.OutputChannel): vscode.Disposable {
  return vscode.commands.registerCommand(
    EXTENSION_COMMANDS.goToTypeFromHover,
    async (serializedLocations: unknown) => {
      const locations = deserializeLocations(serializedLocations);
      if (locations.length === 0) {
        output.appendLine("hover go-to-type command ignored empty or malformed locations");
        return;
      }

      if (locations.length === 1) {
        await vscode.window.showTextDocument(locations[0].uri, {
          preview: true,
          selection: locations[0].range,
        });
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
        "goto",
        "No type definition found",
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

function appendGoToTypeAction(
  hover: vscode.Hover,
  locations: readonly SerializedLocation[],
): vscode.Hover {
  const label =
    locations.length === 1 ? "Go to type" : `Go to ${locations.length} type definitions`;
  const args = encodeURIComponent(JSON.stringify([locations]));
  const action = new vscode.MarkdownString(
    `[${label}](command:${EXTENSION_COMMANDS.goToTypeFromHover}?${args})`,
  );

  // Only this locally generated command link is trusted. The server-rendered docs and signatures
  // keep VS Code's default untrusted Markdown behavior.
  action.isTrusted = { enabledCommands: [EXTENSION_COMMANDS.goToTypeFromHover] };

  const contents = Array.isArray(hover.contents) ? [...hover.contents] : [hover.contents];
  contents.push(action);
  return new vscode.Hover(contents, hover.range);
}

function protocolDefinitionLocations(
  definition: Definition | ProtocolLocationLink[] | null,
): SerializedLocation[] {
  if (definition === null) {
    return [];
  }

  const values = Array.isArray(definition) ? definition : [definition];
  return values.map((value) => {
    if (isLocationLink(value)) {
      return {
        uri: value.targetUri,
        range: value.targetSelectionRange ?? value.targetRange,
      };
    }

    return {
      uri: value.uri,
      range: value.range,
    };
  });
}

function uniqueLocations(locations: SerializedLocation[]): SerializedLocation[] {
  const seen = new Set<string>();
  const unique = [];

  for (const location of locations) {
    const key = JSON.stringify(location);
    if (seen.has(key)) {
      continue;
    }

    seen.add(key);
    unique.push(location);
  }

  return unique;
}

function typeDefinitionLocationsExcludingCurrentHover(
  locations: SerializedLocation[],
  documentUri: vscode.Uri,
  hoverRange: vscode.Range | undefined,
): SerializedLocation[] {
  if (hoverRange === undefined) {
    return locations;
  }

  // Type-definition providers often return the declaration itself when hovering a type
  // declaration. Suppress that self-link so hover actions stay useful rather than decorative.
  return locations.filter(
    (location) =>
      location.uri !== documentUri.toString() ||
      !sameRange(location.range, {
        start: hoverRange.start,
        end: hoverRange.end,
      }),
  );
}

function sameRange(left: SerializedRange, right: SerializedRange): boolean {
  return samePosition(left.start, right.start) && samePosition(left.end, right.end);
}

function samePosition(left: SerializedPosition, right: SerializedPosition): boolean {
  return left.line === right.line && left.character === right.character;
}

function deserializeLocations(value: unknown): vscode.Location[] {
  if (!Array.isArray(value)) {
    return [];
  }

  return value.flatMap((location) => {
    const parsed = deserializeLocation(location);
    return parsed === undefined ? [] : [parsed];
  });
}

function deserializeLocation(value: unknown): vscode.Location | undefined {
  if (!isRecord(value) || typeof value.uri !== "string" || !isRecord(value.range)) {
    return undefined;
  }

  const range = deserializeRange(value.range);
  if (range === undefined) {
    return undefined;
  }

  return new vscode.Location(vscode.Uri.parse(value.uri), range);
}

function deserializeRange(value: unknown): vscode.Range | undefined {
  if (!isRecord(value) || !isRecord(value.start) || !isRecord(value.end)) {
    return undefined;
  }

  const start = deserializePosition(value.start);
  const end = deserializePosition(value.end);
  if (start === undefined || end === undefined) {
    return undefined;
  }

  return new vscode.Range(start, end);
}

function deserializePosition(value: unknown): vscode.Position | undefined {
  if (!isRecord(value) || typeof value.line !== "number" || typeof value.character !== "number") {
    return undefined;
  }

  return new vscode.Position(value.line, value.character);
}

function isLocationLink(
  value: ProtocolLocation | ProtocolLocationLink,
): value is ProtocolLocationLink {
  return "targetUri" in value;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
