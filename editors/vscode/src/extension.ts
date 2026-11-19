// Starts cfx-lua-ls over stdio for Lua files and restarts it on demand or when the
// server path setting changes.

import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import { LanguageClient, LanguageClientOptions, ServerOptions } from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  context.subscriptions.push(
    vscode.commands.registerCommand("cfxLuaLs.restartServer", () => restart(context)),
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("cfxLuaLs.server.path")) {
        void restart(context);
      }
    }),
  );
  await start(context);
}

export async function deactivate(): Promise<void> {
  await stop();
}

/// An absolute path to the server, or an error to show.
/// The user setting, else the binary shipped in `server/`, else the first absolute
/// PATH folder that has one. A workspace setting never counts (it would let a cloned
/// repository choose the program), and a bare name is never spawned: on Windows that
/// searches the current folder, the workspace, before PATH.
function serverCommand(context: vscode.ExtensionContext): { command: string } | { error: string } {
  const configured = vscode.workspace.getConfiguration("cfxLuaLs").inspect<string>("server.path")?.globalValue?.trim();
  if (configured) {
    if (!path.isAbsolute(configured)) {
      return { error: `cfxLuaLs.server.path must be an absolute path, not "${configured}".` };
    }
    return fs.existsSync(configured) ? { command: configured } : { error: `no server at ${configured} (cfxLuaLs.server.path).` };
  }
  const exe = process.platform === "win32" ? "cfx-lua-ls.exe" : "cfx-lua-ls";
  const bundled = path.join(context.extensionPath, "server", exe);
  if (fs.existsSync(bundled)) {
    return { command: bundled };
  }
  for (const dir of (process.env.PATH ?? "").split(path.delimiter)) {
    const candidate = path.join(dir, exe);
    if (path.isAbsolute(dir) && fs.existsSync(candidate)) {
      return { command: candidate };
    }
  }
  return { error: "no cfx-lua-ls found. Set cfxLuaLs.server.path in your user settings." };
}

async function start(context: vscode.ExtensionContext): Promise<void> {
  const found = serverCommand(context);
  if ("error" in found) {
    void vscode.window.showErrorMessage(`cfx-lua-ls: ${found.error}`);
    return;
  }
  const command = found.command;
  const serverOptions: ServerOptions = { command, args: ["lsp"], options: { cwd: context.extensionPath } };
  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: "file", language: "lua" },
      { scheme: "untitled", language: "lua" },
    ],
    // LuaLS's own keys (`Lua.format.enable`, `Lua.format.defaultConfig`), sent as
    // `{ Lua: { format } }` on start and on every change.
    // `Lua.diagnostics.*`, `Lua.type.*`, `Lua.doc.*`, `Lua.hint.*` (inlay
    // hints, off unless `Lua.hint.enable`), `Lua.hover.expandAlias` and
    // `Lua.runtime.special` are LuaLS's own settings: read, not contributed, so
    // both extensions can be installed without clashing.
    synchronize: {
      configurationSection: ["Lua.format", "Lua.diagnostics", "Lua.type", "Lua.doc", "Lua.hint", "Lua.hover", "Lua.runtime"],
      fileEvents: vscode.workspace.createFileSystemWatcher("**/{*.lua,.luarc.json,.luarc.jsonc}"),
    },
  };
  client = new LanguageClient("cfxLuaLs", "CfxLua Language Server", serverOptions, clientOptions);
  try {
    await client.start();
  } catch (e) {
    client = undefined;
    void vscode.window.showErrorMessage(
      `cfx-lua-ls did not start (${command}): ${e instanceof Error ? e.message : String(e)}. ` +
        "Set cfxLuaLs.server.path in your user settings.",
    );
  }
}

async function stop(): Promise<void> {
  const running = client;
  client = undefined;
  if (running?.isRunning()) {
    await running.stop();
  }
}

async function restart(context: vscode.ExtensionContext): Promise<void> {
  await stop();
  await start(context);
}
