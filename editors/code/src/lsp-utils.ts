import * as vscode from "vscode";

export function isRustFile(document: vscode.TextDocument): boolean {
  return document.uri.scheme === "file" && document.languageId === "rust";
}
