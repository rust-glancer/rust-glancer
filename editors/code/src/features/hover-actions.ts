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
  type Definition,
  type ImplementationParams,
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

interface HoverAction {
  readonly command: string;
  readonly label: string;
  readonly locations: readonly SerializedLocation[];
}

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
          document.uri,
          hover.range,
        );
        const implementationTargets = locationsExcludingCurrentHover(
          rawImplementationLocations,
          document.uri,
          hover.range,
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
    async (serializedLocations: unknown) => {
      const locations = deserializeLocations(serializedLocations);
      if (locations.length === 0) {
        output.warn(`${logLabel} command ignored empty or malformed locations`);
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

function appendHoverActions(
  hover: vscode.Hover,
  actions: readonly HoverAction[],
): vscode.Hover {
  if (actions.length === 0) {
    return hover;
  }

  const contents = Array.isArray(hover.contents) ? [...hover.contents] : [hover.contents];
  contents.push(commandLinkLine(actions));
  return new vscode.Hover(contents, hover.range);
}

function hoverAction(
  command: string,
  locations: readonly SerializedLocation[],
  singularLabel: string,
  pluralNoun: string,
): HoverAction {
  const label = locations.length === 1 ? singularLabel : `${locations.length} ${pluralNoun}`;
  return { command, label, locations };
}

function commandLinkLine(actions: readonly HoverAction[]): vscode.MarkdownString {
  const links = actions.map(commandLink).join(" | ");
  const line = new vscode.MarkdownString(`Go to ${links}`);

  // Only these locally generated command links are trusted. The server-rendered docs and signatures
  // keep VS Code's default untrusted Markdown behavior.
  line.isTrusted = { enabledCommands: actions.map((action) => action.command) };
  return line;
}

function commandLink(action: HoverAction): string {
  const args = encodeURIComponent(JSON.stringify([action.locations]));
  return `[${action.label}](command:${action.command}?${args})`;
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

function locationsExcludingCurrentHover(
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
