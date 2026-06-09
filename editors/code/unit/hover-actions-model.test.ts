import * as assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  hoverAction,
  hoverActionLinkLine,
  locationsExcludingCurrentHover,
  protocolDefinitionLocations,
  uniqueLocations,
  type SerializedLocation,
} from "../src/features/hover-actions-model";
import { EXTENSION_COMMANDS } from "../src/commands";

describe("hover action model", () => {
  it("normalizes protocol locations and removes duplicate targets", () => {
    const declaration = location("file:///src/lib.rs", 1, 4, 1, 10);
    const linkedSelection = location("file:///src/types.rs", 3, 8, 3, 14);

    const actual = uniqueLocations(
      protocolDefinitionLocations([
        declaration,
        declaration,
        {
          targetUri: "file:///src/types.rs",
          targetRange: range(3, 0, 3, 20),
          targetSelectionRange: linkedSelection.range,
        },
      ]),
    );

    assert.deepEqual(actual, [declaration, linkedSelection]);
  });

  it("filters hover self-links without hiding other locations", () => {
    const self = location("file:///src/lib.rs", 1, 4, 1, 10);
    const sameFileDifferentRange = location("file:///src/lib.rs", 2, 4, 2, 10);
    const otherFile = location("file:///src/types.rs", 1, 4, 1, 10);

    assert.deepEqual(
      locationsExcludingCurrentHover(
        [self, sameFileDifferentRange, otherFile],
        "file:///src/lib.rs",
        self.range,
      ),
      [sameFileDifferentRange, otherFile],
    );
  });

  it("renders trusted command links with singular and plural labels", () => {
    const typeTarget = location("file:///src/types.rs", 1, 0, 1, 6);
    const implTargets = [
      location("file:///src/lib.rs", 4, 0, 4, 8),
      location("file:///src/lib.rs", 8, 0, 8, 8),
    ];
    const actions = [
      hoverAction(EXTENSION_COMMANDS.goToTypeFromHover, [typeTarget], "type", "type definitions"),
      hoverAction(
        EXTENSION_COMMANDS.goToImplementationFromHover,
        implTargets,
        "implementation",
        "implementations",
      ),
    ];

    const actual = hoverActionLinkLine(actions);

    assert.deepEqual(actual.enabledCommands, [
      EXTENSION_COMMANDS.goToTypeFromHover,
      EXTENSION_COMMANDS.goToImplementationFromHover,
    ]);
    assert.equal(
      actual.markdown,
      `Go to ${commandLink("type", EXTENSION_COMMANDS.goToTypeFromHover, [typeTarget])} | ${commandLink(
        "2 implementations",
        EXTENSION_COMMANDS.goToImplementationFromHover,
        implTargets,
      )}`,
    );
  });
});

function commandLink(
  label: string,
  command: string,
  locations: readonly SerializedLocation[],
): string {
  const args = encodeURIComponent(JSON.stringify([locations]));
  return `[${label}](command:${command}?${args})`;
}

function location(
  uri: string,
  startLine: number,
  startCharacter: number,
  endLine: number,
  endCharacter: number,
): SerializedLocation {
  return {
    uri,
    range: range(startLine, startCharacter, endLine, endCharacter),
  };
}

function range(
  startLine: number,
  startCharacter: number,
  endLine: number,
  endCharacter: number,
): SerializedLocation["range"] {
  return {
    start: { line: startLine, character: startCharacter },
    end: { line: endLine, character: endCharacter },
  };
}
