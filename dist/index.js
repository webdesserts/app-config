/**
 * XDG base-directory resolution and symlink-preserving atomic config I/O shared
 * by webdesserts apps (autonomy, feeds, mesh, notes).
 *
 * The Rust twin (`app-config` crate) lives in `src/lib.rs`; both implementations
 * run the shared golden vectors in `fixtures/xdg-cases.json`.
 *
 * Resolution rules for `resolveAppPaths(app, options)` (per directory kind):
 * 1. `options.envPrefix` set and `<PREFIX>_{CONFIG,DATA,STATE,CACHE}_DIR` usable → that path verbatim.
 * 2. `XDG_{CONFIG,DATA,STATE,CACHE}_HOME` usable → `<xdg>/[<vendor>/]<app>`.
 * 3. otherwise `<home>/{.config,.local/share,.local/state,.cache}/[<vendor>/]<app>`.
 *
 * A value is usable when it is non-empty and absolute (relative XDG values are
 * ignored per the XDG Base Directory spec).
 */
import { randomBytes } from "node:crypto";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { parse as parseToml, stringify as stringifyToml, TomlError } from "smol-toml";
/** Maximum number of symbolic links followed when resolving a write target. */
export const MAX_SYMLINK_DEPTH = 16;
const SLUG_RE = /^[a-z][a-z0-9_-]{0,63}$/;
const ENV_PREFIX_RE = /^[A-Z][A-Z0-9_]{0,63}$/;
export class AppConfigError extends Error {
    code;
    path;
    line;
    column;
    constructor(code, message, details = {}) {
        super(message, details.cause === undefined ? undefined : { cause: details.cause });
        this.name = "AppConfigError";
        this.code = code;
        this.path = details.path;
        this.line = details.line;
        this.column = details.column;
    }
}
/** Resolved per-app XDG directories. Nothing is created until an `ensure*` call. */
export class AppPaths {
    app;
    configDir;
    dataDir;
    stateDir;
    cacheDir;
    constructor(app, dirs) {
        validateSlug("app", app);
        this.app = app;
        this.configDir = dirs.configDir;
        this.dataDir = dirs.dataDir;
        this.stateDir = dirs.stateDir;
        this.cacheDir = dirs.cacheDir;
        Object.freeze(this);
    }
    /** Creates the config dir chain with mode `0o755` (subject to umask); never chmods. */
    ensureConfigDir() {
        createDirChain(this.configDir, 0o755, false);
        return this.configDir;
    }
    /**
     * Creates the config dir chain and enforces `0o700` on the leaf (unless it is a
     * symlink). Use when config files carry credentials or keys.
     */
    ensurePrivateConfigDir() {
        createDirChain(this.configDir, 0o700, true);
        return this.configDir;
    }
    /** Creates the data dir chain and enforces `0o700` on the leaf (unless it is a symlink). */
    ensureDataDir() {
        createDirChain(this.dataDir, 0o700, true);
        return this.dataDir;
    }
    /** Creates the state dir chain and enforces `0o700` on the leaf (unless it is a symlink). */
    ensureStateDir() {
        createDirChain(this.stateDir, 0o700, true);
        return this.stateDir;
    }
    /** Creates the cache dir chain and enforces `0o700` on the leaf (unless it is a symlink). */
    ensureCacheDir() {
        createDirChain(this.cacheDir, 0o700, true);
        return this.cacheDir;
    }
}
function validateSlug(field, value) {
    if (!SLUG_RE.test(value)) {
        throw new AppConfigError("InvalidSlug", `invalid ${field} ${JSON.stringify(value)}: expected ^[a-z][a-z0-9_-]{0,63}$`);
    }
}
const DIR_KINDS = [
    ["configDir", "CONFIG", "XDG_CONFIG_HOME", ".config"],
    ["dataDir", "DATA", "XDG_DATA_HOME", ".local/share"],
    ["stateDir", "STATE", "XDG_STATE_HOME", ".local/state"],
    ["cacheDir", "CACHE", "XDG_CACHE_HOME", ".cache"],
];
/** Resolves `app`'s directories (see the module docs for the rules). */
export function resolveAppPaths(app, options = {}) {
    validateSlug("app", app);
    const { vendor, envPrefix } = options;
    if (vendor !== undefined)
        validateSlug("vendor", vendor);
    if (envPrefix !== undefined && !ENV_PREFIX_RE.test(envPrefix)) {
        throw new AppConfigError("InvalidEnvPrefix", `invalid envPrefix ${JSON.stringify(envPrefix)}: expected ^[A-Z][A-Z0-9_]{0,63}$`);
    }
    const env = options.env ?? process.env;
    const usable = (key) => {
        const value = env[key];
        return value !== undefined && value !== "" && path.isAbsolute(value) ? value : undefined;
    };
    const subpath = vendor === undefined ? app : path.join(vendor, app);
    let home;
    const homeDir = () => {
        if (home !== undefined)
            return home;
        const candidate = usable("HOME") ?? os.homedir();
        if (candidate === "" || !path.isAbsolute(candidate)) {
            throw new AppConfigError("NoHome", "cannot determine an absolute home directory (HOME unset or relative)");
        }
        return (home = candidate);
    };
    const dirs = {};
    for (const [key, kind, xdgVar, homeRel] of DIR_KINDS) {
        const override = envPrefix === undefined ? undefined : usable(`${envPrefix}_${kind}_DIR`);
        const xdg = usable(xdgVar);
        dirs[key] =
            override ??
                (xdg === undefined ? path.join(homeDir(), homeRel, subpath) : path.join(xdg, subpath));
    }
    return new AppPaths(app, dirs);
}
/** Hermetic layout for tests: `<root>/{config,data,state,cache}/<app>`. */
export function resolveAppPathsForTest(root, app) {
    validateSlug("app", app);
    return new AppPaths(app, {
        configDir: path.join(root, "config", app),
        dataDir: path.join(root, "data", app),
        stateDir: path.join(root, "state", app),
        cacheDir: path.join(root, "cache", app),
    });
}
function errorCode(error) {
    return typeof error === "object" && error !== null && "code" in error
        ? String(error.code)
        : undefined;
}
function ioError(at, error) {
    const reason = error instanceof Error ? error.message : String(error);
    return new AppConfigError("Io", `${at}: ${reason}`, { path: at, cause: error });
}
function createDirChain(dir, mode, enforceLeaf) {
    try {
        fs.mkdirSync(dir, { recursive: true, mode });
        if (enforceLeaf && process.platform !== "win32") {
            const stat = fs.lstatSync(dir);
            if (!stat.isSymbolicLink() && (stat.mode & 0o7777) !== mode)
                fs.chmodSync(dir, mode);
        }
    }
    catch (error) {
        throw ioError(dir, error);
    }
}
/**
 * Follows `file`'s symlink chain (up to `MAX_SYMLINK_DEPTH` links, dangling links
 * included) to the file that a write should actually replace.
 */
export function resolveWriteTarget(file) {
    let current = file;
    for (let depth = 0; depth <= MAX_SYMLINK_DEPTH; depth++) {
        let stat;
        let link;
        try {
            stat = fs.lstatSync(current, { throwIfNoEntry: false });
            if (stat === undefined || !stat.isSymbolicLink())
                return current;
            link = fs.readlinkSync(current);
        }
        catch (error) {
            throw ioError(current, error);
        }
        current = path.isAbsolute(link) ? link : path.join(path.dirname(current), link);
    }
    throw new AppConfigError("SymlinkLoop", `${file}: more than ${MAX_SYMLINK_DEPTH} levels of symbolic links`, { path: file });
}
function existingMode(target) {
    try {
        const stat = fs.statSync(target, { throwIfNoEntry: false });
        return stat === undefined ? undefined : stat.mode & 0o7777;
    }
    catch {
        return undefined;
    }
}
/**
 * Atomically replaces the file behind `file` (following symlinks, so a dotfiles
 * symlink stays a symlink). The temp file is created `O_EXCL` with its final mode:
 * `mode` if given, else the existing target's mode, else `0o644`.
 */
export function writeAtomic(file, data, mode) {
    const target = resolveWriteTarget(file);
    const parent = path.dirname(target);
    try {
        fs.mkdirSync(parent, { recursive: true });
    }
    catch (error) {
        throw ioError(parent, error);
    }
    const finalMode = mode ?? existingMode(target) ?? 0o644;
    const suffix = `${process.pid}.${randomBytes(6).toString("hex")}`;
    const tmp = path.join(parent, `.${path.basename(target)}.tmp.${suffix}`);
    let fd;
    try {
        fd = fs.openSync(tmp, "wx", finalMode);
    }
    catch (error) {
        throw ioError(tmp, error);
    }
    try {
        // The open mode is masked by the umask; pin the exact requested mode.
        if (process.platform !== "win32")
            fs.fchmodSync(fd, finalMode);
        fs.writeFileSync(fd, data);
        fs.fsyncSync(fd);
        fs.closeSync(fd);
        fd = undefined;
        fs.renameSync(tmp, target);
    }
    catch (error) {
        if (fd !== undefined) {
            try {
                fs.closeSync(fd);
            }
            catch {
                // best effort
            }
        }
        try {
            fs.unlinkSync(tmp);
        }
        catch {
            // best effort
        }
        throw ioError(target, error);
    }
}
/**
 * `writeAtomic` with mode `0o600`, set when the temp file is created so the bytes
 * are never readable by other users.
 */
export function writeSecretAtomic(file, data) {
    writeAtomic(file, data, 0o600);
}
function readOptional(file) {
    try {
        return fs.readFileSync(file, "utf8");
    }
    catch (error) {
        if (errorCode(error) === "ENOENT")
            return null;
        throw ioError(file, error);
    }
}
/** 1-based (line, column-in-characters) of a UTF-16 offset. */
function lineCol(text, offset) {
    const before = text.slice(0, Math.max(0, Math.min(offset, text.length)));
    const lines = before.split("\n");
    const last = lines[lines.length - 1] ?? "";
    return { line: lines.length, column: Array.from(last).length + 1 };
}
function parseError(file, format, line, column, message, cause) {
    return new AppConfigError("Parse", `${file}:${line}:${column}: invalid ${format}: ${message}`, { path: file, line, column, cause });
}
/**
 * Parses a TOML file; `null` when it does not exist. Malformed files are reported
 * (with path, line and column) and never touched. The result is not validated.
 */
export function loadTomlOptional(file) {
    const text = readOptional(file);
    if (text === null)
        return null;
    try {
        return parseToml(text);
    }
    catch (error) {
        if (error instanceof TomlError) {
            const message = (error.message.split("\n")[0] ?? "").replace(/^Invalid TOML document: /, "");
            throw parseError(file, "TOML", error.line, error.column, message.trim(), error);
        }
        throw error;
    }
}
/** `loadTomlOptional`, with `defaultValue` for a missing file. */
export function loadTomlOrDefault(file, defaultValue) {
    return loadTomlOptional(file) ?? defaultValue;
}
function toToml(file, value) {
    try {
        const text = stringifyToml(value);
        return text.endsWith("\n") || text === "" ? text : `${text}\n`;
    }
    catch (error) {
        const reason = error instanceof Error ? error.message : String(error);
        throw new AppConfigError("Serialize", `${file}: cannot serialize TOML: ${reason}`, {
            path: file,
            cause: error,
        });
    }
}
/** Serializes `value` as TOML and `writeAtomic`s it (existing mode kept, else `0o644`). */
export function saveTomlAtomic(file, value) {
    writeAtomic(file, toToml(file, value));
}
/** Serializes `value` as TOML and `writeSecretAtomic`s it (`0o600`). */
export function saveSecretTomlAtomic(file, value) {
    writeSecretAtomic(file, toToml(file, value));
}
/**
 * Parses a JSON file; `null` when it does not exist. Malformed files are reported
 * (with path, line and column) and never touched. The result is not validated.
 */
export function loadJsonOptional(file) {
    const text = readOptional(file);
    if (text === null)
        return null;
    try {
        return JSON.parse(text);
    }
    catch (error) {
        if (!(error instanceof SyntaxError))
            throw error;
        const lineMatch = /\(line (\d+) column (\d+)\)/.exec(error.message);
        const positionMatch = / at position (\d+)/.exec(error.message);
        const { line, column } = lineMatch !== null
            ? { line: Number(lineMatch[1]), column: Number(lineMatch[2]) }
            : lineCol(text, positionMatch === null ? text.length : Number(positionMatch[1]));
        const message = error.message.replace(/ in JSON at position \d+.*$/s, "").replace(/ \(line \d+ column \d+\)$/, "");
        throw parseError(file, "JSON", line, column, message, error);
    }
}
/** Serializes `value` as pretty JSON (trailing newline) and `writeAtomic`s it. */
export function saveJsonAtomic(file, value) {
    const text = JSON.stringify(value, null, 2);
    if (text === undefined) {
        throw new AppConfigError("Serialize", `${file}: cannot serialize JSON: value is not serializable`, {
            path: file,
        });
    }
    writeAtomic(file, `${text}\n`);
}
