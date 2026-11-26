# Troubleshooting

If Rust Glancer won't start, crashes, or gets stuck indexing, logs are very helpful to identify the issue.
Set `RUST_GLANCER_LOG` to this value, then restart the language server and reproduce the problem.

```text
info,rg_lsp_server=debug,rg_project=debug,rg_def_map::progress=debug
```

- **VS Code and forks.** In `settings.json`, add the variable to `rust-glancer.server.extraEnv`.
  Run **Rust Glancer: Restart Server**, then use **Rust Glancer: Open Logs** to see the output.
  If startup fails, also check the **Rust Glancer Extension** output channel for the launch error
  or exit code.
- **Zed.** In `settings.json`, add the variable under `lsp` → `rust-glancer` → `binary` → `env`.
  Run `editor: restart language server`, then use `dev: open language server logs` and select
  **Server Logs** for Rust Glancer. If the server never starts, check `zed: open log`.
- **nvim.** Set `vim.env.RUST_GLANCER_LOG` to the value above in your config, then restart nvim.
  Run `:lua print(vim.lsp.log.get_filename())` to find the log file.
- **Other editors.** Set the variable in the server's environment and look for the stderr output
  from `rust-glancer lsp` in your editor's logs.

If you're filing an issue, please include the part of the log from startup through the failure
if you can. Feel free to redact private paths. You can remove the setting when you're done.
