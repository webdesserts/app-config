/** Maximum number of symbolic links followed when resolving a write target. */
export declare const MAX_SYMLINK_DEPTH = 16;
/** Stable error codes shared with the Rust implementation's `AppConfigError::code()`. */
export type AppConfigErrorCode = "InvalidSlug" | "InvalidEnvPrefix" | "NoHome" | "Io" | "Parse" | "Serialize" | "SymlinkLoop";
export interface AppConfigErrorDetails {
    readonly path?: string;
    /** 1-based line of a `Parse` error. */
    readonly line?: number;
    /** 1-based column (in characters) of a `Parse` error. */
    readonly column?: number;
    readonly cause?: unknown;
}
export declare class AppConfigError extends Error {
    readonly code: AppConfigErrorCode;
    readonly path: string | undefined;
    readonly line: number | undefined;
    readonly column: number | undefined;
    constructor(code: AppConfigErrorCode, message: string, details?: AppConfigErrorDetails);
}
export interface ResolveOptions {
    /** Optional vendor segment: `<xdg>/<vendor>/<app>` (e.g. legacy `~/.config/webdesserts/mesh`). */
    readonly vendor?: string;
    /** Opt-in `<PREFIX>_{CONFIG,DATA,STATE,CACHE}_DIR` overrides (e.g. `AUTONOMY_XDG`). */
    readonly envPrefix?: string;
    /** Hermetic environment for tests; defaults to `process.env`. */
    readonly env?: Readonly<Record<string, string | undefined>>;
}
export interface AppDirs {
    readonly configDir: string;
    readonly dataDir: string;
    readonly stateDir: string;
    readonly cacheDir: string;
}
/** Resolved per-app XDG directories. Nothing is created until an `ensure*` call. */
export declare class AppPaths implements AppDirs {
    readonly app: string;
    readonly configDir: string;
    readonly dataDir: string;
    readonly stateDir: string;
    readonly cacheDir: string;
    constructor(app: string, dirs: AppDirs);
    /** Creates the config dir chain with mode `0o755` (subject to umask); never chmods. */
    ensureConfigDir(): string;
    /**
     * Creates the config dir chain and enforces `0o700` on the leaf (unless it is a
     * symlink). Use when config files carry credentials or keys.
     */
    ensurePrivateConfigDir(): string;
    /** Creates the data dir chain and enforces `0o700` on the leaf (unless it is a symlink). */
    ensureDataDir(): string;
    /** Creates the state dir chain and enforces `0o700` on the leaf (unless it is a symlink). */
    ensureStateDir(): string;
    /** Creates the cache dir chain and enforces `0o700` on the leaf (unless it is a symlink). */
    ensureCacheDir(): string;
}
/** Resolves `app`'s directories (see the module docs for the rules). */
export declare function resolveAppPaths(app: string, options?: ResolveOptions): AppPaths;
/** Hermetic layout for tests: `<root>/{config,data,state,cache}/<app>`. */
export declare function resolveAppPathsForTest(root: string, app: string): AppPaths;
/**
 * Follows `file`'s symlink chain (up to `MAX_SYMLINK_DEPTH` links, dangling links
 * included) to the file that a write should actually replace.
 */
export declare function resolveWriteTarget(file: string): string;
/**
 * Atomically replaces the file behind `file` (following symlinks, so a dotfiles
 * symlink stays a symlink). The temp file is created `O_EXCL` with its final mode:
 * `mode` if given, else the existing target's mode, else `0o644`.
 */
export declare function writeAtomic(file: string, data: string | Uint8Array, mode?: number): void;
/**
 * `writeAtomic` with mode `0o600`, set when the temp file is created so the bytes
 * are never readable by other users.
 */
export declare function writeSecretAtomic(file: string, data: string | Uint8Array): void;
export type TomlTable = Record<string, unknown>;
/**
 * Parses a TOML file; `null` when it does not exist. Malformed files are reported
 * (with path, line and column) and never touched. The result is not validated.
 */
export declare function loadTomlOptional<T = TomlTable>(file: string): T | null;
/** `loadTomlOptional`, with `defaultValue` for a missing file. */
export declare function loadTomlOrDefault<T = TomlTable>(file: string, defaultValue: T): T;
/** Serializes `value` as TOML and `writeAtomic`s it (existing mode kept, else `0o644`). */
export declare function saveTomlAtomic(file: string, value: TomlTable): void;
/** Serializes `value` as TOML and `writeSecretAtomic`s it (`0o600`). */
export declare function saveSecretTomlAtomic(file: string, value: TomlTable): void;
/**
 * Parses a JSON file; `null` when it does not exist. Malformed files are reported
 * (with path, line and column) and never touched. The result is not validated.
 */
export declare function loadJsonOptional<T = unknown>(file: string): T | null;
/** Serializes `value` as pretty JSON (trailing newline) and `writeAtomic`s it. */
export declare function saveJsonAtomic(file: string, value: unknown): void;
