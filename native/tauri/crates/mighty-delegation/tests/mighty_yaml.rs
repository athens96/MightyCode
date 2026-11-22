//! Verification for `.mighty.yaml` / `.mighty.local.yaml` parsing, overlay merging, SHA-256 content hashing, worktree cache
//! directory sharing (`share_directories` + `.git/info/exclude`), and mapped copy rules (`copy_mapped_rules`) (`MC-8`).

mod support;

use mighty_core::locale::Locale;
use mighty_delegation::git::cleanup::ChildDiscardOutcome;
use mighty_delegation::git::worktree::{ChildWorktreeCheck, ChildWorktreeOutcome};
use mighty_delegation::mighty_yaml::{
    self, CopyRule, DEFAULT_TEARDOWN_TIMEOUT_SECONDS, DEFAULT_WORKTREE_PORTS, MAXIMUM_CONFIG_BYTES, RepoTaskSpec, RunActionSpec, RunUrl,
    load_from_folder, load_from_folder_with_local_fallback, parse_config,
};
use mighty_delegation::pane_worktree as pw;

#[test]
fn parses_comments_scalars_lists_multiline_blocks_and_nested_mappings() {
    let yaml = r#"
# Shared repository configuration
---
version: 1

worktree:
  symlinks: [node_modules, ".next/", ./target] # inline list with comments
  copy:
    - .env.shared
    - ".env.example -> .env"
    - { from: "config/dev.example.json", to: "config/dev.json" }
    - from: 'secrets/sample''s.txt'
      to: "secrets/active.txt"
  bootstrap: |
    # inside literal block
    pnpm install --frozen-lockfile
    pnpm db:generate
  env:
    NODE_ENV: development
    PORT_OFFSET: "10"
    EMPTY_VAR: ~

scripts:
  test: pnpm test -- --run
  lint:
    run: >-
      pnpm eslint
      --max-warnings 0
    label: "Lint all packages"
    cwd: packages/web
  ci:
    command:
      - cargo fmt --check
      - cargo clippy -- -D warnings
    description: Full Rust check
...
"#;

    let cfg = parse_config(yaml).expect("valid .mighty.yaml");
    assert_eq!(cfg.version, 1);
    assert_eq!(cfg.worktree.share, vec!["node_modules", ".next", "target"]);
    assert_eq!(
        cfg.worktree.copy,
        vec![
            CopyRule::same_path(".env.shared"),
            CopyRule::mapped(".env.example", ".env"),
            CopyRule::mapped("config/dev.example.json", "config/dev.json"),
            CopyRule::mapped("secrets/sample's.txt", "secrets/active.txt"),
        ]
    );
    assert_eq!(
        cfg.worktree.setup.as_deref(),
        Some("pnpm install --frozen-lockfile\npnpm db:generate"),
        "a literal block stays a script with real newlines"
    );
    assert_eq!(cfg.worktree.env.get("NODE_ENV").map(String::as_str), Some("development"));
    assert_eq!(cfg.worktree.env.get("PORT_OFFSET").map(String::as_str), Some("10"));
    assert_eq!(cfg.worktree.env.get("EMPTY_VAR").map(String::as_str), Some(""));

    assert_eq!(cfg.tasks.get("test"), Some(&RepoTaskSpec { command: "pnpm test -- --run".into(), description: None, cwd: None }));
    assert_eq!(
        cfg.tasks.get("lint"),
        Some(&RepoTaskSpec {
            command: "pnpm eslint --max-warnings 0".into(),
            description: Some("Lint all packages".into()),
            cwd: Some("packages/web".into()),
        })
    );
    assert_eq!(
        cfg.tasks.get("ci"),
        Some(&RepoTaskSpec {
            command: "cargo fmt --check && cargo clippy -- -D warnings".into(),
            description: Some("Full Rust check".into()),
            cwd: None,
        })
    );
}

#[test]
fn rejects_oversized_unsafe_or_malformed_configs() {
    let oversized = format!("version: 1\n# {}", "x".repeat(MAXIMUM_CONFIG_BYTES));
    assert!(mighty_yaml::parse_yaml(&oversized).is_err(), "rejects > 256 KiB");

    for invalid in [
        "version: 2\n",
        "worktree:\n\tshare: [node_modules]\n",
        "worktree:\n  share: [../outside]\n",
        "worktree:\n  share: [/etc/passwd]\n",
        "worktree:\n  share: [.git/objects]\n",
        "worktree:\n  copy: [\".env.example -> ../escape\"]\n",
        "worktree:\n  env:\n    123BAD: nope\n",
        "worktree:\n  share: [\"x&calc\"]\n",
        "worktree:\n  share: [\"a|b\"]\n",
        "worktree:\n  copy: [\".env -> out%PATH%\"]\n",
        "worktree:\n  copy: [\".GIT/config\"]\n",
        "worktree:\n  copy: [\"GIT~1/config -> x\"]\n",
        "worktree:\n  env:\n    PATH: /tmp/evil\n",
        "worktree:\n  env:\n    node_options: --require x\n",
        "worktree:\n  env:\n    GIT_SSH_COMMAND: evil\n",
        "worktree:\n  env:\n    LD_PRELOAD: x.so\n",
        "tasks:\n  escape:\n    command: ls\n    cwd: ../../outside\n",
        "unknown_root: true\n",
    ] {
        assert!(parse_config(invalid).is_err(), "expected error for:\n{invalid}");
    }
}

#[test]
fn loads_and_merges_local_overlay_and_recomputes_sha256_hash_on_disk_changes() {
    let folder = support::place::temp("mighty-yaml-overlay");
    assert!(load_from_folder(&folder).unwrap().is_none(), "missing files return None");

    std::fs::write(
        folder.join(".mighty.yaml"),
        r#"
version: 1
worktree:
  share:
    - node_modules
    - target
  copy:
    - .env.example -> .env
    - config/base.json
  setup: pnpm install
  env:
    NODE_ENV: test
    SHARED_KEY: base
tasks:
  test: pnpm test
  build: pnpm build
"#,
    )
    .unwrap();

    let base_loaded = load_from_folder(&folder).unwrap().expect("loaded base");
    assert_eq!(base_loaded.files, vec![".mighty.yaml"]);
    assert_eq!(base_loaded.content_hash.len(), 64);
    assert!(base_loaded.content_hash.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));

    std::fs::write(
        folder.join(".mighty.local.yaml"),
        r#"
worktree:
  share:
    - target
    - .turbo
  copy:
    - .env.my -> .env
    - .env.local
  setup: pnpm install --offline
  env:
    SHARED_KEY: local_override
    LOCAL_ONLY: "1"
tasks:
  test: pnpm test --watch=false
  dev: pnpm dev
"#,
    )
    .unwrap();

    let merged = load_from_folder(&folder).unwrap().expect("loaded merged");
    assert_eq!(merged.files, vec![".mighty.yaml", ".mighty.local.yaml"]);
    assert_ne!(merged.content_hash, base_loaded.content_hash, "overlay changes content_hash");
    assert_eq!(merged.config.worktree.share, vec!["node_modules", "target", ".turbo"]);
    assert_eq!(
        merged.config.worktree.copy,
        vec![CopyRule::mapped(".env.my", ".env"), CopyRule::same_path("config/base.json"), CopyRule::same_path(".env.local"),]
    );
    assert_eq!(merged.config.worktree.setup.as_deref(), Some("pnpm install --offline"));
    assert_eq!(merged.config.worktree.env.get("NODE_ENV").map(String::as_str), Some("test"));
    assert_eq!(merged.config.worktree.env.get("SHARED_KEY").map(String::as_str), Some("local_override"));
    assert_eq!(merged.config.worktree.env.get("LOCAL_ONLY").map(String::as_str), Some("1"));
    assert_eq!(merged.config.tasks["test"].command, "pnpm test --watch=false");
    assert_eq!(merged.config.tasks["build"].command, "pnpm build");
    assert_eq!(merged.config.tasks["dev"].command, "pnpm dev");

    // A worktree directory that has `.mighty.yaml` (committed) but not `.mighty.local.yaml` (gitignored) resolves the exact same
    // merged config and content_hash when falling back to the workspace folder for `.mighty.local.yaml`.
    let wt_folder = folder.join("simulated-wt");
    std::fs::create_dir_all(&wt_folder).unwrap();
    std::fs::copy(folder.join(".mighty.yaml"), wt_folder.join(".mighty.yaml")).unwrap();
    let wt_loaded = load_from_folder_with_local_fallback(&wt_folder, &folder).unwrap().expect("loaded from wt");
    assert_eq!(wt_loaded.content_hash, merged.content_hash);
    assert_eq!(wt_loaded.config, merged.config);

    // Modifying `.mighty.yaml` on disk changes `content_hash` even for comment changes.
    std::fs::write(wt_folder.join(".mighty.yaml"), "# edited in worktree\nversion: 1\ntasks:\n  test: echo pwned\n").unwrap();
    let modified = load_from_folder_with_local_fallback(&wt_folder, &folder).unwrap().expect("loaded modified");
    assert_ne!(modified.content_hash, merged.content_hash);

    support::place::remove_folder(&folder);
}

#[tokio::test]
async fn share_directories_and_mapped_copy_keep_git_status_clean_and_survive_worktree_discard() {
    let _lane = support::lane::enter().await;
    let place = support::place::DelegationPlace::new();
    let (git, locale) = (place.git(), Locale::new("en"));
    place.repository_default().await;

    let repo = std::path::PathBuf::from(place.repo());
    // Trailing-slash `.gitignore` entry (`node_modules/`): on Unix a symlink `node_modules` would appear as `?? node_modules`
    // unless `share_directories` adds `/node_modules` to `<git-common-dir>/info/exclude`!
    std::fs::write(repo.join(".gitignore"), "node_modules/\n.env\n").unwrap();
    std::fs::write(repo.join(".env.example"), "API_URL=http://localhost:3000\n").unwrap();
    place.fixture_git(&["add", "-A"]).await;
    place.fixture_git(&["commit", "-q", "-m", "add gitignore and env.example"]).await;

    // Create real cache directory and local files in main checkout.
    let cache_dir = repo.join("node_modules").join("pkg");
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::write(cache_dir.join("index.js"), "module.exports = 42;\n").unwrap();
    std::fs::write(repo.join("plain-file.txt"), "not a directory\n").unwrap();

    // Outside directory + symlink inside repo to verify symlinks/outside paths are rejected.
    let outside = std::path::PathBuf::from(place.base()).join("outside-cache");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "secret\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, repo.join("linked_cache")).unwrap();

    let maker = place.maker();
    let ChildWorktreeCheck::Ready(start) = maker.check(&place.repo()).await else { panic!("repo not ready") };
    let ChildWorktreeOutcome::Created(made) = pw::make_pane(&maker, "pane-share", &start, 300.0).await else {
        panic!("worktree not created");
    };
    let pane = pw::from_made(&made, &start.repository, 4141, None);
    let wt_path = std::path::PathBuf::from(pane.worktree_path());
    let processes = support::place::processes();

    let mut share_list =
        vec!["node_modules".to_string(), "missing_cache".to_string(), "plain-file.txt".to_string(), "../outside-cache".to_string()];
    #[cfg(unix)]
    share_list.push("linked_cache".to_string());

    // Present but not ignored by the source repository: never shared (its exclude entry would hide files git should list).
    std::fs::create_dir_all(repo.join("tracked_dir")).unwrap();
    share_list.push("tracked_dir".to_string());

    let shared = pw::share_directories(&share_list, &place.repo(), pane.run_folder(), &place.paths(), &git, processes.as_ref()).await;
    assert_eq!(shared.shared, vec!["node_modules"]);
    assert_eq!(shared.why("tracked_dir"), Some("notIgnored"));

    // The exclude entry lives under this worktree's marker so a discard can take it out again.
    let common = std::path::PathBuf::from(
        place.git_in(pane.worktree_path(), &["rev-parse", "--path-format=absolute", "--git-common-dir"]).await.trim(),
    );
    let exclude_path = common.join("info").join("exclude");
    let exclude = std::fs::read_to_string(&exclude_path).unwrap();
    assert!(exclude.contains("# mighty-code shared "), "{exclude}");
    assert!(exclude.lines().any(|l| l == "/node_modules"), "{exclude}");
    assert_eq!(shared.why("missing_cache"), Some("missing"));
    assert_eq!(shared.why("plain-file.txt"), Some("notDirectory"));
    assert_eq!(shared.why("../outside-cache"), Some("outside"));
    #[cfg(unix)]
    assert_eq!(shared.why("linked_cache"), Some("link"));

    // Verify shared cache content is readable inside the worktree via link/junction.
    assert_eq!(std::fs::read_to_string(wt_path.join("node_modules").join("pkg").join("index.js")).unwrap(), "module.exports = 42;\n");

    // Mapped copy: `.env.example -> .env` creates `.env`, and a second call never overwrites existing `.env`!
    let copy1 = pw::copy_mapped_rules(
        &[CopyRule::mapped(".env.example", ".env"), CopyRule::mapped("../outside-cache/secret.txt", "stolen.txt")],
        &place.repo(),
        pane.run_folder(),
        &place.paths(),
    );
    assert_eq!(copy1.copied, vec![".env.example -> .env"]);
    assert_eq!(copy1.why("../outside-cache/secret.txt -> stolen.txt"), Some("outside"));
    assert_eq!(std::fs::read_to_string(wt_path.join(".env")).unwrap(), "API_URL=http://localhost:3000\n");

    std::fs::write(wt_path.join(".env"), "API_URL=http://localhost:9999\n").unwrap();
    let copy2 = pw::copy_mapped_rules(&[CopyRule::mapped(".env.example", ".env")], &place.repo(), pane.run_folder(), &place.paths());
    assert!(copy2.copied.is_empty());
    assert_eq!(copy2.why(".env.example -> .env"), Some("exists"));
    assert_eq!(std::fs::read_to_string(wt_path.join(".env")).unwrap(), "API_URL=http://localhost:9999\n", "existing .env preserved");

    // Crucial git status check: neither `node_modules` symlink nor `.env` appears as untracked changes!
    assert_eq!(place.git_in(pane.worktree_path(), &["status", "--porcelain"]).await, "");
    assert_eq!(pw::has_any_changes(&git, pane.worktree_path()).await, Some(false));

    // Discard unlinks `node_modules` safely without touching the workspace's real `node_modules/pkg/index.js`!
    let discarded = pw::discard(&git, &place.paths(), &pane, "pane-share", &place.root(), &locale).await;
    assert_eq!(discarded, ChildDiscardOutcome::Discarded);
    assert!(!wt_path.exists());
    let after = std::fs::read_to_string(&exclude_path).unwrap();
    assert!(!after.contains("mighty-code shared") && !after.lines().any(|l| l == "/node_modules"), "discard removes its entries: {after}");
    assert_eq!(
        std::fs::read_to_string(repo.join("node_modules").join("pkg").join("index.js")).unwrap(),
        "module.exports = 42;\n",
        "main workspace cache stays intact after worktree discard"
    );
}

#[test]
fn a_quote_inside_a_plain_scalar_opens_no_string_and_a_block_script_keeps_its_lines() {
    let cfg = parse_config(
        "tasks:\n  note: echo Don't stop # a comment\n  quoted: 'it''s # kept' # dropped\n  guard:\n    run: |\n      if [ -f a ]; then\n        echo yes\n      fi\n      echo done\n  chain:\n    command:\n      - a\n      - b\n",
    )
    .expect("valid");
    assert_eq!(cfg.tasks["note"].command, "echo Don't stop");
    assert_eq!(cfg.tasks["quoted"].command, "it's # kept");
    assert_eq!(cfg.tasks["guard"].command, "if [ -f a ]; then\necho yes\nfi\necho done");
    assert_eq!(cfg.tasks["chain"].command, "a && b");
    assert_eq!(mighty_yaml::flatten_script("a\nb"), "a && b");
}

#[test]
fn parses_teardown_ports_and_named_run_actions() {
    let yaml = r#"
version: 1
worktree:
  setup: npm ci
  teardown: |
    docker compose down -v
    rm -rf .cache/tmp
  teardownTimeoutSeconds: 90
  ports: 4
run:
  dev:
    command: npm run dev -- --port $MIGHTY_PORT
  e2e:
    command: npm run test:e2e
    concurrent: false
    url: off
  docs:
    cmd: mkdocs serve
    url: "http://localhost:8000/docs"
  storybook: npm run storybook
"#;
    let cfg = parse_config(yaml).expect("valid run and teardown keys");
    assert_eq!(cfg.worktree.teardown.as_deref(), Some("docker compose down -v\nrm -rf .cache/tmp"));
    assert_eq!(cfg.worktree.teardown_timeout_seconds, Some(90));
    assert_eq!(cfg.worktree.teardown_timeout(), std::time::Duration::from_secs(90));
    assert_eq!(cfg.worktree.port_count(), 4);
    assert!(cfg.has_worktree_actions());

    let names: Vec<&str> = cfg.run.keys().map(String::as_str).collect();
    assert_eq!(names, ["dev", "e2e", "docs", "storybook"], "run actions keep the file's order");
    let dev = &cfg.run["dev"];
    assert_eq!(
        dev,
        &RunActionSpec { command: "npm run dev -- --port $MIGHTY_PORT".into(), concurrent: true, url: "auto".into() },
        "$MIGHTY_PORT stays for the shell to expand"
    );
    assert_eq!(dev.url_mode(), RunUrl::Auto);
    assert!(!cfg.run["e2e"].concurrent);
    assert_eq!(cfg.run["e2e"].url_mode(), RunUrl::Off);
    assert_eq!(cfg.run["docs"].url_mode(), RunUrl::Fixed("http://localhost:8000/docs"));
    assert_eq!(cfg.run["storybook"].command, "npm run storybook");

    // Defaults: 60 seconds and 10 ports; `ports: 0` turns the assignment off; a file with only `teardown` has worktree actions.
    let plain = parse_config("worktree:\n  teardown: make clean\n").unwrap();
    assert_eq!(plain.worktree.teardown_timeout(), std::time::Duration::from_secs(DEFAULT_TEARDOWN_TIMEOUT_SECONDS.into()));
    assert_eq!(plain.worktree.port_count(), DEFAULT_WORKTREE_PORTS);
    assert!(plain.has_worktree_actions());
    assert_eq!(parse_config("worktree:\n  ports: 0\n").unwrap().worktree.port_count(), 0);
    assert!(!parse_config("worktree:\n  ports: 0\n").unwrap().has_worktree_actions(), "a port block size alone runs nothing");
}

#[test]
fn rejects_invalid_teardown_ports_and_run_values() {
    for invalid in [
        "worktree:\n  ports: -1\n",
        "worktree:\n  ports: 101\n",
        "worktree:\n  ports: ten\n",
        "worktree:\n  teardownTimeoutSeconds: 0\n",
        "worktree:\n  teardownTimeoutSeconds: 3601\n",
        "worktree:\n  teardown: [\"a\", {x: 1}]\n",
        "worktree:\n  teardown: \"rm -rf \u{202E}x\"\n",
        "worktree:\n  env:\n    MIGHTY_PORT: \"3000\"\n",
        "worktree:\n  env:\n    mighty_root_path: /tmp\n",
        "run: [dev]\n",
        "run:\n  dev:\n",
        "run:\n  dev:\n    concurrent: false\n",
        "run:\n  dev:\n    command: npm run dev\n    concurrent: maybe\n",
        "run:\n  dev:\n    command: npm run dev\n    url: ftp://localhost:21\n",
        "run:\n  dev:\n    command: npm run dev\n    url: \"http://local host:3000\"\n",
        "run:\n  dev:\n    command: npm run dev\n    url: \"http://localhost:3000/\u{200B}\"\n",
        "run:\n  dev:\n    command: npm run dev\n    env: {A: b}\n",
        "run:\n  \" dev\": npm run dev\n",
    ] {
        assert!(parse_config(invalid).is_err(), "expected error for:\n{invalid}");
    }
    let long = format!("run:\n  dev: \"{}\"\n", "x".repeat(mighty_yaml::MAXIMUM_COMMAND_CHARS + 1));
    assert!(parse_config(&long).is_err(), "a run command keeps the command length limit");
    let many: String = (0..=mighty_yaml::MAXIMUM_RUN_ACTIONS).map(|i| format!("  a{i}: echo {i}\n")).collect();
    assert!(parse_config(&format!("run:\n{many}")).is_err(), "at most {} run actions", mighty_yaml::MAXIMUM_RUN_ACTIONS);
}

#[test]
fn the_new_keys_change_the_content_hash_and_a_file_without_them_keeps_its_hash() {
    let folder = support::place::temp("mighty-yaml-lifecycle-hash");
    let hash_of = |text: &str| {
        std::fs::write(folder.join(".mighty.yaml"), text).unwrap();
        load_from_folder(&folder).unwrap().unwrap().content_hash
    };
    let base = "version: 1\nworktree:\n  setup: npm ci\n";
    let before = hash_of(base);
    // The normalized config without the new keys serializes as it did, so approvals made before stay valid.
    let json = serde_json::to_string(&parse_config(base).unwrap()).unwrap();
    assert!(!json.contains("teardown") && !json.contains("ports") && !json.contains("\"run\""), "{json}");

    let mut seen = vec![before.clone()];
    for changed in [
        "version: 1\nworktree:\n  setup: npm ci\n  teardown: docker compose down\n",
        "version: 1\nworktree:\n  setup: npm ci\n  teardown: docker compose down\n  teardownTimeoutSeconds: 30\n",
        "version: 1\nworktree:\n  setup: npm ci\n  ports: 20\n",
        "version: 1\nworktree:\n  setup: npm ci\nrun:\n  dev: npm run dev\n",
        "version: 1\nworktree:\n  setup: npm ci\nrun:\n  dev:\n    command: npm run dev\n    concurrent: false\n",
    ] {
        let hash = hash_of(changed);
        assert!(!seen.contains(&hash), "every change of the new keys asks for approval again:\n{changed}");
        seen.push(hash);
    }
    assert_eq!(hash_of(base), before, "the same file gives the same hash");

    // `.mighty.local.yaml` overrides teardown, its limit and the port count, and adds run actions by name.
    std::fs::write(
        folder.join(".mighty.yaml"),
        "worktree:\n  teardown: make clean\n  ports: 10\nrun:\n  dev: npm run dev\n  e2e:\n    command: npm run e2e\n    concurrent: false\n",
    )
    .unwrap();
    std::fs::write(folder.join(".mighty.local.yaml"), "worktree:\n  teardown: ~\n  ports: 3\nrun:\n  dev: pnpm dev\n").unwrap();
    let merged = load_from_folder(&folder).unwrap().unwrap().config;
    assert_eq!(merged.worktree.teardown, None, "a local `teardown: ~` turns the shared one off");
    assert_eq!(merged.worktree.ports, Some(3));
    assert_eq!(merged.run["dev"].command, "pnpm dev");
    assert!(!merged.run["e2e"].concurrent);
    support::place::remove_folder(&folder);
}
