//! XDG base-directory resolution and symlink-preserving atomic config I/O shared
//! by webdesserts apps (autonomy, feeds, mesh, notes).
//!
//! The TypeScript twin (`@webdesserts/app-config`) lives in `src/index.ts`; both
//! implementations run the shared golden vectors in `fixtures/xdg-cases.json`.
//!
//! Resolution rules for `AppPaths::resolve_with(app, opts)` (per directory kind):
//! 1. `opts.env_prefix` set and `<PREFIX>_{CONFIG,DATA,STATE,CACHE}_DIR` usable → that path verbatim.
//! 2. `XDG_{CONFIG,DATA,STATE,CACHE}_HOME` usable → `<xdg>/[<vendor>/]<app>`.
//! 3. otherwise `<home>/{.config,.local/share,.local/state,.cache}/[<vendor>/]<app>`.
//!
//! A value is usable when it is non-empty and absolute (relative XDG values are
//! ignored per the XDG Base Directory spec).

use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

pub type Result<T, E = AppConfigError> = std::result::Result<T, E>;

/// Maximum number of symbolic links followed when resolving a write target.
pub const MAX_SYMLINK_DEPTH: usize = 16;

/// Serialization format named in parse/serialize errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Toml,
    Json,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Format::Toml => "TOML",
            Format::Json => "JSON",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppConfigError {
    #[error("invalid {field} {value:?}: expected ^[a-z][a-z0-9_-]{{0,63}}$")]
    InvalidSlug { field: &'static str, value: String },
    #[error("invalid envPrefix {0:?}: expected ^[A-Z][A-Z0-9_]{{0,63}}$")]
    InvalidEnvPrefix(String),
    #[error("cannot determine an absolute home directory (HOME unset or relative)")]
    NoHome,
    #[error("{}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{}:{line}:{column}: invalid {format}: {message}", .path.display())]
    Parse {
        path: PathBuf,
        format: Format,
        /// 1-based line of the error.
        line: usize,
        /// 1-based column (in characters) of the error.
        column: usize,
        message: String,
    },
    #[error("{}: cannot serialize {format}: {message}", .path.display())]
    Serialize {
        path: PathBuf,
        format: Format,
        message: String,
    },
    #[error("{}: more than {MAX_SYMLINK_DEPTH} levels of symbolic links", .path.display())]
    SymlinkLoop { path: PathBuf },
}

impl AppConfigError {
    /// Stable error code shared with the TypeScript implementation's `error.code`.
    pub fn code(&self) -> &'static str {
        match self {
            AppConfigError::InvalidSlug { .. } => "InvalidSlug",
            AppConfigError::InvalidEnvPrefix(_) => "InvalidEnvPrefix",
            AppConfigError::NoHome => "NoHome",
            AppConfigError::Io { .. } => "Io",
            AppConfigError::Parse { .. } => "Parse",
            AppConfigError::Serialize { .. } => "Serialize",
            AppConfigError::SymlinkLoop { .. } => "SymlinkLoop",
        }
    }
}

/// Options for [`AppPaths::resolve_with`].
#[derive(Debug, Clone, Default)]
pub struct ResolveOptions {
    /// Optional vendor segment: `<xdg>/<vendor>/<app>` (e.g. legacy `~/.config/webdesserts/mesh`).
    pub vendor: Option<String>,
    /// Opt-in `<PREFIX>_{CONFIG,DATA,STATE,CACHE}_DIR` overrides (e.g. `AUTONOMY_XDG`).
    pub env_prefix: Option<String>,
    /// Hermetic environment for tests; `None` reads the process environment.
    pub env: Option<HashMap<String, String>>,
}

impl ResolveOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn vendor(mut self, vendor: impl Into<String>) -> Self {
        self.vendor = Some(vendor.into());
        self
    }

    pub fn env_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.env_prefix = Some(prefix.into());
        self
    }

    pub fn env(mut self, env: HashMap<String, String>) -> Self {
        self.env = Some(env);
        self
    }

    /// A usable (non-empty, absolute) path from the environment.
    fn var(&self, key: &str) -> Option<PathBuf> {
        let value = match &self.env {
            Some(env) => env.get(key).map(OsString::from),
            None => std::env::var_os(key),
        }?;
        if value.is_empty() {
            return None;
        }
        let path = PathBuf::from(value);
        path.is_absolute().then_some(path)
    }
}

fn is_slug(value: &str) -> bool {
    let bytes = value.as_bytes();
    matches!(bytes.first(), Some(b'a'..=b'z'))
        && bytes.len() <= 64
        && bytes
            .iter()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
}

fn is_env_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    matches!(bytes.first(), Some(b'A'..=b'Z'))
        && bytes.len() <= 64
        && bytes
            .iter()
            .all(|b| matches!(b, b'A'..=b'Z' | b'0'..=b'9' | b'_'))
}

fn validate_slug(field: &'static str, value: &str) -> Result<()> {
    if is_slug(value) {
        Ok(())
    } else {
        Err(AppConfigError::InvalidSlug {
            field,
            value: value.to_owned(),
        })
    }
}

/// Resolved per-app XDG directories. Nothing is created until an `ensure_*` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    app: String,
    config_dir: PathBuf,
    data_dir: PathBuf,
    state_dir: PathBuf,
    cache_dir: PathBuf,
}

impl AppPaths {
    /// Resolves `app`'s directories from the process environment with default options.
    pub fn resolve(app: &str) -> Result<Self> {
        Self::resolve_with(app, &ResolveOptions::default())
    }

    pub fn resolve_with(app: &str, opts: &ResolveOptions) -> Result<Self> {
        validate_slug("app", app)?;
        if let Some(vendor) = &opts.vendor {
            validate_slug("vendor", vendor)?;
        }
        if let Some(prefix) = &opts.env_prefix
            && !is_env_prefix(prefix)
        {
            return Err(AppConfigError::InvalidEnvPrefix(prefix.clone()));
        }
        let subpath = match &opts.vendor {
            Some(vendor) => Path::new(vendor).join(app),
            None => PathBuf::from(app),
        };
        let home = || {
            opts.var("HOME")
                .or_else(|| dirs::home_dir().filter(|home| home.is_absolute()))
                .ok_or(AppConfigError::NoHome)
        };
        let dir = |kind: &str, xdg_var: &str, home_rel: &str| -> Result<PathBuf> {
            if let Some(prefix) = &opts.env_prefix
                && let Some(path) = opts.var(&format!("{prefix}_{kind}_DIR"))
            {
                return Ok(path);
            }
            if let Some(base) = opts.var(xdg_var) {
                return Ok(base.join(&subpath));
            }
            Ok(home()?.join(home_rel).join(&subpath))
        };
        Ok(Self {
            app: app.to_owned(),
            config_dir: dir("CONFIG", "XDG_CONFIG_HOME", ".config")?,
            data_dir: dir("DATA", "XDG_DATA_HOME", ".local/share")?,
            state_dir: dir("STATE", "XDG_STATE_HOME", ".local/state")?,
            cache_dir: dir("CACHE", "XDG_CACHE_HOME", ".cache")?,
        })
    }

    /// Hermetic layout for tests: `<root>/{config,data,state,cache}/<app>`.
    pub fn for_test(root: impl AsRef<Path>, app: &str) -> Result<Self> {
        validate_slug("app", app)?;
        let root = root.as_ref();
        Ok(Self {
            app: app.to_owned(),
            config_dir: root.join("config").join(app),
            data_dir: root.join("data").join(app),
            state_dir: root.join("state").join(app),
            cache_dir: root.join("cache").join(app),
        })
    }

    pub fn app(&self) -> &str {
        &self.app
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Creates the config dir chain with mode `0o755` (subject to umask); never chmods.
    pub fn ensure_config_dir(&self) -> Result<&Path> {
        create_dir_chain(&self.config_dir, 0o755, false)?;
        Ok(&self.config_dir)
    }

    /// Creates the config dir chain and enforces `0o700` on the leaf (unless it is a
    /// symlink). Use when config files carry credentials or keys.
    pub fn ensure_private_config_dir(&self) -> Result<&Path> {
        create_dir_chain(&self.config_dir, 0o700, true)?;
        Ok(&self.config_dir)
    }

    /// Creates the data dir chain and enforces `0o700` on the leaf (unless it is a symlink).
    pub fn ensure_data_dir(&self) -> Result<&Path> {
        create_dir_chain(&self.data_dir, 0o700, true)?;
        Ok(&self.data_dir)
    }

    /// Creates the state dir chain and enforces `0o700` on the leaf (unless it is a symlink).
    pub fn ensure_state_dir(&self) -> Result<&Path> {
        create_dir_chain(&self.state_dir, 0o700, true)?;
        Ok(&self.state_dir)
    }

    /// Creates the cache dir chain and enforces `0o700` on the leaf (unless it is a symlink).
    pub fn ensure_cache_dir(&self) -> Result<&Path> {
        create_dir_chain(&self.cache_dir, 0o700, true)?;
        Ok(&self.cache_dir)
    }
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> AppConfigError + '_ {
    move |source| AppConfigError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg_attr(not(unix), allow(unused_variables))]
fn create_dir_chain(dir: &Path, mode: u32, enforce_leaf: bool) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(mode);
    }
    builder.create(dir).map_err(io_error(dir))?;
    #[cfg(unix)]
    if enforce_leaf {
        use std::os::unix::fs::PermissionsExt;
        let meta = fs::symlink_metadata(dir).map_err(io_error(dir))?;
        if !meta.file_type().is_symlink() && meta.permissions().mode() & 0o7777 != mode {
            fs::set_permissions(dir, fs::Permissions::from_mode(mode)).map_err(io_error(dir))?;
        }
    }
    Ok(())
}

/// Follows `path`'s symlink chain (up to [`MAX_SYMLINK_DEPTH`] links, dangling links
/// included) to the file that a write should actually replace.
pub fn resolve_write_target(path: impl AsRef<Path>) -> Result<PathBuf> {
    let path = path.as_ref();
    let mut current = path.to_path_buf();
    for _ in 0..=MAX_SYMLINK_DEPTH {
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let link = fs::read_link(&current).map_err(io_error(&current))?;
                current = if link.is_absolute() {
                    link
                } else {
                    current.parent().unwrap_or(Path::new("")).join(link)
                };
            }
            Ok(_) => return Ok(current),
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(current),
            Err(err) => return Err(io_error(&current)(err)),
        }
    }
    Err(AppConfigError::SymlinkLoop {
        path: path.to_path_buf(),
    })
}

/// Atomically replaces the file behind `path` (following symlinks, so a dotfiles
/// symlink stays a symlink). The temp file is created `O_EXCL` with its final mode:
/// `mode` if given, else the existing target's mode, else `0o644`.
pub fn write_atomic(
    path: impl AsRef<Path>,
    bytes: impl AsRef<[u8]>,
    mode: Option<u32>,
) -> Result<()> {
    write_atomic_impl(path.as_ref(), bytes.as_ref(), mode)
}

/// [`write_atomic`] with mode `0o600`, set when the temp file is created so the
/// bytes are never readable by other users.
pub fn write_secret_atomic(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> Result<()> {
    write_atomic_impl(path.as_ref(), bytes.as_ref(), Some(0o600))
}

#[cfg_attr(not(unix), allow(unused_variables))]
fn existing_mode(target: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(target)
            .ok()
            .map(|meta| meta.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    None
}

#[cfg_attr(not(unix), allow(unused_variables))]
fn write_atomic_impl(path: &Path, bytes: &[u8], mode: Option<u32>) -> Result<()> {
    let target = resolve_write_target(path)?;
    let parent = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    fs::create_dir_all(&parent).map_err(io_error(&parent))?;
    let Some(name) = target.file_name() else {
        return Err(io_error(&target)(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path has no file name",
        )));
    };
    let mode = mode.or_else(|| existing_mode(&target)).unwrap_or(0o644);
    let mut tmp_name = OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".tmp.{}.{}", std::process::id(), random_hex()));
    let tmp = parent.join(tmp_name);

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    let mut file = options.open(&tmp).map_err(io_error(&tmp))?;
    let written = (|| -> io::Result<()> {
        #[cfg(unix)]
        {
            // The open mode is masked by the umask; pin the exact requested mode.
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, &target)
    })();
    written.map_err(|source| {
        let _ = fs::remove_file(&tmp);
        AppConfigError::Io {
            path: target.clone(),
            source,
        }
    })
}

fn random_hex() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        hasher.write_u128(now.as_nanos());
    }
    format!("{:012x}", hasher.finish() & 0xffff_ffff_ffff)
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(io_error(path)(err)),
    }
}

/// 1-based (line, column-in-characters) of a byte offset.
fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    (line, column)
}

/// Parses a TOML file; `Ok(None)` when it does not exist. Malformed files are
/// reported (with path, line and column) and never touched.
pub fn load_toml_optional<T: DeserializeOwned>(path: impl AsRef<Path>) -> Result<Option<T>> {
    let path = path.as_ref();
    let Some(text) = read_optional(path)? else {
        return Ok(None);
    };
    toml::from_str(&text).map(Some).map_err(|err| {
        let (line, column) = err
            .span()
            .map_or((1, 1), |span| line_col(&text, span.start));
        AppConfigError::Parse {
            path: path.to_path_buf(),
            format: Format::Toml,
            line,
            column,
            message: err.message().trim().to_owned(),
        }
    })
}

/// [`load_toml_optional`], with `T::default()` for a missing file.
pub fn load_toml_or_default<T: DeserializeOwned + Default>(path: impl AsRef<Path>) -> Result<T> {
    Ok(load_toml_optional(path)?.unwrap_or_default())
}

fn to_toml<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<String> {
    toml::to_string_pretty(value).map_err(|err| AppConfigError::Serialize {
        path: path.to_path_buf(),
        format: Format::Toml,
        message: err.to_string(),
    })
}

/// Serializes `value` as TOML and [`write_atomic`]s it (existing mode kept, else `0o644`).
pub fn save_toml_atomic<T: Serialize + ?Sized>(path: impl AsRef<Path>, value: &T) -> Result<()> {
    let path = path.as_ref();
    write_atomic(path, to_toml(path, value)?, None)
}

/// Serializes `value` as TOML and [`write_secret_atomic`]s it (`0o600`).
pub fn save_secret_toml_atomic<T: Serialize + ?Sized>(
    path: impl AsRef<Path>,
    value: &T,
) -> Result<()> {
    let path = path.as_ref();
    write_secret_atomic(path, to_toml(path, value)?)
}

/// Parses a JSON file; `Ok(None)` when it does not exist. Malformed files are
/// reported (with path, line and column) and never touched.
pub fn load_json_optional<T: DeserializeOwned>(path: impl AsRef<Path>) -> Result<Option<T>> {
    let path = path.as_ref();
    let Some(text) = read_optional(path)? else {
        return Ok(None);
    };
    serde_json::from_str(&text).map(Some).map_err(|err| {
        let message = err.to_string();
        let message = match message.rsplit_once(" at line ") {
            Some((head, _)) => head.to_owned(),
            None => message,
        };
        AppConfigError::Parse {
            path: path.to_path_buf(),
            format: Format::Json,
            line: err.line().max(1),
            column: err.column().max(1),
            message,
        }
    })
}

/// Serializes `value` as pretty JSON (trailing newline) and [`write_atomic`]s it.
pub fn save_json_atomic<T: Serialize + ?Sized>(path: impl AsRef<Path>, value: &T) -> Result<()> {
    let path = path.as_ref();
    let mut text =
        serde_json::to_string_pretty(value).map_err(|err| AppConfigError::Serialize {
            path: path.to_path_buf(),
            format: Format::Json,
            message: err.to_string(),
        })?;
    text.push('\n');
    write_atomic(path, text, None)
}

#[cfg(test)]
mod tests;
