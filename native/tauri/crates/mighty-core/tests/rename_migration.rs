//! RenameMigration (C# `Core\RenameMigration.cs`; its four checks in `Tests\RenameMigrationVerification.cs`, ported by name), over
//! temporary folders and an in-memory registry shaped as C#'s `FakeRegistry`; then `rewritten` against C# itself (`tools/parity`,
//! `update rewrite`). Never the real `%APPDATA%`, home, `~/.claude` or HKCU.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use mighty_core::rename_migration::{self, RegistryStore, RegistryValue, claude_project_folder, rewritten};

/// A folder of the test's own, removed however the test ends.
struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Temp {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("mighty-rename-{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        // Canonical (CI's 8.3 %TEMP%, the Mac's /var link), so the paths compared below are the ones the file system names.
        Temp(path.canonicalize().unwrap())
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// One key of the fake: its name as written, and its values by name (type, bytes).
type FakeKey = (String, BTreeMap<String, (u32, Vec<u8>)>);

/// C#'s `FakeRegistry`: keys compared without case, each key's values by name; a key's parents exist below the `Software` key.
#[derive(Default)]
struct FakeRegistry {
    keys: RefCell<BTreeMap<String, FakeKey>>,
}

impl FakeRegistry {
    fn value(&self, path: &str, name: &str) -> Option<(u32, Vec<u8>)> {
        self.keys.borrow().get(&path.to_lowercase()).and_then(|(_, values)| values.get(name).cloned())
    }
    fn has(&self, path: &str) -> bool {
        self.keys.borrow().contains_key(&path.to_lowercase())
    }
    fn count(&self) -> usize {
        self.keys.borrow().len()
    }
    fn put(&self, path: &str, name: &str, kind: u32, data: &[u8]) {
        self.set_value(path, &RegistryValue { name: name.into(), kind, data: data.to_vec() }).unwrap();
    }
}

impl RegistryStore for FakeRegistry {
    fn key_exists(&self, path: &str) -> bool {
        self.has(path)
    }
    fn sub_key_names(&self, path: &str) -> io::Result<Vec<String>> {
        let prefix = format!("{}\\", path.to_lowercase());
        Ok(self
            .keys
            .borrow()
            .iter()
            .filter(|(key, _)| key.starts_with(&prefix) && !key[prefix.len()..].contains('\\'))
            .map(|(_, (written, _))| written[prefix.len()..].to_string())
            .collect())
    }
    fn values(&self, path: &str) -> io::Result<Vec<RegistryValue>> {
        Ok(self.keys.borrow().get(&path.to_lowercase()).map_or_else(Vec::new, |(_, values)| {
            values.iter().map(|(name, (kind, data))| RegistryValue { name: name.clone(), kind: *kind, data: data.clone() }).collect()
        }))
    }
    fn delete_tree(&self, path: &str) -> io::Result<()> {
        let (key, prefix) = (path.to_lowercase(), format!("{}\\", path.to_lowercase()));
        self.keys.borrow_mut().retain(|name, _| *name != key && !name.starts_with(&prefix));
        Ok(())
    }
    fn create_key(&self, path: &str) -> io::Result<()> {
        let parts: Vec<&str> = path.split('\\').collect();
        let mut keys = self.keys.borrow_mut();
        for count in 2..=parts.len() {
            let key = parts[..count].join("\\");
            keys.entry(key.to_lowercase()).or_insert_with(|| (key, BTreeMap::new()));
        }
        Ok(())
    }
    fn set_value(&self, path: &str, value: &RegistryValue) -> io::Result<()> {
        self.create_key(path)?;
        let mut keys = self.keys.borrow_mut();
        let (_, values) = keys.get_mut(&path.to_lowercase()).expect("made above");
        values.insert(value.name.clone(), (value.kind, value.data.clone()));
        Ok(())
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// C# "rename migration: the old folders move once, the records and Claude's conversations follow".
#[test]
fn the_old_folders_move_once_and_the_records_follow() {
    let root = Temp::new("move");
    let (app_data, home) = (root.0.join("AppData"), root.0.join("home"));
    let claude = home.join(".claude");
    let (old_data, old_home) = (app_data.join("MightyClaudeNative"), home.join(".mightyclaude"));
    fs::create_dir_all(old_home.join("worktrees").join("abc")).unwrap();
    fs::create_dir_all(&old_data).unwrap();
    let old_prefix = claude_project_folder(&old_home.join("worktrees").to_string_lossy()) + "-";
    fs::create_dir_all(claude.join("projects").join(format!("{old_prefix}abc"))).unwrap();
    fs::create_dir_all(claude.join("projects").join("other-project")).unwrap();
    let old_text = old_home.to_string_lossy().into_owned();
    fs::write(old_data.join("workspace-state.json"), format!("{{\"path\":\"{}/worktrees/abc\"}}", old_text.replace('\\', "\\\\"))).unwrap();
    fs::write(old_data.join("delegation-state.json"), format!("{{\"worktree\":\"{old_text}/worktrees/abc\"}}")).unwrap();
    fs::write(old_data.join("keep.txt"), "kept").unwrap();

    let report = rename_migration::run(&app_data, &home, &claude, &FakeRegistry::default());
    let new_home = home.join(".mightycode");
    let new_text = new_home.to_string_lossy().into_owned();
    let data = app_data.join("MightyCode");
    assert!(report.moved_data_folder && report.moved_worktrees && !report.copied_registry, "both folders moved, no registry key to copy");
    assert!(
        !old_data.exists()
            && !old_home.exists()
            && read(&data.join("keep.txt")) == "kept"
            && new_home.join("worktrees").join("abc").is_dir(),
        "the contents arrive under the new names"
    );
    assert_eq!(report.worktrees, [new_home.join("worktrees").join("abc")], "the moved worktrees are listed for the git repair");
    let delegation = read(&data.join("delegation-state.json"));
    assert!(
        delegation.contains(&format!("{new_text}/worktrees/abc")) && !delegation.contains(".mightyclaude"),
        "the delegation records name the new worktree folder: {delegation}"
    );
    assert!(
        read(&data.join("workspace-state.json")).contains(&format!("{}/worktrees/abc", new_text.replace('\\', "\\\\"))),
        "the workspace records name the new worktree folder"
    );
    let new_prefix = claude_project_folder(&new_home.join("worktrees").to_string_lossy()) + "-";
    let projects = claude.join("projects");
    assert!(
        projects.join(format!("{new_prefix}abc")).is_dir()
            && !projects.join(format!("{old_prefix}abc")).exists()
            && projects.join("other-project").is_dir(),
        "Claude's conversations for the worktree follow it, others stay"
    );

    // Again: the old folders come back (an older copy of the app ran), and nothing is replaced.
    fs::create_dir_all(&old_data).unwrap();
    fs::write(old_data.join("keep.txt"), "old").unwrap();
    fs::create_dir_all(&old_home).unwrap();
    let second = rename_migration::run(&app_data, &home, &claude, &FakeRegistry::default());
    assert!(
        !second.moved_data_folder
            && !second.moved_worktrees
            && second.worktrees.is_empty()
            && read(&data.join("keep.txt")) == "kept"
            && old_data.is_dir()
            && old_home.is_dir(),
        "a folder moves only while the new one is not there"
    );
}

/// C# "rename migration: nothing old means nothing to do".
#[test]
fn nothing_old_means_nothing_to_do() {
    let root = Temp::new("nothing");
    let registry = FakeRegistry::default();
    let report = rename_migration::run(&root.0.join("AppData"), &root.0.join("home"), &root.0.join("home").join(".claude"), &registry);
    assert!(
        !report.moved_data_folder && !report.moved_worktrees && !report.copied_registry && report.worktrees.is_empty(),
        "nothing old, nothing done"
    );
    assert!(registry.count() == 0 && !root.0.join("AppData").exists() && !root.0.join("home").exists(), "nothing is created");
}

/// C# "rename migration: the registry key is copied once and the old one stays".
#[test]
fn the_registry_key_is_copied_once_and_the_old_one_stays() {
    const DWORD: u32 = 4;
    const EXPAND: u32 = 2;
    const TEXT: u32 = 1;
    let utf16 = |text: &str| text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect::<Vec<u8>>();
    let root = Temp::new("registry");
    let registry = FakeRegistry::default();
    registry.put(r"Software\MightyClaudeNative", "delegation.enabled", DWORD, &0u32.to_le_bytes());
    registry.put(r"Software\MightyClaudeNative", "", TEXT, &utf16("default"));
    registry.put(r"Software\MightyClaudeNative\Child\Deep", "path", EXPAND, &utf16("%USERPROFILE%\\x"));
    registry.create_key(r"Software\MightyClaudeNative\Empty").unwrap();
    let run = || rename_migration::run(&root.0.join("AppData"), &root.0.join("home"), &root.0.join("claude"), &registry);
    assert!(run().copied_registry, "the old key is copied");
    assert!(
        registry.value(r"Software\MightyCode", "delegation.enabled") == Some((DWORD, 0u32.to_le_bytes().to_vec()))
            && registry.value(r"Software\MightyCode", "") == Some((TEXT, utf16("default"))),
        "values keep their data and kind"
    );
    assert!(
        registry.value(r"Software\MightyCode\Child\Deep", "path") == Some((EXPAND, utf16("%USERPROFILE%\\x")))
            && registry.has(r"Software\MightyCode\Empty"),
        "subkeys come along, recursively, empty ones too"
    );
    assert!(registry.has(r"Software\MightyClaudeNative") && registry.count() == 8, "the old key is left");
    registry.put(r"Software\MightyCode", "delegation.enabled", DWORD, &1u32.to_le_bytes());
    assert!(
        !run().copied_registry
            && registry.value(r"Software\MightyCode", "delegation.enabled") == Some((DWORD, 1u32.to_le_bytes().to_vec())),
        "copied once: a later change is not undone"
    );
}

/// C# "rename migration: worktree paths are rewritten plain and JSON-escaped".
#[test]
fn paths_are_rewritten_plain_and_escaped() {
    let (old, target) = (r"C:\Users\me\.mightyclaude", r"C:\Users\me\.mightycode");
    let plain = rewritten(r"cd C:\Users\me\.mightyclaude\worktrees\a and C:\Users\me\.mightyclaude/worktrees/b", old, target);
    assert_eq!(plain, r"cd C:\Users\me\.mightycode\worktrees\a and C:\Users\me\.mightycode/worktrees/b", "plain paths, either separator");
    let json =
        rewritten(r#"{"p":"C:\\Users\\me\\.mightyclaude\\worktrees\\a","q":"C:\\Users\\me\\.mightyclaude\/worktrees"}"#, old, target);
    assert_eq!(
        json, r#"{"p":"C:\\Users\\me\\.mightycode\\worktrees\\a","q":"C:\\Users\\me\\.mightycode\/worktrees"}"#,
        "JSON-escaped paths"
    );
    assert_eq!(
        rewritten(r"C:\Users\me\.mightyclaude-other\x", old, target),
        r"C:\Users\me\.mightyclaude-other\x",
        "a folder that only starts with the old name is left alone"
    );
}

/// A record written with a UTF-8 byte order mark is read as C#'s `ReadAllText` reads it and, changed, written without the mark
/// (`WriteAllText`); an unchanged one is not written at all, and a record that is not there is no error.
#[test]
fn a_record_is_written_only_when_it_changes() {
    let root = Temp::new("records");
    let (old, target) = (r"C:\Users\me\.mightyclaude", r"C:\Users\me\.mightycode");
    let marked = root.0.join("marked.json");
    fs::write(&marked, [b"\xEF\xBB\xBF".as_slice(), br#"{"p":"C:\\Users\\me\\.mightyclaude\\worktrees\\a"}"#].concat()).unwrap();
    rename_migration::rewrite_paths(&marked, old, target);
    assert_eq!(fs::read(&marked).unwrap(), br#"{"p":"C:\\Users\\me\\.mightycode\\worktrees\\a"}"#);
    let untouched = root.0.join("untouched.json");
    fs::write(&untouched, b"\xEF\xBB\xBF{}").unwrap();
    rename_migration::rewrite_paths(&untouched, old, target);
    assert_eq!(fs::read(&untouched).unwrap(), b"\xEF\xBB\xBF{}", "nothing to rewrite: the file stays as it was, mark and all");
    rename_migration::rewrite_paths(&root.0.join("missing.json"), old, target);
    assert!(!root.0.join("missing.json").exists());
}

/// A registry that fails part way (a value refused, or a subkey that does not read): the copy reports nothing copied (C#'s caught
/// exception), and what it made is taken back so the next start copies again (W9 review L4); the old key is left as it was.
#[test]
fn a_registry_copy_that_fails_reports_nothing_copied_and_leaves_nothing() {
    struct Failing {
        inner: FakeRegistry,
        refuse_values: bool,
    }
    impl RegistryStore for Failing {
        fn key_exists(&self, path: &str) -> bool {
            self.inner.key_exists(path)
        }
        fn sub_key_names(&self, path: &str) -> io::Result<Vec<String>> {
            if path.ends_with("Child") {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.inner.sub_key_names(path)
        }
        fn values(&self, path: &str) -> io::Result<Vec<RegistryValue>> {
            self.inner.values(path)
        }
        fn create_key(&self, path: &str) -> io::Result<()> {
            self.inner.create_key(path)
        }
        fn set_value(&self, path: &str, value: &RegistryValue) -> io::Result<()> {
            if self.refuse_values {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.inner.set_value(path, value)
        }
        fn delete_tree(&self, path: &str) -> io::Result<()> {
            self.inner.delete_tree(path)
        }
    }
    let root = Temp::new("failing");
    for refuse_values in [true, false] {
        let registry = Failing { inner: FakeRegistry::default(), refuse_values };
        registry.inner.put(r"Software\MightyClaudeNative", "x", 4, &[1, 0, 0, 0]);
        registry.inner.put(r"Software\MightyClaudeNative\Child\Deep", "y", 4, &[2, 0, 0, 0]);
        let before = registry.inner.count();
        let report = rename_migration::run(&root.0.join("AppData"), &root.0.join("home"), &root.0.join("claude"), &registry);
        assert!(!report.copied_registry, "refused values {refuse_values}: not copied");
        assert!(!registry.inner.has(r"Software\MightyCode"), "refused values {refuse_values}: the half-made key is taken back");
        assert_eq!(registry.inner.count(), before, "the old key is left as it was");
    }
}

/// The migration's home is an absolute folder, or none.
#[test]
fn the_home_is_absolute_or_none() {
    assert!(rename_migration::home().is_none_or(|home| home.is_absolute()));
}

// ---- parity with C# (`update rewrite`) ---------------------------------------------------------------------------------------------

/// Whether a .NET 10 SDK is installed: without it the check says so and passes, or fails under `MIGHTY_PARITY=require`.
#[allow(clippy::disallowed_methods, reason = "the check's own .NET probe, no child of the app")]
fn dotnet_10() -> bool {
    let listed = std::process::Command::new("dotnet").arg("--list-sdks").output();
    let found = matches!(&listed, Ok(output) if output.status.success() && String::from_utf8_lossy(&output.stdout).lines().any(|line| line.starts_with("10.")));
    if !found {
        let why = "no .NET 10 SDK (dotnet --list-sdks)";
        assert!(std::env::var("MIGHTY_PARITY").as_deref() != Ok("require"), "{why}");
        eprintln!("skipped: {why}");
    }
    found
}

/// RenameMigration's `Rewritten` through C# itself (`parity update rewrite <text file> <old> <new> <out>`) and through this crate, on
/// paths of both separators, escaped and not, a near miss, repeated and adjacent occurrences, non-ASCII names and a slash-escaped
/// separator; compared byte for byte.
#[test]
#[allow(clippy::disallowed_methods, reason = "the check's own .NET build and run, no child of the app")]
fn rewritten_matches_csharp() {
    if !dotnet_10() {
        return;
    }
    let tauri = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = tauri.join("target/parity-cs-rename");
    let built = std::process::Command::new("dotnet")
        .arg("build")
        .arg(tauri.join("tools/parity/Parity.csproj"))
        .args(["-c", "Release", "-o"])
        .arg(&out)
        .args(["--nologo", "-v", "quiet"])
        .status();
    assert!(matches!(built, Ok(status) if status.success()), "the C# parity tool did not build ({built:?})");
    let root = Temp::new("parity");
    let cases: [(&str, &str, &str); 8] = [
        (
            r"cd C:\Users\me\.mightyclaude\worktrees\a and C:\Users\me\.mightyclaude/worktrees/b",
            r"C:\Users\me\.mightyclaude",
            r"C:\Users\me\.mightycode",
        ),
        (
            r#"{"p":"C:\\Users\\me\\.mightyclaude\\worktrees\\a","q":"C:\\Users\\me\\.mightyclaude\/worktrees"}"#,
            r"C:\Users\me\.mightyclaude",
            r"C:\Users\me\.mightycode",
        ),
        (r"C:\Users\me\.mightyclaude-other\x C:\Users\me\.mightyclaude", r"C:\Users\me\.mightyclaude", r"C:\Users\me\.mightycode"),
        (r"C:\a\.mightyclaude\\C:\a\.mightyclaude\/C:\a\.mightyclaude//x", r"C:\a\.mightyclaude", r"C:\a\.mightycode"),
        (r#"{"w":"D:\\사용자\\김\\.mightyclaude\\worktrees\\가"}"#, r"D:\사용자\김\.mightyclaude", r"D:\사용자\김\.mightycode"),
        ("/Users/me/.mightyclaude/worktrees/a \"\\/Users\\/me\\/.mightyclaude\\/x\"", "/Users/me/.mightyclaude", "/Users/me/.mightycode"),
        ("", r"C:\x\.mightyclaude", r"C:\x\.mightycode"),
        (r"C:\x\.mightyclaude\.mightyclaude\y", r"C:\x\.mightyclaude", r"C:\x\.mightycode"),
    ];
    for (index, (text, old, target)) in cases.iter().enumerate() {
        let (input, output) = (root.0.join(format!("{index}.txt")), root.0.join(format!("{index}.out")));
        fs::write(&input, text).unwrap();
        let ran = std::process::Command::new("dotnet")
            .arg(out.join("parity.dll"))
            .args(["update", "rewrite"])
            .arg(&input)
            .args([old, target])
            .arg(&output)
            .status();
        assert!(matches!(ran, Ok(status) if status.success()), "the C# parity tool failed on case {index} ({ran:?})");
        assert_eq!(fs::read_to_string(&output).unwrap(), rewritten(text, old, target), "case {index}: {text}");
    }
}
