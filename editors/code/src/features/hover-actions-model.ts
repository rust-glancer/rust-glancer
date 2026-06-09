/**
 * Pure hover-action model helpers. VS Code objects are adapted at the edge in `hover-actions.ts`;
 * this file keeps command-link generation and location filtering easy to unit-test.
 */

export interface SerializedLocation {
  readonly uri: string;
  readonly range: SerializedRange;
}

export interface SerializedRange {
  readonly start: SerializedPosition;
  readonly end: SerializedPosition;
}

export interface SerializedPosition {
  readonly line: number;
  readonly character: number;
}

export interface HoverAction {
  readonly command: string;
  readonly label: string;
  readonly locations: readonly SerializedLocation[];
}

export interface HoverActionLinkLine {
  readonly markdown: string;
  readonly enabledCommands: readonly string[];
}

interface ProtocolLocationLike {
  readonly uri: string;
  readonly range: SerializedRange;
}

interface ProtocolLocationLinkLike {
  readonly targetUri: string;
  readonly targetRange: SerializedRange;
  readonly targetSelectionRange?: SerializedRange;
}

export type ProtocolDefinitionLike =
  | ProtocolLocationLike
  | readonly (ProtocolLocationLike | ProtocolLocationLinkLike)[]
  | null;

export function hoverAction(
  command: string,
  locations: readonly SerializedLocation[],
  singularLabel: string,
  pluralNoun: string,
): HoverAction {
  const label = locations.length === 1 ? singularLabel : `${locations.length} ${pluralNoun}`;
  return { command, label, locations };
}

export function hoverActionLinkLine(actions: readonly HoverAction[]): HoverActionLinkLine {
  const links = actions.map(commandLink).join(" | ");
  return {
    markdown: `Go to ${links}`,
    enabledCommands: actions.map((action) => action.command),
  };
}

export function protocolDefinitionLocations(
  definition: ProtocolDefinitionLike,
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

export function uniqueLocations(locations: SerializedLocation[]): SerializedLocation[] {
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

export function locationsExcludingCurrentHover(
  locations: SerializedLocation[],
  documentUri: string,
  hoverRange: SerializedRange | undefined,
): SerializedLocation[] {
  if (hoverRange === undefined) {
    return locations;
  }

  // Type-definition providers often return the declaration itself when hovering a type
  // declaration. Suppress that self-link so hover actions stay useful rather than decorative.
  return locations.filter(
    (location) => location.uri !== documentUri || !sameRange(location.range, hoverRange),
  );
}

function commandLink(action: HoverAction): string {
  const args = encodeURIComponent(JSON.stringify([action.locations]));
  return `[${action.label}](command:${action.command}?${args})`;
}

function sameRange(left: SerializedRange, right: SerializedRange): boolean {
  return samePosition(left.start, right.start) && samePosition(left.end, right.end);
}

function samePosition(left: SerializedPosition, right: SerializedPosition): boolean {
  return left.line === right.line && left.character === right.character;
}

function isLocationLink(
  value: ProtocolLocationLike | ProtocolLocationLinkLike,
): value is ProtocolLocationLinkLike {
  return "targetUri" in value;
}
