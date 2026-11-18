# CfxLua Language Server for VS Code

Runs [cfx-lua-ls](https://github.com/TGIANN/cfx-lua-ls) for Lua files: syntax
diagnostics, outline (document symbols), folding, expand / shrink selection and
semantic highlighting.

## Settings

| setting | default | |
|---|---|---|
| `cfxLuaLs.server.path` | empty | Absolute path to `cfx-lua-ls`, from user settings only (a workspace cannot choose the program that runs). Empty: the binary shipped in the extension's `server/` folder, else `cfx-lua-ls` from an absolute `PATH` folder. |
| `cfxLuaLs.trace.server` | `off` | `messages` / `verbose` log the protocol to the output channel. |

Command: **CfxLua: Restart Language Server**.

With the Lua (LuaLS) extension enabled too, both servers run and their
diagnostics show twice; disable one of them for the workspace.

## Build from source

```sh
cargo build --release            # in the repository root
cd editors/vscode
npm ci
npm run typecheck
npm run build
```

Then run VS Code with `--extensionDevelopmentPath=<repo>/editors/vscode` and set
`cfxLuaLs.server.path` to `<repo>/target/release/cfx-lua-ls(.exe)`.
