use super::*;
use serde::Deserialize;
use serde_json::Value;
#[cfg(unix)]
use std::os::unix::fs::{PermissionsExt, symlink};

fn fixtures() -> Value {
    let text = include_str!("../fixtures/xdg-cases.json");
    serde_json::from_str(text).expect("fixtures/xdg-cases.json parses")
}

fn str_field<'a>(case: &'a Value, key: &str) -> Option<&'a str> {
    case.get(key).and_then(Value::as_str)
}

fn assert_paths(name: &str, paths: &AppPaths, expect: &Value) {
    for (key, actual) in [
        ("configDir", paths.config_dir()),
        ("dataDir", paths.data_dir()),
        ("stateDir", paths.state_dir()),
        ("cacheDir", paths.cache_dir()),
    ] {
        let want = str_field(expect, key).unwrap_or_else(|| panic!("{name}: expect.{key}"));
        assert_eq!(actual, Path::new(want), "{name}: {key}");
    }
}

fn assert_outcome(name: &str, result: Result<AppPaths>, case: &Value) {
    match (result, str_field(case, "error")) {
        (Ok(paths), None) => assert_paths(name, &paths, &case["expect"]),
        (Err(err), Some(code)) => assert_eq!(err.code(), code, "{name}: {err}"),
        (Ok(paths), Some(code)) => panic!("{name}: expected {code}, got {paths:?}"),
        (Err(err), None) => panic!("{name}: unexpected error {err}"),
    }
}

#[test]
fn golden_resolve_cases() {
    let fixtures = fixtures();
    let cases = fixtures["resolve"].as_array().expect("resolve[]");
    assert!(cases.len() >= 10);
    for case in cases {
        let name = str_field(case, "name").expect("name");
        let env: HashMap<String, String> =
            serde_json::from_value(case["env"].clone()).expect("env map");
        let mut opts = ResolveOptions::new().env(env);
        if let Some(vendor) = case.pointer("/options/vendor").and_then(Value::as_str) {
            opts = opts.vendor(vendor);
        }
        if let Some(prefix) = case.pointer("/options/envPrefix").and_then(Value::as_str) {
            opts = opts.env_prefix(prefix);
        }
        let app = str_field(case, "app").expect("app");
        assert_outcome(name, AppPaths::resolve_with(app, &opts), case);
    }
}

#[test]
fn golden_for_test_cases() {
    let fixtures = fixtures();
    for case in fixtures["forTest"].as_array().expect("forTest[]") {
        let name = str_field(case, "name").expect("name");
        let root = str_field(case, "root").expect("root");
        let app = str_field(case, "app").expect("app");
        assert_outcome(name, AppPaths::for_test(root, app), case);
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Anything {
    #[serde(flatten)]
    rest: HashMap<String, toml::Value>,
}

#[test]
fn golden_parse_errors() {
    let dir = tempfile::tempdir().unwrap();
    let fixtures = fixtures();
    for case in fixtures["parseErrors"].as_array().expect("parseErrors[]") {
        let name = str_field(case, "name").expect("name");
        let format = str_field(case, "format").expect("format");
        let path = dir.path().join(format!("bad.{format}"));
        let text = str_field(case, "text").expect("text");
        fs::write(&path, text).unwrap();
        let err = match format {
            "toml" => load_toml_optional::<Anything>(&path).unwrap_err(),
            _ => load_json_optional::<Value>(&path).unwrap_err(),
        };
        let AppConfigError::Parse {
            path: at,
            line,
            column,
            ..
        } = &err
        else {
            panic!("{name}: expected Parse, got {err}");
        };
        assert_eq!(at, &path, "{name}");
        assert_eq!(
            Some(*line as u64),
            case["line"].as_u64(),
            "{name}: line ({err})"
        );
        assert_eq!(
            Some(*column as u64),
            case["column"].as_u64(),
            "{name}: column ({err})"
        );
        assert!(
            err.to_string()
                .starts_with(&format!("{}:{line}:{column}: ", path.display())),
            "{err}"
        );
        // A malformed file is never touched.
        assert_eq!(fs::read_to_string(&path).unwrap(), text, "{name}");
    }
}

#[test]
fn resolve_reads_process_env_home() {
    let paths = AppPaths::resolve("app-config-test").unwrap();
    assert!(paths.config_dir().is_absolute());
    assert!(paths.config_dir().ends_with("app-config-test"));
    assert_eq!(paths.app(), "app-config-test");
}

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

#[cfg(unix)]
#[test]
fn ensure_dirs_modes() {
    let root = tempfile::tempdir().unwrap();
    let paths = AppPaths::for_test(root.path(), "demo").unwrap();
    // Pre-create the private leaves with loose modes to prove they are enforced.
    for dir in [paths.data_dir(), paths.state_dir(), paths.cache_dir()] {
        fs::create_dir_all(dir).unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert_eq!(paths.ensure_config_dir().unwrap(), paths.config_dir());
    assert_eq!(mode_of(paths.config_dir()) & 0o700, 0o700);
    assert_eq!(mode_of(paths.config_dir()) & 0o055, 0o055 & !umask_probe());
    paths.ensure_data_dir().unwrap();
    paths.ensure_state_dir().unwrap();
    paths.ensure_cache_dir().unwrap();
    for dir in [paths.data_dir(), paths.state_dir(), paths.cache_dir()] {
        assert_eq!(mode_of(dir), 0o700, "{}", dir.display());
    }
    // ensure_config_dir never tightens an existing dir; the private variant does.
    paths.ensure_private_config_dir().unwrap();
    assert_eq!(mode_of(paths.config_dir()), 0o700);
    fs::set_permissions(paths.config_dir(), fs::Permissions::from_mode(0o755)).unwrap();
    paths.ensure_config_dir().unwrap();
    assert_eq!(mode_of(paths.config_dir()), 0o755);
}

#[cfg(unix)]
fn umask_probe() -> u32 {
    let dir = tempfile::tempdir().unwrap();
    let probe = dir.path().join("probe");
    fs::DirBuilder::new().create(&probe).unwrap();
    0o777 & !mode_of(&probe)
}

#[cfg(unix)]
#[test]
fn ensure_private_dir_leaves_symlinked_leaf_alone() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("dots-config");
    fs::create_dir(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).unwrap();
    let paths = AppPaths::for_test(root.path(), "demo").unwrap();
    fs::create_dir_all(paths.config_dir().parent().unwrap()).unwrap();
    symlink(&real, paths.config_dir()).unwrap();
    paths.ensure_private_config_dir().unwrap();
    assert!(
        fs::symlink_metadata(paths.config_dir())
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(mode_of(&real), 0o755);
}

#[cfg(unix)]
#[test]
fn write_atomic_modes() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = dir.path().join("nested/new.toml");
    write_atomic(&fresh, "a = 1\n", None).unwrap();
    assert_eq!(fs::read_to_string(&fresh).unwrap(), "a = 1\n");
    assert_eq!(mode_of(&fresh), 0o644);

    let existing = dir.path().join("existing.toml");
    fs::write(&existing, "old").unwrap();
    fs::set_permissions(&existing, fs::Permissions::from_mode(0o640)).unwrap();
    write_atomic(&existing, "new", None).unwrap();
    assert_eq!(mode_of(&existing), 0o640);
    write_atomic(&existing, "explicit", Some(0o604)).unwrap();
    assert_eq!(mode_of(&existing), 0o604);

    let secret = dir.path().join("secret.toml");
    fs::write(&secret, "loose").unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o644)).unwrap();
    write_secret_atomic(&secret, "key = 1").unwrap();
    assert_eq!(mode_of(&secret), 0o600);
    assert_eq!(fs::read_to_string(&secret).unwrap(), "key = 1");

    // No temp files are left behind.
    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[cfg(unix)]
#[test]
fn write_secret_is_0600_before_bytes_land() {
    // The temp file is created O_EXCL with its final mode, so even under a
    // permissive umask the secret is never group/world readable. Observed via a
    // blocked rename: target is a non-empty directory, so rename fails after the
    // write, and the error path must have removed the temp file.
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("occupied");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("child"), "x").unwrap();
    let err = write_secret_atomic(&target, "secret").unwrap_err();
    assert_eq!(err.code(), "Io");
    let names: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["occupied".to_string()]);
}

#[cfg(unix)]
#[test]
fn write_atomic_preserves_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let dots = dir.path().join("dots");
    fs::create_dir(&dots).unwrap();
    let real = dots.join("models.toml");
    fs::write(&real, "old").unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
    let config = dir.path().join("config");
    fs::create_dir(&config).unwrap();
    let link = config.join("models.toml");
    // Relative two-hop chain: link -> hop -> ../dots/models.toml
    let hop = config.join("hop.toml");
    symlink("../dots/models.toml", &hop).unwrap();
    symlink("hop.toml", &link).unwrap();
    assert_eq!(
        resolve_write_target(&link).unwrap(),
        config.join("../dots/models.toml")
    );

    save_toml_atomic(&link, &toml::toml! { a = 1 }).unwrap();
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(fs::symlink_metadata(&hop).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_to_string(&real).unwrap(), "a = 1\n");
    assert_eq!(mode_of(&real), 0o600);
}

#[cfg(unix)]
#[test]
fn write_atomic_through_dangling_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("config.json");
    let target = dir.path().join("dots/private/config.json");
    symlink(&target, &link).unwrap();
    save_json_atomic(&link, &serde_json::json!({"a": 1})).unwrap();
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "{\n  \"a\": 1\n}\n");
    assert_eq!(mode_of(&target), 0o644);
}

#[cfg(unix)]
#[test]
fn symlink_loop_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    symlink(&b, &a).unwrap();
    symlink(&a, &b).unwrap();
    let err = write_atomic(&a, "x", None).unwrap_err();
    assert_eq!(err.code(), "SymlinkLoop");
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct Settings {
    name: String,
    port: u16,
    #[serde(default)]
    tags: Vec<String>,
}

#[test]
fn toml_and_json_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let toml_path = dir.path().join("settings.toml");
    assert_eq!(load_toml_optional::<Settings>(&toml_path).unwrap(), None);
    assert_eq!(
        load_toml_or_default::<Settings>(&toml_path).unwrap(),
        Settings::default()
    );
    let value = Settings {
        name: "feeds".into(),
        port: 4610,
        tags: vec!["a".into()],
    };
    save_toml_atomic(&toml_path, &value).unwrap();
    assert_eq!(
        load_toml_optional::<Settings>(&toml_path).unwrap(),
        Some(value)
    );

    let json_path = dir.path().join("settings.json");
    assert_eq!(load_json_optional::<Settings>(&json_path).unwrap(), None);
    let value = Settings {
        name: "mesh".into(),
        port: 1,
        tags: vec![],
    };
    save_json_atomic(&json_path, &value).unwrap();
    assert!(fs::read_to_string(&json_path).unwrap().ends_with("}\n"));
    assert_eq!(
        load_json_optional::<Settings>(&json_path).unwrap(),
        Some(value)
    );
}

#[cfg(unix)]
#[test]
fn save_secret_toml_is_0600() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.toml");
    save_secret_toml_atomic(&path, &toml::toml! { token = "t" }).unwrap();
    assert_eq!(mode_of(&path), 0o600);
}

#[test]
fn type_errors_are_parse_errors_with_location() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    fs::write(&path, "name = \"x\"\nport = \"not a number\"\n").unwrap();
    let err = load_toml_optional::<Settings>(&path).unwrap_err();
    let AppConfigError::Parse { line, .. } = err else {
        panic!("{err}")
    };
    assert_eq!(line, 2);
}

#[test]
fn io_errors_name_the_path() {
    let dir = tempfile::tempdir().unwrap();
    // Reading a directory as a file is an I/O error, not "missing".
    let err = load_toml_optional::<Settings>(dir.path()).unwrap_err();
    assert_eq!(err.code(), "Io");
    assert!(
        err.to_string()
            .starts_with(&dir.path().display().to_string())
    );
}

#[test]
fn line_col_counts_characters() {
    assert_eq!(line_col("a\nbé c", 6), (2, 4));
    assert_eq!(line_col("", 10), (1, 1));
}
