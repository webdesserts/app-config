import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import { syncBuiltinESMExports } from "node:module";
import os from "node:os";
import path from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  AppConfigError,
  type AppDirs,
  loadJsonOptional,
  loadTomlOptional,
  loadTomlOrDefault,
  resolveAppPaths,
  resolveAppPathsForTest,
  resolveWriteTarget,
  saveJsonAtomic,
  saveSecretTomlAtomic,
  saveTomlAtomic,
  writeAtomic,
  writeSecretAtomic,
} from "../src/index.ts";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

interface ResolveCase {
  name: string;
  app: string;
  options?: { vendor?: string; envPrefix?: string };
  env: Record<string, string>;
  expect?: AppDirs;
  error?: string;
}
interface ForTestCase {
  name: string;
  root: string;
  app: string;
  expect?: AppDirs;
  error?: string;
}
interface ParseErrorCase {
  name: string;
  format: "toml" | "json";
  text: string;
  line: number;
  column: number;
}
interface Fixtures {
  resolve: ResolveCase[];
  forTest: ForTestCase[];
  parseErrors: ParseErrorCase[];
}

const fixtures = JSON.parse(fs.readFileSync(path.join(repo, "fixtures/xdg-cases.json"), "utf8")) as Fixtures;
const DIR_KEYS = ["configDir", "dataDir", "stateDir", "cacheDir"] as const;

const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), "app-config-test-"));
after(() => fs.rmSync(tmpRoot, { recursive: true, force: true }));
let tmpCounter = 0;
function tmpDir(): string {
  const dir = path.join(tmpRoot, String(tmpCounter++));
  fs.mkdirSync(dir);
  return dir;
}

/** smol-toml yields null-prototype tables (safe against __proto__ keys); compare as plain data. */
function plain(value: unknown): unknown {
  return JSON.parse(JSON.stringify(value));
}

function modeOf(file: string): number {
  return fs.statSync(file).mode & 0o777;
}

function assertAppConfigError(fn: () => unknown, code: string, label: string): AppConfigError {
  try {
    fn();
  } catch (error) {
    assert.ok(error instanceof AppConfigError, `${label}: expected AppConfigError, got ${String(error)}`);
    assert.equal(error.code, code, `${label}: ${error.message}`);
    return error;
  }
  assert.fail(`${label}: expected ${code}, got success`);
}

test("golden resolve cases", () => {
  assert.ok(fixtures.resolve.length > 0);
  for (const c of fixtures.resolve) {
    const run = () => resolveAppPaths(c.app, { ...(c.options ?? {}), env: c.env });
    if (c.error !== undefined) {
      assertAppConfigError(run, c.error, c.name);
      continue;
    }
    assert.ok(c.expect, `${c.name}: expect`);
    const paths = run();
    assert.equal(paths.app, c.app, c.name);
    for (const key of DIR_KEYS) assert.equal(paths[key], c.expect[key], `${c.name}: ${key}`);
  }
});

test("golden forTest cases", () => {
  for (const c of fixtures.forTest) {
    const run = () => resolveAppPathsForTest(c.root, c.app);
    if (c.error !== undefined) {
      assertAppConfigError(run, c.error, c.name);
      continue;
    }
    assert.ok(c.expect, `${c.name}: expect`);
    const paths = run();
    for (const key of DIR_KEYS) assert.equal(paths[key], c.expect[key], `${c.name}: ${key}`);
  }
});

test("golden parse errors report path, line and column and leave the file alone", () => {
  const dir = tmpDir();
  for (const c of fixtures.parseErrors) {
    const file = path.join(dir, `bad.${c.format}`);
    fs.writeFileSync(file, c.text);
    const error = assertAppConfigError(
      () => (c.format === "toml" ? loadTomlOptional(file) : loadJsonOptional(file)),
      "Parse",
      c.name,
    );
    assert.equal(error.path, file, c.name);
    assert.equal(error.line, c.line, `${c.name}: line (${error.message})`);
    assert.equal(error.column, c.column, `${c.name}: column (${error.message})`);
    assert.ok(error.message.startsWith(`${file}:${c.line}:${c.column}: `), error.message);
    assert.equal(fs.readFileSync(file, "utf8"), c.text, c.name);
  }
});

test("resolveAppPaths reads HOME from the process environment by default", () => {
  const home = process.env["HOME"];
  assert.ok(home !== undefined && path.isAbsolute(home));
  const paths = resolveAppPaths("probe-app", { env: { HOME: home } });
  assert.equal(paths.configDir, path.join(home, ".config", "probe-app"));
  const ambient = resolveAppPaths("probe-app");
  const xdg = process.env["XDG_CONFIG_HOME"];
  const expected = xdg !== undefined && path.isAbsolute(xdg) ? path.join(xdg, "probe-app") : paths.configDir;
  assert.equal(ambient.configDir, expected);
});

test("ensure*Dir modes: config 0755 (umask), private/data/state/cache 0700", () => {
  const paths = resolveAppPathsForTest(tmpDir(), "modes");
  const umask = process.umask();
  assert.equal(paths.ensureConfigDir(), paths.configDir);
  assert.equal(modeOf(paths.configDir), 0o755 & ~umask);
  paths.ensurePrivateConfigDir();
  assert.equal(modeOf(paths.configDir), 0o700);
  for (const ensure of ["ensureDataDir", "ensureStateDir", "ensureCacheDir"] as const) {
    const dir = paths[ensure]();
    assert.equal(modeOf(dir), 0o700, ensure);
  }
  // Re-running is idempotent and re-tightens a loosened leaf.
  fs.chmodSync(paths.stateDir, 0o755);
  paths.ensureStateDir();
  assert.equal(modeOf(paths.stateDir), 0o700);
});

test("ensurePrivateConfigDir never chmods a symlinked leaf", () => {
  const root = tmpDir();
  const real = path.join(root, "real");
  fs.mkdirSync(real, { mode: 0o755 });
  fs.chmodSync(real, 0o755);
  const paths = resolveAppPathsForTest(root, "linked");
  fs.mkdirSync(path.dirname(paths.configDir), { recursive: true });
  fs.symlinkSync(real, paths.configDir);
  paths.ensurePrivateConfigDir();
  assert.ok(fs.lstatSync(paths.configDir).isSymbolicLink());
  assert.equal(modeOf(real), 0o755);
});

test("writeAtomic: new 0644, explicit mode, existing mode kept, parents created, no temp left", () => {
  const dir = tmpDir();
  const fresh = path.join(dir, "nested", "a", "fresh.txt");
  writeAtomic(fresh, "one");
  assert.equal(fs.readFileSync(fresh, "utf8"), "one");
  assert.equal(modeOf(fresh), 0o644);
  const explicit = path.join(dir, "explicit.txt");
  writeAtomic(explicit, new TextEncoder().encode("two"), 0o640);
  assert.equal(modeOf(explicit), 0o640);
  fs.chmodSync(explicit, 0o604);
  writeAtomic(explicit, "three");
  assert.equal(modeOf(explicit), 0o604);
  assert.equal(fs.readFileSync(explicit, "utf8"), "three");
  assert.deepEqual(
    fs.readdirSync(dir).filter((name) => name.includes(".tmp.")),
    [],
  );
});

test("writeSecretAtomic: the temp file is 0600 before any bytes land", () => {
  const dir = tmpDir();
  const file = path.join(dir, "secret.toml");
  fs.writeFileSync(file, "old", { mode: 0o644 });
  fs.chmodSync(file, 0o644);
  const seen: number[] = [];
  const original = { writeSync: fs.writeSync, writeFileSync: fs.writeFileSync };
  const record = (target: unknown) => {
    if (typeof target === "number") seen.push(fs.fstatSync(target).mode & 0o777);
  };
  fs.writeSync = ((fd: number, ...rest: unknown[]) => {
    record(fd);
    return (original.writeSync as (...args: unknown[]) => number)(fd, ...rest);
  }) as typeof fs.writeSync;
  fs.writeFileSync = ((target: unknown, ...rest: unknown[]) => {
    record(target);
    return (original.writeFileSync as (...args: unknown[]) => void)(target, ...rest);
  }) as typeof fs.writeFileSync;
  syncBuiltinESMExports();
  try {
    writeSecretAtomic(file, "token = \"s3cret\"\n");
  } finally {
    fs.writeSync = original.writeSync;
    fs.writeFileSync = original.writeFileSync;
    syncBuiltinESMExports();
  }
  assert.ok(seen.length > 0, "the write went through an fd");
  assert.ok(seen.every((mode) => mode === 0o600), `fd modes seen: ${seen.map((m) => m.toString(8)).join(",")}`);
  assert.equal(modeOf(file), 0o600);
  assert.equal(fs.readFileSync(file, "utf8"), "token = \"s3cret\"\n");
});

test("writeAtomic preserves symlink chains (absolute and relative)", () => {
  const dir = tmpDir();
  const real = path.join(dir, "store", "real.toml");
  fs.mkdirSync(path.dirname(real));
  fs.writeFileSync(real, "old");
  fs.chmodSync(real, 0o640);
  const middle = path.join(dir, "middle.toml");
  fs.symlinkSync(path.join("store", "real.toml"), middle);
  const outer = path.join(dir, "links", "outer.toml");
  fs.mkdirSync(path.dirname(outer));
  fs.symlinkSync(middle, outer);
  assert.equal(resolveWriteTarget(outer), real);
  writeAtomic(outer, "new");
  assert.ok(fs.lstatSync(outer).isSymbolicLink());
  assert.ok(fs.lstatSync(middle).isSymbolicLink());
  assert.equal(fs.readFileSync(real, "utf8"), "new");
  assert.equal(modeOf(real), 0o640);
});

test("writeAtomic through a dangling symlink creates the target", () => {
  const dir = tmpDir();
  const link = path.join(dir, "config.toml");
  fs.symlinkSync(path.join("dotfiles", "config.toml"), link);
  writeAtomic(link, "x = 1\n");
  assert.ok(fs.lstatSync(link).isSymbolicLink());
  assert.equal(fs.readFileSync(path.join(dir, "dotfiles", "config.toml"), "utf8"), "x = 1\n");
});

test("symlink loops are reported, not followed forever", () => {
  const dir = tmpDir();
  const a = path.join(dir, "a");
  const b = path.join(dir, "b");
  fs.symlinkSync(b, a);
  fs.symlinkSync(a, b);
  assertAppConfigError(() => writeAtomic(a, "x"), "SymlinkLoop", "loop");
});

test("TOML and JSON round trips; missing files are null or default", () => {
  const dir = tmpDir();
  const tomlFile = path.join(dir, "c.toml");
  const jsonFile = path.join(dir, "c.json");
  assert.equal(loadTomlOptional(tomlFile), null);
  assert.equal(loadJsonOptional(jsonFile), null);
  assert.deepEqual(loadTomlOrDefault(tomlFile, { port: 1 }), { port: 1 });
  const value = { name: "x", server: { port: 4610, hosts: ["a", "b"] } };
  saveTomlAtomic(tomlFile, value);
  assert.deepEqual(plain(loadTomlOptional(tomlFile)), value);
  assert.deepEqual(plain(loadTomlOrDefault(tomlFile, {})), value);
  saveJsonAtomic(jsonFile, value);
  assert.ok(fs.readFileSync(jsonFile, "utf8").endsWith("}\n"));
  assert.deepEqual(loadJsonOptional(jsonFile), value);
  const secret = path.join(dir, "secret.toml");
  saveSecretTomlAtomic(secret, { token: "t" });
  assert.equal(modeOf(secret), 0o600);
});

test("I/O errors name the path", () => {
  const dir = tmpDir();
  const error = assertAppConfigError(() => loadTomlOptional(dir), "Io", "directory");
  assert.equal(error.path, dir);
  assert.ok(error.message.includes(dir), error.message);
});

test("dist/ is fresh: rebuilding src/index.ts reproduces it byte for byte", () => {
  const out = tmpDir();
  const tsc = path.join(repo, "node_modules", "typescript", "bin", "tsc");
  const result = spawnSync(process.execPath, [tsc, "-p", "tsconfig.build.json", "--outDir", out], {
    cwd: repo,
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  const built = fs.readdirSync(out).sort();
  assert.deepEqual(built, ["index.d.ts", "index.js"]);
  for (const name of built) {
    const committed = path.join(repo, "dist", name);
    assert.ok(fs.existsSync(committed), `dist/${name} missing: run npm run build`);
    assert.equal(
      fs.readFileSync(committed, "utf8"),
      fs.readFileSync(path.join(out, name), "utf8"),
      `dist/${name} is stale: run npm run build`,
    );
  }
});
