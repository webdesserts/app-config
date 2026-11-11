# app-config

Uniform XDG Base Directory resolution and symlink-preserving atomic configuration I/O for `webdesserts` apps — one implementation in Rust (`app-config`) and one in TypeScript (`@webdesserts/app-config`).

Both implementations run the same golden vectors (`fixtures/xdg-cases.json`), so a Rust CLI and a macOS `launchd` app resolve identical directories and write identical bytes.

## Overview

- **One path model everywhere.** `~/.config/<app>`, `~/.local/share/<app>`, `~/.local/state/<app>` and `~/.cache/<app>` resolve the same way on macOS and Linux, including under `launchd` where `XDG_*` variables are unset. Empty or relative `XDG_*` values are ignored, per the XDG Base Directory specification.
- **Optional vendor and env-prefix isolation.** `vendor` inserts a segment (`<xdg>/<vendor>/<app>`, e.g. the legacy `~/.config/webdesserts/mesh`); `envPrefix` opts into `<PREFIX>_{CONFIG,DATA,STATE,CACHE}_DIR` overrides (e.g. `AUTONOMY_XDG`) without colliding with unrelated application variables.
- **Strict directory modes.** Config directories are created `0755` (subject to `umask`); private config, data, state and cache leaves are enforced `0700`. `chmod` is never applied to a symlinked leaf.
- **Symlink-preserving atomic writes.** Temp file, `fsync`, `rename` — following symlink chains (including dangling dotfile symlinks) up to 16 levels, so a symlinked config file stays a symlink. Secret writes set `0600` when the temp file is created, before any byte lands.
- **Diagnostics that leave the file intact.** Missing files return `None` / `null`; malformed files raise an error carrying `path:line:column` and never modify the file.

## Installation

Rust — pin the tag:

```toml
[dependencies]
app-config = { git = "https://github.com/webdesserts/app-config.git", tag = "v0.1.0" }
```

TypeScript / Node.js (>= 22) — pin the tag:

```json
{
  "dependencies": {
    "@webdesserts/app-config": "github:webdesserts/app-config#v0.1.0"
  }
}
```

## Quickstart

### Rust

```rust
use app_config::{AppPaths, ResolveOptions};

// ~/.config/mesh, ~/.local/share/mesh, ~/.local/state/mesh, ~/.cache/mesh
let paths = AppPaths::resolve("mesh")?;

// Optional isolation:
// AppPaths::resolve_with("mesh", &ResolveOptions::new().vendor("webdesserts"))?
// AppPaths::resolve_with("autonomy", &ResolveOptions::new().env_prefix("AUTONOMY_XDG"))?

paths.ensure_private_config_dir()?;                 // 0700 leaf
let config = paths.config_dir().join("config.toml");

let settings: Settings = app_config::load_toml_or_default(&config)?;
app_config::save_toml_atomic(&config, &settings)?;  // atomic, symlink-preserving
app_config::write_secret_atomic(&paths.config_dir().join("admin.token"), token)?; // 0600
```

### TypeScript

```ts
import { resolveAppPaths, loadTomlOrDefault, saveTomlAtomic, writeSecretAtomic } from "@webdesserts/app-config";

// ~/.config/mesh, ~/.local/share/mesh, ~/.local/state/mesh, ~/.cache/mesh
const paths = resolveAppPaths("mesh");

// Optional isolation:
// resolveAppPaths("mesh", { vendor: "webdesserts" });
// resolveAppPaths("autonomy", { envPrefix: "AUTONOMY_XDG" });

paths.ensurePrivateConfigDir();                     // 0700 leaf
const config = `${paths.configDir}/config.toml`;

const settings = loadTomlOrDefault<Settings>(config, {}); // {} when absent
saveTomlAtomic(config, settings);                         // atomic, symlink-preserving
writeSecretAtomic(`${paths.configDir}/admin.token`, token); // 0600
```

## XDG Directories & Permissions

| Directory | Default path | Created mode |
| --- | --- | --- |
| `config` | `~/.config/<app>` | `0755` (subject to `umask`); `0700` leaf via `ensure_private_config_dir` |
| `data` | `~/.local/share/<app>` | `0700` leaf |
| `state` | `~/.local/state/<app>` | `0700` leaf |
| `cache` | `~/.cache/<app>` | `0700` leaf |

- Atomic writes place a temp file in the target directory, `fsync` it, then `rename`. Existing symlinks are resolved and the write lands on the target, preserving the link (depth limit 16; deeper chains raise `SymlinkLoop`).
- Secret writes (`write_secret_atomic` / `writeSecretAtomic`, `save_secret_toml_atomic` / `saveSecretTomlAtomic`) create the temp file with `0600` before writing any bytes.
- Errors expose a stable code — `InvalidSlug`, `InvalidEnvPrefix`, `NoHome`, `Io`, `Parse`, `Serialize`, `SymlinkLoop` — and parse failures render as `path:line:column: invalid <format>: <message>`.

## Development

```sh
cargo test     # Rust suite
npm run build  # rebuild dist/ from src/index.ts
npm test       # Node suite; also asserts dist/ is byte-for-byte fresh
```

Both suites run the shared golden vectors in `fixtures/xdg-cases.json`. For hermetic tests, use `AppPaths::for_test(root, app)` / `resolveAppPathsForTest(root, app)`.

## License

Apache-2.0
