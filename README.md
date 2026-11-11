# app-config

Shared XDG Base Directory resolution and symlink-preserving atomic configuration I/O for `webdesserts` Rust crates (`app-config`) and TypeScript/Node.js packages (`@webdesserts/app-config`).

## Features

- **Uniform XDG paths on macOS & Linux:** Resolves `~/.config/<app>`, `~/.local/share/<app>`, `~/.local/state/<app>`, and `~/.cache/<app>` identically even under `launchd` where `XDG_*` environment variables are unset, ignoring empty or relative `XDG_*` values per the XDG Base Directory specification.
- **Optional vendor & `envPrefix` isolation:** Supports optional `vendor` subpaths (`<xdg>/<vendor>/<app>`) and opt-in `<PREFIX>_{CONFIG,DATA,STATE,CACHE}_DIR` environment overrides without colliding with existing application environment variables.
- **Strict directory modes:** `0755` (subject to `umask`) for general config directories and enforced `0700` leaf permissions for private config, data, state, and cache directories (never calling `chmod` on symlinked leaves).
- **Symlink-preserving atomic writes:** `write_atomic` / `writeAtomic` and `write_secret_atomic` / `writeSecretAtomic` (`0600` before first byte written) follow symlink chains (including dangling dotfile symlinks) so atomic temp-file + `fsync` + `rename` writes preserve symlinks in place.
- **Diagnostic-preserving TOML & JSON helpers:** Missing files return `None` / `null`; malformed files raise `AppConfigError` (`code: "Parse"`) with exact `path:line:column` without modifying the file.
- **Shared golden test vectors:** Both `cargo test` and `npm test` execute `fixtures/xdg-cases.json` plus a `dist/` byte-for-byte freshness check.

## License

Apache-2.0
